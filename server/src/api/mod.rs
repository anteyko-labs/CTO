pub mod auth;
pub mod catalog;
pub mod receipts;
pub mod refs;
pub mod sales;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(catalog::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(sales::routes())
}
