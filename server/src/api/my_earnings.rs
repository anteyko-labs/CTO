//! «Мой заработок» (задача 005 Н1, ADR-170): вход, связанный с сотрудником (мастер, кассир,
//! администратор), видит в программе только своё — сколько заработал, сколько ему выплачено и
//! сколько ему должны. Права «Расчёт» не нужно: выборка ограничена сотрудником самого входа,
//! чужого сотрудника запросить нельзя. Расчёт — тот же, что в кабинете работника в боте
//! (`api::bot_staff`) и на экране «Расчёт» (`api::payroll::month_data`).

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{Months, NaiveDate};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::bot_staff::{self, Employee};
use crate::api::payroll;
use crate::auth::CurrentUser;
use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

/// На сколько месяцев назад листается «Мой заработок» — как «Месяц» в боте.
const MONTHS_BACK: u32 = 12;

pub fn routes() -> Router<AppState> {
    Router::new().route("/me/earnings", routing::get(my_earnings))
}

#[derive(Deserialize)]
struct EarningsQuery {
    /// Любой день месяца; без него — текущий месяц.
    month: Option<NaiveDate>,
}

#[derive(Serialize)]
struct DayAmount {
    date: NaiveDate,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct PayoutRow {
    date: NaiveDate,
    amount_tyiyn: i64,
    /// Вид выплаты: общая, проценты, мастеру, бонус, оклад, аванс (SPEC-07, ADR-122).
    purpose: String,
    /// Отмена прежней выплаты (сторно).
    reversal: bool,
}

#[derive(Serialize)]
struct EarningsOut {
    employee_id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
    today: NaiveDate,
    /// Заработано сегодня и виды словами — как «💼 Сегодня» в боте.
    today_tyiyn: i64,
    today_lines: Vec<String>,
    month_from: NaiveDate,
    month_to: NaiveDate,
    /// Месяцы, которые можно открыть: от текущего назад.
    months: Vec<NaiveDate>,
    /// Должны на начало месяца, начислено, выплачено (авансы тоже), должны на конец месяца.
    opening_tyiyn: i64,
    accrued_tyiyn: i64,
    paid_tyiyn: i64,
    advance_tyiyn: i64,
    closing_tyiyn: i64,
    /// Начисления месяца по видам словами — как «📅 Месяц» в боте.
    lines: Vec<String>,
    /// Оклад за период: начислен, авансом, выдано, к выплате; без оклада — пусто.
    salary_line: Option<String>,
    days: Vec<DayAmount>,
    payouts: Vec<PayoutRow>,
    /// Сколько должны сейчас: всё начисленное минус всё выданное.
    owed_now_tyiyn: i64,
}

async fn my_earnings(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<EarningsQuery>,
) -> AppResult<Json<EarningsOut>> {
    // Только вход с сотрудником; чужого сотрудника в запросе нет — смотреть можно лишь своё.
    let employee_id = user.employee_id.ok_or(AppError::NotFound)?;
    let mut conn = state.pool.acquire().await?;
    let r = sqlx::query!(
        "select branch_id, full_name, is_cashier, is_master from employees where id = $1",
        employee_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let e = Employee {
        id: employee_id,
        branch_id: r.branch_id,
        name: r.full_name,
        is_cashier: r.is_cashier,
        is_master: r.is_master,
    };
    let today = bot_staff::today(&mut conn).await?;
    let current = bot_staff::first_of(today);
    let earliest = current
        .checked_sub_months(Months::new(MONTHS_BACK))
        .unwrap_or(current);
    let m = q.month.map_or(current, bot_staff::first_of);
    if m > current || m < earliest {
        return Err(invalid(format!(
            "месяц — не раньше чем {MONTHS_BACK} месяцев назад и не позже текущего"
        )));
    }
    let mut months = Vec::new();
    let mut x = current;
    while x >= earliest {
        months.push(x);
        match x.checked_sub_months(Months::new(1)) {
            Some(prev) => x = prev,
            None => break,
        }
    }

    // Сегодня — те же строки, что «💼 Сегодня» в боте.
    let mut today_lines = Vec::new();
    let mut today_tyiyn = 0;
    if let Some(p) = bot_staff::mine(&mut conn, &e, today, today).await? {
        bot_staff::push_items(&mut conn, &e, (today, today), &p.items, &mut today_lines).await?;
        today_tyiyn = p.total_tyiyn;
    }

    // Месяц — строка экрана «Расчёт» этого сотрудника и виды начислений, как «📅 Месяц» в боте.
    let data = payroll::month_data(&mut conn, e.branch_id, Some(m)).await?;
    let row = data.rows.iter().find(|r| r.employee_id == e.id);
    let mut lines = Vec::new();
    if let Some(p) = bot_staff::mine(&mut conn, &e, data.from, data.to).await? {
        bot_staff::push_items(&mut conn, &e, (data.from, data.to), &p.items, &mut lines).await?;
    }
    let lines = lines.into_iter().map(|l| l.trim().to_string()).collect();
    let today_lines = today_lines
        .into_iter()
        .map(|l| l.trim().to_string())
        .collect();
    let on = if m == current { today } else { data.from };
    let salary_line = bot_staff::salary_line(&mut conn, &e, on).await?;
    let days = payroll::employee_days(&mut conn, e.branch_id, e.id, data.from, data.to)
        .await?
        .into_iter()
        .map(|(date, amount_tyiyn)| DayAmount { date, amount_tyiyn })
        .collect();
    let payouts = sqlx::query_as!(
        PayoutRow,
        r#"select pay_date as date, amount_tyiyn, purpose, reversal_of is not null as "reversal!"
           from payouts
           where employee_id = $1 and branch_id = $2 and pay_date between $3 and $4
           order by pay_date, created_at"#,
        e.id,
        e.branch_id,
        data.from,
        data.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let owed_now_tyiyn = sqlx::query_scalar!(
        r#"select (coalesce((select sum(amount_tyiyn) from payroll_accruals where employee_id = $1), 0)
                  - coalesce((select sum(amount_tyiyn) from payouts where employee_id = $1), 0))::bigint as "owed!""#,
        e.id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Json(EarningsOut {
        employee_id: e.id,
        full_name: e.name,
        is_cashier: e.is_cashier,
        is_master: e.is_master,
        today,
        today_tyiyn,
        today_lines,
        month_from: data.from,
        month_to: data.to,
        months,
        opening_tyiyn: row.map_or(0, |r| r.opening_tyiyn),
        accrued_tyiyn: row.map_or(0, |r| r.accrued_tyiyn),
        paid_tyiyn: row.map_or(0, |r| r.paid_tyiyn),
        advance_tyiyn: row.map_or(0, |r| r.advance_tyiyn),
        closing_tyiyn: row.map_or(0, |r| r.closing_tyiyn),
        lines,
        salary_line,
        days,
        payouts,
        owed_now_tyiyn,
    }))
}
