//! Приход, сторно прихода и остатки (SPEC-03).

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductFilter, ProductOut, query_products};
use crate::auth::{Ctx, CurrentUser, Owner};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/receipts", routing::get(list_receipts).post(post_receipt))
        .route("/receipts/{id}", routing::get(get_receipt))
        .route("/receipts/{id}/reverse", routing::post(reverse_receipt))
        .route("/stock", routing::get(stock))
        .route("/stock/verify", routing::get(verify_stock))
        .route(
            "/stock/{product_id}/review-done",
            routing::post(review_done),
        )
}

#[derive(Deserialize)]
pub struct ReceiptLineReq {
    pub product_id: Uuid,
    pub qty: i64,
    pub cost_tyiyn: i64,
}

#[derive(Deserialize)]
pub struct ReceiptReq {
    pub op_id: Uuid,
    pub supplier_id: Option<Uuid>,
    #[serde(default)]
    pub supplier_doc: String,
    #[serde(default)]
    pub comment: String,
    pub lines: Vec<ReceiptLineReq>,
}

#[derive(Serialize, Deserialize)]
pub struct ReceiptLineOut {
    pub line_no: i32,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    pub cost_tyiyn: i64,
}

#[derive(Serialize, Deserialize)]
pub struct ReceiptOut {
    pub id: Uuid,
    pub number: i64,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    pub supplier_doc: String,
    pub comment: String,
    pub total_tyiyn: i64,
    pub reversal_of: Option<Uuid>,
    pub reversed_by: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<ReceiptLineOut>,
}

async fn load_receipt(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<ReceiptOut> {
    let h = sqlx::query!(
        r#"select r.id, r.number, r.supplier_id, s.name as "supplier_name?", r.supplier_doc, r.comment,
                  r.total_tyiyn, r.reversal_of, u.full_name as user_name, r.created_at,
                  (select x.id from receipts x where x.reversal_of = r.id) as reversed_by
           from receipts r
           join users u on u.id = r.user_id
           left join suppliers s on s.id = r.supplier_id
           where r.id = $1 and r.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let lines = sqlx::query_as!(
        ReceiptLineOut,
        r#"select l.line_no, l.product_id, p.name as product_name, p.unit, p.container_ml, l.qty, l.cost_tyiyn
           from receipt_lines l join products p on p.id = l.product_id
           where l.receipt_id = $1 order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(ReceiptOut {
        id: h.id,
        number: h.number,
        supplier_id: h.supplier_id,
        supplier_name: h.supplier_name,
        supplier_doc: h.supplier_doc,
        comment: h.comment,
        total_tyiyn: h.total_tyiyn,
        reversal_of: h.reversal_of,
        reversed_by: h.reversed_by,
        user_name: h.user_name,
        created_at: h.created_at,
        lines,
    })
}

/// Блокирует строки остатков в порядке идентификаторов, чтобы параллельные документы не взаимоблокировались.
pub async fn lock_products(
    conn: &mut PgConnection,
    branch_id: Uuid,
    ids: impl IntoIterator<Item = Uuid>,
) -> AppResult<()> {
    let sorted: BTreeSet<Uuid> = ids.into_iter().collect();
    for id in sorted {
        ops::lock_pool(conn, branch_id, id).await?;
    }
    Ok(())
}

pub async fn post_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ReceiptReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.post";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if req.lines.is_empty() {
        return Err(invalid("в приходе нет строк"));
    }
    let mut total: i64 = 0;
    for l in &req.lines {
        if l.qty <= 0 || l.cost_tyiyn < 0 {
            return Err(invalid("количество больше нуля, сумма не отрицательна"));
        }
        total = total.checked_add(l.cost_tyiyn).ok_or_else(overflow)?;
    }
    let ids: Vec<Uuid> = req.lines.iter().map(|l| l.product_id).collect();
    let known = sqlx::query_scalar!(
        "select count(*) as \"n!\" from products where id = any($1)",
        &ids
    )
    .fetch_one(&mut *conn)
    .await?;
    let distinct: BTreeSet<Uuid> = ids.iter().copied().collect();
    if usize::try_from(known).ok() != Some(distinct.len()) {
        return Err(invalid("товар не найден"));
    }
    if let Some(sid) = req.supplier_id {
        sqlx::query_scalar!("select id from suppliers where id = $1", sid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("поставщик не найден"))?;
    }
    let branch_id = ctx.user.branch_id;
    lock_products(conn, branch_id, distinct).await?;
    let number = ops::next_counter(conn, branch_id, "receipt").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        branch_id,
        number,
        req.supplier_id,
        req.supplier_doc.trim(),
        req.comment.trim(),
        total,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (i, l) in req.lines.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            "insert into receipt_lines (id, receipt_id, line_no, product_id, qty, cost_tyiyn) values ($1, $2, $3, $4, $5, $6)",
            new_id(),
            id,
            line_no,
            l.product_id,
            l.qty,
            l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: l.product_id,
                qty_delta: l.qty,
                value_delta: l.cost_tyiyn,
                doc_type: "receipt",
                doc_id: id,
            },
        )
        .await?;
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            l.product_id,
            l.qty,
            l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(id),
        json!({ "number": number, "total": total }),
    )
    .await?;
    let out = load_receipt(conn, branch_id, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_receipt(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ReceiptReq>,
) -> AppResult<Json<ReceiptOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_receipt_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

pub async fn reverse_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.reverse";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    // Блокировка исходного документа от параллельного сторно.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let orig = load_receipt(conn, branch_id, id).await?;
    if orig.reversal_of.is_some() {
        return Err(AppError::Conflict("сторно не сторнируется".into()));
    }
    if orig.reversed_by.is_some() {
        return Err(AppError::Conflict("приход уже сторнирован".into()));
    }
    lock_products(conn, branch_id, orig.lines.iter().map(|l| l.product_id)).await?;
    let number = ops::next_counter(conn, branch_id, "receipt").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        rid,
        branch_id,
        number,
        orig.supplier_id,
        orig.supplier_doc,
        req.comment.trim(),
        -orig.total_tyiyn,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for l in &orig.lines {
        sqlx::query!(
            "insert into receipt_lines (id, receipt_id, line_no, product_id, qty, cost_tyiyn) values ($1, $2, $3, $4, $5, $6)",
            new_id(),
            rid,
            l.line_no,
            l.product_id,
            -l.qty,
            -l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: l.product_id,
                qty_delta: -l.qty,
                value_delta: -l.cost_tyiyn,
                doc_type: "receipt_reversal",
                doc_id: rid,
            },
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(rid),
        json!({ "original": id, "number": number }),
    )
    .await?;
    let out = load_receipt(conn, branch_id, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn reverse_receipt(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<ReceiptOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_receipt_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn get_receipt(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ReceiptOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_receipt(&mut conn, user.branch_id, id).await?))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

#[derive(Serialize)]
struct ReceiptListItem {
    id: Uuid,
    number: i64,
    supplier_name: Option<String>,
    supplier_doc: String,
    total_tyiyn: i64,
    reversal_of: Option<Uuid>,
    reversed: bool,
    user_name: String,
    created_at: DateTime<Utc>,
}

async fn list_receipts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ReceiptListItem>>> {
    let rows = sqlx::query_as!(
        ReceiptListItem,
        r#"select r.id, r.number, s.name as "supplier_name?", r.supplier_doc, r.total_tyiyn, r.reversal_of,
                  exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!",
                  u.full_name as user_name, r.created_at
           from receipts r
           join users u on u.id = r.user_id
           left join suppliers s on s.id = r.supplier_id
           where r.branch_id = $1
             and ($2::date is null or r.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek')
             and ($3::date is null or r.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by r.created_at desc
           limit 500"#,
        user.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct StockQuery {
    category_id: Option<Uuid>,
    #[serde(default)]
    low: bool,
    q: Option<String>,
}

async fn stock(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<StockQuery>,
) -> AppResult<Json<Vec<ProductOut>>> {
    let mut conn = state.pool.acquire().await?;
    let f = ProductFilter {
        q: q.q,
        category_id: q.category_id,
        low_only: q.low,
        limit: 500,
        ..Default::default()
    };
    Ok(Json(query_products(&mut conn, &user, f).await?))
}

#[derive(Serialize)]
pub struct StockMismatch {
    pub product_id: Uuid,
    pub cached_qty: i64,
    pub moved_qty: i64,
    pub cached_value_tyiyn: i64,
    pub moved_value_tyiyn: i64,
}

/// Расхождения кэша остатков с суммой движений (инвариант 6). Пустой список — норма.
pub async fn verify_stock_tx(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<Vec<StockMismatch>> {
    let rows = sqlx::query_as!(
        StockMismatch,
        r#"select bp.product_id, bp.stock_qty as cached_qty,
                  coalesce(m.qty, 0) as "moved_qty!",
                  bp.stock_value_tyiyn as cached_value_tyiyn,
                  coalesce(m.value, 0) as "moved_value_tyiyn!"
           from branch_products bp
           left join (select product_id, sum(qty_delta)::bigint as qty, sum(value_delta_tyiyn)::bigint as value
                      from stock_movements where branch_id = $1 group by product_id) m
             on m.product_id = bp.product_id
           where bp.branch_id = $1
             and (bp.stock_qty <> coalesce(m.qty, 0) or bp.stock_value_tyiyn <> coalesce(m.value, 0))"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

async fn verify_stock(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<Vec<StockMismatch>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(verify_stock_tx(&mut conn, user.branch_id).await?))
}

async fn review_done(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(product_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    ctx.user.require_owner()?;
    let mut tx = state.pool.begin().await?;
    let n = sqlx::query!(
        "update branch_products set needs_review = false where branch_id = $1 and product_id = $2",
        ctx.user.branch_id,
        product_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    ops::audit(
        &mut tx,
        &ctx,
        "stock.review_done",
        "product",
        Some(product_id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({})))
}
