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
        .route("/reports/heatmap", routing::get(heatmap))
        .route("/reports/top-products", routing::get(top_products))
}

#[derive(Serialize, Default)]
pub struct Totals {
    pub goods_tyiyn: i64,
    pub services_tyiyn: i64,
    pub cost_tyiyn: i64,
    pub gross_tyiyn: i64,
    pub payroll_tyiyn: i64,
    /// В том числе оклады за месяц (начисления вида `monthly_salary`) — расшифровка строки
    /// «Зарплата сотрудникам», в чистую прибыль отдельно не входит.
    pub salary_tyiyn: i64,
    pub bank_fee_tyiyn: i64,
    pub expenses_tyiyn: i64,
    /// Оплачено баллами за вычетом возвращённых — скидка клиентам (SPEC-19).
    pub bonus_tyiyn: i64,
    /// Недостача (минус) или излишек (плюс) кассы по пересчётам смен (ADR-065).
    pub cash_diff_tyiyn: i64,
    /// «Списание брака»: стоимость списанного брака, убыток (SPEC-25).
    pub defect_writeoff_tyiyn: i64,
    /// «Брак: разница возмещения»: возмещение поставщика минус стоимость брака — минус убыток,
    /// плюс доход (SPEC-25).
    pub defect_diff_tyiyn: i64,
    pub net_tyiyn: i64,
    pub margin_bp: Option<i64>,
    pub sales_count: i64,
}

#[derive(Serialize)]
pub(crate) struct CategoryRow {
    pub(crate) name: String,
    pub(crate) revenue_tyiyn: i64,
    pub(crate) cost_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct DayRow {
    pub(crate) date: NaiveDate,
    pub(crate) revenue_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
    pub(crate) payroll_tyiyn: i64,
    pub(crate) expenses_tyiyn: i64,
    /// Комиссия банка и скидки баллами — чтобы «чистая» сходилась на глаз.
    pub(crate) fee_tyiyn: i64,
    pub(crate) bonus_tyiyn: i64,
    /// Недостача (минус) или излишек (плюс) кассы.
    pub(crate) cash_diff_tyiyn: i64,
    /// Списание брака и разница возмещения (SPEC-25).
    pub(crate) defect_writeoff_tyiyn: i64,
    pub(crate) defect_diff_tyiyn: i64,
    pub(crate) net_tyiyn: i64,
}

/// Часы одного дня — для графика «Сегодня».
#[derive(Serialize)]
pub(crate) struct HourRow {
    pub(crate) hour: i32,
    pub(crate) revenue_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
    pub(crate) sales_count: i64,
}

#[derive(Serialize)]
pub(crate) struct ArticleRow {
    pub(crate) name: String,
    pub(crate) amount_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct ProfitOut {
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
    pub(crate) totals: Totals,
    pub(crate) categories: Vec<CategoryRow>,
    pub(crate) days: Vec<DayRow>,
    pub(crate) articles: Vec<ArticleRow>,
    pub(crate) warnings: Vec<String>,
    /// Кто сколько заработал за период.
    pub(crate) staff: Vec<crate::api::payroll::StaffPay>,
    /// По часам — только когда период один день.
    pub(crate) hours: Vec<HourRow>,
}

#[derive(Deserialize)]
struct Period {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

pub(crate) async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Период отчёта: без дат — сегодня, без начала — один день `to`; не больше года (366 дней).
/// Один для прибыли, сводки, «Куда сколько поступило» и экрана «Счёт» (SPEC-27 §6).
pub(crate) async fn resolve_period(
    conn: &mut PgConnection,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<(NaiveDate, NaiveDate)> {
    let to = match to {
        Some(d) => d,
        None => today(&mut *conn).await?,
    };
    let from = from.unwrap_or(to);
    if from > to {
        return Err(invalid("начало периода позже конца"));
    }
    if (to - from).num_days() > 366 {
        return Err(invalid("период не больше года"));
    }
    Ok((from, to))
}

/// Деньги за период: выручка, себестоимость, оплата труда, комиссия и расходы.
/// Открыт для проверок: ответ бота на вопрос сверяется с этим расчётом (SPEC-21).
pub async fn totals_for(
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
    let pay = sqlx::query!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "all!",
                  coalesce(sum(amount_tyiyn) filter (where kind = 'monthly_salary'), 0)::bigint as "salary!"
           from payroll_accruals
           where branch_id = $1 and business_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    let payroll = pay.all;
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
    // Недостача и излишек кассы — по учётному дню смены: и пересчёт при открытии, и при
    // закрытии. Удержание с кассира идёт в оплату труда, поэтому удержанная недостача прибыль
    // не меняет, а прощённая уменьшает её (ADR-065).
    let cash_diff = sqlx::query_scalar!(
        r#"select coalesce(sum(m.amount_tyiyn), 0)::bigint as "v!"
           from cash_movements m join shifts s on s.id = m.doc_id
           where m.branch_id = $1 and m.kind = 'count_diff' and m.doc_type = 'shift'
             and s.business_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    // Брак (SPEC-25) — по дню решения владельца; отмена решения входит со своим знаком.
    let defects = sqlx::query!(
        r#"select coalesce(sum(writeoff_tyiyn), 0)::bigint as "writeoff!",
                  coalesce(sum(diff_tyiyn), 0)::bigint as "diff!"
           from defect_resolutions
           where branch_id = $1 and (created_at at time zone 'Asia/Bishkek')::date between $2 and $3"#,
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
        salary_tyiyn: pay.salary,
        bank_fee_tyiyn: fee,
        expenses_tyiyn: expenses,
        bonus_tyiyn: bonus,
        cash_diff_tyiyn: cash_diff,
        defect_writeoff_tyiyn: defects.writeoff,
        defect_diff_tyiyn: defects.diff,
        net_tyiyn: gross
            .saturating_sub(payroll)
            .saturating_sub(fee)
            .saturating_sub(expenses)
            .saturating_sub(bonus)
            .saturating_add(cash_diff)
            .saturating_sub(defects.writeoff)
            .saturating_add(defects.diff),
        margin_bp: (revenue != 0)
            .then(|| div_round(i128::from(gross) * 10_000, i128::from(revenue)))
            .flatten(),
        sales_count: count,
    })
}

/// Расходы по статьям за период, с личными: для отчёта и для вопросов боту (SPEC-21).
pub(crate) async fn articles_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<ArticleRow>> {
    Ok(sqlx::query!(
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
    .collect())
}

/// Выручка, себестоимость и валовая по категориям товаров за период: отчёт о прибыли и
/// «Куда сколько поступило» (SPEC-27 §6). Работы (услуги) в категории не входят.
pub(crate) async fn categories_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<CategoryRow>> {
    Ok(sqlx::query!(
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
    .collect())
}

async fn profit(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<Period>,
) -> AppResult<Json<ProfitOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(profit_data(&mut conn, &user, q.from, q.to).await?))
}

/// Отчёт о прибыли за период — экран и бот владельца (SPEC-24) берут его отсюда.
pub(crate) async fn profit_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<ProfitOut> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let (from, to) = resolve_period(&mut *conn, from, to).await?;
    let branch_id = user.branch_id;
    let totals = totals_for(&mut *conn, branch_id, from, to).await?;

    let categories = categories_for(&mut *conn, branch_id, from, to).await?;

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
                       where s.branch_id = $1 and s.business_date = d.day and p.method = 'bonus'), 0)::bigint as "bonus!",
             coalesce((select sum(m.amount_tyiyn) from cash_movements m join shifts s on s.id = m.doc_id
                       where m.branch_id = $1 and m.kind = 'count_diff' and m.doc_type = 'shift'
                         and s.business_date = d.day), 0)::bigint as "cash_diff!",
             coalesce((select sum(x.writeoff_tyiyn) from defect_resolutions x
                       where x.branch_id = $1
                         and (x.created_at at time zone 'Asia/Bishkek')::date = d.day), 0)::bigint as "defect_writeoff!",
             coalesce((select sum(x.diff_tyiyn) from defect_resolutions x
                       where x.branch_id = $1
                         and (x.created_at at time zone 'Asia/Bishkek')::date = d.day), 0)::bigint as "defect_diff!"
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
        cash_diff_tyiyn: r.cash_diff,
        defect_writeoff_tyiyn: r.defect_writeoff,
        defect_diff_tyiyn: r.defect_diff,
        net_tyiyn: r
            .gross
            .saturating_sub(r.payroll)
            .saturating_sub(r.expenses)
            .saturating_sub(r.fee)
            .saturating_sub(r.bonus)
            .saturating_add(r.cash_diff)
            .saturating_sub(r.defect_writeoff)
            .saturating_add(r.defect_diff),
    })
    .collect();

    let articles = articles_for(&mut *conn, branch_id, from, to).await?;

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
    // Строки, проданные без цены закупки: в прибыли их себестоимость — ноль (ADR-062).
    let no_cost = sqlx::query_scalar!(
        r#"select count(*) as "n!" from sale_lines l join sales s on s.id = l.sale_id
           where s.branch_id = $1 and s.kind = 'sale' and l.cost_unknown
             and s.business_date between $2 and $3"#,
        branch_id,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;
    if no_cost > 0 {
        warnings.push(format!("есть продажи без себестоимости: {no_cost}"));
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

    let staff = crate::api::payroll::staff_pay(&mut *conn, branch_id, from, to).await?;
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
    Ok(ProfitOut {
        from,
        to,
        totals,
        categories,
        days,
        articles,
        warnings,
        staff,
        hours,
    })
}

#[derive(Serialize)]
pub(crate) struct AccountRow {
    pub(crate) name: String,
    pub(crate) balance_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct Dashboard {
    /// Последний день выбранного периода (раньше — всегда сегодня).
    pub(crate) date: NaiveDate,
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
    pub(crate) today: NaiveDate,
    pub(crate) totals: Totals,
    pub(crate) returns_tyiyn: i64,
    pub(crate) average_check_tyiyn: i64,
    /// Остатки касс — всегда на сейчас.
    pub(crate) accounts: Vec<AccountRow>,
    pub(crate) money_total_tyiyn: i64,
    pub(crate) shift_open: bool,
    pub(crate) shift_cashier: Option<String>,
    pub(crate) to_pay_tyiyn: i64,
    pub(crate) debts_in_tyiyn: i64,
    pub(crate) debts_out_tyiyn: i64,
    pub(crate) low_stock: i64,
    pub(crate) needs_review: i64,
    /// Залежалые: остаток есть, продаж нет дольше срока (ADR-039).
    pub(crate) stale_stock: i64,
    /// Кто сколько заработал за период.
    pub(crate) staff: Vec<crate::api::payroll::StaffPay>,
    /// «Сколько по карте ушло»: безнал чеков за период (SPEC-27 §6).
    pub(crate) noncash: crate::api::income::Noncash,
    /// Один день: сравнение с предыдущим днём и тем же днём неделю назад.
    pub(crate) compare: Option<Compare>,
    /// Период: те же числа за предыдущий период такой же длины.
    pub(crate) previous: Option<PeriodPart>,
}

#[derive(Serialize, Default)]
pub(crate) struct DayPart {
    pub(crate) revenue_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
    pub(crate) sales_count: i64,
}

#[derive(Serialize, Default)]
pub(crate) struct Compare {
    pub(crate) today: DayPart,
    pub(crate) yesterday: DayPart,
    pub(crate) week_ago: DayPart,
    /// Чистая прибыль предыдущего дня за весь день: зарплата и расходы считаются по дням,
    /// поэтому «к этому часу» для чистой честно не посчитать.
    pub(crate) yesterday_net_tyiyn: i64,
    /// Сегодня — «к этому часу»; прошедший день сравнивается целиком.
    pub(crate) until_now: bool,
}

/// Итоги предыдущего периода для сравнения — из того же `totals_for`.
#[derive(Serialize)]
pub(crate) struct PeriodPart {
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
    pub(crate) revenue_tyiyn: i64,
    pub(crate) gross_tyiyn: i64,
    pub(crate) net_tyiyn: i64,
    pub(crate) sales_count: i64,
}

/// Продажи дня: целиком или до того же времени суток, что сейчас («к вчера на этот час»).
async fn day_part(
    conn: &mut PgConnection,
    branch_id: Uuid,
    day: NaiveDate,
    until_now: bool,
) -> AppResult<DayPart> {
    let r = sqlx::query!(
        r#"select coalesce(sum(l.amount_tyiyn), 0)::bigint as "revenue!",
                  coalesce(sum(l.amount_tyiyn - l.cost_tyiyn), 0)::bigint as "gross!",
                  count(distinct s.id) filter (where s.kind = 'sale') as "count!"
           from sales s join sale_lines l on l.sale_id = s.id
           where s.branch_id = $1 and s.business_date = $2
             and (not $3 or (s.created_at at time zone 'Asia/Bishkek')::time <= (now() at time zone 'Asia/Bishkek')::time)"#,
        branch_id,
        day,
        until_now
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(DayPart {
        revenue_tyiyn: r.revenue,
        gross_tyiyn: r.gross,
        sales_count: r.count,
    })
}

#[derive(Serialize)]
pub(crate) struct HeatCell {
    /// День недели: 1 — понедельник … 7 — воскресенье.
    pub(crate) dow: i32,
    pub(crate) hour: i32,
    pub(crate) revenue_tyiyn: i64,
    pub(crate) sales_count: i64,
}

#[derive(Deserialize)]
struct HeatQuery {
    days: Option<i64>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

/// Когда идут продажи: сумма по дню недели и часу за последние N дней (по умолчанию 28)
/// или за выбранный на сводке период.
async fn heatmap(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<HeatQuery>,
) -> AppResult<Json<Vec<HeatCell>>> {
    let mut conn = state.pool.acquire().await?;
    let range = if q.from.is_some() || q.to.is_some() {
        Some(resolve_period(&mut conn, q.from, q.to).await?)
    } else {
        None
    };
    Ok(Json(heatmap_data(&mut conn, &user, q.days, range).await?))
}

/// Продажи по дню недели и часу — экран и бот владельца (SPEC-24).
pub(crate) async fn heatmap_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    days: Option<i64>,
    range: Option<(NaiveDate, NaiveDate)>,
) -> AppResult<Vec<HeatCell>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let (from, to) = match range {
        Some(r) => r,
        None => {
            let to = today(&mut *conn).await?;
            let days = days.unwrap_or(28).clamp(7, 366);
            (to - chrono::Duration::days(days - 1), to)
        }
    };
    let rows = sqlx::query!(
        r#"select extract(isodow from s.created_at at time zone 'Asia/Bishkek')::int as "dow!",
                  extract(hour from s.created_at at time zone 'Asia/Bishkek')::int as "hour!",
                  coalesce(sum(s.total_tyiyn), 0)::bigint as "revenue!",
                  count(*) filter (where s.kind = 'sale') as "count!"
           from sales s
           where s.branch_id = $1 and s.business_date between $2 and $3
           group by 1, 2"#,
        user.branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| HeatCell {
        dow: r.dow,
        hour: r.hour,
        revenue_tyiyn: r.revenue,
        sales_count: r.count,
    })
    .collect();
    Ok(rows)
}

async fn dashboard(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<Period>,
) -> AppResult<Json<Dashboard>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(dashboard_data(&mut conn, &user, q.from, q.to).await?))
}

/// Сводка владельца за день или период (без дат — сегодня) — экран и бот владельца (SPEC-24,
/// SPEC-27 §6). Деньги, итоги и заработок — из тех же функций, что отчёт о прибыли.
pub(crate) async fn dashboard_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<Dashboard> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let (from, to) = resolve_period(&mut *conn, from, to).await?;
    let today = today(&mut *conn).await?;
    let branch_id = user.branch_id;
    let totals = totals_for(&mut *conn, branch_id, from, to).await?;
    let returns = sqlx::query_scalar!(
        r#"select coalesce(sum(-total_tyiyn), 0)::bigint as "v!" from sales
           where branch_id = $1 and business_date between $2 and $3 and kind = 'return'"#,
        branch_id,
        from,
        to
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
        // «Заканчивается» — как на складе: архивные и влитые карточки не считаем (они не
        // продаются и в списке не видны), иначе число на сводке не сходится со складом.
        r#"select
             coalesce(count(*) filter (where bp.stock_qty < bp.min_stock and not p.archived), 0)::bigint as "low!",
             coalesce(count(*) filter (where bp.needs_review), 0)::bigint as "review!"
           from branch_products bp join products p on p.id = bp.product_id
           where bp.branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let (_, stale) = crate::api::receipts::stale_list(&mut *conn, branch_id).await?;
    let revenue = totals.goods_tyiyn.saturating_add(totals.services_tyiyn);
    let (compare, previous) = if from == to {
        // Сегодня — «к этому часу»; прошедший день — целиком.
        let until_now = to == today;
        let prev = to - chrono::Duration::days(1);
        (
            Some(Compare {
                today: day_part(&mut *conn, branch_id, to, until_now).await?,
                yesterday: day_part(&mut *conn, branch_id, prev, until_now).await?,
                week_ago: day_part(
                    &mut *conn,
                    branch_id,
                    to - chrono::Duration::days(7),
                    until_now,
                )
                .await?,
                yesterday_net_tyiyn: totals_for(&mut *conn, branch_id, prev, prev)
                    .await?
                    .net_tyiyn,
                until_now,
            }),
            None,
        )
    } else {
        let len = (to - from).num_days() + 1;
        let (pf, pt) = (
            from - chrono::Duration::days(len),
            from - chrono::Duration::days(1),
        );
        let p = totals_for(&mut *conn, branch_id, pf, pt).await?;
        (
            None,
            Some(PeriodPart {
                from: pf,
                to: pt,
                revenue_tyiyn: p.goods_tyiyn.saturating_add(p.services_tyiyn),
                gross_tyiyn: p.gross_tyiyn,
                net_tyiyn: p.net_tyiyn,
                sales_count: p.sales_count,
            }),
        )
    };
    let staff = crate::api::payroll::staff_pay(&mut *conn, branch_id, from, to).await?;
    let noncash = crate::api::income::noncash_for(&mut *conn, branch_id, from, to).await?;
    Ok(Dashboard {
        compare,
        previous,
        staff,
        noncash,
        date: to,
        from,
        to,
        today,
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
    })
}

#[derive(Deserialize)]
pub(crate) struct GiftsQuery {
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
}

#[derive(Serialize)]
pub(crate) struct GiftRow {
    pub(crate) product_id: Uuid,
    pub(crate) name: String,
    pub(crate) unit: String,
    /// Подарено за вычетом возвратов: штук или мл.
    pub(crate) qty: i64,
    pub(crate) checks: i64,
    pub(crate) cost_tyiyn: i64,
}

/// Отчёт по подаркам: что и сколько подарили и во что это обошлось (SPEC-11). Только владелец.
async fn gifts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<GiftsQuery>,
) -> AppResult<Json<Vec<GiftRow>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(gifts_data(&mut conn, &user, q).await?))
}

/// Подарки за период — экран и бот владельца (SPEC-24).
pub(crate) async fn gifts_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: GiftsQuery,
) -> AppResult<Vec<GiftRow>> {
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
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

// ---------- Самые продаваемые товары (SPEC-27 §4) ----------

#[derive(Deserialize, Default)]
pub struct TopQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub category_id: Option<Uuid>,
    /// Что покупает клиент: без дат — за всё время.
    pub party_id: Option<Uuid>,
    /// `qty` — по количеству, `checks` — по числу чеков (как часто покупают), иначе по сумме.
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TopProduct {
    pub product_id: Uuid,
    pub name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub category_id: Uuid,
    /// Продано за вычетом возвратов: штуки, мл (канистры и розлив вместе) или граммы.
    pub qty: i64,
    /// Выручка за вычетом возвратов.
    pub amount_tyiyn: i64,
    /// Чеков с этим товаром.
    pub checks: i64,
    /// Валовая прибыль: выручка минус себестоимость строк, за вычетом возвратов (SPEC-28 §4).
    /// Только владельцу: администратору поле не отдаётся вовсе (инвариант 13).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profit_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TopProducts {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub items: Vec<TopProduct>,
}

/// Отчёт открыт и администратору: в нём выручка и количество, без себестоимости и маржи
/// (инвариант 13). «Больше приносят» (`sort=profit`) и прибыль строк — только владельцу.
async fn top_products(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<TopQuery>,
) -> AppResult<Json<TopProducts>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(top_products_data(&mut conn, &user, q).await?))
}

/// Самые продаваемые товары за период — экран, карточка клиента и бот владельца.
pub async fn top_products_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: TopQuery,
) -> AppResult<TopProducts> {
    let to = match q.to {
        Some(d) => d,
        None => today(conn).await?,
    };
    let from = match (q.from, q.party_id) {
        (Some(d), _) => d,
        // Клиент: за всё время.
        (None, Some(_)) => NaiveDate::from_ymd_opt(2000, 1, 1).unwrap_or(to),
        (None, None) => to
            .checked_sub_signed(chrono::Duration::days(29))
            .unwrap_or(to),
    };
    if from > to {
        return Err(invalid("начало периода позже конца"));
    }
    if q.party_id.is_none() && (to - from).num_days() > 366 {
        return Err(invalid("период не больше года"));
    }
    let sort = q.sort.unwrap_or_default();
    let owner = user.is_owner();
    if sort == "profit" && !owner {
        return Err(AppError::Forbidden);
    }
    let mut items = sqlx::query_as!(
        TopProduct,
        r#"select p.id as "product_id!", p.name as "name!", p.unit as "unit!",
                  p.container_ml as "container_ml?", p.category_id as "category_id!",
                  coalesce(sum(case when s.kind = 'return' then -l.units else l.units end), 0)::bigint as "qty!",
                  coalesce(sum(l.amount_tyiyn), 0)::bigint as "amount_tyiyn!",
                  count(distinct s.id) filter (where s.kind = 'sale') as "checks!",
                  coalesce(sum(l.amount_tyiyn - l.cost_tyiyn), 0)::bigint as "profit_tyiyn?"
           from sale_lines l
           join sales s on s.id = l.sale_id
           join products p on p.id = l.product_id
           where s.branch_id = $1 and s.business_date between $2 and $3 and not l.gift
             and ($4::uuid is null or p.category_id = $4)
             and ($5::uuid is null or s.party_id = $5)
           group by p.id, p.name, p.unit, p.container_ml, p.category_id
           having sum(case when s.kind = 'return' then -l.units else l.units end) > 0
           order by case when $6 = 'qty' then sum(case when s.kind = 'return' then -l.units else l.units end) end desc nulls last,
                    case when $6 = 'checks' then count(distinct s.id) filter (where s.kind = 'sale') end desc nulls last,
                    case when $6 = 'profit' then sum(l.amount_tyiyn - l.cost_tyiyn) end desc nulls last,
                    sum(l.amount_tyiyn) desc, p.name
           limit $7"#,
        user.branch_id,
        from,
        to,
        q.category_id,
        q.party_id,
        sort,
        q.limit.unwrap_or(50).clamp(1, 500)
    )
    .fetch_all(&mut *conn)
    .await?;
    if !owner {
        for i in &mut items {
            i.profit_tyiyn = None;
        }
    }
    Ok(TopProducts { from, to, items })
}
