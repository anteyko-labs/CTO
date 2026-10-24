//! Конфигурация из переменных окружения (docs/tier-3/arch-core.md, раздел «Сервер»).

use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: String,
    pub web_dir: String,
    pub cookie_secure: bool,
    pub bootstrap_owner: Option<(String, String)>,
    /// Ключ бота от @BotFather; без него бот выключен (ADR-050).
    pub telegram_token: Option<String>,
    /// Вопросы владельца боту текстом (SPEC-21): ключ сервиса разбора вопросов; без него выключено.
    pub assistant_key: Option<SecretKey>,
    /// Модель и адрес сервиса; пусто — значения по умолчанию из `api::assistant`.
    pub assistant_model: Option<String>,
    pub assistant_url: Option<String>,
    /// Модель для накладных по фото (SPEC-30 §9); пусто — по умолчанию из `api::assistant`.
    pub assistant_invoice_model: Option<String>,
    /// Сколько вопросов в сутки на филиал.
    pub assistant_daily_limit: i64,
    /// Ключ API Optima Business (SPEC-31, ADR-106): QR на сумму чека и сверка с выпиской;
    /// без него всё это выключено.
    pub optima_key: Option<SecretKey>,
    /// Адрес API банка; пусто — `https://api.optimabusiness.kg`.
    pub optima_url: Option<String>,
    /// WhatsApp Business Platform (SPEC-32, ADR-107): постоянный ключ и ID номера отправителя;
    /// без обоих WhatsApp выключен.
    pub whatsapp_token: Option<SecretKey>,
    pub whatsapp_phone_id: Option<String>,
    /// Адрес Graph API; пусто — `api::client_messages::DEFAULT_WHATSAPP_URL`.
    pub whatsapp_url: Option<String>,
    /// Внешний адрес программы (`https://…`) — ссылка на кабинет юрлица в сообщениях.
    pub public_url: Option<String>,
}

/// Ключ, который не печатается в журнал даже через `{:?}`.
#[derive(Clone)]
pub struct SecretKey(pub String);

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey(***)")
    }
}

/// Вопросов боту в сутки по умолчанию (SPEC-21).
pub const DEFAULT_ASSISTANT_DAILY_LIMIT: i64 = 100;

/// Непустое значение переменной окружения без пробелов по краям.
fn non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let database_url =
            env::var("DATABASE_URL").map_err(|_| "не задан DATABASE_URL".to_string())?;
        let bootstrap_owner = match (
            env::var("BOOTSTRAP_OWNER_LOGIN"),
            env::var("BOOTSTRAP_OWNER_PASSWORD"),
        ) {
            (Ok(l), Ok(p)) if !l.is_empty() && !p.is_empty() => Some((l, p)),
            _ => None,
        };
        let assistant_daily_limit = match non_empty("ASSISTANT_DAILY_LIMIT") {
            None => DEFAULT_ASSISTANT_DAILY_LIMIT,
            Some(v) => v
                .parse::<i64>()
                .ok()
                .filter(|n| *n >= 0)
                .ok_or_else(|| "ASSISTANT_DAILY_LIMIT: нужно целое число от 0".to_string())?,
        };
        Ok(Self {
            database_url,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into()),
            web_dir: env::var("WEB_DIR").unwrap_or_else(|_| "../web/dist".into()),
            cookie_secure: env::var("COOKIE_SECURE")
                .map(|v| v == "true")
                .unwrap_or(false),
            bootstrap_owner,
            telegram_token: non_empty("TELEGRAM_BOT_TOKEN"),
            assistant_key: non_empty("ASSISTANT_API_KEY").map(SecretKey),
            assistant_model: non_empty("ASSISTANT_MODEL"),
            assistant_url: non_empty("ASSISTANT_API_URL"),
            assistant_invoice_model: non_empty("ASSISTANT_INVOICE_MODEL"),
            assistant_daily_limit,
            optima_key: non_empty("OPTIMA_API_KEY").map(SecretKey),
            optima_url: non_empty("OPTIMA_API_URL"),
            whatsapp_token: non_empty("WHATSAPP_TOKEN").map(SecretKey),
            whatsapp_phone_id: non_empty("WHATSAPP_PHONE_ID"),
            whatsapp_url: non_empty("WHATSAPP_API_URL"),
            public_url: non_empty("PUBLIC_URL").map(|u| u.trim_end_matches('/').to_string()),
        })
    }
}
