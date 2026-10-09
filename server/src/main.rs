//! Точка входа сервера: конфигурация, подключение к базе, миграции, первичная настройка, запуск HTTP.

use std::path::PathBuf;

use avtodom_server::config::Config;
use avtodom_server::{MIGRATOR, app, bootstrap, build_state};
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        .init();
    if let Err(e) = run().await {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&config.database_url)
        .await
        .map_err(|e| format!("нет подключения к базе: {e}"))?;
    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| format!("миграции: {e}"))?;
    bootstrap::ensure_owner(&pool, config.bootstrap_owner.clone())
        .await
        .map_err(|e| format!("первичная настройка: {e}"))?;
    let mut state = build_state(pool.clone(), config.cookie_secure)
        .await
        .map_err(|e| e.to_string())?;
    // Телеграм-бот владельца — только если задан ключ (ADR-050).
    if let Some(token) = config.telegram_token.clone() {
        state.telegram_bot = true;
        avtodom_server::api::telegram::spawn(pool, token);
    }
    let web_dir = PathBuf::from(&config.web_dir);
    let web = web_dir
        .join("index.html")
        .exists()
        .then_some(web_dir.as_path());
    if web.is_none() {
        tracing::warn!(dir = %config.web_dir, "клиент не собран, раздаётся только API");
    }
    let listener = tokio::net::TcpListener::bind(&config.bind_addr)
        .await
        .map_err(|e| format!("не удалось занять {}: {e}", config.bind_addr))?;
    tracing::info!(addr = %config.bind_addr, "сервер запущен");
    axum::serve(listener, app(state, web))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}
