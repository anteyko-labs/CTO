//! Прибыль и сводка владельца (SPEC-08). Администратору не отдаётся (инвариант 13).

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/reports/profit", routing::get(profit))
        .route("/owner/dashboard", routing::get(dashboard))
        .route("/reports/gifts", routing::get(gifts))
}

#[derive(Serialize, Default)]
pub(crate) struct Totals {
    pub(crate) goods_tyiyn: i64,
    pub(crate) services_tyiyn: i64,
    pub(crate) cost_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
    pub(crate) payroll_tyiyn: i64,
    pub(crate) bank_fee_tyiyn: i64,
    pub(crate) expenses_tyiyn: i64,
    /// Оплачено баллами за вычетом возвращённых — скидка клиентам (SPEC-19).
    pub(crate) bonus_tyiyn: i64,
    pub(crate) net_tyiyn: i64,
    pub(crate) margin_bp: Option<i64>,
    pub(crate) sales_count: i64,
}

#[derive(Serialize)]
struct CategoryRow {
    name: String,
    revenue_tyiyn: i64,
    cost_tyiyn: i64,
    gross_tyiyn: i64,
}

#[derive(Serialize)]
struct DayRow {
    date: NaiveDate,
    revenue_tyiyn: i64,
    gross_tyiyn: i64,
    payroll_tyiyn: i64,
    expenses_tyiyn: i64,
    /// Комиссия банка и скидки баллами — чтобы «чистая» сходилась на глаз.
    fee_tyiyn: i64,
    bonus_tyiyn: i64,
    net_tyiyn: i64,
}

/// Часы одного дня — для графика «Сегодня».
#[derive(Serialize)]
struct HourRow {
    hour: i32,
    revenue_tyiyn: i64,
    gross_tyiyn: i64,
    sales_count: i64,
}

#[derive(Serialize)]
struct ArticleRow {
    name: String,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct ProfitOut {
    from: NaiveDate,
    to: NaiveDate,
    totals: Totals,
    categories: Vec<CategoryRow>,
    days: Vec<DayRow>,
    articles: Vec<ArticleRow>,
    warnings: Vec<String>,
    /// Кто сколько заработал за период.
    staff: Vec<crate::api::payroll::StaffPay>,
    /// По часам — только когда период один день.
    hours: Vec<HourRow>,
}

#[derive(Deserialize)]
struct Period {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Деньги за период: выручка, себестоимость, оплата труда, комиссия и расходы.
pub(crate) async fn totals_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Totals> {
    let lines = sqlx::query!(
        r#"select
             coalesce(sum(l.amount_tyiyn) filter (where l.kind <> 'service'), 0)::bigint as "goods!",
             coalesce(sum(l.amount_tyiyn) filter (where l.kind = 'service'), 0)::bigint as "services!",
             coalesce(sum(l.cost_tyiyn), 0)::bigint as "cost!"
           from sale_lines l join sales s on s.id = l.sale_id
           where s.branch_id = $1 and s.business_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    // Переоценка остатков (ADR-044): списанная стоимость — тоже себестоимость.
    let revaluation = sqlx::query_scalar!(
        r#"select coalesce(-sum(value_delta_tyiyn), 0)::bigint as "v!" from stock_movements
           where branch_id = $1 and doc_type in ('revaluation', 'revision')
             and (created_at at time zone 'Asia/Bishkek')::date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    let count = sqlx::query_scalar!(
        r#"select count(*) as "n!" from sales
           where branch_id = $1 and business_date between $2 and $3 and kind = 'sale'"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    let payroll = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from payroll_accruals
           where branch_id = $1 and business_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    // Комиссия банка: с чеков и с погашений долга картой или переводом (ADR-022).
    let fee = sqlx::query_scalar!(
        r#"select (coalesce((select sum(p.fee_tyiyn) from sale_payments p join sales s on s.id = p.sale_id
                             where s.branch_id = $1 and s.business_date between $2 and $3), 0)
                 + coalesce((select -sum(m.amount_tyiyn) from cash_movements m
                             where m.branch_id = $1 and m.kind = 'bank_fee' and m.doc_type = 'repayment'
                               and (m.created_at at time zone 'Asia/Bishkek')::date between $2 and $3), 0)
                 )::bigint as "v!""#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    let expenses = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from expenses
           where branch_id = $1 and expense_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    let bonus = sqlx::query_scalar!(
        r#"select coalesce(sum(p.amount_tyiyn), 0)::bigint as "v!" from sale_payments p join sales s on s.id = p.sale_id
           where s.branch_id = $1 and s.business_date between $2 and $3 and p.method = 'bonus'"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    // Суммы из базы; насыщение вместо паники на заведомо нереальных значениях.
    let revenue = lines.goods.saturating_add(lines.services);
    let cost = lines.cost.saturating_add(revaluation);
    let gross = revenue.saturating_sub(cost);
    Ok(Totals {
        goods_tyiyn: lines.goods,
        services_tyiyn: lines.services,
        cost_tyiyn: cost,
        gross_tyiyn: gross,
        payroll_tyiyn: payroll,
        bank_fee_tyiyn: fee,
        expenses_tyiyn: expenses,
        bonus_tyiyn: bonus,
        net_tyiyn: gross
            .saturating_sub(payroll)
            .saturating_sub(fee)
            .saturating_sub(expenses)
            .saturating_sub(bonus),
        margin_bp: (revenue != 0)
            .then(|| div_round(i128::from(gross) * 10_000, i128::from(revenue)))
            .flatten(),
        sales_count: count,
    })
}

async fn profit(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<Period>,
) -> AppResult<Json<ProfitOut>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    let today = today(&mut conn).await?;
    let to = q.to.unwrap_or(today);
    let from = q.from.unwrap_or(to);
    if from > to {
        return Err(invalid("начало периода позже конца"));
    }
    if (to - from).num_days() > 366 {
        return Err(invalid("период не больше года"));
    }
    let branch_id = user.branch_id;
    let totals = totals_for(&mut conn, branch_id, from, to).await?;

    let categories = sqlx::query!(
        r#"select c.name as "name!",
             coalesce(sum(l.amount_tyiyn), 0)::bigint as "revenue!",
             coalesce(sum(l.cost_tyiyn), 0)::bigint as "cost!"
           from sale_lines l
           join sales s on s.id = l.sale_id
           join products p on p.id = l.product_id
           join categories c on c.id = p.category_id
           where s.branch_id = $1 and s.business_date between $2 and $3
           group by c.name order by 2 desc"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| CategoryRow {
        name: r.name,
        revenue_tyiyn: r.revenue,
        cost_tyiyn: r.cost,
        gross_tyiyn: r.revenue - r.cost,
    })
    .collect();

    let days = sqlx::query!(
        r#"with d as (select generate_series($2::date, $3::date, interval '1 day')::date as day)
           select d.day as "day!",
             coalesce((select sum(l.amount_tyiyn) from sale_lines l join sales s on s.id = l.sale_id
                       where s.branch_id = $1 and s.business_date = d.day), 0)::bigint as "revenue!",
             (coalesce((select sum(l.amount_tyiyn - l.cost_tyiyn) from sale_lines l join sales s on s.id = l.sale_id
                        where s.branch_id = $1 and s.business_date = d.day), 0)
              + coalesce((select sum(m.value_delta_tyiyn) from stock_movements m
                          where m.branch_id = $1 and m.doc_type in ('revaluation', 'revision')
                            and (m.created_at at time zone 'Asia/Bishkek')::date = d.day), 0))::bigint as "gross!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.branch_id = $1 and a.business_date = d.day), 0)::bigint as "payroll!",
             coalesce((select sum(e.amount_tyiyn) from expenses e
                       where e.branch_id = $1 and e.expense_date = d.day), 0)::bigint as "expenses!",
             (coalesce((select sum(p.fee_tyiyn) from sale_payments p join sales s on s.id = p.sale_id
                        where s.branch_id = $1 and s.business_date = d.day), 0)
              + coalesce((select -sum(m.amount_tyiyn) from cash_movements m
                          where m.branch_id = $1 and m.kind = 'bank_fee' and m.doc_type = 'repayment'
                            and (m.created_at at time zone 'Asia/Bishkek')::date = d.day), 0))::bigint as "fee!",
             coalesce((select sum(p.amount_tyiyn) from sale_payments p join sales s on s.id = p.sale_id
                       where s.branch_id = $1 and s.business_date = d.day and p.method = 'bonus'), 0)::bigint as "bonus!"
           from d order by d.day desc"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| DayRow {
        date: r.day,
        revenue_tyiyn: r.revenue,
        gross_tyiyn: r.gross,
        payroll_tyiyn: r.payroll,
        expenses_tyiyn: r.expenses,
        fee_tyiyn: r.fee,
        bonus_tyiyn: r.bonus,
        net_tyiyn: r
            .gross
            .saturating_sub(r.payroll)
            .saturating_sub(r.expenses)
            .saturating_sub(r.fee)
            .saturating_sub(r.bonus),
    })
    .collect();

    let articles = sqlx::query!(
        r#"select a.name as "name!", coalesce(sum(e.amount_tyiyn), 0)::bigint as "amount!"
           from expenses e join expense_articles a on a.id = e.article_id
           where e.branch_id = $1 and e.expense_date between $2 and $3
           group by a.name having coalesce(sum(e.amount_tyiyn), 0) <> 0 order by 2 desc"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| ArticleRow {
        name: r.name,
        amount_tyiyn: r.amount,
    })
    .collect();

    let mut warnings = Vec::new();
    let review = sqlx::query_scalar!(
        r#"select count(*) as "n!" from branch_products where branch_id = $1 and needs_review"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if review > 0 {
        warnings.push(format!(
            "у {review} товаров себестоимость оценена по последней закупке: проверьте остатки"
        ));
    }
    let pending = sqlx::query_scalar!(
        r#"select count(*) as "n!" from payroll_pending
           where branch_id = $1 and debt_remaining_tyiyn > 0"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if pending > 0 {
        warnings.push(format!(
            "по {pending} чекам в долг процент кассира начислится после погашения"
        ));
    }

    let staff = crate::api::payroll::staff_pay(&mut conn, branch_id, from, to).await?;
    let hours = if from == to {
        sqlx::query!(
            r#"select extract(hour from s.created_at at time zone 'Asia/Bishkek')::int as "hour!",
                      coalesce(sum(l.amount_tyiyn), 0)::bigint as "revenue!",
                      coalesce(sum(l.amount_tyiyn - l.cost_tyiyn), 0)::bigint as "gross!",
                      count(distinct s.id) filter (where s.kind = 'sale') as "count!"
               from sales s join sale_lines l on l.sale_id = s.id
               where s.branch_id = $1 and s.business_date = $2
               group by 1 order by 1"#,
            branch_id,
            from
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| HourRow {
            hour: r.hour,
            revenue_tyiyn: r.revenue,
            gross_tyiyn: r.gross,
            sales_count: r.count,
        })
        .collect()
    } else {
        Vec::new()
    };
    Ok(Json(ProfitOut {
        from,
        to,
        totals,
        categories,
        days,
        articles,
        warnings,
        staff,
        hours,
    }))
}

#[derive(Serialize)]
struct AccountRow {
    name: String,
    balance_tyiyn: i64,
}

#[derive(Serialize)]
struct Dashboard {
    date: NaiveDate,
    totals: Totals,
    returns_tyiyn: i64,
    average_check_tyiyn: i64,
    accounts: Vec<AccountRow>,
    money_total_tyiyn: i64,
    shift_open: bool,
    shift_cashier: Option<String>,
    to_pay_tyiyn: i64,
    debts_in_tyiyn: i64,
    debts_out_tyiyn: i64,
    low_stock: i64,
    needs_review: i64,
    /// Залежалые: остаток есть, продаж нет дольше срока (ADR-039).
    stale_stock: i64,
    /// Кто сколько заработал сегодня.
    staff: Vec<crate::api::payroll::StaffPay>,
}

async fn dashboard(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Dashboard>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    let date = today(&mut conn).await?;
    let branch_id = user.branch_id;
    let totals = totals_for(&mut conn, branch_id, date, date).await?;
    let returns = sqlx::query_scalar!(
        r#"select coalesce(sum(-total_tyiyn), 0)::bigint as "v!" from sales
           where branch_id = $1 and business_date = $2 and kind = 'return'"#,
        branch_id,
        date
    )
    .fetch_one(&mut *conn)
    .await?;
    let accounts: Vec<AccountRow> = sqlx::query!(
        r#"select name as "name!", balance_tyiyn as "balance!" from cash_accounts
           where branch_id = $1 and active order by is_default desc, kind, name"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| AccountRow {
        name: r.name,
        balance_tyiyn: r.balance,
    })
    .collect();
    let shift = sqlx::query!(
        r#"select e.full_name as "cashier!" from shifts s
           join employees e on e.id = s.cashier_employee_id
           where s.branch_id = $1
             and not exists (select 1 from shift_closes c
                             where c.shift_id = s.id
                               and not exists (select 1 from shift_reopens r where r.close_id = c.id))
           order by s.opened_at desc limit 1"#,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let to_pay = sqlx::query_scalar!(
        r#"select coalesce(sum(b), 0)::bigint as "v!" from (
             select coalesce((select sum(a.amount_tyiyn) from payroll_accruals a where a.employee_id = e.id), 0)
                  - coalesce((select sum(p.amount_tyiyn) from payouts p where p.employee_id = e.id), 0) as b
             from employees e where e.branch_id = $1
           ) x where b > 0"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let debts = sqlx::query!(
        r#"select
             coalesce(sum(balance_tyiyn) filter (where balance_tyiyn > 0), 0)::bigint as "owe_us!",
             coalesce(sum(-balance_tyiyn) filter (where balance_tyiyn < 0), 0)::bigint as "we_owe!"
           from parties where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let stock = sqlx::query!(
        r#"select
             coalesce(count(*) filter (where stock_qty < min_stock), 0)::bigint as "low!",
             coalesce(count(*) filter (where needs_review), 0)::bigint as "review!"
           from branch_products where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let (_, stale) = crate::api::receipts::stale_list(&mut conn, branch_id).await?;
    let revenue = totals.goods_tyiyn + totals.services_tyiyn;
    let staff = crate::api::payroll::staff_pay(&mut conn, branch_id, date, date).await?;
    Ok(Json(Dashboard {
        staff,
        date,
        average_check_tyiyn: if totals.sales_count > 0 {
            div_round(i128::from(revenue), i128::from(totals.sales_count)).unwrap_or(0)
        } else {
            0
        },
        totals,
        returns_tyiyn: returns,
        money_total_tyiyn: accounts
            .iter()
            .fold(0i64, |acc, a| acc.saturating_add(a.balance_tyiyn)),
        accounts,
        shift_open: shift.is_some(),
        shift_cashier: shift.map(|s| s.cashier),
        to_pay_tyiyn: to_pay,
        debts_in_tyiyn: debts.owe_us,
        debts_out_tyiyn: debts.we_owe,
        low_stock: stock.low,
        needs_review: stock.review,
        stale_stock: i64::try_from(stale.len()).unwrap_or(i64::MAX),
    }))
}

#[derive(Deserialize)]
struct GiftsQuery {
    from: NaiveDate,
    to: NaiveDate,
}

#[derive(Serialize)]
struct GiftRow {
    product_id: Uuid,
    name: String,
    unit: String,
    /// Подарено за вычетом возвратов: штук или мл.
    qty: i64,
    checks: i64,
    cost_tyiyn: i64,
}

/// Отчёт по подаркам: что и сколько подарили и во что это обошлось (SPEC-11). Только владелец.
async fn gifts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<GiftsQuery>,
) -> AppResult<Json<Vec<GiftRow>>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if q.from > q.to {
        return Err(invalid("начало периода позже конца"));
    }
    let rows = sqlx::query_as!(
        GiftRow,
        r#"select p.id as "product_id!", p.name as "name!", p.unit as "unit!",
                  coalesce(sum(case when s.kind = 'return' then -l.units else l.units end), 0)::bigint as "qty!",
                  count(distinct s.id) filter (where s.kind = 'sale') as "checks!",
                  coalesce(sum(l.cost_tyiyn), 0)::bigint as "cost_tyiyn!"
           from sale_lines l
           join sales s on s.id = l.sale_id
           join products p on p.id = l.product_id
           where l.gift and s.branch_id = $1 and s.business_date between $2 and $3
           group by p.id, p.name, p.unit
           order by 6 desc"#,
        user.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}
