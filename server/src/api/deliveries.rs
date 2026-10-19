//! Что привёз поставщик и что из этого осталось (SPEC-26, ADR-069).
//!
//! Склад ведётся по средней без партий (инвариант 3), поэтому доля остатка от поставщика —
//! только оценка: остаток раскладывается по приходам от новых к старым («первым продаётся
//! пришедшее раньше»). Количества и суммы закупок видят владелец и администратор (ADR-008),
//! себестоимость остатка — только владелец (инвариант 13).

use std::collections::{BTreeMap, HashMap};

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/suppliers/{id}/deliveries", routing::get(deliveries))
        .route(
            "/suppliers/{id}/deliveries/{product_id}",
            routing::get(product_receipts),
        )
}

/// Сколько из остатка `stock` пришло от поставщика, если первым продаётся пришедшее раньше.
/// `inflows` — приходы товара от новых к старым: (от этого ли поставщика, количество).
/// Результат не больше остатка и не больше `cap` (привёз и получил взамен брака минус вернули).
pub fn fifo_estimate(stock: i64, inflows: &[(bool, i64)], cap: i64) -> i64 {
    let mut left = stock.max(0);
    let mut mine: i64 = 0;
    for &(ours, qty) in inflows {
        if left <= 0 {
            break;
        }
        let take = qty.max(0).min(left);
        left -= take;
        if ours {
            mine = mine.saturating_add(take);
        }
    }
    mine.min(cap.max(0)).min(stock.max(0))
}

/// Приход товара на склад: накладная или товар взамен брака (у него накладной нет).
pub struct Inflow {
    pub supplier_id: Option<Uuid>,
    pub receipt_id: Option<Uuid>,
    pub qty: i64,
}

/// Все приходы товаров `ids` (основных карточек, с влитыми в них) от всех поставщиков и товар
/// взамен брака — от новых к старым. Общая основа оценок «первым продаётся пришедшее раньше»:
/// поставки поставщика (SPEC-26) и проданное по накладным (SPEC-27 §6).
pub(crate) async fn inflows(
    conn: &mut PgConnection,
    branch_id: Uuid,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, Vec<Inflow>>> {
    let rows = sqlx::query!(
        r#"with recursive fam (id, root, depth) as (
             select id, id, 0 from products where merged_into is null
             union all
             select p.id, fam.root, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           )
           select u.product_id as "product_id!", u.supplier_id, u.receipt_id, u.qty as "qty!"
           from (
             select fam.root as product_id, r.supplier_id, r.id as receipt_id, l.qty,
                    r.created_at as at, l.line_no as ord
             from receipt_lines l join receipts r on r.id = l.receipt_id
             join fam on fam.id = l.product_id
             where r.branch_id = $1 and fam.root = any($2) and l.qty > 0
               and r.reversal_of is null
               and not exists (select 1 from receipts x where x.reversal_of = r.id)
             union all
             select fam.root, x.supplier_id, null::uuid, x.replace_qty, x.created_at, 0
             from defect_resolutions x join defects d on d.id = x.defect_id
             join fam on fam.id = d.product_id
             where x.branch_id = $1 and fam.root = any($2) and x.kind = 'replace'
               and x.reversal_of is null and x.replace_qty > 0
               and not exists (select 1 from defect_resolutions y where y.reversal_of = x.id)
           ) u
           order by u.at desc, u.ord desc"#,
        branch_id,
        ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: HashMap<Uuid, Vec<Inflow>> = HashMap::new();
    for f in rows {
        out.entry(f.product_id).or_default().push(Inflow {
            supplier_id: f.supplier_id,
            receipt_id: f.receipt_id,
            qty: f.qty,
        });
    }
    Ok(out)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeliveryRow {
    pub product_id: Uuid,
    pub name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// Накладных за период.
    pub receipts: i64,
    /// «Привёз»: проведённые накладные без сторнированных, в единицах товара.
    pub delivered_qty: i64,
    /// «Вернули»: возвраты брака этому поставщику (SPEC-25) и возвраты товара (SPEC-29 §9).
    pub returned_qty: i64,
    /// «Сумма поставок» — закупка, её видит и администратор (ADR-008).
    pub amount_tyiyn: i64,
    /// Последняя цена: за штуку, за канистру, за килограмм.
    pub last_price_tyiyn: Option<i64>,
    pub last_at: Option<DateTime<Utc>>,
    /// Весь остаток товара на точке, от всех поставщиков.
    pub stock_qty: i64,
    /// Оценка: сколько из остатка пришло от этого поставщика.
    pub estimate_qty: i64,
    /// Себестоимость этой оценки по средней — только владельцу.
    pub estimate_value_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeliveriesOut {
    pub supplier_id: Uuid,
    pub supplier_name: String,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    pub rows: Vec<DeliveryRow>,
    /// Сумма поставок за период.
    pub amount_tyiyn: i64,
    /// Баланс: меньше нуля — мы должны поставщику, больше — он нам (возмещения брака).
    pub balance_tyiyn: i64,
    /// Брак этого поставщика, ожидающий решения владельца.
    pub pending_defects: i64,
    /// Его стоимость — только владельцу.
    pub pending_defects_tyiyn: Option<i64>,
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

async fn deliveries(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<DeliveriesOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(
        deliveries_data(&mut conn, &user, id, q.from, q.to).await?,
    ))
}

/// Единиц в цене: штука, канистра масла, килограмм.
fn price_per(unit: &str, container_ml: Option<i64>) -> i64 {
    match (unit, container_ml) {
        ("g", _) => 1000,
        ("ml", Some(c)) if c > 0 => c,
        _ => 1,
    }
}

#[derive(Default)]
struct Acc {
    name: String,
    unit: String,
    container_ml: Option<i64>,
    receipts: std::collections::BTreeSet<Uuid>,
    delivered: i64,
    amount: i64,
    last: Option<(DateTime<Utc>, i64, i64)>,
    all_delivered: i64,
    returned: i64,
    all_returned: i64,
    all_replaced: i64,
    seen: bool,
}

/// Поставки поставщика по товарам — экран карточки и бот владельца (SPEC-24).
pub(crate) async fn deliveries_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    supplier_id: Uuid,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<DeliveriesOut> {
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(invalid("начало периода позже конца"));
    }
    let branch_id = user.branch_id;
    let owner = user.is_owner();
    let supplier_name =
        sqlx::query_scalar!("select name from suppliers where id = $1", supplier_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
    // Строки проведённых накладных без сторнированных: отменённая поставка не считается.
    // Приход на карточку, влитую потом в другую, считается за основную (ADR-044, ADR-070).
    let lines = sqlx::query!(
        r#"with recursive fam (id, root, depth) as (
             select id, id, 0 from products where merged_into is null
             union all
             select p.id, fam.root, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           )
           select fam.root as "product_id!", p.name, p.unit, p.container_ml, r.id as receipt_id, r.created_at,
                  l.line_no, l.qty, l.cost_tyiyn,
                  (($1::date is null or r.created_at >= ($1::date)::timestamp at time zone 'Asia/Bishkek')
                   and ($2::date is null or r.created_at < ($2::date + 1)::timestamp at time zone 'Asia/Bishkek'))
                    as "in_period!"
           from receipt_lines l
           join receipts r on r.id = l.receipt_id
           join fam on fam.id = l.product_id
           join products p on p.id = fam.root
           where r.branch_id = $3 and r.supplier_id = $4 and r.reversal_of is null
             and not exists (select 1 from receipts x where x.reversal_of = r.id)
           order by r.created_at, l.line_no"#,
        from,
        to,
        branch_id,
        supplier_id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Возвраты брака этому поставщику (решения, которые не отменены) и возвраты товара.
    let returns = sqlx::query!(
        r#"with recursive fam (id, root, depth) as (
             select id, id, 0 from products where merged_into is null
             union all
             select p.id, fam.root, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           )
           select fam.root as "product_id!", p.name as "name!", p.unit as "unit!", p.container_ml,
                  d.qty as "qty!", x.replace_qty,
                  (($1::date is null or x.created_at >= ($1::date)::timestamp at time zone 'Asia/Bishkek')
                   and ($2::date is null or x.created_at < ($2::date + 1)::timestamp at time zone 'Asia/Bishkek'))
                    as "in_period!"
           from defect_resolutions x
           join defects d on d.id = x.defect_id
           join fam on fam.id = d.product_id
           join products p on p.id = fam.root
           where x.branch_id = $3 and x.supplier_id = $4 and x.reversal_of is null
             and x.kind in ('replace', 'credit', 'cash')
             and not exists (select 1 from defect_resolutions y where y.reversal_of = x.id)
           union all
           -- Возвраты товара поставщику, не брак (SPEC-29 §9): без сторнированных.
           select fam.root, p.name, p.unit, p.container_ml, l.qty, null::bigint,
                  (($1::date is null or r.created_at >= ($1::date)::timestamp at time zone 'Asia/Bishkek')
                   and ($2::date is null or r.created_at < ($2::date + 1)::timestamp at time zone 'Asia/Bishkek'))
           from supplier_return_lines l
           join supplier_returns r on r.id = l.return_id
           join fam on fam.id = l.product_id
           join products p on p.id = fam.root
           where r.branch_id = $3 and r.supplier_id = $4 and r.reversal_of is null
             and not exists (select 1 from supplier_returns y where y.reversal_of = r.id)"#,
        from,
        to,
        branch_id,
        supplier_id
    )
    .fetch_all(&mut *conn)
    .await?;

    let mut acc: BTreeMap<Uuid, Acc> = BTreeMap::new();
    for l in lines {
        let a = acc.entry(l.product_id).or_default();
        a.name = l.name;
        a.unit = l.unit;
        a.container_ml = l.container_ml;
        a.all_delivered = a.all_delivered.saturating_add(l.qty);
        if l.in_period {
            a.seen = true;
            a.receipts.insert(l.receipt_id);
            a.delivered = a.delivered.saturating_add(l.qty);
            a.amount = a.amount.saturating_add(l.cost_tyiyn);
            if l.qty > 0 {
                // Строки идут по времени: последняя перезаписывает.
                a.last = Some((l.created_at, l.qty, l.cost_tyiyn));
            }
        }
    }
    for r in returns {
        let a = acc.entry(r.product_id).or_default();
        a.name = r.name;
        a.unit = r.unit;
        a.container_ml = r.container_ml;
        a.all_returned = a.all_returned.saturating_add(r.qty);
        // Новый товар взамен брака — тоже от этого поставщика.
        a.all_replaced = a.all_replaced.saturating_add(r.replace_qty.unwrap_or(0));
        if r.in_period {
            a.seen = true;
            a.returned = a.returned.saturating_add(r.qty);
        }
    }
    acc.retain(|_, a| a.seen);
    let ids: Vec<Uuid> = acc.keys().copied().collect();

    let stock: HashMap<Uuid, (i64, i64)> = sqlx::query!(
        r#"select product_id, stock_qty, stock_value_tyiyn from branch_products
           where branch_id = $1 and product_id = any($2)"#,
        branch_id,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.product_id, (r.stock_qty, r.stock_value_tyiyn)))
    .collect();
    let all = inflows(&mut *conn, branch_id, &ids).await?;
    let by_product: HashMap<Uuid, Vec<(bool, i64)>> = all
        .into_iter()
        .map(|(id, list)| {
            let tagged = list
                .iter()
                .map(|f| (f.supplier_id == Some(supplier_id), f.qty))
                .collect();
            (id, tagged)
        })
        .collect();

    let mut rows: Vec<DeliveryRow> = acc
        .into_iter()
        .map(|(product_id, a)| {
            let (stock_qty, stock_value) = stock.get(&product_id).copied().unwrap_or((0, 0));
            let cap = a
                .all_delivered
                .saturating_add(a.all_replaced)
                .saturating_sub(a.all_returned);
            let estimate_qty = fifo_estimate(
                stock_qty,
                by_product
                    .get(&product_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
                cap,
            );
            let estimate_value_tyiyn = owner.then(|| {
                if stock_qty > 0 {
                    div_round(
                        i128::from(stock_value) * i128::from(estimate_qty),
                        i128::from(stock_qty),
                    )
                    .unwrap_or(0)
                } else {
                    0
                }
            });
            let per = price_per(&a.unit, a.container_ml);
            DeliveryRow {
                product_id,
                receipts: i64::try_from(a.receipts.len()).unwrap_or(i64::MAX),
                delivered_qty: a.delivered,
                returned_qty: a.returned,
                amount_tyiyn: a.amount,
                last_price_tyiyn: a.last.and_then(|(_, q, c)| {
                    div_round(i128::from(c) * i128::from(per), i128::from(q))
                }),
                last_at: a.last.map(|(t, _, _)| t),
                stock_qty,
                estimate_qty,
                estimate_value_tyiyn,
                name: a.name,
                unit: a.unit,
                container_ml: a.container_ml,
            }
        })
        .collect();
    rows.sort_by(|x, y| y.last_at.cmp(&x.last_at).then_with(|| x.name.cmp(&y.name)));
    let amount_tyiyn = rows
        .iter()
        .fold(0i64, |s, r| s.saturating_add(r.amount_tyiyn));
    let balance_tyiyn = sqlx::query_scalar!(
        "select balance_tyiyn from parties where id = $1 and branch_id = $2",
        supplier_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(0);
    let pending = sqlx::query!(
        r#"select count(*) as "n!", coalesce(sum(cost_tyiyn), 0)::bigint as "cost!"
           from defects where branch_id = $1 and supplier_id = $2 and status = 'pending'"#,
        branch_id,
        supplier_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(DeliveriesOut {
        supplier_id,
        supplier_name,
        from,
        to,
        rows,
        amount_tyiyn,
        balance_tyiyn,
        pending_defects: pending.n,
        pending_defects_tyiyn: owner.then_some(pending.cost),
    })
}

#[derive(Serialize)]
struct ReceiptRow {
    receipt_id: Uuid,
    number: i64,
    created_at: DateTime<Utc>,
    supplier_doc: String,
    qty: i64,
    cost_tyiyn: i64,
    /// Цена за штуку, канистру или килограмм.
    price_tyiyn: Option<i64>,
    /// Накладную сторнировали — в «Привёз» она не входит.
    reversed: bool,
}

/// Накладные поставщика с этим товаром: дата, номер, количество, цена.
async fn product_receipts(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, product_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ReceiptRow>>> {
    let mut conn = state.pool.acquire().await?;
    let p = sqlx::query!(
        "select unit, container_ml from products where id = $1",
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let per = price_per(&p.unit, p.container_ml);
    // С накладными на карточки, влитые в эту (ADR-070).
    let rows = sqlx::query!(
        r#"with recursive fam (id, depth) as (
             select $3::uuid, 0
             union all
             select p.id, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           )
           select r.id, r.number, r.created_at, r.supplier_doc, l.qty, l.cost_tyiyn,
                  exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!"
           from receipt_lines l join receipts r on r.id = l.receipt_id
           where r.branch_id = $1 and r.supplier_id = $2 and l.product_id in (select id from fam)
             and r.reversal_of is null
             and ($4::date is null or r.created_at >= ($4::date)::timestamp at time zone 'Asia/Bishkek')
             and ($5::date is null or r.created_at < ($5::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by r.created_at desc, l.line_no
           limit 500"#,
        user.branch_id,
        id,
        product_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| ReceiptRow {
                receipt_id: r.id,
                number: r.number,
                created_at: r.created_at,
                supplier_doc: r.supplier_doc,
                qty: r.qty,
                cost_tyiyn: r.cost_tyiyn,
                price_tyiyn: (r.qty > 0)
                    .then(|| {
                        div_round(
                            i128::from(r.cost_tyiyn) * i128::from(per),
                            i128::from(r.qty),
                        )
                    })
                    .flatten(),
                reversed: r.reversed,
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::fifo_estimate;

    #[test]
    fn fifo_takes_newest_receipts_first() {
        // Остаток 7: последние 5 от нас, до них 10 от другого — наши 5 целиком.
        assert_eq!(fifo_estimate(7, &[(true, 5), (false, 10)], 100), 5);
        // Последние 5 от другого, до них 10 от нас — наших 2.
        assert_eq!(fifo_estimate(7, &[(false, 5), (true, 10)], 100), 2);
        // Один поставщик — весь остаток.
        assert_eq!(fifo_estimate(7, &[(true, 4), (true, 6)], 100), 7);
    }

    #[test]
    fn fifo_is_capped() {
        // Не больше «привёз − вернули» и не больше остатка; минус остатка — ноль.
        assert_eq!(fifo_estimate(10, &[(true, 10)], 6), 6);
        assert_eq!(fifo_estimate(3, &[(true, 10)], 6), 3);
        assert_eq!(fifo_estimate(-2, &[(true, 10)], 6), 0);
        assert_eq!(fifo_estimate(5, &[], 6), 0);
    }
}
