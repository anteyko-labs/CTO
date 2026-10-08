//! Все маршруты API `/api/v1`. Каждый модуль отвечает за свою спецификацию — см. заголовок модуля.

pub mod auth;
pub mod cash;
pub mod catalog;
pub mod expenses;
pub mod gifts;
pub mod notifications;
pub mod offline;
pub mod parties;
pub mod payroll;
pub mod receipts;
pub mod refs;
pub mod reports;
pub mod sales;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(cash::routes())
        .merge(catalog::routes())
        .merge(expenses::routes())
        .merge(gifts::routes())
        .merge(notifications::routes())
        .merge(offline::routes())
        .merge(parties::routes())
        .merge(payroll::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(reports::routes())
        .merge(sales::routes())
}
