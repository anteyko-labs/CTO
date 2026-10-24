//! Все маршруты API `/api/v1`. Каждый модуль отвечает за свою спецификацию — см. заголовок модуля.

pub mod analogs;
pub mod assistant;
pub mod auth;
pub mod bank_qr;
pub mod bank_recon;
pub mod batteries;
pub mod bot_book;
pub mod bot_cars;
pub mod bot_debt;
pub mod bot_drafts;
pub mod bot_reports;
pub mod bot_staff;
pub mod cabinet;
pub mod cancelled_lines;
pub mod cars;
pub mod cash;
pub mod catalog;
pub mod client_messages;
pub mod corrections;
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
pub mod optima;
pub mod paging;
pub mod parties;
pub mod payroll;
pub mod receipt_drafts;
pub mod receipt_files;
pub mod receipts;
pub mod refs;
pub mod reports;
pub mod revisions;
pub mod sales;
pub mod shift_report;
pub mod stock_in;
pub mod supplier_debts;
pub mod supplier_returns;
pub mod telegram;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(analogs::routes())
        .merge(auth::routes())
        .merge(bank_qr::routes())
        .merge(bank_recon::routes())
        .merge(batteries::routes())
        .merge(cabinet::routes())
        .merge(cancelled_lines::routes())
        .merge(cash::routes())
        .merge(cars::routes())
        .merge(catalog::routes())
        .merge(client_messages::routes())
        .merge(corrections::routes())
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
        .merge(optima::routes())
        .merge(parties::routes())
        .merge(payroll::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(receipt_drafts::routes())
        .merge(receipt_files::routes())
        .merge(reports::routes())
        .merge(revisions::routes())
        .merge(sales::routes())
        .merge(shift_report::routes())
        .merge(stock_in::routes())
        .merge(supplier_debts::routes())
        .merge(supplier_returns::routes())
        .merge(telegram::routes())
}
