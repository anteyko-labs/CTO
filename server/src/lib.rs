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
        assistant_daily_limit: None,
        assistant: None,
        optima: None,
        telegram_api: None,
        whatsapp: None,
        public_url: None,
    })
}

pub fn app(state: AppState, web_dir: Option<&Path>) -> Router {
    // Обычный запрос программы — до 512 КБ (чек на сотни строк — десятки КБ): мегабайтный
    // комментарий не проходит. Загрузки списка старой программы и фото накладных ставят свой
    // предел на своих маршрутах (ADR-090).
    let api = api::routes()
        .fallback(any(|| async { StatusCode::NOT_FOUND }))
        .layer(axum::extract::DefaultBodyLimit::max(512 * 1024))
        .layer(axum::middleware::map_response(json_rejections))
        // Ответы сжимаются, если клиент просит: снимок кассы и списки — сотни КБ, а владелец
        // ходит через Tailscale мимо Caddy (ADR-094).
        // Быстрое сжатие: снимок кассы ужимается в 6–7 раз, а время ответа почти не растёт.
        .layer(
            tower_http::compression::CompressionLayer::new()
                .quality(tower_http::CompressionLevel::Fastest),
        );
    let router = Router::new().nest("/api/v1", api);
    let router = match web_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            let web = Router::new()
                .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
                .layer(axum::middleware::from_fn(fresh_shell));
            router.fallback_service(web)
        }
        None => router,
    };
    router.layer(TraceLayer::new_for_http()).with_state(state)
}

/// Оболочка программы (index.html, sw.js, манифест) отдаётся всегда целиком и с `no-cache`: ответ
/// 304 по дате изменения оставил бы кассу на прежней версии — после отката на точке файлы
/// возвращаются со старыми датами, а у двух сборок в одну секунду даты совпадают. Файлы
/// `/assets/` — с хэшем в имени, им проверка по дате не мешает.
async fn fresh_shell(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::http::HeaderValue;
    use axum::http::header::{CACHE_CONTROL, IF_MODIFIED_SINCE, IF_NONE_MATCH};
    let shell = !req.uri().path().starts_with("/assets/");
    if shell {
        req.headers_mut().remove(IF_MODIFIED_SINCE);
        req.headers_mut().remove(IF_NONE_MATCH);
    }
    let mut res = next.run(req).await;
    if shell {
        res.headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
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
