//! Общее состояние обработчиков: пул соединений с базой и настройки сессий.

use std::sync::Arc;

use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cookie_secure: bool,
    /// Хеш для выравнивания времени ответа при неизвестном логине.
    pub dummy_hash: Arc<String>,
    /// Телеграм-бот включён: на сервере задан TELEGRAM_BOT_TOKEN (ADR-050).
    pub telegram_bot: bool,
}
