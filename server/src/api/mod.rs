//! Все маршруты API `/api/v1`. Каждый модуль отвечает за свою спецификацию — см. заголовок модуля.

pub mod analogs;
pub mod assistant;
pub mod auth;
pub mod batteries;
pub mod bot_cars;
pub mod bot_debt;
pub mod bot_drafts;
pub mod bot_reports;
pub mod bot_staff;
pub mod cabinet;
pub mod cash;
pub mod catalog;
pub mod defects;
pub mod deliveries;
pub mod expenses;
pub mod favorites;
pub mod fees;
pub mod gifts;
pub mod income;
pub mod loyalty;
pub mod notifications;
pub mod offline;
pub mod oil;
pub mod oil_book;
pub mod old_catalog;
pub mod paging;
pub mod parties;
pub mod payroll;
pub mod receipts;
pub mod refs;
pub mod reports;
pub mod revisions;
pub mod sales;
pub mod supplier_debts;
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
        .merge(defects::routes())
        .merge(deliveries::routes())
        .merge(expenses::routes())
        .merge(favorites::routes())
        .merge(fees::routes())
        .merge(gifts::routes())
        .merge(income::routes())
        .merge(loyalty::routes())
        .merge(notifications::routes())
        .merge(offline::routes())
        .merge(oil::routes())
        .merge(oil_book::routes())
        .merge(old_catalog::routes())
        .merge(parties::routes())
        .merge(payroll::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(reports::routes())
        .merge(revisions::routes())
        .merge(sales::routes())
        .merge(supplier_debts::routes())
        .merge(telegram::routes())
}
