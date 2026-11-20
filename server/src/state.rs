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
    /// Свои ключи филиалов из `OPTIMA_BRANCH_KEYS` (ADR-206): у филиала здесь — его клиент,
    /// у остальных — общий `optima`.
    pub optima_branches: Arc<std::collections::HashMap<uuid::Uuid, crate::api::optima::Optima>>,
    /// Отправка в Телеграм для сообщений клиентам (SPEC-32): есть, когда задан ключ бота.
    pub telegram_api: Option<crate::api::telegram::Api>,
    /// WhatsApp Business Platform (SPEC-32): `WHATSAPP_TOKEN` и `WHATSAPP_PHONE_ID`; `None` — выключен.
    pub whatsapp: Option<crate::api::client_messages::WhatsApp>,
    /// Внешний адрес программы `PUBLIC_URL` — для ссылки на кабинет юрлица в сообщениях.
    pub public_url: Option<String>,
}

impl AppState {
    /// Клиент банка филиала: свой ключ (`OPTIMA_BRANCH_KEYS`), иначе общий; `None` — выключено.
    pub fn optima_for(&self, branch_id: uuid::Uuid) -> Option<&crate::api::optima::Optima> {
        self.optima_branches
            .get(&branch_id)
            .or(self.optima.as_ref())
    }

    /// Чей ключ у филиала — для «Настройки → Optima».
    pub fn optima_source(&self, branch_id: uuid::Uuid) -> crate::api::optima::KeySource {
        self.banks().source(branch_id)
    }

    /// Все ключи банка — для фонового опроса.
    pub fn banks(&self) -> crate::api::optima::Banks {
        crate::api::optima::Banks {
            shared: self.optima.clone(),
            branches: self.optima_branches.clone(),
        }
    }
}
