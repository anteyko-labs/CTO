use std::sync::Arc;

use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub cookie_secure: bool,
    /// Хеш для выравнивания времени ответа при неизвестном логине.
    pub dummy_hash: Arc<String>,
}
