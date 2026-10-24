//! Точка входа сервера: конфигурация, подключение к базе, миграции, первичная настройка, запуск HTTP.

use std::path::PathBuf;
use std::str::FromStr;

use avtodom_server::config::Config;
use avtodom_server::{MIGRATOR, app, bootstrap, build_state};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=info")),
        )
        // В файл журнала (служба на точке) — без цветовых кодов терминала.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();
    if let Err(e) = run().await {
        tracing::error!("{e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let config = Config::from_env()?;
    // Запросы с необязательными условиями (`$1 is null or …`) — план под каждый вызов: общий
    // план подготовленного запроса после пятого вызова читает таблицы целиком (ADR-094).
    let options = PgConnectOptions::from_str(&config.database_url)
        .map_err(|e| format!("адрес базы: {e}"))?
        .options([("plan_cache_mode", "force_custom_plan")]);
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect_with(options)
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
    // Внешний сервис по ключу: вопросы боту (SPEC-21) и накладные по фото (SPEC-30 §9).
    let assistant = config.assistant_key.clone().map(|k| {
        avtodom_server::api::assistant::Assistant::new(
            k.0,
            config.assistant_url.clone(),
            config.assistant_model.clone(),
            config.assistant_daily_limit,
        )
        .with_invoice_model(config.assistant_invoice_model.clone())
    });
    // Накладная по фото с экрана работает и без бота.
    state.assistant = assistant.clone();
    // Optima Business (SPEC-31): QR на сумму чека, опрос статуса и сверка с выпиской — только
    // с ключом банка.
    if let Some(key) = config.optima_key.clone() {
        let optima = avtodom_server::api::optima::Optima::new(key, config.optima_url.clone());
        state.optima = Some(optima.clone());
        avtodom_server::api::optima::spawn(pool.clone(), optima);
    }
    // Телеграм-бот владельца — только если задан ключ (ADR-050).
    if let Some(token) = config.telegram_token.clone() {
        state.telegram_bot = true;
        // Вопросы владельца текстом — только если задан и ключ сервиса разбора (SPEC-21).
        state.assistant_daily_limit = assistant.as_ref().map(|a| a.daily_limit());
        let api = avtodom_server::api::telegram::api_from_env(&token);
        state.telegram_api = Some(api.clone());
        avtodom_server::api::telegram::spawn(pool.clone(), api, assistant);
    }
    // Сообщения клиентам (SPEC-32): Телеграм и WhatsApp Business Platform.
    if let (Some(token), Some(phone_id)) = (
        config.whatsapp_token.clone(),
        config.whatsapp_phone_id.clone(),
    ) {
        state.whatsapp = Some(avtodom_server::api::client_messages::WhatsApp::new(
            token,
            phone_id,
            config.whatsapp_url.clone(),
        ));
    }
    state.public_url = config.public_url.clone();
    avtodom_server::api::client_messages::spawn(pool, &state);
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
    // Адрес соединения нужен пределу попыток входа: заголовкам верим только от своего прокси.
    axum::serve(
        listener,
        app(state, web).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
    .map_err(|e| e.to_string())
}
