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
    })
}

pub fn app(state: AppState, web_dir: Option<&Path>) -> Router {
    let api = api::routes().fallback(any(|| async { StatusCode::NOT_FOUND }));
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
