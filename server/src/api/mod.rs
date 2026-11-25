//! Все маршруты API `/api/v1`. Каждый модуль отвечает за свою спецификацию — см. заголовок модуля.

pub mod acceptances;
pub mod analogs;
pub mod assistant;
pub mod auth;
pub mod bank_qr;
pub mod bank_recon;
pub mod batteries;
pub mod book_settings;
pub mod bot_admin;
pub mod bot_book;
pub mod bot_cars;
pub mod bot_debt;
pub mod bot_debtors;
pub mod bot_drafts;
pub mod bot_feed;
pub mod bot_reports;
pub mod bot_signs;
pub mod bot_staff;
pub mod branches;
pub mod cabinet;
pub mod cancelled_lines;
pub mod cars;
pub mod cash;
pub mod cashier_rights;
pub mod cashier_settings;
pub mod catalog;
pub mod client_messages;
pub mod corrections;
pub mod cross_pick;
pub mod days_off;
pub mod defects;
pub mod deliveries;
pub mod entry_drafts;
pub mod expenses;
pub mod faceid;
pub mod favorites;
pub mod fees;
pub mod gifts;
pub mod income;
pub mod label_jobs;
pub mod legal_drivers;
pub mod legal_garage;
pub mod loyalty;
pub mod my_earnings;
pub mod my_telegram;
pub mod notifications;
pub mod offline;
pub mod oil;
pub mod oil_book;
pub mod old_catalog;
pub mod optima;
pub mod own_use;
pub mod paging;
pub mod parties;
pub mod party_merge;
pub mod pay_fix;
pub mod payroll;
pub mod pnl;
pub mod product_units;
pub mod receipt_drafts;
pub mod receipt_files;
pub mod receipts;
pub mod refs;
pub mod reports;
pub mod revisions;
pub mod sale_approvals;
pub mod sales;
pub mod shift_report;
pub mod spending;
pub mod stock_in;
pub mod supplier_cabinet;
pub mod supplier_debts;
pub mod supplier_line_map;
pub mod supplier_returns;
pub mod telegram;
pub mod umag_intake;
pub mod whatsapp_qr;

use axum::Router;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(acceptances::routes())
        .merge(analogs::routes())
        .merge(auth::routes())
        .merge(bank_qr::routes())
        .merge(bank_recon::routes())
        .merge(batteries::routes())
        .merge(book_settings::routes())
        .merge(bot_admin::routes())
        .merge(bot_feed::routes())
        .merge(branches::routes())
        .merge(cabinet::routes())
        .merge(supplier_cabinet::routes())
        .merge(cancelled_lines::routes())
        .merge(cash::routes())
        .merge(cashier_settings::routes())
        .merge(cashier_rights::routes())
        .merge(cars::routes())
        .merge(catalog::routes())
        .merge(product_units::routes())
        .merge(client_messages::routes())
        .merge(whatsapp_qr::routes())
        .merge(corrections::routes())
        .merge(cross_pick::routes())
        .merge(defects::routes())
        .merge(deliveries::routes())
        .merge(expenses::routes())
        .merge(days_off::routes())
        .merge(faceid::routes())
        .merge(own_use::routes())
        .merge(favorites::routes())
        .merge(label_jobs::routes())
        .merge(fees::routes())
        .merge(gifts::routes())
        .merge(income::routes())
        .merge(loyalty::routes())
        .merge(my_earnings::routes())
        .merge(my_telegram::routes())
        .merge(notifications::routes())
        .merge(offline::routes())
        .merge(oil::routes())
        .merge(oil_book::routes())
        .merge(old_catalog::routes())
        .merge(umag_intake::routes())
        .merge(optima::routes())
        .merge(parties::routes())
        .merge(party_merge::routes())
        .merge(pay_fix::routes())
        .merge(pnl::routes())
        .merge(spending::routes())
        .merge(payroll::routes())
        .merge(refs::routes())
        .merge(receipts::routes())
        .merge(entry_drafts::routes())
        .merge(receipt_drafts::routes())
        .merge(receipt_files::routes())
        .merge(reports::routes())
        .merge(revisions::routes())
        .merge(sale_approvals::routes())
        .merge(legal_garage::routes())
        .merge(legal_drivers::routes())
        .merge(sales::routes())
        .merge(shift_report::routes())
        .merge(stock_in::routes())
        .merge(supplier_debts::routes())
        .merge(supplier_line_map::routes())
        .merge(supplier_returns::routes())
        .merge(telegram::routes())
}
