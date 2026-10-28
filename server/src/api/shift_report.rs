//! Отчёт смены, как в Умаге (SPEC-30 §7): обзор (движение денег кассы смены, выручка по
//! способам оплаты, себестоимость и валовая — только владельцу, сравнение с прошлой сменой,
//! продажи по часам), продажи, возвраты, закупки, денежные операции, долги, выплаты и
//! отменённые товары (§8).
//!
//! Источники те же, что у экрана «Смена» (`cash::load_shift`), чтобы цифры совпадали:
//! движения кассы смены — по времени смены (пересчёт при закрытии не входит), чеки — по
//! времени смены. Всё читается одним снимком базы: остаток кассы и её движения не расходятся
//! из-за чека, проведённого посреди запроса.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Duration, DurationRound, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cancelled_lines::{self, CancelledRow};
use crate::api::cash::{self, ShiftOut};
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::{div_round, format_som, format_som_signed};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/shifts/{id}/report", routing::get(get_report))
        .route(
            "/settings/shift",
            routing::get(get_settings).put(put_settings),
        )
}

/// Настройки смены (SPEC-36 §10): печатать ли чек смены сам после сдачи кассы.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ShiftSettings {
    #[serde(default = "yes")]
    pub print_on_close: bool,
}

fn yes() -> bool {
    true
}

impl Default for ShiftSettings {
    fn default() -> Self {
        Self {
            print_on_close: true,
        }
    }
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<ShiftSettings>> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'shift'",
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(
        v.and_then(|v| serde_json::from_value::<ShiftSettings>(v).ok())
            .unwrap_or_default(),
    ))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ShiftSettings>,
) -> AppResult<Json<ShiftSettings>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let value = serde_json::json!(req);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'shift', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    crate::ops::audit(&mut tx, &ctx, "settings.shift", "settings", None, value).await?;
    tx.commit().await?;
    Ok(Json(req))
}

/// Строка итогов: вид, подпись и сумма.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub key: String,
    pub label: String,
    pub amount_tyiyn: i64,
}

/// Движение наличных кассы смены (SPEC-30 §7, «Обзор»): на начало + приход − расход = должно быть.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CashFlow {
    /// Сколько ждали в кассе при открытии, до пересчёта.
    pub opening_expected_tyiyn: i64,
    /// Остаток на начало смены: пересчитанный при открытии (без пересчёта — ожидаемый).
    pub opening_tyiyn: i64,
    /// Поступление наличных в кассу.
    pub income: Vec<Line>,
    pub income_tyiyn: i64,
    /// Расход наличных из кассы.
    pub outgo: Vec<Line>,
    pub outgo_tyiyn: i64,
    /// Должно быть: у закрытой смены — при закрытии, у открытой — сейчас.
    pub expected_tyiyn: i64,
    /// Пересчитали при закрытии; у открытой смены — пусто.
    pub counted_tyiyn: Option<i64>,
    /// Излишек (+) или недостача (−).
    pub diff_tyiyn: Option<i64>,
    /// После закрытия: сколько сдали и куда, сколько осталось в кассе.
    pub handover_tyiyn: Option<i64>,
    pub handover_to: Option<String>,
    pub left_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MethodSum {
    /// Способ оплаты: cash, card, transfer, invoice, debt, bonus, battery.
    pub method: String,
    pub amount_tyiyn: i64,
}

/// Выручка смены: продажи − возвраты = итог = сумма по способам оплаты.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Revenue {
    pub sales_count: i64,
    pub sales_tyiyn: i64,
    pub returns_count: i64,
    /// Возвращено покупателям, положительное.
    pub returns_tyiyn: i64,
    pub net_tyiyn: i64,
    /// По способам оплаты, уже с учётом возвратов.
    pub methods: Vec<MethodSum>,
    /// Итог минус сумма по способам; не ноль — значит, есть чеки, которые не сходятся.
    pub rest_tyiyn: i64,
    pub bank_fee_tyiyn: i64,
    /// Только владельцу (инвариант 6): себестоимость проданного и возвращённого, валовая.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_sold_tyiyn: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_returned_tyiyn: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tyiyn: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gross_tyiyn: Option<i64>,
    /// Строк, проданных без цены закупки (их себестоимость — ноль, ADR-062).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_unknown_lines: Option<i64>,
    /// Только владельцу (ADR-120): чистая прибыль за смену — формула прибыли дня по времени
    /// смены, без оклада.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_profit_tyiyn: Option<i64>,
    /// Только владельцу: из чего она сложилась, со знаком (валовая, начисления, комиссия…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profit: Option<Vec<Line>>,
}

/// Выручка прошлой смены этой кассы и разница с ней.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Compare {
    pub shift_id: Uuid,
    pub number: i64,
    pub net_tyiyn: i64,
    pub diff_tyiyn: i64,
    /// Разница в сотых долях процента от прошлой выручки; прошлая — ноль: пусто.
    pub change_bp: Option<i64>,
    /// Смена ещё идёт: прошлая взята за то же время от открытия, а не целиком.
    #[serde(default)]
    pub same_time: bool,
}

/// Час смены (начало часа): продажи и возвраты.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct HourRow {
    pub at: DateTime<Utc>,
    pub sales_tyiyn: i64,
    pub returns_tyiyn: i64,
    pub count: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayRow {
    pub method: String,
    pub amount_tyiyn: i64,
    /// Куда легли деньги: касса, безналичный счёт; у долга и баллов — пусто.
    pub account_name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SaleRow {
    pub id: Uuid,
    pub number: i64,
    /// `sale` — продажа, `return` — возврат.
    pub kind: String,
    pub sale_type: String,
    pub created_at: DateTime<Utc>,
    pub cashier_name: String,
    pub user_name: String,
    pub party_name: Option<String>,
    pub total_tyiyn: i64,
    pub reversal_of: Option<Uuid>,
    pub reversal_of_number: Option<i64>,
    pub payments: Vec<PayRow>,
}

/// Итоги вкладки «Продажи»: продано, получено деньгами, баллами, в долг.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SalesTotals {
    pub count: i64,
    pub sales_tyiyn: i64,
    pub paid_tyiyn: i64,
    pub bonus_tyiyn: i64,
    pub debt_tyiyn: i64,
}

/// Итоги вкладки «Возвраты»: сколько и по способам, положительными.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ReturnsTotals {
    pub count: i64,
    pub total_tyiyn: i64,
    pub methods: Vec<MethodSum>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReceiptRow {
    pub id: Uuid,
    pub number: i64,
    pub created_at: DateTime<Utc>,
    pub supplier_name: Option<String>,
    pub supplier_doc: String,
    pub total_tyiyn: i64,
    /// Сторно накладной: ссылка на исходную.
    pub reversal_of: Option<Uuid>,
    pub user_name: String,
    /// Оплачено из кассы смены (у сторно — вернулось в неё, с минусом).
    pub paid_till_tyiyn: i64,
    /// Оплачено с других счетов.
    pub paid_other_tyiyn: i64,
    pub paid_account_name: Option<String>,
    /// Взято в долг у поставщика.
    pub on_debt_tyiyn: i64,
}

/// Денежная операция за время смены по всем счетам (кроме чеков и комиссии банка).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MoneyRow {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    /// Вид: cash_in, cash_out, transfers_in, transfers_out, transfers, expenses, payouts,
    /// supplier_payments, debt_repayments, supplier_refunds, battery_intake, other.
    pub kind: String,
    pub label: String,
    /// Сторно (отмена) операции.
    pub reversal: bool,
    /// Со знаком: + на счёт, − со счёта; у перевода — относительно кассы смены.
    pub amount_tyiyn: i64,
    pub account_name: String,
    /// Перевод: второй счёт.
    pub to_account_name: Option<String>,
    pub user_name: String,
    /// Контрагент, сотрудник или статья расхода.
    pub counterparty: Option<String>,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DebtRow {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    /// `sale` — продажа в долг, `return` — возврат такого чека, `repayment` — оплата долга,
    /// `adjust` — правка долга владельцем.
    pub kind: String,
    pub reversal: bool,
    pub party_id: Uuid,
    pub party_name: String,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
    /// + долг вырос, − уменьшился.
    pub amount_tyiyn: i64,
    pub account_name: Option<String>,
    pub user_name: String,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayoutRow {
    pub id: Uuid,
    pub number: i64,
    pub created_at: DateTime<Utc>,
    pub employee_name: String,
    /// У сторно — с минусом.
    pub amount_tyiyn: i64,
    /// За что (ADR-122): `percent`, `service_fee`, `bonus`, `salary`, `advance`, у прежних — `general`.
    pub purpose: String,
    pub comment: String,
    pub account_name: Option<String>,
    pub reversal: bool,
    pub user_name: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ShiftReport {
    pub shift: ShiftOut,
    /// Отчёт собран для владельца: есть себестоимость и валовая.
    pub owner_view: bool,
    pub cash: CashFlow,
    pub revenue: Revenue,
    pub compare: Option<Compare>,
    pub hours: Vec<HourRow>,
    pub sales: Vec<SaleRow>,
    pub sales_totals: SalesTotals,
    pub returns_totals: ReturnsTotals,
    pub receipts: Vec<ReceiptRow>,
    pub money: Vec<MoneyRow>,
    pub money_totals: Vec<Line>,
    pub debts: Vec<DebtRow>,
    pub payouts: Vec<PayoutRow>,
    pub payouts_tyiyn: i64,
    pub cancelled: Vec<CancelledRow>,
    pub cancelled_tyiyn: i64,
    /// Оплачено по QR банка за смену, а чека с этой оплатой нет (SPEC-31).
    #[serde(default)]
    pub qr_unclaimed: Vec<crate::api::bank_qr::UnclaimedQr>,
    /// Переносы «из наличных в QR» этой смены и их отмены (SPEC-37 §3).
    #[serde(default)]
    pub transfers: Vec<crate::api::pay_fix::TransferRow>,
    /// Изменения после пересчёта наличных этой смены (SPEC-37 §4): блок «После закрытия».
    #[serde(default)]
    pub after_close: Option<crate::api::pay_fix::AfterClose>,
}

/// Вид движения наличных и сторона: приход (`true`) или расход.
const FLOW: [(&str, &str, bool); 21] = [
    ("cash_sales", "Продажи наличными", true),
    ("pay_fix_in", "Изменение оплаты чеков", true),
    ("debt_repayments", "Оплаты долга покупателей", true),
    ("cash_in", "Внесения", true),
    ("transfers_in", "Переводы в кассу", true),
    ("supplier_refunds", "Деньги от поставщиков", true),
    ("count_plus", "Излишек при прошлом пересчёте", true),
    ("other_in", "Прочие поступления", true),
    ("supplier_payments", "Оплаты накладных и поставщикам", false),
    ("sale_returns", "Возвраты покупателям", false),
    ("expenses", "Расходы", false),
    ("payouts", "Выплаты сотрудникам", false),
    ("advances", "Авансы", false),
    ("cash_out", "Изъятия", false),
    ("transfers_out", "Переводы из кассы", false),
    ("battery_intake", "Приём аккумуляторов", false),
    ("count_minus", "Недостача при прошлом пересчёте", false),
    ("pay_fix_out", "Изменение оплаты чеков", false),
    ("cash_to_bank", "Из наличных в QR", false),
    ("other_out", "Прочие списания", false),
    ("transfers", "Переводы между счетами", false),
];

/// Строки движения денег, которые видны всегда, даже нулевые (как в Умаге).
const ALWAYS: [&str; 10] = [
    "cash_sales",
    "debt_repayments",
    "cash_in",
    "transfers_in",
    "supplier_payments",
    "sale_returns",
    "expenses",
    "payouts",
    "cash_out",
    "transfers_out",
];

/// Вид движения наличных для отчёта: сторно попадает в вид того, что отменяет, — расход и его
/// сторно в одной смене дают ноль.
fn flow_key(kind: &str, doc_type: &str, orig_kind: Option<&str>, amount: i64) -> &'static str {
    let by_sign = |plus: &'static str, minus: &'static str| if amount >= 0 { plus } else { minus };
    match kind {
        "sale" => "cash_sales",
        "sale_return" => "sale_returns",
        "debt_repayment" => "debt_repayments",
        "cash_in" => "cash_in",
        "cash_out" => "cash_out",
        "transfer_in" => "transfers_in",
        "transfer_out" => "transfers_out",
        "supplier_refund" => "supplier_refunds",
        "supplier_payment" => "supplier_payments",
        "expense" => "expenses",
        "payout" => "payouts",
        "battery_intake" => "battery_intake",
        "cash_to_bank" => "cash_to_bank",
        "count_diff" => by_sign("count_plus", "count_minus"),
        "reversal" => match doc_type {
            "cash_movement" => match orig_kind {
                Some("cash_in") => "cash_in",
                Some("cash_out") => "cash_out",
                _ => by_sign("other_in", "other_out"),
            },
            // Сторно перевода: деньги вернулись туда, откуда ушли.
            "transfer" => by_sign("transfers_out", "transfers_in"),
            "expense" => "expenses",
            "payout" => "payouts",
            "receipt_reversal" => "supplier_payments",
            // Сторно оплаты долга покупателя забирает деньги, оплаты поставщику — возвращает.
            "ledger_reversal" => by_sign("supplier_payments", "debt_repayments"),
            "battery_intake_reversal" => "battery_intake",
            "defect_reversal" | "supplier_return_reversal" => "supplier_refunds",
            _ => by_sign("other_in", "other_out"),
        },
        _ => by_sign("other_in", "other_out"),
    }
}

/// Аванс — отдельной строкой от «Выплат сотрудникам» (ADR-122), сторно аванса — туда же.
fn advance_key(key: &'static str, payout_purpose: Option<&str>) -> &'static str {
    if key == "payouts" && payout_purpose == Some("advance") {
        "advances"
    } else {
        key
    }
}

fn is_income(key: &str) -> bool {
    FLOW.iter().any(|(k, _, inc)| *k == key && *inc)
}

/// Способы оплаты в порядке экрана; остальные — после, по алфавиту.
const METHOD_ORDER: [&str; 7] = [
    "cash", "card", "transfer", "invoice", "debt", "bonus", "battery",
];

fn method_rank(m: &str) -> usize {
    METHOD_ORDER
        .iter()
        .position(|x| *x == m)
        .unwrap_or(METHOD_ORDER.len())
}

pub fn method_label(m: &str) -> &str {
    match m {
        "cash" => "наличные",
        "card" => "карта / QR банка",
        "transfer" => "QR владельцу",
        "invoice" => "по счёту",
        "debt" => "в долг",
        "bonus" => "баллами",
        "battery" => "аккумулятором",
        other => other,
    }
}

fn sorted_methods(by: HashMap<String, i64>) -> Vec<MethodSum> {
    let mut out: Vec<MethodSum> = by
        .into_iter()
        .filter(|(_, v)| *v != 0)
        .map(|(method, amount_tyiyn)| MethodSum {
            method,
            amount_tyiyn,
        })
        .collect();
    out.sort_by(|a, b| {
        method_rank(&a.method)
            .cmp(&method_rank(&b.method))
            .then_with(|| a.method.cmp(&b.method))
    });
    out
}

fn add(map: &mut HashMap<String, i64>, key: &str, v: i64) -> AppResult<()> {
    let e = map.entry(key.to_string()).or_insert(0);
    *e = e.checked_add(v).ok_or_else(crate::error::overflow)?;
    Ok(())
}

fn sum<'a>(it: impl IntoIterator<Item = &'a i64>) -> AppResult<i64> {
    it.into_iter()
        .try_fold(0i64, |a, v| a.checked_add(*v))
        .ok_or_else(crate::error::overflow)
}

/// Время смены: с открытия до последнего действующего закрытия (у открытой — до сейчас).
struct Window {
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
}

/// Отчёт смены. Администратору — без себестоимости и валовой (инвариант 6), без счетов
/// владельца и его личных расходов.
pub async fn load(
    conn: &mut PgConnection,
    branch_id: Uuid,
    owner: bool,
    id: Uuid,
) -> AppResult<ShiftReport> {
    let shift = cash::load_shift(conn, branch_id, id).await?;
    let w = Window {
        start: shift.opened_at,
        end: shift.closed_at,
    };
    let cash = cash_flow(conn, &shift, &w).await?;
    let (sales, revenue, sales_totals, returns_totals) =
        sales_part(conn, branch_id, owner, &shift, &w).await?;
    let compare = compare(conn, branch_id, &shift, revenue.net_tyiyn).await?;
    let hours = hours(conn, branch_id, &w).await?;
    let receipts = receipts(conn, branch_id, &shift, &w).await?;
    let (money, money_totals) = money(conn, branch_id, owner, &shift, &w).await?;
    let debts = debts(conn, branch_id, &w).await?;
    let payouts = payouts(conn, branch_id, owner, &w).await?;
    let payouts_tyiyn = sum(payouts.iter().map(|p| &p.amount_tyiyn))?;
    let cancelled = cancelled_lines::of_shift(conn, id).await?;
    let qr_unclaimed =
        crate::api::bank_qr::unclaimed_between(conn, branch_id, w.start, w.end).await?;
    let cancelled_tyiyn = sum(cancelled.iter().map(|c| &c.amount_tyiyn))?;
    let transfers = crate::api::pay_fix::transfers_of(conn, id).await?;
    let after_close = crate::api::pay_fix::after_close(
        conn,
        branch_id,
        owner,
        &shift,
        shift.cash_sales_tyiyn,
        revenue.net_profit_tyiyn,
    )
    .await?;
    Ok(ShiftReport {
        shift,
        owner_view: owner,
        cash,
        revenue,
        compare,
        hours,
        sales,
        sales_totals,
        returns_totals,
        receipts,
        money,
        money_totals,
        debts,
        payouts,
        payouts_tyiyn,
        cancelled,
        cancelled_tyiyn,
        qr_unclaimed,
        transfers,
        after_close,
    })
}

/// Движение наличных кассы смены: те же движения, что разбивка экрана «Смена».
async fn cash_flow(conn: &mut PgConnection, s: &ShiftOut, w: &Window) -> AppResult<CashFlow> {
    let rows = sqlx::query!(
        r#"select m.kind, m.doc_type, m.doc_id, m.amount_tyiyn, m.created_at,
                  (select o.kind from cash_movements o
                   where m.doc_type = 'cash_movement' and o.id = m.doc_id) as "orig_kind?",
                  (select p.purpose from payouts p
                   where m.doc_type = 'payout' and p.id = m.doc_id) as "payout_purpose?",
                  (m.doc_type = 'sale' and exists (
                     select 1 from pay_fixes f
                     where f.counted_shift_id is null
                       and (f.reversal_sale_id = m.doc_id or f.new_sale_id = m.doc_id))) as "pay_fix!"
           from cash_movements m
           where m.account_id = $1 and m.created_at >= $2
             and (m.created_at < coalesce($3, 'infinity'::timestamptz)
                  or (m.created_at = $3 and m.kind <> 'count_diff'))
             and not public.pay_fix_counted_move(m.id, m.doc_type, m.doc_id)"#,
        s.account_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    // Пересчёт при открытии — часть остатка на начало: на начало — то, что пересчитали.
    let mut opening = s.opening_expected_tyiyn;
    let mut by: HashMap<String, i64> = HashMap::new();
    for r in &rows {
        if r.kind == "count_diff"
            && r.doc_type == "shift"
            && r.doc_id == Some(s.id)
            && r.created_at == s.opened_at
        {
            opening = opening
                .checked_add(r.amount_tyiyn)
                .ok_or_else(crate::error::overflow)?;
            continue;
        }
        // Изменение оплаты чека, чьи наличные ещё не пересчитаны, — своей строкой, а не
        // продажами и возвратами (SPEC-37 §4).
        let key = if r.pay_fix {
            if r.amount_tyiyn >= 0 {
                "pay_fix_in"
            } else {
                "pay_fix_out"
            }
        } else {
            advance_key(
                flow_key(&r.kind, &r.doc_type, r.orig_kind.as_deref(), r.amount_tyiyn),
                r.payout_purpose.as_deref(),
            )
        };
        add(&mut by, key, r.amount_tyiyn)?;
    }
    // «Должно быть» — то же, что на экране «Смена»; если движения разошлись с ним (гонка при
    // записи), разница видна отдельной строкой, а не прячется.
    let flowed = opening
        .checked_add(sum(by.values())?)
        .ok_or_else(crate::error::overflow)?;
    let rest = s
        .expected_tyiyn
        .checked_sub(flowed)
        .ok_or_else(crate::error::overflow)?;
    if rest != 0 {
        add(
            &mut by,
            if rest > 0 { "other_in" } else { "other_out" },
            rest,
        )?;
    }
    let mut income = Vec::new();
    let mut outgo = Vec::new();
    for (key, label, inc) in FLOW {
        let raw = by.get(key).copied().unwrap_or(0);
        if raw == 0 && !ALWAYS.contains(&key) {
            continue;
        }
        let line = Line {
            key: key.into(),
            label: label.into(),
            amount_tyiyn: if inc { raw } else { -raw },
        };
        if inc {
            income.push(line);
        } else {
            outgo.push(line);
        }
    }
    let income_tyiyn = sum(income.iter().map(|l| &l.amount_tyiyn))?;
    let outgo_tyiyn = sum(outgo.iter().map(|l| &l.amount_tyiyn))?;
    let handover_to = sqlx::query_scalar!(
        r#"select a.name from shift_handovers h
           join shift_closes c on c.id = h.close_id
           join cash_transfers t on t.id = h.transfer_id
           join cash_accounts a on a.id = t.to_account_id
           where h.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
             and not exists (select 1 from cash_transfers x where x.reversal_of = t.id)
           order by c.seq desc limit 1"#,
        s.id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(CashFlow {
        opening_expected_tyiyn: s.opening_expected_tyiyn,
        opening_tyiyn: opening,
        income,
        income_tyiyn,
        outgo,
        outgo_tyiyn,
        expected_tyiyn: s.expected_tyiyn,
        counted_tyiyn: s.counted_tyiyn,
        diff_tyiyn: s.diff_tyiyn,
        handover_tyiyn: s.to_safe_tyiyn,
        handover_to,
        left_tyiyn: s.left_tyiyn,
    })
}

/// Чеки смены (те же, что «Продажи смены» на экране «Смена»), выручка и итоги вкладок.
async fn sales_part(
    conn: &mut PgConnection,
    branch_id: Uuid,
    owner: bool,
    s: &ShiftOut,
    w: &Window,
) -> AppResult<(Vec<SaleRow>, Revenue, SalesTotals, ReturnsTotals)> {
    let heads = sqlx::query!(
        r#"select s.id, s.number, s.kind, s.sale_type, s.created_at, c.full_name as cashier_name,
                  u.full_name as user_name, pt.name as "party_name?", s.total_tyiyn, s.reversal_of,
                  o.number as "reversal_of_number?"
           from sales s
           join employees c on c.id = s.cashier_id
           join users u on u.id = s.user_id
           left join parties pt on pt.id = s.party_id
           left join sales o on o.id = s.reversal_of
           where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
             and not public.pay_fix_counted_sale(s.id)
           order by s.created_at desc, s.number desc"#,
        branch_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    let pays = sqlx::query!(
        r#"select p.sale_id, p.method, p.amount_tyiyn, a.name as "account_name?"
           from sale_payments p
           join sales s on s.id = p.sale_id
           left join cash_accounts a on a.id = p.account_id
           where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
             and not public.pay_fix_counted_sale(s.id)
           order by p.method, a.name"#,
        branch_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    let kinds: HashMap<Uuid, bool> = heads.iter().map(|h| (h.id, h.kind == "sale")).collect();
    let mut by_sale: HashMap<Uuid, Vec<PayRow>> = HashMap::new();
    let mut methods: HashMap<String, i64> = HashMap::new();
    let mut returned: HashMap<String, i64> = HashMap::new();
    let mut totals = SalesTotals::default();
    for p in pays {
        add(&mut methods, &p.method, p.amount_tyiyn)?;
        let sale = kinds.get(&p.sale_id).copied().unwrap_or(true);
        if sale {
            match p.method.as_str() {
                "bonus" => totals.bonus_tyiyn = totals.bonus_tyiyn.saturating_add(p.amount_tyiyn),
                "debt" => totals.debt_tyiyn = totals.debt_tyiyn.saturating_add(p.amount_tyiyn),
                _ => totals.paid_tyiyn = totals.paid_tyiyn.saturating_add(p.amount_tyiyn),
            }
        } else {
            add(&mut returned, &p.method, -p.amount_tyiyn)?;
        }
        // Наличные чека ложатся в кассу смены; у долга, баллов и аккумулятора счёта нет.
        let account_name = match p.method.as_str() {
            "cash" => Some(s.account_name.clone()),
            _ => p.account_name,
        };
        by_sale.entry(p.sale_id).or_default().push(PayRow {
            method: p.method,
            amount_tyiyn: p.amount_tyiyn,
            account_name,
        });
    }
    let mut rows = Vec::with_capacity(heads.len());
    let mut returns = ReturnsTotals::default();
    for h in heads {
        if h.kind == "sale" {
            totals.count += 1;
            totals.sales_tyiyn = totals.sales_tyiyn.saturating_add(h.total_tyiyn);
        } else {
            returns.count += 1;
            returns.total_tyiyn = returns.total_tyiyn.saturating_sub(h.total_tyiyn);
        }
        let mut payments = by_sale.remove(&h.id).unwrap_or_default();
        payments.sort_by_key(|p| method_rank(&p.method));
        rows.push(SaleRow {
            id: h.id,
            number: h.number,
            kind: h.kind,
            sale_type: h.sale_type,
            created_at: h.created_at,
            cashier_name: h.cashier_name,
            user_name: h.user_name,
            party_name: h.party_name,
            total_tyiyn: h.total_tyiyn,
            reversal_of: h.reversal_of,
            reversal_of_number: h.reversal_of_number,
            payments,
        });
    }
    returns.methods = sorted_methods(returned);
    let methods = sorted_methods(methods);
    let net = totals
        .sales_tyiyn
        .checked_sub(returns.total_tyiyn)
        .ok_or_else(crate::error::overflow)?;
    let rest = net
        .checked_sub(sum(methods.iter().map(|m| &m.amount_tyiyn))?)
        .ok_or_else(crate::error::overflow)?;
    let mut revenue = Revenue {
        sales_count: totals.count,
        sales_tyiyn: totals.sales_tyiyn,
        returns_count: returns.count,
        returns_tyiyn: returns.total_tyiyn,
        net_tyiyn: net,
        methods,
        rest_tyiyn: rest,
        bank_fee_tyiyn: s.bank_fee_tyiyn,
        cost_sold_tyiyn: None,
        cost_returned_tyiyn: None,
        cost_tyiyn: None,
        gross_tyiyn: None,
        cost_unknown_lines: None,
        net_profit_tyiyn: None,
        profit: None,
    };
    // Себестоимость и валовая — только владельцу (инвариант 6): администратору их не считаем.
    if owner {
        let c = sqlx::query!(
            r#"select coalesce(sum(l.cost_tyiyn) filter (where s.kind = 'sale'), 0)::bigint as "sold!",
                      coalesce(-sum(l.cost_tyiyn) filter (where s.kind = 'return'), 0)::bigint as "returned!",
                      count(*) filter (where s.kind = 'sale' and l.cost_unknown) as "unknown!"
               from sale_lines l join sales s on s.id = l.sale_id
               where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
                 and not public.pay_fix_counted_sale(s.id)"#,
            branch_id,
            w.start,
            w.end
        )
        .fetch_one(&mut *conn)
        .await?;
        let cost = c
            .sold
            .checked_sub(c.returned)
            .ok_or_else(crate::error::overflow)?;
        revenue.cost_sold_tyiyn = Some(c.sold);
        revenue.cost_returned_tyiyn = Some(c.returned);
        revenue.cost_tyiyn = Some(cost);
        revenue.gross_tyiyn = Some(net.checked_sub(cost).ok_or_else(crate::error::overflow)?);
        revenue.cost_unknown_lines = Some(c.unknown);
        let t = crate::api::reports::totals_between(conn, branch_id, s.id, w.start, w.end).await?;
        revenue.net_profit_tyiyn = Some(t.net_tyiyn);
        revenue.profit = Some(profit_lines(&t));
    }
    Ok((rows, revenue, totals, returns))
}

/// Слагаемые чистой прибыли смены со знаком; нулевые, кроме валовой, не показываем. Сумма строк —
/// чистая прибыль.
fn profit_lines(t: &crate::api::reports::Totals) -> Vec<Line> {
    let gross = t.gross_tyiyn.saturating_add(t.revaluation_tyiyn);
    let parts: [(&str, &str, i64); 10] = [
        ("gross", "Валовая прибыль", gross),
        ("revaluation", "Переоценка и ревизия", -t.revaluation_tyiyn),
        (
            "payroll",
            "Начисления сотрудникам (без оклада)",
            -t.payroll_tyiyn,
        ),
        ("bank_fee", "Комиссия банка", -t.bank_fee_tyiyn),
        ("expenses", "Расходы", -t.expenses_tyiyn),
        ("bonus", "Оплачено баллами", -t.bonus_tyiyn),
        (
            "cash_diff",
            "Недостача или излишек кассы",
            t.cash_diff_tyiyn,
        ),
        (
            "defect_writeoff",
            "Списанный брак",
            -t.defect_writeoff_tyiyn,
        ),
        (
            "defect_diff",
            "Брак: разница возмещения",
            t.defect_diff_tyiyn,
        ),
        (
            "return_diff",
            "Возврат поставщику: разница",
            t.return_diff_tyiyn,
        ),
    ];
    parts
        .into_iter()
        .filter(|(k, _, v)| *k == "gross" || *v != 0)
        .map(|(key, label, amount_tyiyn)| Line {
            key: key.into(),
            label: label.into(),
            amount_tyiyn,
        })
        .collect()
}

/// Конец действующего закрытия смены; открытая — `None`.
async fn close_time(conn: &mut PgConnection, shift_id: Uuid) -> AppResult<Option<DateTime<Utc>>> {
    Ok(sqlx::query_scalar!(
        r#"select c.closed_at from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        shift_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

/// Выручка прошлой смены этой кассы (SPEC-30 §7, «Сравнение»).
async fn compare(
    conn: &mut PgConnection,
    branch_id: Uuid,
    s: &ShiftOut,
    net: i64,
) -> AppResult<Option<Compare>> {
    let Some(prev) = sqlx::query!(
        r#"select id, number, opened_at from shifts
           where branch_id = $1 and account_id = $2 and opened_at < $3
           order by opened_at desc limit 1"#,
        branch_id,
        s.account_id,
        s.opened_at
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    // Прошлая смена кончилась не позже открытия этой: чеки после — уже этой смены.
    let mut end = close_time(conn, prev.id).await?.unwrap_or(s.opened_at);
    // Эта смена ещё идёт — прошлую берём за столько же времени от её открытия: полдня против
    // целой смены не сравнивают.
    let same_time = s.closed_at.is_none();
    if same_time {
        let elapsed = Utc::now().signed_duration_since(s.opened_at);
        if let Some(cut) = prev.opened_at.checked_add_signed(elapsed) {
            end = end.min(cut);
        }
    }
    let prev_net = sqlx::query_scalar!(
        r#"select coalesce(sum(total_tyiyn), 0)::bigint as "v!" from sales
           where branch_id = $1 and created_at >= $2 and created_at <= $3"#,
        branch_id,
        prev.opened_at,
        end
    )
    .fetch_one(&mut *conn)
    .await?;
    let diff = net
        .checked_sub(prev_net)
        .ok_or_else(crate::error::overflow)?;
    let change_bp = if prev_net == 0 {
        None
    } else {
        div_round(i128::from(diff) * 10_000, i128::from(prev_net).abs())
    };
    Ok(Some(Compare {
        shift_id: prev.id,
        number: prev.number,
        net_tyiyn: prev_net,
        diff_tyiyn: diff,
        change_bp,
        same_time,
    }))
}

/// Продажи смены по часам: каждый час от открытия до закрытия (или до сейчас), без пропусков.
async fn hours(conn: &mut PgConnection, branch_id: Uuid, w: &Window) -> AppResult<Vec<HourRow>> {
    let rows = sqlx::query!(
        r#"select date_trunc('hour', s.created_at) as "at!",
                  coalesce(sum(s.total_tyiyn) filter (where s.kind = 'sale'), 0)::bigint as "sales!",
                  coalesce(-sum(s.total_tyiyn) filter (where s.kind = 'return'), 0)::bigint as "returns!",
                  count(*) filter (where s.kind = 'sale') as "count!"
           from sales s
           where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
             and not public.pay_fix_counted_sale(s.id)
           group by 1 order by 1"#,
        branch_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    let now = sqlx::query_scalar!(r#"select now() as "t!""#)
        .fetch_one(&mut *conn)
        .await?;
    let hour = Duration::hours(1);
    let trunc = |t: DateTime<Utc>| t.duration_trunc(hour).unwrap_or(t);
    let first = trunc(w.start);
    let last = trunc(w.end.unwrap_or(now)).max(first);
    let by: HashMap<DateTime<Utc>, HourRow> = rows
        .into_iter()
        .map(|r| {
            (
                r.at,
                HourRow {
                    at: r.at,
                    sales_tyiyn: r.sales,
                    returns_tyiyn: r.returns,
                    count: r.count,
                },
            )
        })
        .collect();
    // Смена длиннее двух суток — столбики только с часами продаж, без пустых.
    if last - first > Duration::hours(48) {
        let mut v: Vec<HourRow> = by.into_values().collect();
        v.sort_by_key(|h| h.at);
        return Ok(v);
    }
    let mut out = Vec::new();
    let mut at = first;
    while at <= last {
        out.push(by.get(&at).cloned().unwrap_or(HourRow {
            at,
            sales_tyiyn: 0,
            returns_tyiyn: 0,
            count: 0,
        }));
        at += hour;
    }
    Ok(out)
}

/// Приходы от поставщиков за время смены и сколько по ним заплачено из кассы смены.
async fn receipts(
    conn: &mut PgConnection,
    branch_id: Uuid,
    s: &ShiftOut,
    w: &Window,
) -> AppResult<Vec<ReceiptRow>> {
    Ok(sqlx::query_as!(
        ReceiptRow,
        r#"select r.id, r.number, r.created_at, sp.name as "supplier_name?", r.supplier_doc,
                  r.total_tyiyn, r.reversal_of, u.full_name as user_name,
                  coalesce((select -sum(m.amount_tyiyn) from cash_movements m
                            where m.doc_id = r.id and m.doc_type in ('receipt', 'receipt_reversal')
                              and m.account_id = $4), 0)::bigint as "paid_till_tyiyn!",
                  coalesce((select -sum(m.amount_tyiyn) from cash_movements m
                            where m.doc_id = r.id and m.doc_type in ('receipt', 'receipt_reversal')
                              and m.account_id <> $4), 0)::bigint as "paid_other_tyiyn!",
                  (select a.name from cash_accounts a where a.id = r.paid_account_id) as "paid_account_name?",
                  abs(coalesce((select sum(l.amount_tyiyn) from party_ledger l
                                where l.doc_type = 'receipt' and l.doc_id = r.id), 0))::bigint as "on_debt_tyiyn!"
           from receipts r
           join users u on u.id = r.user_id
           left join suppliers sp on sp.id = r.supplier_id
           where r.branch_id = $1 and r.created_at >= $2 and r.created_at <= coalesce($3, now())
           order by r.created_at, r.number"#,
        branch_id,
        w.start,
        w.end,
        s.account_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Денежные операции за время смены по всем счетам: внесения, изъятия, переводы, расходы,
/// выплаты, оплаты долга и поставщикам. Чеки — во вкладках «Продажи» и «Возвраты».
async fn money(
    conn: &mut PgConnection,
    branch_id: Uuid,
    owner: bool,
    s: &ShiftOut,
    w: &Window,
) -> AppResult<(Vec<MoneyRow>, Vec<Line>)> {
    let moves = sqlx::query!(
        r#"select m.id, m.created_at, m.kind, m.doc_type, m.amount_tyiyn, m.comment,
                  a.name as account_name, u.full_name as user_name,
                  (select o.kind from cash_movements o
                   where m.doc_type = 'cash_movement' and o.id = m.doc_id) as "orig_kind?",
                  (select o.comment from cash_movements o
                   where m.doc_type = 'cash_movement' and o.id = m.doc_id) as "orig_comment?",
                  ea.name as "article?", ea.owner_only as "article_owner_only?",
                  (select emp.full_name from payouts p join employees emp on emp.id = p.employee_id
                   where m.doc_type = 'payout' and p.id = m.doc_id) as "employee?",
                  (select p.purpose from payouts p
                   where m.doc_type = 'payout' and p.id = m.doc_id) as "payout_purpose?",
                  (select pt.name from party_ledger l join parties pt on pt.id = l.party_id
                   where (m.doc_type = 'repayment' and l.doc_type = 'repayment' and l.doc_id = m.doc_id)
                      or (m.doc_type = 'ledger_reversal' and l.id = m.doc_id)
                   limit 1) as "party?",
                  (select sp.name from receipts r join suppliers sp on sp.id = r.supplier_id
                   where m.doc_type in ('receipt', 'receipt_reversal') and r.id = m.doc_id) as "supplier?"
           from cash_movements m
           join cash_accounts a on a.id = m.account_id
           join users u on u.id = m.user_id
           left join expenses e on m.doc_type = 'expense' and e.id = m.doc_id
           left join expense_articles ea on ea.id = e.article_id
           where m.branch_id = $1 and m.created_at >= $2 and m.created_at <= coalesce($3, now())
             and m.kind not in ('sale', 'sale_return', 'bank_fee', 'count_diff', 'transfer_in', 'transfer_out',
                                'cash_to_bank')
             and m.doc_type not in ('transfer', 'pay_fix')
             and (not a.owner_only or $4)
           order by m.created_at, m.id"#,
        branch_id,
        w.start,
        w.end,
        owner
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<MoneyRow> = Vec::with_capacity(moves.len());
    for m in moves {
        let key = advance_key(
            flow_key(&m.kind, &m.doc_type, m.orig_kind.as_deref(), m.amount_tyiyn),
            m.payout_purpose.as_deref(),
        );
        let key = match key {
            "count_plus" | "count_minus" | "other_in" | "other_out" => "other",
            k => k,
        };
        // Личный расход владельца администратору — только суммой (SPEC-06, ADR-088).
        let hidden = !owner && m.article_owner_only == Some(true);
        let counterparty = if hidden {
            Some("расход владельца".to_string())
        } else {
            m.article.or(m.employee).or(m.party).or(m.supplier)
        };
        let comment = if hidden {
            String::new()
        } else if m.comment.is_empty() {
            m.orig_comment.unwrap_or_default()
        } else {
            m.comment
        };
        // Оплата долга пишет в комментарий имя клиента («ОсОО Ак: за май») — оно уже в колонке.
        let comment = match counterparty.as_deref() {
            Some(name) if comment == name => String::new(),
            Some(name) => comment
                .strip_prefix(&format!("{name}: "))
                .map_or(comment.clone(), str::to_string),
            None => comment,
        };
        out.push(MoneyRow {
            id: m.id,
            created_at: m.created_at,
            kind: key.into(),
            label: money_label(key).into(),
            reversal: m.kind == "reversal",
            amount_tyiyn: m.amount_tyiyn,
            account_name: m.account_name,
            to_account_name: None,
            user_name: m.user_name,
            counterparty,
            comment,
        });
    }
    let transfers = sqlx::query!(
        r#"select t.id, t.created_at, t.amount_tyiyn, t.comment, t.reversal_of,
                  t.from_account_id, fa.name as from_name, fa.owner_only as from_owner_only,
                  t.to_account_id, ta.name as to_name, ta.owner_only as to_owner_only,
                  u.full_name as user_name
           from cash_transfers t
           join cash_accounts fa on fa.id = t.from_account_id
           join cash_accounts ta on ta.id = t.to_account_id
           join users u on u.id = t.user_id
           where t.branch_id = $1 and t.created_at >= $2 and t.created_at <= coalesce($3, now())
           order by t.created_at, t.id"#,
        branch_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    for t in transfers {
        // Перевод между счетами владельца администратору не виден, как и сами счета (ADR-037).
        if !owner && t.from_owner_only && t.to_owner_only {
            continue;
        }
        let (key, amount, account, other) = if t.from_account_id == s.account_id {
            ("transfers_out", -t.amount_tyiyn, t.from_name, t.to_name)
        } else if t.to_account_id == s.account_id {
            ("transfers_in", t.amount_tyiyn, t.to_name, t.from_name)
        } else {
            ("transfers", t.amount_tyiyn, t.from_name, t.to_name)
        };
        out.push(MoneyRow {
            id: t.id,
            created_at: t.created_at,
            kind: key.into(),
            label: money_label(key).into(),
            reversal: t.reversal_of.is_some(),
            amount_tyiyn: amount,
            account_name: account,
            to_account_name: Some(other),
            user_name: t.user_name,
            counterparty: None,
            comment: t.comment,
        });
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    // Итоги — положительными: внесено, переведено, оплачено, потрачено.
    let mut by: HashMap<String, i64> = HashMap::new();
    for r in &out {
        add(&mut by, &r.kind, r.amount_tyiyn)?;
    }
    let order = [
        "cash_in",
        "transfers_in",
        "debt_repayments",
        "supplier_refunds",
        "expenses",
        "payouts",
        "advances",
        "supplier_payments",
        "battery_intake",
        "cash_out",
        "transfers_out",
        "transfers",
        "other",
    ];
    let always = [
        "cash_in",
        "transfers_in",
        "debt_repayments",
        "expenses",
        "payouts",
        "cash_out",
        "transfers_out",
    ];
    let mut totals = Vec::new();
    for key in order {
        let raw = by.get(key).copied().unwrap_or(0);
        if raw == 0 && !always.contains(&key) {
            continue;
        }
        let amount = if key == "other" || key == "transfers" || is_income(key) {
            raw
        } else {
            -raw
        };
        totals.push(Line {
            key: key.into(),
            label: money_label(key).into(),
            amount_tyiyn: amount,
        });
    }
    Ok((out, totals))
}

/// Подпись денежной операции.
fn money_label(key: &str) -> &'static str {
    match key {
        "cash_in" => "Внесение",
        "cash_out" => "Изъятие",
        "transfers_in" => "Перевод в кассу",
        "transfers_out" => "Перевод из кассы",
        "transfers" => "Перевод между счетами",
        "expenses" => "Расход",
        "payouts" => "Выплата",
        "advances" => "Аванс",
        "supplier_payments" => "Оплата поставщику",
        "debt_repayments" => "Оплата долга",
        "supplier_refunds" => "Деньги от поставщика",
        "battery_intake" => "Приём аккумуляторов",
        _ => "Прочее",
    }
}

/// Долги покупателей за смену: продажи в долг, их возвраты, оплаты и правки.
async fn debts(conn: &mut PgConnection, branch_id: Uuid, w: &Window) -> AppResult<Vec<DebtRow>> {
    let rows = sqlx::query!(
        r#"select l.id, l.created_at, l.kind, l.amount_tyiyn, l.reversal_of is not null as "reversal!",
                  l.comment, p.id as party_id, p.name as party_name, u.full_name as user_name,
                  s.id as "sale_id?", s.number as "sale_number?", s.kind as "sale_kind?",
                  (select a.name from cash_movements m join cash_accounts a on a.id = m.account_id
                   where m.doc_type = 'repayment' and m.doc_id = l.doc_id and m.kind = 'debt_repayment'
                   limit 1) as "account_name?"
           from party_ledger l
           join parties p on p.id = l.party_id
           join users u on u.id = l.user_id
           left join sales s on l.doc_type = 'sale' and s.id = l.doc_id
           where l.branch_id = $1 and p.role = 'customer'
             and l.created_at >= $2 and l.created_at <= coalesce($3, now())
             -- Старый долг из тетради и его сторно — не продажа смены (SPEC-38).
             and l.doc_type <> 'old_debt'
             and not exists (select 1 from party_ledger o
                             where o.id = l.reversal_of and o.doc_type = 'old_debt')
             and not (l.doc_type = 'sale' and public.pay_fix_counted_sale(l.doc_id))
           order by l.created_at, l.id"#,
        branch_id,
        w.start,
        w.end
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DebtRow {
            id: r.id,
            created_at: r.created_at,
            kind: match (r.kind.as_str(), r.sale_kind.as_deref()) {
                ("debt", Some("return")) => "return".into(),
                ("debt", _) => "sale".into(),
                (k, _) => k.into(),
            },
            reversal: r.reversal,
            party_id: r.party_id,
            party_name: r.party_name,
            sale_id: r.sale_id,
            sale_number: r.sale_number,
            amount_tyiyn: r.amount_tyiyn,
            account_name: r.account_name,
            user_name: r.user_name,
            comment: r.comment,
        })
        .collect())
}

/// Выплаты сотрудникам за время смены: кому, сколько, за что и откуда.
async fn payouts(
    conn: &mut PgConnection,
    branch_id: Uuid,
    owner: bool,
    w: &Window,
) -> AppResult<Vec<PayoutRow>> {
    Ok(sqlx::query_as!(
        PayoutRow,
        r#"select p.id, p.number, p.created_at, e.full_name as employee_name, p.amount_tyiyn,
                  p.purpose, p.comment, a.name as "account_name?", p.reversal_of is not null as "reversal!",
                  u.full_name as user_name
           from payouts p
           join employees e on e.id = p.employee_id
           join users u on u.id = p.user_id
           left join cash_accounts a on a.id = p.account_id
           where p.branch_id = $1 and p.created_at >= $2 and p.created_at <= coalesce($3, now())
             and (a.id is null or not a.owner_only or $4)
           order by p.created_at, p.id"#,
        branch_id,
        w.start,
        w.end,
        owner
    )
    .fetch_all(&mut *conn)
    .await?)
}

async fn get_report(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ShiftReport>> {
    let mut tx = state.pool.begin().await?;
    // Один снимок базы: «должно быть» и движения кассы читаются на один момент.
    sqlx::query!("set transaction isolation level repeatable read")
        .execute(&mut *tx)
        .await?;
    let report = load(&mut tx, user.branch_id, user.is_owner(), id).await?;
    // Администратору — те же последние 7 дней, что и в списке смен (SPEC-05); открытая смена
    // видна всегда.
    if !user.is_owner() && report.shift.closed_at.is_some() {
        let earliest = sqlx::query_scalar!(
            r#"select ((now() at time zone 'Asia/Bishkek')::date - 6) as "d!""#
        )
        .fetch_one(&mut *tx)
        .await?;
        if report.shift.business_date < earliest {
            return Err(AppError::NotFound);
        }
    }
    tx.commit().await?;
    Ok(Json(report))
}

/// Время по Бишкеку для бота: «20.10 09:07» (UTC+6 круглый год).
fn local(t: DateTime<Utc>) -> String {
    (t + Duration::hours(6)).format("%d.%m %H:%M").to_string()
}

/// Сотые доли процента словами: 1234 → «+12,34 %».
fn percent_bp(bp: i64) -> String {
    let sign = if bp > 0 {
        "+"
    } else if bp < 0 {
        "−"
    } else {
        ""
    };
    let a = bp.unsigned_abs();
    format!("{sign}{},{:02} %", a / 100, a % 100)
}

/// Безнал одного счёта за смену строкой: «Счёт»: итог (карта …, долги …, возвраты …), комиссия,
/// упало на счёт.
pub fn bank_text(b: &crate::api::income::BankSplit) -> String {
    let mut parts = Vec::new();
    for (label, v) in [
        ("карта / QR банка", b.card_tyiyn),
        ("QR владельцу", b.transfer_tyiyn),
        ("по счёту", b.invoice_tyiyn),
        ("оплаты долга", b.debt_tyiyn),
    ] {
        if v != 0 {
            parts.push(format!("{label} {}", format_som(v)));
        }
    }
    if b.returns_tyiyn != 0 {
        parts.push(format!("возвраты −{}", format_som(b.returns_tyiyn)));
    }
    let mut out = format!("безнал «{}»: {}", b.name, format_som(b.total_tyiyn));
    if !parts.is_empty() {
        out.push_str(&format!(" ({})", parts.join(", ")));
    }
    if b.fee_tyiyn != 0 {
        out.push_str(&format!(
            ", комиссия {}, упало на счёт {}",
            format_som(b.fee_tyiyn),
            format_som(b.landed_tyiyn)
        ));
    }
    out
}

/// Не больше `n` пунктов и «ещё N».
fn first(items: Vec<String>, n: usize) -> String {
    let total = items.len();
    let mut shown: Vec<String> = items.into_iter().take(n).collect();
    if total > n {
        shown.push(format!("ещё {}", total - n));
    }
    shown.join("; ")
}

/// Выплаты смены по сотрудникам и видам (ADR-122): «👥 Выплачено» — заработанное, авансы —
/// отдельной строкой. Сторно уменьшает свой вид; вид, ушедший в ноль, не показывается. У прежних
/// выплат без вида — только сумма.
pub fn payout_lines(rows: &[PayoutRow]) -> Vec<String> {
    let mut paid: Vec<(&str, Vec<(&str, i64)>)> = Vec::new();
    let mut advances: Vec<(&str, i64)> = Vec::new();
    for p in rows {
        if p.purpose == "advance" {
            match advances.iter_mut().find(|(n, _)| *n == p.employee_name) {
                Some(a) => a.1 = a.1.saturating_add(p.amount_tyiyn),
                None => advances.push((&p.employee_name, p.amount_tyiyn)),
            }
            continue;
        }
        let kinds = match paid.iter().position(|(n, _)| *n == p.employee_name) {
            Some(i) => paid.get_mut(i).map(|(_, k)| k),
            None => {
                paid.push((&p.employee_name, Vec::new()));
                paid.last_mut().map(|(_, k)| k)
            }
        };
        if let Some(kinds) = kinds {
            match kinds.iter_mut().find(|(k, _)| *k == p.purpose) {
                Some(k) => k.1 = k.1.saturating_add(p.amount_tyiyn),
                None => kinds.push((&p.purpose, p.amount_tyiyn)),
            }
        }
    }
    let mut out = Vec::new();
    let mut total: i64 = 0;
    let mut people = Vec::new();
    for (name, kinds) in paid {
        let kinds: Vec<(&str, i64)> = kinds.into_iter().filter(|(_, v)| *v != 0).collect();
        total = kinds.iter().fold(total, |a, (_, v)| a.saturating_add(*v));
        match kinds.as_slice() {
            [] => {}
            [("general", v)] => people.push(format!("{name} {}", format_som(*v))),
            many => people.push(format!(
                "{name} — {}",
                many.iter()
                    .map(|(k, v)| format!(
                        "{} {}",
                        crate::api::payroll::purpose_label(k),
                        format_som(*v)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    if !people.is_empty() {
        out.push(format!(
            "👥 Выплачено {}: {}",
            format_som(total),
            first(people, 6)
        ));
    }
    let advances: Vec<(&str, i64)> = advances.into_iter().filter(|(_, v)| *v != 0).collect();
    if !advances.is_empty() {
        let sum = advances.iter().fold(0i64, |a, (_, v)| a.saturating_add(*v));
        out.push(format!(
            "💸 Авансы {}: {}",
            format_som(sum),
            first(
                advances
                    .iter()
                    .map(|(n, v)| format!("{n} {}", format_som(*v)))
                    .collect(),
                6
            )
        ));
    }
    out
}

/// Долги смены: дано в долг по чекам и снято с долга возвратами (оплаты долга — отдельно).
pub fn debt_lines(rows: &[DebtRow]) -> Vec<String> {
    // Возврат долгового чека пишет в журнал долга «debt» с минусом.
    let pick = |up: bool| -> Vec<String> {
        rows.iter()
            .filter(|d| {
                matches!(d.kind.as_str(), "sale" | "return")
                    && !d.reversal
                    && if up {
                        d.amount_tyiyn > 0
                    } else {
                        d.amount_tyiyn < 0
                    }
            })
            .map(|d| {
                format!(
                    "{} {}{}",
                    d.party_name,
                    format_som(d.amount_tyiyn.saturating_abs()),
                    d.sale_number
                        .map(|n| format!(" (чек № {n})"))
                        .unwrap_or_default()
                )
            })
            .collect()
    };
    let mut out = Vec::new();
    let given = pick(true);
    if !given.is_empty() {
        out.push(format!("📒 В долг: {}", first(given, 6)));
    }
    let back = pick(false);
    if !back.is_empty() {
        out.push(format!("📒 Снято с долга возвратами: {}", first(back, 6)));
    }
    out
}

/// Отчёт смены коротко для бота (SPEC-30 §7): деньги и разница, выручка по способам, кому
/// выплачено, оплаты долгов, расходы, переводы, отменённые товары; владельцу — валовая.
/// `extra` — дополнительные блоки перед ссылкой на полный отчёт.
pub fn report_text(r: &ShiftReport, extra: &[String]) -> String {
    let s = &r.shift;
    let c = &r.cash;
    let when = match s.closed_at {
        // Закрыли в тот же день — второй раз дату не повторяем.
        Some(at) if local(at).get(..5) == local(s.opened_at).get(..5) => format!(
            "{} — {}",
            local(s.opened_at),
            local(at).get(6..).unwrap_or_default()
        ),
        Some(at) => format!("{} — {}", local(s.opened_at), local(at)),
        None => format!("с {}, открыта", local(s.opened_at)),
    };
    let mut out = vec![format!(
        "💵 Смена № {} · кассир {} · касса «{}»\n{when}",
        s.number, s.cashier_name, s.account_name
    )];
    let parts = |lines: &[Line]| {
        lines
            .iter()
            .filter(|l| l.amount_tyiyn != 0)
            .map(|l| format!("{} {}", l.label.to_lowercase(), format_som(l.amount_tyiyn)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut cash = vec![
        "💰 Деньги в кассе".to_string(),
        format!("  на начало: {}", format_som(c.opening_tyiyn)),
    ];
    let inc = parts(&c.income);
    cash.push(format!(
        "  + приход: {}{}",
        format_som(c.income_tyiyn),
        if inc.is_empty() {
            String::new()
        } else {
            format!(" ({inc})")
        }
    ));
    let outg = parts(&c.outgo);
    cash.push(format!(
        "  − расход: {}{}",
        format_som(c.outgo_tyiyn),
        if outg.is_empty() {
            String::new()
        } else {
            format!(" ({outg})")
        }
    ));
    cash.push(format!("  должно быть: {}", format_som(c.expected_tyiyn)));
    if let (Some(counted), Some(diff)) = (c.counted_tyiyn, c.diff_tyiyn) {
        let result = match diff {
            0 => "сошлось".to_string(),
            d if d < 0 => format!("недостача {}", format_som(-d)),
            d => format!("излишек {}", format_som(d)),
        };
        cash.push(format!("  пересчитали: {} — {result}", format_som(counted)));
    }
    if let Some(v) = c.handover_tyiyn.filter(|v| *v > 0) {
        cash.push(format!(
            "  сдали{}: {}, в кассе осталось {}",
            c.handover_to
                .as_ref()
                .map(|n| format!(" в «{n}»"))
                .unwrap_or_default(),
            format_som(v),
            format_som(c.left_tyiyn.unwrap_or(0))
        ));
    }
    out.push(cash.join("\n"));

    let v = &r.revenue;
    let mut rev = vec![format!(
        "🧾 Выручка: {} (продажи {}, чеков {}{})",
        format_som(v.net_tyiyn),
        format_som(v.sales_tyiyn),
        v.sales_count,
        if v.returns_tyiyn != 0 {
            format!(", возвраты {}", format_som(v.returns_tyiyn))
        } else {
            String::new()
        }
    )];
    if !v.methods.is_empty() {
        rev.push(format!(
            "  {}",
            v.methods
                .iter()
                .map(|m| format!("{} {}", method_label(&m.method), format_som(m.amount_tyiyn)))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    // Безнал по счетам — всегда, даже при одном счёте (ADR-120): итог, комиссия, упало.
    for b in &s.bank_split {
        rev.push(format!("  {}", bank_text(b)));
    }
    // Оплачено по QR банка, а чека нет (SPEC-31).
    if !r.qr_unclaimed.is_empty() {
        rev.push(format!(
            "  ⚠️ QR оплачен, чек не проведён: {}",
            r.qr_unclaimed
                .iter()
                .map(|q| format_som(q.amount_tyiyn))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(g) = v.gross_tyiyn {
        rev.push(format!("  валовая прибыль: {}", format_som(g)));
    }
    if let Some(n) = v.net_profit_tyiyn {
        rev.push(format!("  чистая прибыль: {} (без оклада)", format_som(n)));
    }
    if let Some(cmp) = &r.compare {
        rev.push(format!(
            "  к смене № {}{}: {}{}",
            cmp.number,
            if cmp.same_time {
                " за то же время"
            } else {
                ""
            },
            format_som_signed(cmp.diff_tyiyn),
            cmp.change_bp
                .map(|bp| format!(" ({})", percent_bp(bp)))
                .unwrap_or_default()
        ));
    }
    out.push(rev.join("\n"));
    if let Some(a) = &r.after_close {
        out.push(crate::api::pay_fix::after_close_lines(a).join("\n"));
    }

    let mut more = payout_lines(&r.payouts);
    more.extend(debt_lines(&r.debts));
    let repaid: Vec<String> = r
        .debts
        .iter()
        .filter(|d| d.kind == "repayment")
        .map(|d| format!("{} {}", d.party_name, format_som(-d.amount_tyiyn)))
        .collect();
    if !repaid.is_empty() {
        more.push(format!("📒 Оплаты долгов: {}", first(repaid, 6)));
    }
    let spent: Vec<String> = r
        .money
        .iter()
        .filter(|m| m.kind == "expenses")
        .map(|m| {
            format!(
                "{} {}",
                m.counterparty.as_deref().unwrap_or("расход"),
                format_som(-m.amount_tyiyn)
            )
        })
        .collect();
    if !spent.is_empty() {
        more.push(format!("🧾 Расходы: {}", first(spent, 6)));
    }
    let moved: Vec<String> = r
        .money
        .iter()
        .filter(|m| m.kind.starts_with("transfers"))
        .map(|m| {
            let other = m.to_account_name.as_deref().unwrap_or("");
            match m.kind.as_str() {
                "transfers_out" => format!("в «{other}» {}", format_som(-m.amount_tyiyn)),
                "transfers_in" => format!("из «{other}» {}", format_som(m.amount_tyiyn)),
                _ => format!(
                    "«{}» → «{other}» {}",
                    m.account_name,
                    format_som(m.amount_tyiyn)
                ),
            }
        })
        .collect();
    if !moved.is_empty() {
        more.push(format!("↔ Переводы: {}", first(moved, 6)));
    }
    if !r.cancelled.is_empty() {
        more.push(format!(
            "🗑 Отменённые товары: {} на {}",
            r.cancelled.len(),
            format_som(r.cancelled_tyiyn)
        ));
    }
    if !more.is_empty() {
        out.push(more.join("\n"));
    }
    out.extend(extra.iter().cloned());
    out.push(format!(
        "Полный отчёт — в программе: «Смена» → «Отчёт смены № {}».",
        s.number
    ));
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversal_lands_in_the_kind_it_cancels() {
        assert_eq!(flow_key("reversal", "expense", None, 3000), "expenses");
        assert_eq!(flow_key("reversal", "payout", None, 3000), "payouts");
        assert_eq!(
            flow_key("reversal", "cash_movement", Some("cash_in"), -500),
            "cash_in"
        );
        assert_eq!(flow_key("reversal", "transfer", None, 500), "transfers_out");
        assert_eq!(flow_key("reversal", "transfer", None, -500), "transfers_in");
        assert_eq!(
            flow_key("reversal", "ledger_reversal", None, -500),
            "debt_repayments"
        );
        assert_eq!(flow_key("count_diff", "shift", None, -1), "count_minus");
        assert_eq!(flow_key("bank_fee", "sale", None, -1), "other_out");
        assert!(is_income("cash_sales") && !is_income("expenses"));
    }

    #[test]
    fn methods_follow_screen_order() {
        let by = HashMap::from([
            ("debt".to_string(), 5),
            ("invoice".to_string(), 4),
            ("cash".to_string(), 1),
            ("zzz".to_string(), 7),
            ("card".to_string(), 0),
        ]);
        let m: Vec<String> = sorted_methods(by).into_iter().map(|m| m.method).collect();
        assert_eq!(m, ["cash", "invoice", "debt", "zzz"]);
        assert_eq!(method_label("invoice"), "по счёту");
    }

    #[test]
    fn bank_line_shows_landed_after_fee() {
        let b = crate::api::income::BankSplit {
            name: "Счёт".into(),
            card_tyiyn: 50_000,
            debt_tyiyn: 30_000,
            returns_tyiyn: 10_000,
            fee_tyiyn: 400,
            total_tyiyn: 70_000,
            landed_tyiyn: 69_600,
            ..Default::default()
        };
        assert_eq!(
            bank_text(&b),
            "безнал «Счёт»: 700 с (карта / QR банка 500 с, оплаты долга 300 с, возвраты −100 с), \
             комиссия 4 с, упало на счёт 696 с"
        );
        // Один счёт без движений — строка всё равно есть.
        let empty = crate::api::income::BankSplit {
            name: "Счёт".into(),
            ..Default::default()
        };
        assert_eq!(bank_text(&empty), "безнал «Счёт»: 0 с");
    }

    #[test]
    fn payouts_by_kind_and_advances_apart() {
        let row = |name: &str, purpose: &str, amount: i64| PayoutRow {
            id: Uuid::nil(),
            number: 1,
            created_at: Utc::now(),
            employee_name: name.into(),
            amount_tyiyn: amount,
            purpose: purpose.into(),
            comment: String::new(),
            account_name: None,
            reversal: amount < 0,
            user_name: String::new(),
        };
        let lines = payout_lines(&[
            row("Нурлан", "percent", 50_000),
            row("Бек", "general", 15_000),
            row("Нурлан", "bonus", 100_000),
            row("Нурлан", "advance", 20_000),
            // Сторно бонуса Беку — вид ушёл в ноль и не показывается.
            row("Бек", "bonus", 3_000),
            row("Бек", "bonus", -3_000),
        ]);
        assert_eq!(
            lines,
            [
                "👥 Выплачено 1 650 с: Нурлан — проценты 500 с, бонус 1 000 с; Бек 150 с",
                "💸 Авансы 200 с: Нурлан 200 с",
            ]
        );
        assert!(payout_lines(&[row("Бек", "advance", 1), row("Бек", "advance", -1)]).is_empty());
    }

    #[test]
    fn percent_is_signed() {
        assert_eq!(percent_bp(1234), "+12,34 %");
        assert_eq!(percent_bp(-5), "−0,05 %");
        assert_eq!(percent_bp(0), "0,00 %");
        assert_eq!(
            first(vec!["a".into(), "b".into(), "c".into()], 2),
            "a; b; ещё 1"
        );
    }
}
