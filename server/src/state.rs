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
    /// API банка Optima Business по ключу `OPTIMA_API_KEY` (SPEC-31); `None` — QR на сумму и
    /// сверка с выпиской выключены, касса работает как раньше.
    pub optima: Option<crate::api::optima::Optima>,
    /// Отправка в Телеграм для сообщений клиентам (SPEC-32): есть, когда задан ключ бота.
    pub telegram_api: Option<crate::api::telegram::Api>,
    /// WhatsApp Business Platform (SPEC-32): `WHATSAPP_TOKEN` и `WHATSAPP_PHONE_ID`; `None` — выключен.
    pub whatsapp: Option<crate::api::client_messages::WhatsApp>,
    /// Внешний адрес программы `PUBLIC_URL` — для ссылки на кабинет юрлица в сообщениях.
    pub public_url: Option<String>,
}
