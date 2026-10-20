//! Возврат товара поставщику — не брак (SPEC-29 §9).
//!
//! Товар уходит со склада по средней себестоимости (инвариант 3), сумма возврата договорная —
//! по умолчанию цена последнего прихода от этого поставщика × количество. Итог — меньше наш
//! долг поставщику (запись долга с плюсом, ложится на старые накладные, ADR-073) или деньги от
//! поставщика в выбранный счёт. Разница «сумма − себестоимость» — строка прибыли «Возврат
//! поставщику: разница». Проводят владелец и администратор; деньги не в кассу смены — только
//! владелец (как в SPEC-25). Себестоимость и разницу видит только владелец (инвариант 13).

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::price_unit;
use crate::api::paging::{MAX_ROWS, PageQuery, Paged, paged};
use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/supplier-returns", routing::get(list).post(post))
        .route("/supplier-returns/price", routing::get(price_hint))
        .route("/supplier-returns/{id}", routing::get(get_one))
        .route("/supplier-returns/{id}/reverse", routing::post(reverse))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReturnLineOut {
    pub line_no: i32,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    pub amount_tyiyn: i64,
    /// Себестоимость по средней — только владельцу.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SupplierReturnOut {
    pub id: Uuid,
    pub number: i64,
    pub supplier_id: Uuid,
    pub supplier_name: String,
    /// `debt` — уменьшили наш долг поставщику; `money` — поставщик отдал деньги.
    pub settle: String,
    pub account_id: Option<Uuid>,
    pub account_name: Option<String>,
    pub amount_tyiyn: i64,
    /// Себестоимость и разница — только владельцу.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tyiyn: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_tyiyn: Option<i64>,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub reversed_by: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<ReturnLineOut>,
}

async fn load(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Option<Uuid>,
    f: ListQuery,
) -> AppResult<Vec<SupplierReturnOut>> {
    let owner = user.is_owner();
    let heads = sqlx::query!(
        r#"select r.id, r.number, r.supplier_id, s.name as supplier_name, r.settle, r.account_id,
                  a.name as "account_name?", r.amount_tyiyn, r.cost_tyiyn, r.diff_tyiyn, r.comment,
                  r.reversal_of, (select x.id from supplier_returns x where x.reversal_of = r.id) as reversed_by,
                  u.full_name as user_name, r.created_at
           from supplier_returns r
           join suppliers s on s.id = r.supplier_id
           join users u on u.id = r.user_id
           left join cash_accounts a on a.id = r.account_id
           where r.branch_id = $1 and ($2::uuid is null or r.id = $2)
             and ($3::uuid is null or r.supplier_id = $3)
             and ($4::date is null or r.created_at >= ($4::date)::timestamp at time zone 'Asia/Bishkek')
             and ($5::date is null or r.created_at < ($5::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by r.created_at desc
           limit $6"#,
        user.branch_id,
        id,
        f.supplier_id,
        f.from,
        f.to,
        MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = heads.iter().map(|h| h.id).collect();
    let lines = sqlx::query!(
        r#"select l.return_id, l.line_no, l.product_id, p.name, p.unit, p.container_ml, l.qty,
                  l.amount_tyiyn, l.cost_tyiyn
           from supplier_return_lines l join products p on p.id = l.product_id
           where l.return_id = any($1)
           order by l.return_id, l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(heads
        .into_iter()
        .map(|h| SupplierReturnOut {
            lines: lines
                .iter()
                .filter(|l| l.return_id == h.id)
                .map(|l| ReturnLineOut {
                    line_no: l.line_no,
                    product_id: l.product_id,
                    product_name: l.name.clone(),
                    unit: l.unit.clone(),
                    container_ml: l.container_ml,
                    qty: l.qty,
                    amount_tyiyn: l.amount_tyiyn,
                    cost_tyiyn: owner.then_some(l.cost_tyiyn),
                })
                .collect(),
            id: h.id,
            number: h.number,
            supplier_id: h.supplier_id,
            supplier_name: h.supplier_name,
            settle: h.settle,
            account_id: h.account_id,
            account_name: h.account_name,
            amount_tyiyn: h.amount_tyiyn,
            cost_tyiyn: owner.then_some(h.cost_tyiyn),
            diff_tyiyn: owner.then_some(h.diff_tyiyn),
            comment: h.comment,
            reversal_of: h.reversal_of,
            reversed_by: h.reversed_by,
            user_name: h.user_name,
            created_at: h.created_at,
        })
        .collect())
}

pub(crate) async fn load_one(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
) -> AppResult<SupplierReturnOut> {
    load(conn, user, Some(id), ListQuery::default())
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

/// Отбор возвратов: поставщик и период по дню Бишкека (оба края включительно).
#[derive(Deserialize, Default, Clone, Copy)]
struct ListQuery {
    supplier_id: Option<Uuid>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

/// Возвраты поставщикам (SPEC-29 §25): экран «Возврат поставщику» — все поставщики, период и
/// страницы; карточка поставщика — по `supplier_id`. Без `limit` — как раньше, до 500 строк.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
    Query(page): Query<PageQuery>,
) -> AppResult<Paged<SupplierReturnOut>> {
    let mut conn = state.pool.acquire().await?;
    let (items, total) = page.slice(load(&mut conn, &user, None, q).await?, 500);
    Ok(paged(items, total))
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<SupplierReturnOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_one(&mut conn, &user, id).await?))
}

/// Цена последнего непогашенного прихода товара (и влитых в него карточек) от этого поставщика —
/// за штуку, канистру или килограмм. Закупочные цены видит и администратор (ADR-008).
pub async fn last_supplier_price(
    conn: &mut PgConnection,
    branch_id: Uuid,
    supplier_id: Uuid,
    product_id: Uuid,
) -> AppResult<Option<i64>> {
    let r = sqlx::query!(
        r#"with recursive fam as (
             select $3::uuid as id
             union select p.id from products p join fam on p.merged_into = fam.id
           )
           select l.qty, l.cost_tyiyn, p.unit, p.container_ml
           from receipt_lines l
           join receipts r on r.id = l.receipt_id
           join products p on p.id = $3
           where r.branch_id = $1 and r.supplier_id = $2 and l.product_id in (select id from fam)
             and l.qty > 0 and r.reversal_of is null
             and not exists (select 1 from receipts x where x.reversal_of = r.id)
           order by r.created_at desc, l.line_no desc
           limit 1"#,
        branch_id,
        supplier_id,
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(r.and_then(|r| {
        div_round(
            i128::from(r.cost_tyiyn) * i128::from(price_unit(&r.unit, r.container_ml)),
            i128::from(r.qty),
        )
    }))
}

#[derive(Deserialize)]
struct PriceQuery {
    supplier_id: Uuid,
    product_id: Uuid,
}

#[derive(Serialize)]
struct PriceOut {
    /// Цена последнего прихода от этого поставщика: за штуку, канистру или килограмм.
    price_tyiyn: Option<i64>,
}

async fn price_hint(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PriceQuery>,
) -> AppResult<Json<PriceOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(PriceOut {
        price_tyiyn: last_supplier_price(&mut conn, user.branch_id, q.supplier_id, q.product_id)
            .await?,
    }))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReturnLineReq {
    pub product_id: Uuid,
    /// В единицах учёта: штуки, миллилитры, граммы.
    pub qty: i64,
    /// Договорная сумма строки; без поля — цена последнего прихода от поставщика × количество,
    /// а без прихода от него — себестоимость (разницы нет).
    #[serde(default)]
    pub amount_tyiyn: Option<i64>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReturnReq {
    pub op_id: Uuid,
    pub supplier_id: Uuid,
    /// `debt` — в счёт долга поставщику; `money` — поставщик отдал деньги в `account_id`.
    pub settle: String,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub comment: String,
    pub lines: Vec<ReturnLineReq>,
}

async fn post(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ReturnReq>,
) -> AppResult<Json<SupplierReturnOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Возврат поставщику — одна транзакция: документ, движения склада, долг или деньги, журнал.
pub async fn post_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ReturnReq,
) -> AppResult<SupplierReturnOut> {
    const KIND: &str = "supplier_return.post";
    if req.lines.is_empty() {
        return Err(invalid("в возврате нет строк"));
    }
    if req.lines.len() > 200 {
        return Err(invalid("строк не больше 200"));
    }
    let mut seen = BTreeSet::new();
    for l in &req.lines {
        if l.qty <= 0 || l.qty > 1_000_000_000 {
            return Err(invalid("количество больше нуля"));
        }
        if let Some(a) = l.amount_tyiyn {
            if a < 0 {
                return Err(invalid("сумма строки не отрицательна"));
            }
            ops::check_amount(a)?;
        }
        if !seen.insert(l.product_id) {
            return Err(invalid("один товар — одна строка: сложите количество"));
        }
    }
    let comment = req.comment.trim();
    if comment.chars().count() > 500 {
        return Err(invalid("комментарий не длиннее 500 знаков"));
    }
    let settle = req.settle.as_str();
    if !matches!(settle, "debt" | "money") {
        return Err(invalid("итог возврата: в счёт долга или деньгами"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let supplier_name =
        sqlx::query_scalar!("select name from suppliers where id = $1", req.supplier_id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("поставщик не найден"))?;
    let account = match (settle, req.account_id) {
        ("money", None) => return Err(invalid("выберите счёт, куда поставщик отдал деньги")),
        ("money", Some(id)) => {
            // Администратор — только касса смены; счёт и сейфы — владелец (ADR-076, SPEC-25).
            crate::api::cash::check_pay_account(conn, &ctx.user, id).await?;
            crate::api::cash::require_open_shift(conn, branch_id, id).await?;
            Some(id)
        }
        (_, Some(_)) => return Err(invalid("в счёт долга деньги ни на какой счёт не приходят")),
        _ => None,
    };
    // Товар влитой карточки лежит на основной: возвращаем с неё (ADR-044).
    let mut targets = Vec::with_capacity(req.lines.len());
    for l in &req.lines {
        let exists = sqlx::query_scalar!("select id from products where id = $1", l.product_id)
            .fetch_optional(&mut *conn)
            .await?;
        if exists.is_none() {
            return Err(invalid("товар не найден"));
        }
        targets.push(ops::live_product(conn, l.product_id).await?);
    }
    let distinct: BTreeSet<Uuid> = targets.iter().copied().collect();
    if distinct.len() != targets.len() {
        return Err(invalid("один товар — одна строка: сложите количество"));
    }
    crate::api::receipts::lock_products(conn, branch_id, distinct.iter().copied()).await?;
    ops::ensure_not_merged(conn, &targets).await?;
    let number = ops::next_counter(conn, branch_id, "supplier_return").await?;
    let id = new_id();
    let mut rows = Vec::with_capacity(req.lines.len());
    let (mut total, mut cost_total) = (0i64, 0i64);
    for (l, target) in req.lines.iter().zip(&targets) {
        let p = sqlx::query!(
            "select name, unit, container_ml, archived from products where id = $1",
            target
        )
        .fetch_one(&mut *conn)
        .await?;
        if p.archived {
            return Err(invalid(format!("«{}» в архиве", p.name)));
        }
        let pool = ops::lock_pool(conn, branch_id, *target).await?;
        if pool.qty < l.qty {
            return Err(invalid(format!(
                "«{}»: на складе только {} — больше вернуть нельзя",
                p.name,
                crate::api::bot_drafts::qty_text(pool.qty.max(0), &p.unit)
            )));
        }
        let cost = cost_of(l.qty, &pool).ok_or_else(overflow)?;
        let amount = match l.amount_tyiyn {
            Some(a) => a,
            None => match last_supplier_price(conn, branch_id, req.supplier_id, *target).await? {
                Some(price) => div_round(
                    i128::from(price) * i128::from(l.qty),
                    i128::from(price_unit(&p.unit, p.container_ml)),
                )
                .ok_or_else(overflow)?,
                // Средняя себестоимость — только владельцу (ADR-008, инвариант 13): остальные
                // называют сумму сами, если от этого поставщика товар не приходил.
                None if ctx.user.is_owner() => cost,
                None => {
                    return Err(invalid(format!(
                        "«{}»: от этого поставщика не приходил — укажите сумму возврата",
                        p.name
                    )));
                }
            },
        };
        ops::check_amount(amount)?;
        total = total.checked_add(amount).ok_or_else(overflow)?;
        cost_total = cost_total.checked_add(cost).ok_or_else(overflow)?;
        rows.push((*target, l.qty, cost, amount, p.name, p.unit));
    }
    ops::check_amount(total)?;
    if total <= 0 {
        return Err(invalid("сумма возврата больше нуля"));
    }
    let diff = total.checked_sub(cost_total).ok_or_else(overflow)?;
    sqlx::query!(
        r#"insert into supplier_returns (id, branch_id, number, supplier_id, settle, account_id,
                                         amount_tyiyn, cost_tyiyn, diff_tyiyn, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        id,
        branch_id,
        number,
        req.supplier_id,
        settle,
        account,
        total,
        cost_total,
        diff,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (i, (product_id, qty, cost, amount, _, _)) in rows.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            r#"insert into supplier_return_lines (id, return_id, line_no, product_id, qty, cost_tyiyn, amount_tyiyn)
               values ($1, $2, $3, $4, $5, $6, $7)"#,
            new_id(),
            id,
            line_no,
            product_id,
            qty,
            cost,
            amount
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: *product_id,
                qty_delta: -qty,
                value_delta: -cost,
                doc_type: "supplier_return",
                doc_id: id,
            },
        )
        .await?;
    }
    let note = format!("возврат поставщику № {number}");
    match account {
        None => {
            // Наш долг поставщику меньше: запись с плюсом закрывает самые старые накладные (ADR-073).
            crate::api::parties::add_ledger(
                conn,
                ctx,
                crate::api::parties::LedgerEntry {
                    party_id: req.supplier_id,
                    kind: "debt",
                    amount: total,
                    doc_type: "supplier_return",
                    doc_id: Some(id),
                    comment: &note,
                },
            )
            .await
            .map_err(|e| match e {
                AppError::NotFound => invalid("поставщик не найден в этом филиале"),
                other => other,
            })?;
        }
        Some(acc) => {
            crate::api::cash::add_movement(
                conn,
                ctx,
                crate::api::cash::CashEntry {
                    account_id: acc,
                    kind: "supplier_refund",
                    amount: total,
                    doc_type: "supplier_return",
                    doc_id: Some(id),
                    comment: &note,
                },
            )
            .await?;
        }
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "supplier_return",
        Some(id),
        json!({
            "number": number,
            "supplier": supplier_name,
            "settle": settle,
            "amount": total,
            "cost": cost_total,
            "diff": diff,
            "lines": rows.iter().map(|(_, q, _, a, name, unit)| json!({
                "name": name,
                "qty_text": crate::api::bot_drafts::qty_text(*q, unit),
                "amount": a,
            })).collect::<Vec<_>>(),
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

async fn reverse(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<SupplierReturnOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно возврата: товар возвращается на склад по той же стоимости, долг или деньги — обратно.
pub async fn reverse_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<SupplierReturnOut> {
    const KIND: &str = "supplier_return.reverse";
    let comment = req.comment.trim();
    if comment.is_empty() {
        return Err(invalid("укажите причину сторно"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let h = sqlx::query!(
        r#"select r.number, r.supplier_id, r.settle, r.account_id, r.amount_tyiyn, r.cost_tyiyn,
                  r.diff_tyiyn, r.reversal_of, s.name as supplier_name,
                  exists (select 1 from supplier_returns x where x.reversal_of = r.id) as "reversed!"
           from supplier_returns r join suppliers s on s.id = r.supplier_id
           where r.id = $1 and r.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if h.reversal_of.is_some() {
        return Err(AppError::Conflict("сторно не сторнируется".into()));
    }
    if h.reversed {
        return Err(AppError::Conflict("возврат уже сторнирован".into()));
    }
    if let Some(acc) = h.account_id {
        // Деньги уходят обратно с того же счёта: не касса смены — только владелец (SPEC-05).
        crate::api::cash::check_pay_account(conn, &ctx.user, acc).await?;
        crate::api::cash::require_open_shift(conn, branch_id, acc).await?;
    }
    let lines = sqlx::query!(
        "select line_no, product_id, qty, cost_tyiyn, amount_tyiyn from supplier_return_lines where return_id = $1 order by line_no",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut targets = Vec::with_capacity(lines.len());
    for l in &lines {
        targets.push(ops::live_product(conn, l.product_id).await?);
    }
    crate::api::receipts::lock_products(conn, branch_id, targets.iter().copied()).await?;
    ops::ensure_not_merged(conn, &targets).await?;
    let number = ops::next_counter(conn, branch_id, "supplier_return").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into supplier_returns (id, branch_id, number, supplier_id, settle, account_id,
                                         amount_tyiyn, cost_tyiyn, diff_tyiyn, comment, reversal_of,
                                         user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        rid,
        branch_id,
        number,
        h.supplier_id,
        h.settle,
        h.account_id,
        -h.amount_tyiyn,
        -h.cost_tyiyn,
        -h.diff_tyiyn,
        comment,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (l, target) in lines.iter().zip(&targets) {
        sqlx::query!(
            r#"insert into supplier_return_lines (id, return_id, line_no, product_id, qty, cost_tyiyn, amount_tyiyn)
               values ($1, $2, $3, $4, $5, $6, $7)"#,
            new_id(),
            rid,
            l.line_no,
            l.product_id,
            -l.qty,
            -l.cost_tyiyn,
            -l.amount_tyiyn
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: *target,
                qty_delta: l.qty,
                value_delta: l.cost_tyiyn,
                doc_type: "supplier_return_reversal",
                doc_id: rid,
            },
        )
        .await?;
    }
    let note = format!("сторно возврата поставщику № {}", h.number);
    match h.account_id {
        None => {
            crate::api::parties::add_ledger(
                conn,
                ctx,
                crate::api::parties::LedgerEntry {
                    party_id: h.supplier_id,
                    kind: "debt",
                    amount: -h.amount_tyiyn,
                    doc_type: "supplier_return_reversal",
                    doc_id: Some(rid),
                    comment: &note,
                },
            )
            .await?;
        }
        Some(acc) => {
            crate::api::cash::add_movement(
                conn,
                ctx,
                crate::api::cash::CashEntry {
                    account_id: acc,
                    kind: "reversal",
                    amount: -h.amount_tyiyn,
                    doc_type: "supplier_return_reversal",
                    doc_id: Some(rid),
                    comment: &note,
                },
            )
            .await?;
        }
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "supplier_return",
        Some(rid),
        json!({
            "number": h.number,
            "original": id,
            "supplier": h.supplier_name,
            "amount": h.amount_tyiyn,
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
