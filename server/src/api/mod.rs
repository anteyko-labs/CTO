//! Все маршруты API `/api/v1`. Каждый модуль отвечает за свою спецификацию — см. заголовок модуля.

pub mod analogs;
pub mod auth;
pub mod batteries;
pub mod cabinet;
pub mod cash;
pub mod catalog;
pub mod expenses;
pub mod gifts;
pub mod loyalty;
pub mod notifications;
pub mod offline;
pub mod oil;
pub mod oil_book;
pub mod parties;
pub mod payroll;
pub mod receipts;
pub mod refs;
pub mod reports;
pub mod revisions;
pub mod sales;
pub mod telegram;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(analogs::routes())
        .merge(auth::routes())
        .merge(batteries::routes())
        .merge(cabinet::routes())
        .merge(cash::routes())
        .merge(catalog::routes())
        .merge(expenses::routes())
        .merge(gifts::routes())
        .merge(loyalty::routes())
        .merge(notifications::routes())
        .merge(offline::routes())
        .merge(oil::routes())
        .merge(oil_book::routes())
        .merge(parties::routes())
        .merge(payroll::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(reports::routes())
        .merge(revisions::routes())
        .merge(sales::routes())
        .merge(telegram::routes())
}
