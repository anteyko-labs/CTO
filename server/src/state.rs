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
    /// Вопросы боту текстом включены (SPEC-21): дневной предел; `None` — выключены.
    pub assistant_daily_limit: Option<i64>,
    /// Внешний сервис по ключу `ASSISTANT_API_KEY`: накладные по фото с экрана «Новый приход»
    /// (SPEC-30 §9); `None` — распознавание не настроено.
    pub assistant: Option<crate::api::assistant::Assistant>,
}
