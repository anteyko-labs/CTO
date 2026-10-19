//! Оприходование без поставщика (SPEC-29 §8): товар пришёл без накладной и без денег.
//!
//! Количество ложится на склад по цене закупки, которую вписали (за штуку, канистру масла или
//! килограмм), — как приход, но без поставщика и без движения денег. Комментарий обязателен.
//! Проводят владелец и администратор (цены закупки администратор видит и вводит, ADR-008);
//! владелец получает уведомление. Исправление — только сторно со ссылкой (инвариант 4).

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
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/stock-ins", routing::get(list).post(post))
        .route("/stock-ins/{id}", routing::get(get_one))
        .route("/stock-ins/{id}/reverse", routing::post(reverse))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StockInOut {
    pub id: Uuid,
    pub number: i64,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// В единицах учёта: штуки, миллилитры, граммы; у сторно — с минусом.
    pub qty: i64,
    /// Цена закупки за штуку, канистру или килограмм.
    pub price_tyiyn: i64,
    /// Стоимость всего количества.
    pub cost_tyiyn: i64,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub reversed_by: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
}

/// Отбор списка оприходований: товар и период по дню Бишкека (оба края включительно).
#[derive(Deserialize, Default, Clone, Copy)]
struct Filter {
    product_id: Option<Uuid>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn load(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Option<Uuid>,
    f: Filter,
) -> AppResult<Vec<StockInOut>> {
    Ok(sqlx::query_as!(
        StockInOut,
        r#"select s.id, s.number, s.product_id, p.name as product_name, p.unit, p.container_ml,
                  s.qty, s.price_tyiyn, s.cost_tyiyn, s.comment, s.reversal_of,
                  (select x.id from stock_ins x where x.reversal_of = s.id) as reversed_by,
                  u.full_name as user_name, s.created_at
           from stock_ins s
           join products p on p.id = s.product_id
           join users u on u.id = s.user_id
           where s.branch_id = $1 and ($2::uuid is null or s.id = $2)
             and ($3::uuid is null or s.product_id = $3)
             and ($4::date is null or s.created_at >= ($4::date)::timestamp at time zone 'Asia/Bishkek')
             and ($5::date is null or s.created_at < ($5::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by s.created_at desc
           limit $6"#,
        branch_id,
        id,
        f.product_id,
        f.from,
        f.to,
        MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?)
}

pub(crate) async fn load_one(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<StockInOut> {
    load(conn, branch_id, Some(id), Filter::default())
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

/// Список оприходований (SPEC-29 §25): экран «Оприходование» — период и страницы, окно товара —
/// по `product_id`. Без `limit` — как раньше, до 500 строк.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(f): Query<Filter>,
    Query(page): Query<PageQuery>,
) -> AppResult<Paged<StockInOut>> {
    let mut conn = state.pool.acquire().await?;
    let (items, total) = page.slice(load(&mut conn, user.branch_id, None, f).await?, 500);
    Ok(paged(items, total))
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<StockInOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_one(&mut conn, user.branch_id, id).await?))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StockInReq {
    pub op_id: Uuid,
    pub product_id: Uuid,
    /// В единицах учёта: штуки, миллилитры (масло), граммы (на вес).
    pub qty: i64,
    /// Цена закупки за штуку, канистру масла или килограмм.
    pub price_tyiyn: i64,
    pub comment: String,
}

async fn post(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<StockInReq>,
) -> AppResult<Json<StockInOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Оприходование — одна транзакция: документ, движение склада, последняя закупочная цена, журнал.
pub async fn post_tx(conn: &mut PgConnection, ctx: &Ctx, req: StockInReq) -> AppResult<StockInOut> {
    const KIND: &str = "stock_in.post";
    if req.qty <= 0 || req.qty > 1_000_000_000 {
        return Err(invalid("количество больше нуля"));
    }
    if req.price_tyiyn < 0 {
        return Err(invalid("цена закупки не отрицательна"));
    }
    ops::check_amount(req.price_tyiyn)?;
    let comment = req.comment.trim();
    if comment.is_empty() {
        return Err(invalid(
            "напишите, откуда товар: без накладной оприходование только с комментарием",
        ));
    }
    if comment.chars().count() > 500 {
        return Err(invalid("комментарий не длиннее 500 знаков"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let p = sqlx::query!(
        "select name, unit, container_ml, archived from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.archived {
        return Err(invalid(
            "товар в архиве: если карточку объединили, оприходуйте на основную",
        ));
    }
    let per = price_unit(&p.unit, p.container_ml);
    let cost = div_round(
        i128::from(req.price_tyiyn) * i128::from(req.qty),
        i128::from(per),
    )
    .ok_or_else(overflow)?;
    ops::check_amount(cost)?;
    ops::lock_pool(conn, branch_id, req.product_id).await?;
    ops::ensure_not_merged(conn, &[req.product_id]).await?;
    let number = ops::next_counter(conn, branch_id, "stock_in").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into stock_ins (id, branch_id, number, product_id, qty, price_tyiyn, cost_tyiyn, comment,
                                  user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        req.product_id,
        req.qty,
        req.price_tyiyn,
        cost,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: req.product_id,
            qty_delta: req.qty,
            value_delta: cost,
            doc_type: "stock_in",
            doc_id: id,
        },
    )
    .await?;
    // Цена закупки — последняя закупочная: по ней пойдёт продажа в минус и излишек ревизии.
    if req.price_tyiyn > 0 {
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            req.product_id,
            req.qty,
            cost
        )
        .execute(&mut *conn)
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "product",
        Some(req.product_id),
        json!({
            "number": number,
            "stock_in": id,
            "name": p.name,
            "qty_text": crate::api::bot_drafts::qty_text(req.qty, &p.unit),
            "price": req.price_tyiyn,
            "cost": cost,
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, branch_id, id).await?;
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
) -> AppResult<Json<StockInOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно оприходования: товар уходит со склада по той же стоимости. Если его уже продали и
/// остаток уйдёт в минус — так исправляют только ошибку, и решает владелец (как у прихода, ADR-065).
pub async fn reverse_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<StockInOut> {
    const KIND: &str = "stock_in.reverse";
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
    let orig = load_one(conn, branch_id, id).await?;
    if orig.reversal_of.is_some() {
        return Err(AppError::Conflict("сторно не сторнируется".into()));
    }
    if orig.reversed_by.is_some() {
        return Err(AppError::Conflict("оприходование уже сторнировано".into()));
    }
    let target = ops::live_product(conn, orig.product_id).await?;
    let pool = ops::lock_pool(conn, branch_id, target).await?;
    ops::ensure_not_merged(conn, &[target]).await?;
    let negative = pool.qty < orig.qty;
    if negative && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let number = ops::next_counter(conn, branch_id, "stock_in").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into stock_ins (id, branch_id, number, product_id, qty, price_tyiyn, cost_tyiyn, comment,
                                  reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        rid,
        branch_id,
        number,
        orig.product_id,
        -orig.qty,
        orig.price_tyiyn,
        -orig.cost_tyiyn,
        comment,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    // Ошибочная цена не остаётся «последней закупочной»: берём последнюю живую закупку —
    // приход или другое оприходование; не было — ноль (как при сторно прихода, SPEC-03).
    let last = sqlx::query!(
        r#"with recursive fam as (
             select $2::uuid as id
             union select p.id from products p join fam on p.merged_into = fam.id
           )
           select qty as "qty!", cost as "cost!" from (
             select l.qty, l.cost_tyiyn as cost, r.created_at as at
             from receipt_lines l join receipts r on r.id = l.receipt_id
             where r.branch_id = $1 and l.product_id in (select id from fam) and l.qty > 0
               and r.reversal_of is null
               and not exists (select 1 from receipts x where x.reversal_of = r.id)
             union all
             select s.qty, s.cost_tyiyn, s.created_at
             from stock_ins s
             where s.branch_id = $1 and s.product_id in (select id from fam) and s.qty > 0
               and s.id <> $3 and s.price_tyiyn > 0
               and not exists (select 1 from stock_ins x where x.reversal_of = s.id)
           ) u
           order by at desc limit 1"#,
        branch_id,
        target,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if orig.price_tyiyn > 0 {
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            target,
            last.as_ref().map_or(0, |r| r.qty),
            last.as_ref().map_or(0, |r| r.cost)
        )
        .execute(&mut *conn)
        .await?;
    }
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: target,
            qty_delta: -orig.qty,
            value_delta: -orig.cost_tyiyn,
            doc_type: "stock_in_reversal",
            doc_id: rid,
        },
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "product",
        Some(target),
        json!({
            "number": orig.number,
            "stock_in": id,
            "reversal": rid,
            "name": orig.product_name,
            "qty_text": crate::api::bot_drafts::qty_text(orig.qty, &orig.unit),
            "cost": orig.cost_tyiyn,
            "comment": comment,
            "negative": negative.then_some(pool.qty - orig.qty),
        }),
    )
    .await?;
    let out = load_one(conn, branch_id, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
