//! Сервер учёта Avtodom.

pub mod api;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod domain;
pub mod error;
pub mod ops;
pub mod state;

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::any;
use sqlx::PgPool;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn build_state(pool: PgPool, cookie_secure: bool) -> Result<AppState, error::AppError> {
    let dummy_hash = auth::hash_password("неизвестный-пользователь".into()).await?;
    Ok(AppState {
        pool,
        cookie_secure,
        dummy_hash: Arc::new(dummy_hash),
        telegram_bot: false,
    })
}

pub fn app(state: AppState, web_dir: Option<&Path>) -> Router {
    let api = api::routes()
        .fallback(any(|| async { StatusCode::NOT_FOUND }))
        .layer(axum::middleware::map_response(json_rejections));
    let router = Router::new().nest("/api/v1", api);
    let router = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            router.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => router,
    };
    router.layer(TraceLayer::new_for_http()).with_state(state)
}

/// Отказы разбора запроса (кривой JSON, не те типы, неверный id в пути) axum отдаёт текстом
/// на английском. Касса ждёт `{error: {code, message}}` по-русски — приводим к нему.
async fn json_rejections(res: axum::response::Response) -> axum::response::Response {
    use axum::http::header::CONTENT_TYPE;
    use axum::response::IntoResponse;
    let status = res.status();
    let plain = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/plain"));
    // 404 и 405 axum отдаёт вовсе без тела.
    let empty = res.headers().get(CONTENT_TYPE).is_none();
    if !status.is_client_error() || !(plain || empty) {
        return res;
    }
    let message = match status {
        StatusCode::NOT_FOUND => "не найдено",
        StatusCode::METHOD_NOT_ALLOWED => "такой операции нет",
        StatusCode::PAYLOAD_TOO_LARGE => "запрос слишком большой",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "запрос должен быть в формате JSON",
        _ => "неверный формат запроса: проверьте поля и значения",
    };
    (
        status,
        axum::Json(serde_json::json!({ "error": { "code": "bad_request", "message": message } })),
    )
        .into_response()
}
