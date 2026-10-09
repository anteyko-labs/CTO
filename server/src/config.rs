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
        Ok(Self {
            database_url,
            bind_addr: env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".into()),
            web_dir: env::var("WEB_DIR").unwrap_or_else(|_| "../web/dist".into()),
            cookie_secure: env::var("COOKIE_SECURE")
                .map(|v| v == "true")
                .unwrap_or(false),
            bootstrap_owner,
            telegram_token: env::var("TELEGRAM_BOT_TOKEN")
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
        })
    }
}
