//! «Изменить оплату» чека и «из наличных в QR» за смену (SPEC-37, ADR-132).
//!
//! Главное правило (§1): пересчитанные наличные не трогаем. Если наличные уже вошли в пересчёт
//! «Кассы» — закрытие смены (в том числе потом переоткрытой) или открытие следующей смены с
//! «Пересчитал», — остаток кассы не меняется: меняется расхождение этого пересчёта (движение
//! «пересчёт» с документом той смены), а у пересчёта при закрытии — и удержание с кассира. Иначе
//! меняется остаток «Кассы» и «Должно быть» открытой смены.
//!
//! Изменение оплаты чека — это «Исправить» (ADR-085): сторно и новый чек одной операцией, связь в
//! `doc_corrections`. Новый чек повторяет исходный (строки, цены, себестоимость, кассир, учётный
//! день), меняется только оплата. Перенос за смену чеки не меняет. Обе операции записываются в
//! `pay_fixes`; деньги пишутся в конце — сначала с плюсом, потом с минусом, чтобы касса не уходила
//! в минус даже на миг.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::{self, CashMove};
use crate::api::corrections::CorrectionOut;
use crate::api::sales::PaymentOut;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::{format_som, format_som_signed};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/sales/{id}/pay-change",
            routing::get(pay_change_info).post(pay_change),
        )
        .route(
            "/shifts/{id}/cash-to-bank",
            routing::get(cash_to_bank_info).post(cash_to_bank),
        )
        .route(
            "/cash-to-bank/{id}/reverse",
            routing::post(reverse_transfer),
        )
}

fn yes() -> bool {
    true
}

const OWNER_ONLY: &str = "изменяет оплату и переносит наличные в банк только владелец";

fn require_owner(user: &CurrentUser) -> AppResult<()> {
    if user.is_owner() {
        Ok(())
    } else {
        Err(AppError::ForbiddenWith(OWNER_ONLY.into()))
    }
}

/// Причина изменения: обязательна, до 500 знаков. У пробного проведения может быть пустой.
fn reason(comment: &str, dry_run: bool) -> AppResult<String> {
    let c = comment.trim();
    if c.chars().count() > 500 {
        return Err(invalid("причина — не длиннее 500 знаков"));
    }
    if c.is_empty() {
        if dry_run {
            return Ok("пробное проведение".into());
        }
        return Err(invalid("укажите, что и почему меняете"));
    }
    Ok(c.to_string())
}

// ---------- Пересчёт ----------

/// Пересчёт «Кассы», в который вошли наличные (SPEC-37 §1).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Counted {
    pub shift_id: Uuid,
    pub number: i64,
    /// `close` — пересчёт при закрытии смены, `open` — при открытии с «Пересчитал».
    pub at: String,
    pub business_date: NaiveDate,
    pub cashier_id: Uuid,
    pub cashier_name: String,
    /// Смена открыта сейчас: переоткрыта после этого закрытия или пересчёт был при её открытии.
    pub open_now: bool,
}

impl Counted {
    fn at_close(&self) -> bool {
        self.at == "close"
    }
}

async fn counted_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    shift_id: Uuid,
    at: &str,
) -> AppResult<Counted> {
    let s = sqlx::query!(
        r#"select s.number, s.business_date, s.account_id, s.cashier_employee_id, e.full_name
           from shifts s join employees e on e.id = s.cashier_employee_id
           where s.id = $1 and s.branch_id = $2"#,
        shift_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let open_now = cash::open_shift_id(conn, branch_id, s.account_id).await? == Some(shift_id);
    Ok(Counted {
        shift_id,
        number: s.number,
        at: at.to_string(),
        business_date: s.business_date,
        cashier_id: s.cashier_employee_id,
        cashier_name: s.full_name,
        open_now,
    })
}

/// Первый пересчёт кассы после момента `at`: закрытие любой смены (и потом переоткрытой) или
/// открытие смены с «Пересчитал». Нет — наличные ещё не пересчитаны.
pub(crate) async fn counted_after(
    conn: &mut PgConnection,
    branch_id: Uuid,
    register: Uuid,
    at: DateTime<Utc>,
) -> AppResult<Option<Counted>> {
    let first = sqlx::query!(
        r#"select x.shift_id as "shift_id!", x.at as "at!" from (
             select c.shift_id, 'close'::text as at, c.closed_at as t
             from shift_closes c join shifts s on s.id = c.shift_id
             where s.branch_id = $1 and s.account_id = $2 and c.closed_at > $3
             union all
             select s.id, 'open'::text, s.opened_at
             from shifts s
             where s.branch_id = $1 and s.account_id = $2 and s.opened_at > $3
               and s.opening_counted_tyiyn is not null
           ) x order by x.t limit 1"#,
        branch_id,
        register,
        at
    )
    .fetch_optional(&mut *conn)
    .await?;
    match first {
        Some(f) => Ok(Some(counted_of(conn, branch_id, f.shift_id, &f.at).await?)),
        None => Ok(None),
    }
}

/// Пересчёт, в который вошли наличные чека. Новый чек прежнего изменения для уже пересчитанных
/// наличных — тот же пересчёт: его деньги относятся к той смене, а не ко времени изменения.
pub(crate) async fn sale_counted(
    conn: &mut PgConnection,
    branch_id: Uuid,
    sale_id: Uuid,
) -> AppResult<Option<Counted>> {
    if let Some(f) = sqlx::query!(
        r#"select counted_shift_id as "shift_id!", counted_at as "at!" from pay_fixes
           where new_sale_id = $1 and counted_shift_id is not null limit 1"#,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(Some(counted_of(conn, branch_id, f.shift_id, &f.at).await?));
    }
    let created = sqlx::query_scalar!(
        "select created_at from sales where id = $1 and branch_id = $2",
        sale_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let register = cash::default_account(conn, branch_id).await?;
    counted_after(conn, branch_id, register, created).await
}

async fn cash_of(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from sale_payments
           where sale_id = $1 and method = 'cash'"#,
        sale_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// «Исправить» поменял у чека наличные, а они уже пересчитаны (SPEC-37 §2): такое — только
/// «Изменить оплату».
pub(crate) async fn counted_cash_changes(
    conn: &mut PgConnection,
    branch_id: Uuid,
    original_id: Uuid,
    new_id: Uuid,
) -> AppResult<bool> {
    if cash_of(conn, original_id).await? == cash_of(conn, new_id).await? {
        return Ok(false);
    }
    Ok(sale_counted(conn, branch_id, original_id).await?.is_some())
}

/// Расхождение пересчёта смены (< 0 — недостача): при закрытии — сумма расхождений её закрытий и
/// прежних поправок закрытия; при открытии — пересчёт при открытии и его поправки.
async fn diff_of(conn: &mut PgConnection, c: &Counted) -> AppResult<i64> {
    let base = if c.at_close() {
        sqlx::query_scalar!(
            r#"select coalesce(sum(diff_tyiyn), 0)::bigint as "v!" from shift_closes where shift_id = $1"#,
            c.shift_id
        )
        .fetch_one(&mut *conn)
        .await?
    } else {
        sqlx::query_scalar!(
            r#"select coalesce(sum(m.amount_tyiyn), 0)::bigint as "v!"
               from cash_movements m join shifts s on s.id = m.doc_id
               where m.kind = 'count_diff' and m.doc_type = 'shift' and m.doc_id = $1
                 and m.created_at = s.opened_at"#,
            c.shift_id
        )
        .fetch_one(&mut *conn)
        .await?
    };
    let fixes = sqlx::query_scalar!(
        r#"select coalesce(-sum(cash_delta_tyiyn), 0)::bigint as "v!" from pay_fixes
           where counted_shift_id = $1 and counted_at = $2 and count_movement_id is not null"#,
        c.shift_id,
        c.at
    )
    .fetch_one(&mut *conn)
    .await?;
    base.checked_add(fixes).ok_or_else(overflow)
}

/// Удержано с кассира по смене сейчас: удержания недостачи минус прощённое и возвращённое.
async fn withheld_of(conn: &mut PgConnection, shift_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(-sum(a.amount_tyiyn), 0)::bigint as "v!" from payroll_accruals a
           where a.kind = 'shortage'
             and ((a.doc_type = 'shift' and a.doc_id = $1)
                  or (a.doc_type = 'reversal' and exists (
                        select 1 from payroll_accruals o
                        where o.id = a.doc_id and o.doc_type = 'shift' and o.doc_id = $1)))"#,
        shift_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Поправка удержания с кассира (SPEC-37 §1): `d` — расхождение закрытия, `delta` — изменение
/// наличных, `w` — удержано сейчас. Недостача уменьшилась — кассиру возвращается удержанное, не
/// больше уменьшения (> 0); выросла — удерживается рост (< 0). Только у пересчёта при закрытии и
/// с галочкой «Поправить удержание кассира».
pub fn withhold_change(d: i64, delta: i64, w: i64, fix: bool, at_close: bool) -> i64 {
    if !fix || !at_close {
        return 0;
    }
    let after = d.saturating_sub(delta);
    let (n, n2) = (d.saturating_neg().max(0), after.saturating_neg().max(0));
    if n2 < n {
        (n - n2).min(w.max(0))
    } else if n2 > n {
        -(n2 - n)
    } else {
        0
    }
}

/// Пишет деньги операции: сначала все с плюсом, потом с минусом — остаток каждого счёта по ходу
/// не опускается ниже меньшего из «до» и «после», ложного «ушёл в минус» нет. Наличные уже
/// пересчитаны — движения относятся к смене пересчёта. Возвращает id движения «пересчёт».
async fn write_moves(
    conn: &mut PgConnection,
    ctx: &Ctx,
    mut moves: Vec<CashMove>,
    shift: Option<Uuid>,
) -> AppResult<Option<Uuid>> {
    moves.sort_by_key(|m| m.amount < 0);
    let mut count_id = None;
    for m in &moves {
        if m.amount == 0 {
            continue;
        }
        let (id, _) = match shift {
            Some(s) => cash::add_movement_in_shift(conn, ctx, m.entry(), s).await?,
            None => cash::add_movement_id(conn, ctx, m.entry()).await?,
        };
        if m.kind == "count_diff" {
            count_id = Some(id);
        }
    }
    Ok(count_id)
}

/// Деньги по счетам: изменение без комиссии и комиссия.
async fn account_effects(
    conn: &mut PgConnection,
    moves: &[CashMove],
) -> AppResult<Vec<AccountEffect>> {
    let mut by: BTreeMap<Uuid, (i64, i64)> = BTreeMap::new();
    for m in moves {
        let e = by.entry(m.account_id).or_insert((0, 0));
        if m.kind == "bank_fee" {
            e.1 = e.1.saturating_sub(m.amount);
        } else {
            e.0 = e.0.saturating_add(m.amount);
        }
    }
    let mut out = Vec::new();
    for (id, (amount, fee)) in by {
        let a = sqlx::query!("select name, kind from cash_accounts where id = $1", id)
            .fetch_one(&mut *conn)
            .await?;
        out.push(AccountEffect {
            account_id: id,
            name: a.name,
            kind: a.kind,
            amount_tyiyn: amount,
            fee_tyiyn: fee,
        });
    }
    // Касса первой, потом безналичные.
    out.sort_by_key(|a| (a.kind != "register", a.name.clone()));
    Ok(out)
}

// ---------- Что изменится ----------

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountEffect {
    pub account_id: Uuid,
    pub name: String,
    pub kind: String,
    /// Изменение остатка без комиссии: + пришло, − ушло.
    pub amount_tyiyn: i64,
    /// Изменение комиссии банка (+ — комиссии больше).
    pub fee_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct DebtEffect {
    pub party_name: String,
    pub before_tyiyn: i64,
    pub after_tyiyn: i64,
    /// По исходному чеку уже погашено: станет авансом клиента.
    pub repaid_tyiyn: i64,
}

/// «Что изменится»: сервер считает пробным проведением — та же функция, транзакция откатывается.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Effect {
    pub accounts: Vec<AccountEffect>,
    /// Изменение наличных: новые − прежние (у переноса — минус сумма).
    pub cash_delta_tyiyn: i64,
    /// Пересчёт, в который вошли наличные; нет — меняется остаток «Кассы».
    pub counted: Option<Counted>,
    pub diff_before_tyiyn: Option<i64>,
    pub diff_after_tyiyn: Option<i64>,
    /// Удержано с кассира по смене до изменения.
    pub withheld_tyiyn: Option<i64>,
    /// Поправка удержания: > 0 вернётся кассиру, < 0 удержится.
    pub withhold_tyiyn: i64,
    /// Поправка с галочкой «Поправить удержание кассира»: окно показывает галочку, только когда
    /// есть что поправить.
    pub withhold_possible_tyiyn: i64,
    /// Открытая сейчас смена, у которой меняется «Должно быть» (наличные ещё не пересчитаны).
    pub open_shift_number: Option<i64>,
    pub debt: Option<DebtEffect>,
    /// Процент кассира: изменение начисленного сейчас.
    pub percent_tyiyn: i64,
    /// Процент с долговой части — по мере погашения.
    pub percent_pending: bool,
    /// Комиссия банка: изменение.
    pub fee_tyiyn: i64,
    /// Изменение прибыли: по учётному дню чека и смены.
    pub profit_tyiyn: i64,
    /// Чек или смена прошлого месяца: «изменятся прибыль и расчёт за сентябрь».
    pub month_note: Option<String>,
    /// Всё это словами — строки блока «Что изменится».
    pub lines: Vec<String>,
}

fn diff_text(d: i64) -> String {
    match d {
        0 => "без расхождения".into(),
        d if d < 0 => format!("недостача {}", format_som(-d)),
        d => format!("излишек {}", format_som(d)),
    }
}

const MONTHS: [&str; 12] = [
    "январь",
    "февраль",
    "март",
    "апрель",
    "май",
    "июнь",
    "июль",
    "август",
    "сентябрь",
    "октябрь",
    "ноябрь",
    "декабрь",
];

async fn month_note(conn: &mut PgConnection, day: NaiveDate) -> AppResult<Option<String>> {
    let today = crate::api::fees::today(conn).await?;
    if (day.year(), day.month()) == (today.year(), today.month()) {
        return Ok(None);
    }
    let name = usize::try_from(day.month0())
        .ok()
        .and_then(|m| MONTHS.get(m))
        .copied()
        .unwrap_or("");
    Ok(Some(format!(
        "{:02}.{} — прошлый месяц: изменятся прибыль и расчёт за {name}",
        day.month(),
        day.year()
    )))
}

/// Строки «Что изменится» о смене и удержании.
fn shift_lines(e: &Effect, lines: &mut Vec<String>) {
    let delta = e.cash_delta_tyiyn;
    if delta == 0 {
        return;
    }
    match &e.counted {
        Some(c) => {
            let (before, after) = (
                e.diff_before_tyiyn.unwrap_or(0),
                e.diff_after_tyiyn.unwrap_or(0),
            );
            let head = if !c.at_close() {
                format!("Смена № {}, пересчёт при открытии", c.number)
            } else if c.open_now {
                format!("Смена № {} (переоткрыта), закрытие", c.number)
            } else {
                format!("Смена № {} закрыта", c.number)
            };
            let mut text = format!("{head}: {} → {}", diff_text(before), diff_text(after));
            if e.withhold_tyiyn > 0 {
                text.push_str(&format!(
                    ", кассиру {} вернётся {}",
                    c.cashier_name,
                    format_som(e.withhold_tyiyn)
                ));
            } else if e.withhold_tyiyn < 0 {
                text.push_str(&format!(
                    ", с кассира {} удержится {}",
                    c.cashier_name,
                    format_som(-e.withhold_tyiyn)
                ));
            } else if e.withhold_possible_tyiyn != 0 {
                text.push_str(", удержание кассира не меняется");
            } else if !c.at_close() {
                text.push_str(" (при открытии недостачу не удерживают)");
            }
            lines.push(text);
            lines.push("Остаток «Кассы» не меняется: наличные уже пересчитаны".into());
        }
        None => match e.open_shift_number {
            Some(n) => lines.push(format!(
                "В кассе смены № {n} должно быть на {} {}",
                format_som(delta.abs()),
                if delta < 0 {
                    "меньше"
                } else {
                    "больше"
                }
            )),
            None => lines.push(format!(
                "Смена не открыта: остаток «Кассы» {}",
                format_som_signed(delta)
            )),
        },
    }
}

fn account_lines(e: &Effect, lines: &mut Vec<String>) {
    for a in &e.accounts {
        if a.kind == "register" && e.counted.is_some() {
            continue;
        }
        if a.amount_tyiyn == 0 && a.fee_tyiyn == 0 {
            continue;
        }
        let mut text = format!("«{}»: {}", a.name, format_som_signed(a.amount_tyiyn));
        if a.fee_tyiyn != 0 {
            text.push_str(&format!(
                ", комиссия банка {}",
                format_som_signed(a.fee_tyiyn)
            ));
        }
        lines.push(text);
    }
}

fn profit_line(e: &Effect, lines: &mut Vec<String>) {
    if e.profit_tyiyn == 0 {
        lines.push("Прибыль дня не меняется".into());
    } else if e.profit_tyiyn == -e.fee_tyiyn {
        lines.push(format!(
            "Прибыль дня: {} (комиссия банка)",
            format_som_signed(e.profit_tyiyn)
        ));
    } else {
        lines.push(format!(
            "Прибыль дня: {}",
            format_som_signed(e.profit_tyiyn)
        ));
    }
}

// ---------- Изменить оплату чека ----------

/// Оплата чека словами: «наличные 1 000 с, карта / QR банка (Optima) 500 с».
async fn payments_text(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<String> {
    let rows = sqlx::query!(
        r#"select p.method, a.name as "account?", sum(p.amount_tyiyn)::bigint as "amount!"
           from sale_payments p left join cash_accounts a on a.id = p.account_id
           where p.sale_id = $1
           group by p.method, a.name order by min(p.id::text)"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let parts: Vec<String> = rows
        .into_iter()
        .map(|r| {
            let method = crate::api::shift_report::method_label(&r.method);
            match r.account {
                Some(a) if matches!(r.method.as_str(), "card" | "transfer" | "invoice") => {
                    format!("{method} ({a}) {}", format_som(r.amount))
                }
                _ => format!("{method} {}", format_som(r.amount)),
            }
        })
        .collect();
    Ok(if parts.is_empty() {
        "—".into()
    } else {
        parts.join(", ")
    })
}

/// Сколько по долгу чека уже погашено: долг чека минус его неоплаченная часть (старые долги
/// гасятся первыми — то же правило, что «неоплаченные чеки», SPEC-28 §3).
async fn repaid_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    sale_id: Uuid,
    party_id: Uuid,
) -> AppResult<i64> {
    let rows = sqlx::query!(
        r#"select d.doc_id, d.amount as "amount!" from (
             select l.doc_id, l.business_date, l.created_at, (l.amount_tyiyn
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x join sales r on r.id = x.doc_id
                              where l.doc_type = 'sale' and x.doc_type = 'sale_return'
                                and r.reversal_of = l.doc_id and x.party_id = l.party_id), 0)
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x where x.reversal_of = l.id), 0))::bigint as amount
             from party_ledger l
             where l.party_id = $1 and l.branch_id = $2 and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
           ) d where d.amount > 0
           order by d.business_date desc, d.created_at desc"#,
        party_id,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let Some(pos) = rows.iter().position(|r| r.doc_id == Some(sale_id)) else {
        return Ok(0);
    };
    let balance = sqlx::query_scalar!(
        "select balance_tyiyn from party_balances where party_id = $1 and branch_id = $2",
        party_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(0);
    let amounts: Vec<i64> = rows.iter().map(|r| r.amount).collect();
    let unpaid = crate::domain::debt::unpaid_parts(balance, &amounts);
    let debt = amounts.get(pos).copied().unwrap_or(0);
    Ok(debt
        .saturating_sub(unpaid.get(pos).copied().unwrap_or(0))
        .max(0))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BankAccountBrief {
    pub id: Uuid,
    pub name: String,
    pub default_card: bool,
    pub default_transfer: bool,
    pub default_invoice: bool,
    /// Ставка комиссии в учётный день чека или смены, сотые доли процента.
    pub fee_rate_bp: i32,
}

async fn bank_accounts(
    conn: &mut PgConnection,
    user: &CurrentUser,
    on: NaiveDate,
) -> AppResult<Vec<BankAccountBrief>> {
    Ok(sqlx::query_as!(
        BankAccountBrief,
        r#"select id, name, default_card, default_transfer, default_invoice,
                  account_fee_on(id, $3) as "fee_rate_bp!"
           from cash_accounts
           where branch_id = $1 and kind = 'bank' and active and (not owner_only or $2)
           order by default_card desc, default_transfer desc, name"#,
        user.branch_id,
        user.is_owner(),
        on
    )
    .fetch_all(&mut *conn)
    .await?)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PartyBrief {
    pub id: Uuid,
    pub name: String,
    /// `person` или `company`: «По счёту» — только фирме (ADR-095).
    pub kind: String,
    pub inn: String,
    pub balance_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct QrBrief {
    pub id: Uuid,
    pub amount_tyiyn: i64,
    pub paid_at: Option<DateTime<Utc>>,
}

/// Окно «Изменить оплату»: чек, оплата сейчас, что не меняется, счета, QR без чека, пересчёт.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayChangeInfo {
    pub sale_id: Uuid,
    pub number: i64,
    pub business_date: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub cashier_name: String,
    pub total_tyiyn: i64,
    /// Оплата сейчас: способы, суммы, счёт у безнала.
    pub payments: Vec<PaymentOut>,
    /// Баллы и аккумулятор — не меняются.
    pub fixed: Vec<PaymentOut>,
    /// Сколько распределить: итог − баллы − аккумулятор.
    pub to_split_tyiyn: i64,
    pub party: Option<PartyBrief>,
    pub accounts: Vec<BankAccountBrief>,
    /// Оплаченные QR без чека за учётный день чека (SPEC-31 §2).
    pub qr: Vec<QrBrief>,
    /// Пересчёт, в который вошли наличные чека; нет — наличные ещё в «Кассе».
    pub counted: Option<Counted>,
    pub diff_tyiyn: Option<i64>,
    pub withheld_tyiyn: Option<i64>,
    /// Открытая сейчас смена (без пересчёта наличных чека меняется её «Должно быть»).
    pub open_shift_number: Option<i64>,
    /// По долгу чека уже погашено.
    pub repaid_tyiyn: i64,
}

async fn pay_change_info(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<PayChangeInfo>> {
    require_owner(&user)?;
    let mut conn = state.pool.acquire().await?;
    crate::api::corrections::check_sale_correctable(&mut conn, user.branch_id, id).await?;
    let sale = crate::api::sales::load_sale(&mut conn, &user, id).await?;
    let business_date = sale
        .business_date
        .ok_or_else(|| AppError::Internal("нет учётного дня чека".into()))?;
    let (fixed, payments): (Vec<PaymentOut>, Vec<PaymentOut>) = sale
        .payments
        .iter()
        .cloned()
        .partition(|p| matches!(p.method.as_str(), "bonus" | "battery"));
    let fixed_sum = fixed
        .iter()
        .try_fold(0i64, |a, p| a.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    let party = match sale.party_id {
        Some(pid) => Some(party_brief(&mut conn, user.branch_id, pid).await?),
        None => None,
    };
    let accounts = bank_accounts(&mut conn, &user, business_date).await?;
    let qr = sqlx::query_as!(
        QrBrief,
        r#"select id, coalesce(paid_tyiyn, amount_tyiyn) as "amount_tyiyn!", paid_at
           from bank_qr
           where branch_id = $1 and status = 'paid' and sale_id is null
             and (paid_at at time zone 'Asia/Bishkek')::date = $2
           order by paid_at"#,
        user.branch_id,
        business_date
    )
    .fetch_all(&mut *conn)
    .await?;
    let counted = sale_counted(&mut conn, user.branch_id, id).await?;
    let (diff_tyiyn, withheld_tyiyn) = match &counted {
        Some(c) => (
            Some(diff_of(&mut conn, c).await?),
            Some(withheld_of(&mut conn, c.shift_id).await?),
        ),
        None => (None, None),
    };
    let register = cash::default_account(&mut conn, user.branch_id).await?;
    let open_shift_number = match cash::open_shift_id(&mut conn, user.branch_id, register).await? {
        Some(s) => {
            sqlx::query_scalar!("select number from shifts where id = $1", s)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let repaid_tyiyn = match sale.party_id {
        Some(pid) if payments.iter().any(|p| p.method == "debt") => {
            repaid_of(&mut conn, user.branch_id, id, pid).await?
        }
        _ => 0,
    };
    Ok(Json(PayChangeInfo {
        sale_id: sale.id,
        number: sale.number,
        business_date,
        created_at: sale.created_at,
        cashier_name: sale.cashier_name,
        total_tyiyn: sale.total_tyiyn,
        payments,
        fixed,
        to_split_tyiyn: sale.total_tyiyn.saturating_sub(fixed_sum),
        party,
        accounts,
        qr,
        counted,
        diff_tyiyn,
        withheld_tyiyn,
        open_shift_number,
        repaid_tyiyn,
    }))
}

async fn party_brief(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<PartyBrief> {
    let p = sqlx::query!(
        r#"select p.name, p.kind, p.inn, p.role,
                  coalesce((select b.balance_tyiyn from party_balances b
                            where b.party_id = p.id and b.branch_id = $2), 0)::bigint as "balance!"
           from parties p where p.id = $1"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("клиент не найден"))?;
    if p.role != "customer" {
        return Err(invalid("это поставщик, а не клиент"));
    }
    Ok(PartyBrief {
        id,
        name: p.name,
        kind: p.kind,
        inn: p.inn,
        balance_tyiyn: p.balance,
    })
}

#[derive(Deserialize, Clone, Debug)]
pub struct PayLine {
    pub method: String,
    pub amount_tyiyn: i64,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub bank_qr_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct PayChangeReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    /// Новая оплата: наличные, карта / QR, перевод, «По счёту», в долг. Баллы и аккумулятор не
    /// присылаются — они остаются как в исходном чеке.
    pub payments: Vec<PayLine>,
    /// Клиент для долга, если у чека клиента нет.
    #[serde(default)]
    pub party_id: Option<Uuid>,
    /// ПИН или ИНН клиента — пишется в карточку, если там пусто (SPEC-10).
    #[serde(default)]
    pub party_inn: Option<String>,
    #[serde(default = "yes")]
    pub fix_withholding: bool,
    /// Пробное проведение для «Что изменится»: транзакция откатывается.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayChangeOut {
    pub correction: Option<CorrectionOut>,
    pub new_sale_id: Option<Uuid>,
    pub new_number: Option<i64>,
    pub effect: Effect,
}

async fn pay_change(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PayChangeReq>,
) -> AppResult<Json<PayChangeOut>> {
    let dry = req.dry_run;
    let mut tx = state.pool.begin().await?;
    let out = pay_change_tx(&mut tx, &ctx, id, req).await?;
    if dry {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(out))
}

/// Безналичный счёт новой оплаты: выбранный (банковский, этого филиала, не в архиве — иначе 409)
/// или счёт по умолчанию для способа.
async fn pay_account(
    conn: &mut PgConnection,
    user: &CurrentUser,
    method: &str,
    requested: Option<Uuid>,
) -> AppResult<Uuid> {
    let Some(id) = requested else {
        return cash::bank_account(conn, user.branch_id, method)
            .await?
            .ok_or_else(|| invalid("нет безналичного счёта: заведите его в «Обзоре счетов»"));
    };
    let a = sqlx::query!(
        "select name, kind, owner_only, active from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("выбранный счёт не найден"))?;
    if a.kind != "bank" || (a.owner_only && !user.is_owner()) {
        return Err(invalid(format!("«{}» — не безналичный счёт", a.name)));
    }
    if !a.active {
        return Err(AppError::Conflict(format!(
            "счёт «{}» в архиве — верните его из архива в «Счетах» и повторите",
            a.name
        )));
    }
    Ok(id)
}

pub async fn pay_change_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: PayChangeReq,
) -> AppResult<PayChangeOut> {
    const KIND: &str = "sale.pay_change";
    require_owner(&ctx.user)?;
    let dry = req.dry_run;
    let comment = reason(&req.comment, dry)?;
    if !dry && let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    // Два изменения одного чека одновременно: второе ждёт и видит, что чек уже исправлен.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 3))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    crate::api::corrections::check_sale_correctable(&mut *conn, branch_id, id).await?;
    let orig = sqlx::query!(
        r#"select number, business_date, total_tyiyn, master_fee_tyiyn, party_id as "party_id?"
           from sales where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if req.payments.is_empty() || req.payments.len() > 20 {
        return Err(invalid("оплат — от одной до 20"));
    }
    for p in &req.payments {
        if !matches!(
            p.method.as_str(),
            "cash" | "card" | "transfer" | "invoice" | "debt"
        ) {
            return Err(invalid(
                "способ оплаты: наличные, карта / QR, перевод, «По счёту» или в долг — баллы и аккумулятор не меняются",
            ));
        }
        if p.amount_tyiyn <= 0 || p.amount_tyiyn > ops::MAX_AMOUNT_TYIYN {
            return Err(invalid("сумма оплаты больше нуля"));
        }
        if p.bank_qr_id.is_some() && p.method != "card" {
            return Err(invalid("QR банка — только у оплаты «Карта / QR банка»"));
        }
    }
    let snapshot = crate::api::corrections::sale_snapshot(&mut *conn, &ctx.user, id).await?;
    let draft = snapshot.draft().clone();
    // Баллы и аккумулятор не меняются: из исходного чека как есть (вес и цена за кг — тоже).
    let fixed: Vec<crate::api::sales::PaymentReq> = draft
        .get("payments")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|p| {
                    matches!(
                        p.get("method").and_then(Value::as_str),
                        Some("bonus" | "battery")
                    )
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .map(|v| serde_json::from_value(Value::Array(v)))
        .transpose()
        .map_err(|e| AppError::Internal(e.to_string()))?
        .unwrap_or_default();
    let fixed_sum = fixed
        .iter()
        .try_fold(0i64, |a, p| a.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    let to_split = orig.total_tyiyn.saturating_sub(fixed_sum);
    let sum = req
        .payments
        .iter()
        .try_fold(0i64, |a, p| a.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    if sum != to_split {
        return Err(invalid(format!(
            "сумма оплат {} не равна итогу без баллов и аккумулятора {}",
            format_som(sum),
            format_som(to_split)
        )));
    }
    // Клиент: исходного чека; клиента нет — выбранный для долга или «По счёту».
    let debt: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "debt")
        .map(|p| p.amount_tyiyn)
        .sum();
    let needs_party = debt > 0 || req.payments.iter().any(|p| p.method == "invoice");
    let party_id = match (orig.party_id, req.party_id) {
        (Some(o), Some(r)) if o != r => {
            return Err(invalid(
                "у чека уже есть клиент — долг записывается ему; другого клиента выберите через «Исправить»",
            ));
        }
        (Some(o), _) => Some(o),
        (None, Some(r)) if needs_party => Some(r),
        (None, _) if needs_party => {
            return Err(invalid("для долга и «По счёту» выберите клиента"));
        }
        (None, _) => None,
    };
    let party = match party_id {
        Some(pid) => Some(party_brief(&mut *conn, branch_id, pid).await?),
        None => None,
    };
    if req.payments.iter().any(|p| p.method == "invoice")
        && party.as_ref().is_some_and(|p| p.kind != "company")
    {
        return Err(invalid("«По счёту» платит только фирма (ADR-095)"));
    }
    // Долг — документом с ПИН или ИНН (SPEC-10): номер из окна пишется в карточку.
    let mut inn_written: Option<String> = None;
    if debt > 0
        && let Some(p) = &party
        && p.inn.trim().is_empty()
    {
        let inn = req
            .party_inn
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| invalid("для долга укажите ПИН (физлицо) или ИНН (фирма) клиента"))?;
        if inn.chars().count() > 20 || !inn.chars().all(|c| c.is_ascii_digit()) {
            return Err(invalid("ПИН или ИНН — только цифры, не длиннее 20"));
        }
        inn_written = Some(inn.to_string());
    }
    // Безнал — на выбранный счёт; счёт исходной оплаты в архиве — 409, как везде (ADR-088).
    let mut new_payments: Vec<crate::api::sales::PaymentReq> = Vec::new();
    for p in &req.payments {
        let account_id = if matches!(p.method.as_str(), "card" | "transfer" | "invoice") {
            Some(pay_account(&mut *conn, &ctx.user, &p.method, p.account_id).await?)
        } else {
            None
        };
        new_payments.push(crate::api::sales::PaymentReq {
            method: p.method.clone(),
            amount_tyiyn: p.amount_tyiyn,
            grams: None,
            price_per_kg_tyiyn: None,
            account_id,
            bank_qr_id: p.bank_qr_id,
        });
    }
    if let Some(name) = sqlx::query_scalar!(
        r#"select a.name from sale_payments p join cash_accounts a on a.id = p.account_id
           where p.sale_id = $1 and not a.active limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Err(AppError::Conflict(format!(
            "счёт «{name}» исходной оплаты в архиве — верните его из архива в «Счетах» и повторите"
        )));
    }
    let old_cash = cash_of(&mut *conn, id).await?;
    let new_cash = new_payments
        .iter()
        .filter(|p| p.method == "cash")
        .try_fold(0i64, |a, p| a.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    let delta = new_cash.checked_sub(old_cash).ok_or_else(overflow)?;
    let counted = sale_counted(&mut *conn, branch_id, id).await?;
    let register = cash::default_account(&mut *conn, branch_id).await?;
    let open_shift = cash::open_shift_id(&mut *conn, branch_id, register).await?;
    let open_shift_number = match open_shift {
        Some(s) => {
            sqlx::query_scalar!("select number from shifts where id = $1", s)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let (diff_before, withheld) = match &counted {
        Some(c) => (
            Some(diff_of(&mut *conn, c).await?),
            Some(withheld_of(&mut *conn, c.shift_id).await?),
        ),
        None => (None, None),
    };
    let correction_delta = if counted.is_some() { delta } else { 0 };
    let possible = match (&counted, diff_before, withheld) {
        (Some(c), Some(d), Some(w)) => withhold_change(d, correction_delta, w, true, c.at_close()),
        _ => 0,
    };
    let withhold = if req.fix_withholding { possible } else { 0 };
    let repaid = match orig.party_id {
        Some(pid) => repaid_of(&mut *conn, branch_id, id, pid).await?,
        None => 0,
    };
    let balance_before = party.as_ref().map_or(0, |p| p.balance_tyiyn);
    let old_text = payments_text(&mut *conn, id).await?;

    // ---- Проведение ----
    if let (Some(inn), Some(pid)) = (&inn_written, party_id) {
        sqlx::query!("update parties set inn = $2 where id = $1", pid, inn)
            .execute(&mut *conn)
            .await?;
        ops::audit(
            &mut *conn,
            ctx,
            "party.update",
            "party",
            Some(pid),
            json!({ "name": party.as_ref().map(|p| p.name.clone()),
                    "changes": { "inn": { "old": "", "new": inn } },
                    "source": "pay_change" }),
        )
        .await?;
    }
    let lines = sqlx::query!(
        "select line_no, qty, cost_tyiyn, cost_unknown, master_fee_tyiyn, product_id from sale_lines where sale_id = $1 order by line_no",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut replay = crate::api::sales::Replay {
        party_id: orig.party_id,
        ..Default::default()
    };
    let mut line_fees: i64 = 0;
    for l in &lines {
        if l.product_id.is_some() && !l.cost_unknown {
            replay.costs.insert(l.line_no, l.cost_tyiyn);
        }
        replay.fees.insert(l.line_no, l.master_fee_tyiyn);
        line_fees = line_fees.saturating_add(l.master_fee_tyiyn);
    }
    replay.check_fee = orig.master_fee_tyiyn.saturating_sub(line_fees).max(0);
    let mut moves: Vec<CashMove> = Vec::new();
    // Сторно — возврат всего чека теми же способами, каждая оплата на свой счёт, в учётный день
    // чека (как «Исправить», ADR-088). Наличные не выдаются — открытая смена не нужна.
    let pays = sqlx::query!(
        r#"select method, account_id, sum(amount_tyiyn)::bigint as "amount!" from sale_payments
           where sale_id = $1 group by method, account_id order by method, account_id"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let storno = crate::api::sales::return_sale_core(
        &mut *conn,
        ctx,
        id,
        crate::api::sales::ReturnReq {
            op_id: new_id(),
            comment: format!("изменение оплаты: {comment}"),
            lines: lines
                .iter()
                .map(|l| crate::api::sales::ReturnLineReq {
                    line_no: l.line_no,
                    qty: l.qty,
                })
                .collect(),
            payments: pays
                .into_iter()
                .map(|p| crate::api::sales::PaymentReq {
                    method: p.method,
                    amount_tyiyn: p.amount,
                    grams: None,
                    price_per_kg_tyiyn: None,
                    account_id: p.account_id,
                    bank_qr_id: None,
                })
                .collect(),
            battery_cash: false,
            business_date: Some(orig.business_date),
        },
        crate::api::sales::ReturnMode {
            pay_change: true,
            defer: Some(&mut moves),
        },
    )
    .await?;
    // Новый чек — те же строки, цены, мастер, машина, кассир и учётный день; меняется оплата.
    let mut doc = draft;
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| AppError::Internal("нет черновика чека".into()))?;
    obj.insert("op_id".into(), json!(new_id()));
    obj.insert("party_id".into(), json!(party_id));
    let mut all_payments = new_payments;
    all_payments.extend(fixed);
    let mut sale_req: crate::api::sales::SaleReq =
        serde_json::from_value(doc).map_err(|e| AppError::Internal(e.to_string()))?;
    sale_req.payments = all_payments;
    sale_req.offline = false;
    sale_req.client_time = None;
    sale_req.correction = Some(crate::api::sales::SaleCorrection {
        business_date: orig.business_date,
        replay: Some(replay),
    });
    let new_sale =
        crate::api::sales::post_sale_core(&mut *conn, ctx, sale_req, Some(&mut moves)).await?;
    let money_comment = format!("изменение оплаты чека № {}", orig.number);
    for m in moves.iter_mut() {
        m.comment = money_comment.clone();
    }
    // Наличные уже пересчитаны: остаток «Кассы» не меняется — поправка пересчёта той смены.
    if let Some(c) = &counted
        && delta != 0
    {
        moves.push(CashMove {
            account_id: register,
            kind: "count_diff",
            amount: -delta,
            doc_type: "shift",
            doc_id: Some(c.shift_id),
            comment: format!("поправка пересчёта: {money_comment}"),
        });
    }
    let accounts = account_effects(&mut *conn, &moves).await?;
    let count_movement_id =
        write_moves(&mut *conn, ctx, moves, counted.as_ref().map(|c| c.shift_id)).await?;
    let accrual_id = match &counted {
        Some(c) if withhold != 0 => {
            crate::api::payroll::add_accrual(
                &mut *conn,
                ctx,
                Some(c.business_date),
                crate::api::payroll::Accrual {
                    employee_id: c.cashier_id,
                    kind: "shortage",
                    amount: withhold,
                    base: None,
                    rule_id: None,
                    doc_type: "shift",
                    doc_id: Some(c.shift_id),
                    comment: &if withhold > 0 {
                        format!(
                            "возврат удержания: смена № {}, чек № {}",
                            c.number, orig.number
                        )
                    } else {
                        format!(
                            "недостача: смена № {}, изменение оплаты чека № {}",
                            c.number, orig.number
                        )
                    },
                },
            )
            .await?
        }
        _ => None,
    };
    let new_text = payments_text(&mut *conn, new_sale.id).await?;
    let mut effect = Effect {
        accounts,
        cash_delta_tyiyn: delta,
        counted: counted.clone(),
        diff_before_tyiyn: diff_before,
        diff_after_tyiyn: diff_before.map(|d| d.saturating_sub(correction_delta)),
        withheld_tyiyn: withheld,
        withhold_tyiyn: withhold,
        withhold_possible_tyiyn: possible,
        open_shift_number: if counted.is_none() {
            open_shift_number
        } else {
            None
        },
        ..Effect::default()
    };
    // Деньги, процент и прибыль — из проведённого: сторно, новый чек, поправка удержания.
    let totals = sqlx::query!(
        r#"select
             coalesce((select sum(l.amount_tyiyn) - sum(l.cost_tyiyn) from sale_lines l
                       where l.sale_id in ($1, $2)), 0)::bigint as "gross!",
             coalesce((select sum(p.fee_tyiyn) from sale_payments p where p.sale_id in ($1, $2)), 0)::bigint as "fee!",
             coalesce((select sum(p.amount_tyiyn) from sale_payments p
                       where p.sale_id in ($1, $2) and p.method = 'bonus'), 0)::bigint as "bonus!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where (a.doc_type = 'sale' and a.doc_id = $2)
                          or (a.doc_type = 'sale_return' and a.doc_id = $1)
                          or a.id = $3), 0)::bigint as "payroll!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.kind = 'revenue_percent'
                         and ((a.doc_type = 'sale' and a.doc_id = $2)
                              or (a.doc_type = 'sale_return' and a.doc_id = $1))), 0)::bigint as "percent!",
             exists (select 1 from payroll_pending x
                     where x.sale_id = $2 and x.debt_remaining_tyiyn > 0) as "pending!""#,
        storno.id,
        new_sale.id,
        accrual_id
    )
    .fetch_one(&mut *conn)
    .await?;
    effect.fee_tyiyn = totals.fee;
    effect.percent_tyiyn = totals.percent;
    effect.percent_pending = totals.pending;
    effect.profit_tyiyn = totals
        .gross
        .saturating_sub(totals.payroll)
        .saturating_sub(totals.fee)
        .saturating_sub(totals.bonus)
        .saturating_sub(correction_delta);
    if let Some(p) = &party {
        let after = sqlx::query_scalar!(
            r#"select coalesce((select balance_tyiyn from party_balances
                                where party_id = $1 and branch_id = $2), 0)::bigint as "v!""#,
            p.id,
            branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if after != balance_before || repaid > 0 || debt > 0 {
            effect.debt = Some(DebtEffect {
                party_name: p.name.clone(),
                before_tyiyn: balance_before,
                after_tyiyn: after,
                repaid_tyiyn: repaid,
            });
        }
    }
    effect.month_note = month_note(&mut *conn, orig.business_date).await?;
    let mut text = vec![format!("Оплата: {old_text} → {new_text}")];
    account_lines(&effect, &mut text);
    shift_lines(&effect, &mut text);
    if let Some(d) = &effect.debt {
        text.push(format!(
            "Долг клиента {}: {} → {}",
            d.party_name,
            format_som(d.before_tyiyn),
            format_som(d.after_tyiyn)
        ));
        if d.repaid_tyiyn > 0 {
            text.push(format!(
                "По чеку уже погашено {} — это аванс клиента: он гасит долг нового чека, а без долга остаётся авансом",
                format_som(d.repaid_tyiyn)
            ));
        }
    }
    if effect.percent_tyiyn != 0 {
        text.push(format!(
            "Процент кассира сейчас: {}",
            format_som_signed(effect.percent_tyiyn)
        ));
    }
    if effect.percent_pending {
        text.push("2 % с долговой части — по мере погашения".into());
    }
    profit_line(&effect, &mut text);
    if let Some(m) = &effect.month_note {
        text.push(format!("Чек за {m}"));
    }
    effect.lines = text;
    // Связь «исправлен → новый» и журнал: `correction.post` с пометкой «оплата».
    let shift_text = effect
        .lines
        .iter()
        .find(|l| l.starts_with("Смена") || l.starts_with("В кассе"))
        .cloned();
    let correction = crate::api::corrections::link_sale(
        &mut *conn,
        ctx,
        crate::api::corrections::SaleLink {
            op_id: req.op_id,
            original_id: id,
            before: &snapshot,
            reversal_id: storno.id,
            new_id: new_sale.id,
            new_number: Some(new_sale.number),
            comment: &comment,
            extra: json!({
                "payment": true,
                "pay_old": old_text,
                "pay_new": new_text,
                "shift_text": shift_text,
                "withhold": withhold,
            }),
        },
    )
    .await?;
    let fix_id = new_id();
    sqlx::query!(
        r#"insert into pay_fixes (id, branch_id, op_id, kind, correction_id, sale_id, reversal_sale_id,
                                  new_sale_id, counted_shift_id, counted_at, cash_delta_tyiyn,
                                  diff_before_tyiyn, diff_after_tyiyn, count_movement_id,
                                  withhold_tyiyn, accrual_id, comment, user_id, device_id)
           values ($1, $2, $3, 'sale', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)"#,
        fix_id,
        branch_id,
        req.op_id,
        correction.id,
        id,
        storno.id,
        new_sale.id,
        counted.as_ref().map(|c| c.shift_id),
        counted.as_ref().map(|c| c.at.clone()),
        delta,
        diff_before,
        effect.diff_after_tyiyn,
        count_movement_id,
        withhold,
        accrual_id,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    let out = PayChangeOut {
        correction: (!dry).then_some(correction),
        new_sale_id: (!dry).then_some(new_sale.id),
        new_number: (!dry).then_some(new_sale.number),
        effect,
    };
    if !dry {
        ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    }
    Ok(out)
}

// ---------- Из наличных в QR за смену ----------

/// Перенос «из наличных в QR» в списке отчёта смены.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TransferRow {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub user_name: String,
    pub amount_tyiyn: i64,
    pub fee_tyiyn: i64,
    pub account_id: Uuid,
    pub account_name: String,
    pub comment: String,
    /// Перенос отменён: когда и почему.
    pub reversed_at: Option<DateTime<Utc>>,
    pub reverse_comment: Option<String>,
    /// Пересчёт, который поправил перенос (смена была закрыта); пусто — менялся остаток «Кассы».
    pub counted: bool,
}

pub(crate) async fn transfers_of(
    conn: &mut PgConnection,
    shift_id: Uuid,
) -> AppResult<Vec<TransferRow>> {
    Ok(sqlx::query_as!(
        TransferRow,
        r#"select f.id, f.created_at, u.full_name as user_name, f.amount_tyiyn, f.fee_tyiyn,
                  f.account_id as "account_id!", a.name as account_name, f.comment,
                  r.created_at as "reversed_at?", r.comment as "reverse_comment?",
                  f.counted_shift_id is not null as "counted!"
           from pay_fixes f
           join users u on u.id = f.user_id
           join cash_accounts a on a.id = f.account_id
           left join pay_fixes r on r.reversal_of = f.id
           where f.kind = 'shift' and f.shift_id = $1 and f.reversal_of is null
           order by f.created_at"#,
        shift_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Сколько наличных по чекам смены можно перенести (SPEC-37 §3): наличные по чекам смены минус
/// возвраты наличными минус прежние переносы. Чеки — как в отчёте смены, с изменениями оплаты
/// для её пересчёта.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct CashLimit {
    pub cash_sales_tyiyn: i64,
    pub cash_returns_tyiyn: i64,
    pub moved_tyiyn: i64,
    pub limit_tyiyn: i64,
}

async fn cash_limit(
    conn: &mut PgConnection,
    branch_id: Uuid,
    shift: &cash::ShiftOut,
) -> AppResult<CashLimit> {
    let r = sqlx::query!(
        r#"select coalesce(sum(p.amount_tyiyn) filter (where s.kind = 'sale'), 0)::bigint as "sales!",
                  coalesce(-sum(p.amount_tyiyn) filter (where s.kind = 'return'), 0)::bigint as "returns!"
           from sale_payments p join sales s on s.id = p.sale_id
           where p.method = 'cash' and s.branch_id = $1
             and ((s.created_at >= $2 and s.created_at <= coalesce($3, now())
                   and not pay_fix_counted_sale(s.id))
                  or exists (select 1 from pay_fixes f where f.counted_shift_id = $4
                               and (f.reversal_sale_id = s.id or f.new_sale_id = s.id)))"#,
        branch_id,
        shift.opened_at,
        shift.closed_at,
        shift.id
    )
    .fetch_one(&mut *conn)
    .await?;
    let moved = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from pay_fixes
           where kind = 'shift' and shift_id = $1"#,
        shift.id
    )
    .fetch_one(&mut *conn)
    .await?;
    let limit = r
        .sales
        .saturating_sub(r.returns)
        .saturating_sub(moved)
        .max(0);
    Ok(CashLimit {
        cash_sales_tyiyn: r.sales,
        cash_returns_tyiyn: r.returns,
        moved_tyiyn: moved,
        limit_tyiyn: limit,
    })
}

/// Окно «Из наличных в QR»: предел, счета, состояние смены и прежние переносы.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CashToBankInfo {
    pub shift_id: Uuid,
    pub number: i64,
    pub business_date: NaiveDate,
    /// Смена открыта: меняется «Касса» и «Должно быть»; закрыта — расхождение закрытия.
    pub open: bool,
    pub cashier_name: String,
    #[serde(flatten)]
    pub limit: CashLimit,
    pub diff_tyiyn: Option<i64>,
    pub withheld_tyiyn: i64,
    pub accounts: Vec<BankAccountBrief>,
    pub items: Vec<TransferRow>,
}

async fn shift_in_branch(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<cash::ShiftOut> {
    cash::load_shift(conn, branch_id, id).await
}

async fn cash_to_bank_info(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CashToBankInfo>> {
    require_owner(&user)?;
    let mut conn = state.pool.acquire().await?;
    let shift = shift_in_branch(&mut conn, user.branch_id, id).await?;
    let limit = cash_limit(&mut conn, user.branch_id, &shift).await?;
    let open = shift.closed_at.is_none();
    let diff_tyiyn = if open {
        None
    } else {
        let c = counted_of(&mut conn, user.branch_id, id, "close").await?;
        Some(diff_of(&mut conn, &c).await?)
    };
    Ok(Json(CashToBankInfo {
        shift_id: id,
        number: shift.number,
        business_date: shift.business_date,
        open,
        cashier_name: shift.cashier_name.clone(),
        limit,
        diff_tyiyn,
        withheld_tyiyn: withheld_of(&mut conn, id).await?,
        accounts: bank_accounts(&mut conn, &user, shift.business_date).await?,
        items: transfers_of(&mut conn, id).await?,
    }))
}

#[derive(Deserialize)]
pub struct CashToBankReq {
    pub op_id: Uuid,
    pub amount_tyiyn: i64,
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub comment: String,
    #[serde(default = "yes")]
    pub fix_withholding: bool,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CashToBankOut {
    pub id: Option<Uuid>,
    pub effect: Effect,
}

async fn cash_to_bank(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CashToBankReq>,
) -> AppResult<Json<CashToBankOut>> {
    let dry = req.dry_run;
    let mut tx = state.pool.begin().await?;
    let out = cash_to_bank_tx(&mut tx, &ctx, id, req).await?;
    if dry {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(out))
}

/// Что меняет перенос (и его отмена): деньги, смена, удержание, прибыль. `amount` — со знаком
/// переноса: + из наличных на счёт, − обратно.
struct Move<'a> {
    shift: &'a cash::ShiftOut,
    register: Uuid,
    account_id: Uuid,
    amount: i64,
    fee: i64,
    counted: Option<Counted>,
    fix_withholding: bool,
    /// Отменяемый перенос: его поправка удержания сторнируется, а не считается заново.
    undo: Option<Undo>,
    fix_id: Uuid,
    comment: &'a str,
}

struct Undo {
    accrual_id: Option<Uuid>,
    /// Исходный перенос поправил пересчёт (смена была закрыта).
    counted: bool,
}

/// Поправка удержания переноса: сторно прежней, новая по правилу §1 или никакой.
#[derive(Clone, Copy)]
enum Hold {
    Nothing,
    Reverse(Uuid, i64),
    New(i64),
}

struct Moved {
    effect: Effect,
    count_movement_id: Option<Uuid>,
    accrual_id: Option<Uuid>,
}

async fn apply_transfer(conn: &mut PgConnection, ctx: &Ctx, m: Move<'_>) -> AppResult<Moved> {
    let delta = m.amount.saturating_neg();
    let (diff_before, withheld) = match &m.counted {
        Some(c) => (
            Some(diff_of(&mut *conn, c).await?),
            Some(withheld_of(&mut *conn, c.shift_id).await?),
        ),
        None => (None, None),
    };
    // Поправка удержания. Отмена переноса с поправкой — её сторно, ровно она; отмена переноса
    // для закрытой смены без поправки — ничего (обратное движение); отмена переноса открытой
    // смены, которую с тех пор закрыли, и сам перенос — по правилу §1.
    let hold = match (&m.undo, &m.counted, diff_before, withheld) {
        (Some(Undo { accrual_id: Some(a), .. }), _, _, _) => sqlx::query_scalar!(
            r#"select -amount_tyiyn as "v!" from payroll_accruals a
               where a.id = $1 and not exists (select 1 from payroll_accruals r where r.reversal_of = a.id)"#,
            a
        )
        .fetch_optional(&mut *conn)
        .await?
        .map_or(Hold::Nothing, |v| Hold::Reverse(*a, v)),
        (Some(Undo { counted: true, .. }), _, _, _) => Hold::Nothing,
        (_, Some(c), Some(d), Some(w)) => {
            Hold::New(withhold_change(d, delta, w, true, c.at_close()))
        }
        _ => Hold::Nothing,
    };
    let possible = match hold {
        Hold::Reverse(_, v) | Hold::New(v) => v,
        Hold::Nothing => 0,
    };
    let withhold = match hold {
        Hold::Reverse(_, v) => v,
        Hold::New(v) if m.fix_withholding => v,
        _ => 0,
    };
    let comment = format!("перенос из наличных, смена № {}", m.shift.number);
    let mut moves = vec![
        CashMove {
            account_id: m.register,
            kind: "cash_to_bank",
            amount: delta,
            doc_type: "pay_fix",
            doc_id: Some(m.fix_id),
            comment: comment.clone(),
        },
        CashMove {
            account_id: m.account_id,
            kind: "cash_to_bank",
            amount: m.amount,
            doc_type: "pay_fix",
            doc_id: Some(m.fix_id),
            comment: comment.clone(),
        },
    ];
    if m.fee != 0 {
        moves.push(CashMove {
            account_id: m.account_id,
            kind: "bank_fee",
            amount: -m.fee,
            doc_type: "pay_fix",
            doc_id: Some(m.fix_id),
            comment: comment.clone(),
        });
    }
    if let Some(c) = &m.counted {
        moves.push(CashMove {
            account_id: m.register,
            kind: "count_diff",
            amount: m.amount,
            doc_type: "shift",
            doc_id: Some(c.shift_id),
            comment: format!("поправка пересчёта: {comment}"),
        });
    }
    let accounts = account_effects(&mut *conn, &moves).await?;
    let count_movement_id = write_moves(
        &mut *conn,
        ctx,
        moves,
        m.counted.as_ref().map(|c| c.shift_id),
    )
    .await?;
    let accrual_id = match (hold, &m.counted) {
        // Отмена: сторно поправки удержания исходного переноса (SPEC-37 §3).
        (Hold::Reverse(a, _), _) => {
            let rid = new_id();
            let orig = sqlx::query!(
                "select employee_id, business_date, kind, amount_tyiyn from payroll_accruals where id = $1",
                a
            )
            .fetch_one(&mut *conn)
            .await?;
            sqlx::query!(
                r#"insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn,
                                                 doc_type, doc_id, comment, reversal_of, user_id, device_id)
                   values ($1, $2, $3, $4, $5, $6, 'reversal', $7, $8, $7, $9, $10)"#,
                rid,
                ctx.user.branch_id,
                orig.employee_id,
                orig.business_date,
                orig.kind,
                -orig.amount_tyiyn,
                a,
                format!("отмена переноса: {}", m.comment),
                ctx.user.id,
                ctx.device_id
            )
            .execute(&mut *conn)
            .await?;
            Some(rid)
        }
        (Hold::New(_), Some(c)) if withhold != 0 => {
            crate::api::payroll::add_accrual(
                &mut *conn,
                ctx,
                Some(c.business_date),
                crate::api::payroll::Accrual {
                    employee_id: c.cashier_id,
                    kind: "shortage",
                    amount: withhold,
                    base: None,
                    rule_id: None,
                    doc_type: "shift",
                    doc_id: Some(c.shift_id),
                    comment: &match (withhold > 0, m.undo.is_some()) {
                        (true, _) => format!(
                            "возврат удержания: смена № {}, перенос из наличных в QR",
                            c.number
                        ),
                        (false, true) => format!(
                            "недостача: смена № {}, отмена переноса из наличных в QR",
                            c.number
                        ),
                        (false, false) => {
                            format!("недостача: смена № {}, перенос из наличных в QR", c.number)
                        }
                    },
                },
            )
            .await?
        }
        _ => None,
    };
    let correction = if m.counted.is_some() { delta } else { 0 };
    let mut effect = Effect {
        accounts,
        cash_delta_tyiyn: delta,
        counted: m.counted.clone(),
        diff_before_tyiyn: diff_before,
        diff_after_tyiyn: diff_before.map(|d| d.saturating_sub(correction)),
        withheld_tyiyn: withheld,
        withhold_tyiyn: withhold,
        withhold_possible_tyiyn: possible,
        open_shift_number: if m.counted.is_none() && m.shift.closed_at.is_none() {
            Some(m.shift.number)
        } else {
            None
        },
        fee_tyiyn: m.fee,
        // Прибыль дня смены: комиссия, поправка пересчёта и поправка удержания (ADR-065).
        profit_tyiyn: m
            .fee
            .saturating_neg()
            .saturating_sub(correction)
            .saturating_sub(withhold),
        ..Effect::default()
    };
    effect.month_note = month_note(&mut *conn, m.shift.business_date).await?;
    let mut text = Vec::new();
    account_lines(&effect, &mut text);
    shift_lines(&effect, &mut text);
    profit_line(&effect, &mut text);
    if let Some(n) = &effect.month_note {
        text.push(format!("Смена за {n}"));
    }
    effect.lines = text;
    Ok(Moved {
        effect,
        count_movement_id,
        accrual_id,
    })
}

#[allow(clippy::too_many_arguments)]
async fn insert_transfer_fix(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    op_id: Uuid,
    shift_id: Uuid,
    account_id: Uuid,
    amount: i64,
    fee: i64,
    moved: &Moved,
    comment: &str,
    reversal_of: Option<Uuid>,
) -> AppResult<()> {
    let c = moved.effect.counted.as_ref();
    sqlx::query!(
        r#"insert into pay_fixes (id, branch_id, op_id, kind, shift_id, account_id, amount_tyiyn,
                                  fee_tyiyn, counted_shift_id, counted_at, cash_delta_tyiyn,
                                  diff_before_tyiyn, diff_after_tyiyn, count_movement_id,
                                  withhold_tyiyn, accrual_id, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, 'shift', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                   $17, $18, $19)"#,
        id,
        ctx.user.branch_id,
        op_id,
        shift_id,
        account_id,
        amount,
        fee,
        c.map(|c| c.shift_id),
        c.map(|c| c.at.clone()),
        moved.effect.cash_delta_tyiyn,
        moved.effect.diff_before_tyiyn,
        moved.effect.diff_after_tyiyn,
        moved.count_movement_id,
        moved.effect.withhold_tyiyn,
        moved.accrual_id,
        comment,
        reversal_of,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("перенос уже отменён".into())
        } else {
            AppError::from(e)
        }
    })?;
    Ok(())
}

pub async fn cash_to_bank_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    shift_id: Uuid,
    req: CashToBankReq,
) -> AppResult<CashToBankOut> {
    const KIND: &str = "shift.cash_to_bank";
    require_owner(&ctx.user)?;
    let dry = req.dry_run;
    let comment = reason(&req.comment, dry)?;
    if !dry && let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let shift = shift_in_branch(&mut *conn, branch_id, shift_id).await?;
    if req.amount_tyiyn <= 0 || req.amount_tyiyn > ops::MAX_AMOUNT_TYIYN {
        return Err(invalid("сумма переноса больше нуля"));
    }
    // Переносы одной смены — по очереди: предел считается под блокировкой кассы.
    sqlx::query!(
        "select id from cash_accounts where id = $1 for update",
        shift.account_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let limit = cash_limit(&mut *conn, branch_id, &shift).await?;
    if req.amount_tyiyn > limit.limit_tyiyn {
        return Err(invalid(format!(
            "перенести можно не больше {}: наличными по чекам смены {}, возвраты {}, уже перенесено {}",
            format_som(limit.limit_tyiyn),
            format_som(limit.cash_sales_tyiyn),
            format_som(limit.cash_returns_tyiyn),
            format_som(limit.moved_tyiyn)
        )));
    }
    let account_id = pay_account(&mut *conn, &ctx.user, "card", req.account_id).await?;
    let open =
        cash::open_shift_id(&mut *conn, branch_id, shift.account_id).await? == Some(shift_id);
    // Смена закрыта: наличные пересчитаны при её закрытии — поправка закрытия (§1).
    let counted = if open {
        None
    } else {
        Some(counted_of(&mut *conn, branch_id, shift_id, "close").await?)
    };
    let fee = crate::api::fees::fee_on(
        &mut *conn,
        account_id,
        shift.business_date,
        req.amount_tyiyn,
    )
    .await?;
    let id = new_id();
    let moved = apply_transfer(
        &mut *conn,
        ctx,
        Move {
            shift: &shift,
            register: shift.account_id,
            account_id,
            amount: req.amount_tyiyn,
            fee,
            counted,
            fix_withholding: req.fix_withholding,
            undo: None,
            fix_id: id,
            comment: &comment,
        },
    )
    .await?;
    insert_transfer_fix(
        &mut *conn,
        ctx,
        id,
        req.op_id,
        shift_id,
        account_id,
        req.amount_tyiyn,
        fee,
        &moved,
        &comment,
        None,
    )
    .await?;
    let account_name = moved
        .effect
        .accounts
        .iter()
        .find(|a| a.account_id == account_id)
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let c = moved.effect.counted.as_ref();
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "shift",
        Some(shift_id),
        json!({
            "number": shift.number,
            "amount": req.amount_tyiyn,
            "account": account_name,
            "fee": fee,
            "closed": c.is_some(),
            "diff_before": moved.effect.diff_before_tyiyn,
            "diff_after": moved.effect.diff_after_tyiyn,
            "withhold": moved.effect.withhold_tyiyn,
            "cashier": c.map(|c| c.cashier_name.clone()),
            "comment": comment,
        }),
    )
    .await?;
    let out = CashToBankOut {
        id: (!dry).then_some(id),
        effect: moved.effect,
    };
    if !dry {
        ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    }
    Ok(out)
}

#[derive(Deserialize)]
pub struct ReverseTransferReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    #[serde(default = "yes")]
    pub fix_withholding: bool,
    #[serde(default)]
    pub dry_run: bool,
}

async fn reverse_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseTransferReq>,
) -> AppResult<Json<CashToBankOut>> {
    let dry = req.dry_run;
    let mut tx = state.pool.begin().await?;
    let out = reverse_transfer_tx(&mut tx, &ctx, id, req).await?;
    if dry {
        tx.rollback().await?;
    } else {
        tx.commit().await?;
    }
    Ok(Json(out))
}

/// «Отменить перенос» — сторно (SPEC-37 §3): обратные движения счёта, комиссии, «Кассы» и
/// поправки; поправка удержания сторнируется. Перенос в открытой смене, которую с тех пор
/// закрыли, — наличные уже пересчитаны: «Касса» не меняется, меняется расхождение закрытия (§1).
pub async fn reverse_transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseTransferReq,
) -> AppResult<CashToBankOut> {
    const KIND: &str = "shift.cash_to_bank_reverse";
    require_owner(&ctx.user)?;
    let dry = req.dry_run;
    let comment = reason(&req.comment, dry)?;
    if !dry && let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let f = sqlx::query!(
        r#"select f.kind, f.shift_id as "shift_id!", f.account_id as "account_id!", f.amount_tyiyn,
                  f.fee_tyiyn, f.counted_shift_id, f.counted_at, f.accrual_id, f.reversal_of,
                  f.created_at
           from pay_fixes f where f.id = $1 and f.branch_id = $2 and f.kind = 'shift'"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if f.reversal_of.is_some() {
        return Err(AppError::Conflict("это уже отмена переноса".into()));
    }
    let shift = shift_in_branch(&mut *conn, branch_id, f.shift_id).await?;
    sqlx::query!(
        "select id from cash_accounts where id = $1 for update",
        shift.account_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if sqlx::query_scalar!(
        r#"select exists (select 1 from pay_fixes where reversal_of = $1) as "v!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?
    {
        return Err(AppError::Conflict("перенос уже отменён".into()));
    }
    // Поправленный пересчёт — тот же; перенос без пересчёта — первый пересчёт после него.
    let counted = match (f.counted_shift_id, f.counted_at.as_deref()) {
        (Some(s), Some(at)) => Some(counted_of(&mut *conn, branch_id, s, at).await?),
        _ => counted_after(&mut *conn, branch_id, shift.account_id, f.created_at).await?,
    };
    let undo = Undo {
        accrual_id: f.accrual_id,
        counted: f.counted_shift_id.is_some(),
    };
    let rid = new_id();
    let moved = apply_transfer(
        &mut *conn,
        ctx,
        Move {
            shift: &shift,
            register: shift.account_id,
            account_id: f.account_id,
            amount: -f.amount_tyiyn,
            fee: -f.fee_tyiyn,
            counted,
            fix_withholding: req.fix_withholding,
            undo: Some(undo),
            fix_id: rid,
            comment: &comment,
        },
    )
    .await?;
    insert_transfer_fix(
        &mut *conn,
        ctx,
        rid,
        req.op_id,
        f.shift_id,
        f.account_id,
        -f.amount_tyiyn,
        -f.fee_tyiyn,
        &moved,
        &comment,
        Some(id),
    )
    .await?;
    let account = sqlx::query_scalar!("select name from cash_accounts where id = $1", f.account_id)
        .fetch_one(&mut *conn)
        .await?;
    let c = moved.effect.counted.as_ref();
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "shift",
        Some(f.shift_id),
        json!({
            "number": shift.number,
            "amount": f.amount_tyiyn,
            "account": account,
            "fee": f.fee_tyiyn,
            "closed": c.is_some(),
            "diff_before": moved.effect.diff_before_tyiyn,
            "diff_after": moved.effect.diff_after_tyiyn,
            "withhold": moved.effect.withhold_tyiyn,
            "cashier": c.map(|c| c.cashier_name.clone()),
            "comment": comment,
        }),
    )
    .await?;
    let out = CashToBankOut {
        id: (!dry).then_some(rid),
        effect: moved.effect,
    };
    if !dry {
        ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    }
    Ok(out)
}

// ---------- Отчёт смены: «После закрытия» ----------

/// Изменение после пересчёта в отчёте смены (SPEC-37 §4).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AfterItem {
    pub id: Uuid,
    /// `sale` — изменение оплаты чека, `shift` — перенос из наличных в QR.
    pub kind: String,
    /// Отмена переноса.
    pub reversal: bool,
    pub created_at: DateTime<Utc>,
    pub user_name: String,
    pub original_id: Option<Uuid>,
    pub original_number: Option<i64>,
    pub new_id: Option<Uuid>,
    pub new_number: Option<i64>,
    /// Было → стало: оплата чека или перенос.
    pub old_text: String,
    pub new_text: String,
    pub cash_delta_tyiyn: i64,
    pub diff_before_tyiyn: Option<i64>,
    pub diff_after_tyiyn: Option<i64>,
    /// Поправка удержания: + вернули кассиру, − удержали.
    pub withhold_tyiyn: i64,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AfterBank {
    pub account_id: Uuid,
    pub name: String,
    pub amount_tyiyn: i64,
    pub fee_tyiyn: i64,
}

/// Итоги «при закрытии» и «сейчас».
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AfterTotals {
    /// Наличными по чекам смены.
    pub cash_sales_tyiyn: i64,
    /// Из наличных в QR — отдельной строкой: чеки перенос не меняет.
    pub cash_to_bank_tyiyn: i64,
    /// Безнал по счетам и комиссия.
    pub bank: Vec<AfterBank>,
    /// Расхождение пересчёта (< 0 — недостача).
    pub diff_tyiyn: i64,
    /// Удержано с кассира по смене.
    pub withheld_tyiyn: i64,
    /// Только владельцу: чистая прибыль смены (SPEC-36 §9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_profit_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AfterClose {
    /// `close` — после закрытия; `open` — поправки пересчёта при открытии смены.
    pub at: String,
    pub items: Vec<AfterItem>,
    pub before: AfterTotals,
    pub after: AfterTotals,
}

/// Блок «После закрытия» (SPEC-37 §4): изменения оплаты и переносы, поправившие пересчёт смены.
/// Основная часть отчёта — как при закрытии; здесь — что изменилось и итоги «при закрытии →
/// сейчас». Администратору — без прибыли (инвариант 6).
pub(crate) async fn after_close(
    conn: &mut PgConnection,
    branch_id: Uuid,
    owner: bool,
    shift: &cash::ShiftOut,
    cash_sales: i64,
    net_profit: Option<i64>,
) -> AppResult<Option<AfterClose>> {
    let rows = sqlx::query!(
        r#"select f.id, f.kind, f.reversal_of is not null as "reversal!", f.created_at,
                  u.full_name as user_name, f.sale_id, o.number as "original_number?",
                  f.new_sale_id, n.number as "new_number?", f.amount_tyiyn, f.fee_tyiyn,
                  a.name as "account?", f.counted_at as "counted_at!", f.cash_delta_tyiyn,
                  f.diff_before_tyiyn, f.diff_after_tyiyn, f.withhold_tyiyn, f.comment
           from pay_fixes f
           join users u on u.id = f.user_id
           left join sales o on o.id = f.sale_id
           left join sales n on n.id = f.new_sale_id
           left join cash_accounts a on a.id = f.account_id
           where f.branch_id = $1 and f.counted_shift_id = $2
           order by f.created_at, f.id"#,
        branch_id,
        shift.id
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let at = if rows.iter().any(|r| r.counted_at == "close") {
        "close"
    } else {
        "open"
    };
    let mut items = Vec::with_capacity(rows.len());
    let mut sale_delta: i64 = 0;
    let mut moved: i64 = 0;
    for r in rows {
        let (old_text, new_text) = match (r.kind.as_str(), r.sale_id, r.new_sale_id) {
            ("sale", Some(o), Some(n)) => (
                payments_text(&mut *conn, o).await?,
                payments_text(&mut *conn, n).await?,
            ),
            _ => {
                let account = r.account.clone().unwrap_or_default();
                if r.reversal {
                    (
                        format!("перенос в «{account}» {}", format_som(-r.amount_tyiyn)),
                        "отменён".into(),
                    )
                } else {
                    (
                        format!("наличные {}", format_som(r.amount_tyiyn)),
                        format!("в «{account}» {}", format_som(r.amount_tyiyn)),
                    )
                }
            }
        };
        if r.kind == "sale" {
            sale_delta = sale_delta.saturating_add(r.cash_delta_tyiyn);
        } else {
            moved = moved.saturating_add(r.amount_tyiyn);
        }
        items.push(AfterItem {
            id: r.id,
            kind: r.kind,
            reversal: r.reversal,
            created_at: r.created_at,
            user_name: r.user_name,
            original_id: r.sale_id,
            original_number: r.original_number,
            new_id: r.new_sale_id,
            new_number: r.new_number,
            old_text,
            new_text,
            cash_delta_tyiyn: r.cash_delta_tyiyn,
            diff_before_tyiyn: r.diff_before_tyiyn,
            diff_after_tyiyn: r.diff_after_tyiyn,
            withhold_tyiyn: r.withhold_tyiyn,
            comment: r.comment,
        });
    }
    // Безнал по счетам: при закрытии — как в отчёте; сейчас — плюс деньги изменений на счетах.
    let mut bank: BTreeMap<Uuid, AfterBank> = shift
        .bank_split
        .iter()
        .map(|b| {
            (
                b.account_id,
                AfterBank {
                    account_id: b.account_id,
                    name: b.name.clone(),
                    amount_tyiyn: b.total_tyiyn,
                    fee_tyiyn: b.fee_tyiyn,
                },
            )
        })
        .collect();
    let before_bank: Vec<AfterBank> = bank.values().cloned().collect();
    let fix_moves = sqlx::query!(
        r#"select m.account_id, a.name,
                  coalesce(sum(m.amount_tyiyn) filter (where m.kind <> 'bank_fee'), 0)::bigint as "amount!",
                  coalesce(-sum(m.amount_tyiyn) filter (where m.kind = 'bank_fee'), 0)::bigint as "fee!"
           from pay_fixes f
           join cash_movements m
             on (m.doc_type = 'sale' and (m.doc_id = f.reversal_sale_id or m.doc_id = f.new_sale_id))
             or (m.doc_type = 'pay_fix' and m.doc_id = f.id)
           join cash_accounts a on a.id = m.account_id
           where f.branch_id = $1 and f.counted_shift_id = $2 and a.kind = 'bank'
           group by m.account_id, a.name"#,
        branch_id,
        shift.id
    )
    .fetch_all(&mut *conn)
    .await?;
    for m in fix_moves {
        let e = bank.entry(m.account_id).or_insert_with(|| AfterBank {
            account_id: m.account_id,
            name: m.name.clone(),
            ..AfterBank::default()
        });
        e.amount_tyiyn = e.amount_tyiyn.saturating_add(m.amount);
        e.fee_tyiyn = e.fee_tyiyn.saturating_add(m.fee);
    }
    let corrections: i64 = items
        .iter()
        .filter(|i| i.diff_before_tyiyn.is_some())
        .fold(0i64, |a, i| a.saturating_sub(i.cash_delta_tyiyn));
    let withheld_now = withheld_of(&mut *conn, shift.id).await?;
    let fix_withhold: i64 = items
        .iter()
        .fold(0i64, |a, i| a.saturating_add(i.withhold_tyiyn));
    let diff_before = if at == "close" {
        shift.diff_tyiyn.unwrap_or(0)
    } else {
        sqlx::query_scalar!(
            r#"select coalesce(sum(m.amount_tyiyn), 0)::bigint as "v!"
               from cash_movements m join shifts s on s.id = m.doc_id
               where m.kind = 'count_diff' and m.doc_type = 'shift' and m.doc_id = $1
                 and m.created_at = s.opened_at"#,
            shift.id
        )
        .fetch_one(&mut *conn)
        .await?
    };
    let profit_fix = if owner {
        Some(
            crate::api::reports::fixes_totals(&mut *conn, branch_id, shift.id)
                .await?
                .net_tyiyn,
        )
    } else {
        None
    };
    let before = AfterTotals {
        cash_sales_tyiyn: cash_sales,
        cash_to_bank_tyiyn: 0,
        bank: before_bank,
        diff_tyiyn: diff_before,
        // Возврат удержания (+) уменьшает удержанное: до изменений было больше.
        withheld_tyiyn: withheld_now.saturating_add(fix_withhold),
        net_profit_tyiyn: net_profit.filter(|_| owner),
    };
    let after = AfterTotals {
        cash_sales_tyiyn: cash_sales.saturating_add(sale_delta),
        cash_to_bank_tyiyn: moved,
        bank: bank.into_values().collect(),
        diff_tyiyn: diff_before.saturating_add(corrections),
        withheld_tyiyn: withheld_now,
        net_profit_tyiyn: match (net_profit, profit_fix) {
            (Some(p), Some(f)) if owner => Some(p.saturating_add(f)),
            _ => None,
        },
    };
    Ok(Some(AfterClose {
        at: at.into(),
        items,
        before,
        after,
    }))
}

/// Блок «После закрытия» словами — для чека смены и бота (SPEC-37 §4).
pub fn after_close_lines(a: &AfterClose) -> Vec<String> {
    let mut out = vec![if a.at == "close" {
        "✏️ После закрытия:".to_string()
    } else {
        "✏️ Поправки пересчёта при открытии:".to_string()
    }];
    for i in &a.items {
        let what = match (i.kind.as_str(), i.original_number, i.new_number) {
            ("sale", Some(o), Some(n)) => format!("чек № {o} → № {n}"),
            _ if i.reversal => "отмена переноса".to_string(),
            _ => "перенос".to_string(),
        };
        let mut line = format!(
            "  {} {} · {what}: {} → {}",
            (i.created_at + chrono::Duration::hours(6)).format("%d.%m %H:%M"),
            i.user_name,
            i.old_text,
            i.new_text
        );
        if i.withhold_tyiyn != 0 {
            line.push_str(&format!(
                "; кассиру {}",
                format_som_signed(i.withhold_tyiyn)
            ));
        }
        line.push_str(&format!(". Причина: {}", i.comment));
        out.push(line);
    }
    let (b, n) = (&a.before, &a.after);
    out.push(format!(
        "  наличными по чекам: {} → {}",
        format_som(b.cash_sales_tyiyn),
        format_som(n.cash_sales_tyiyn)
    ));
    if n.cash_to_bank_tyiyn != 0 {
        out.push(format!(
            "  из наличных в QR: {}",
            format_som(n.cash_to_bank_tyiyn)
        ));
    }
    for nb in &n.bank {
        let old = b
            .bank
            .iter()
            .find(|x| x.account_id == nb.account_id)
            .map_or((0, 0), |x| (x.amount_tyiyn, x.fee_tyiyn));
        if old != (nb.amount_tyiyn, nb.fee_tyiyn) {
            out.push(format!(
                "  {}: {} → {}, комиссия {} → {}",
                nb.name,
                format_som(old.0),
                format_som(nb.amount_tyiyn),
                format_som(old.1),
                format_som(nb.fee_tyiyn)
            ));
        }
    }
    out.push(format!(
        "  расхождение: {} → {}",
        diff_text(b.diff_tyiyn),
        diff_text(n.diff_tyiyn)
    ));
    out.push(format!(
        "  удержано с кассира: {} → {}",
        format_som(b.withheld_tyiyn),
        format_som(n.withheld_tyiyn)
    ));
    if let (Some(x), Some(y)) = (b.net_profit_tyiyn, n.net_profit_tyiyn) {
        out.push(format!(
            "  чистая прибыль: {} → {}",
            format_som(x),
            format_som(y)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Таблица примеров SPEC-37 §1 (наличные → QR и обратно, сумма 1 000 с).
    #[test]
    fn withholding_follows_the_examples() {
        let s = 100_000;
        // Недостача 1 000, удержано 1 000; наличные → QR: вернуть 1 000.
        assert_eq!(withhold_change(-s, -s, s, true, true), s);
        // Удержание прощено: возвращать нечего.
        assert_eq!(withhold_change(-s, -s, 0, true, true), 0);
        // Недостача 300, удержано 300: излишек 700, вернуть 300.
        assert_eq!(withhold_change(-30_000, -s, 30_000, true, true), 30_000);
        // Без расхождения: излишек 1 000, кассиру ничего.
        assert_eq!(withhold_change(0, -s, 0, true, true), 0);
        // QR → наличные без расхождения: недостача 1 000 — удержать; без галочки — нет.
        assert_eq!(withhold_change(0, s, 0, true, true), -s);
        assert_eq!(withhold_change(0, s, 0, false, true), 0);
        // Излишек 1 000, в долг → наличные: расхождение 0, кассиру ничего.
        assert_eq!(withhold_change(s, s, 0, true, true), 0);
        // Пересчёт при открытии — недостачу не удерживают.
        assert_eq!(withhold_change(-s, -s, s, true, false), 0);
        // Недостача уменьшилась, а удержано меньше уменьшения — не больше удержанного.
        assert_eq!(withhold_change(-s, -s, 40_000, true, true), 40_000);
    }
}
