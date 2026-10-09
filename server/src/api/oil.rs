//! Перелив масла: остаток из одного масла в другое со средневзвешенной себестоимостью (SPEC-14).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::receipts::lock_products;
use crate::auth::{Ctx, Owner};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/oil/transfers", routing::get(list).post(post_transfer))
        .route(
            "/oil/transfers/{id}/reverse",
            routing::post(reverse_transfer),
        )
}

#[derive(Deserialize)]
pub struct TransferReq {
    pub op_id: Uuid,
    pub from_product_id: Uuid,
    pub to_product_id: Uuid,
    pub qty_ml: i64,
    #[serde(default)]
    pub comment: String,
}

/// Остаток масла и его средняя себестоимость за литр.
#[derive(Serialize, Deserialize, Clone)]
pub struct OilState {
    pub product_id: Uuid,
    pub name: String,
    pub stock_ml: i64,
    pub value_tyiyn: i64,
    pub avg_per_l_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TransferOut {
    pub id: Uuid,
    pub number: i64,
    pub qty_ml: i64,
    pub value_tyiyn: i64,
    pub from: OilState,
    pub to: OilState,
}

async fn oil_state(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<OilState> {
    let r = sqlx::query!(
        r#"select p.name, coalesce(bp.stock_qty, 0) as "qty!", coalesce(bp.stock_value_tyiyn, 0) as "value!"
           from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
           where p.id = $1"#,
        id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(OilState {
        product_id: id,
        name: r.name,
        stock_ml: r.qty,
        value_tyiyn: r.value,
        avg_per_l_tyiyn: (r.qty > 0)
            .then(|| div_round(i128::from(r.value) * 1000, i128::from(r.qty)))
            .flatten(),
    })
}

/// Перелив проводится одной транзакцией: два движения склада и запись документа (инвариант 7).
pub async fn post_transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: TransferReq,
) -> AppResult<TransferOut> {
    const KIND: &str = "oil.transfer";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    // Перелив и цена за литр — только владелец (инвариант 14).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.from_product_id == req.to_product_id {
        return Err(invalid("выберите разные масла"));
    }
    if req.qty_ml <= 0 {
        return Err(invalid("сколько переливаем — больше нуля"));
    }
    let branch_id = ctx.user.branch_id;
    for id in [req.from_product_id, req.to_product_id] {
        let p = sqlx::query!("select unit, archived from products where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("товар не найден"))?;
        if p.archived {
            return Err(invalid("товар в архиве"));
        }
        if p.unit != "ml" {
            return Err(invalid("переливать можно только масло"));
        }
    }
    // Блокировка остатков в порядке id: встречные переливы не ждут друг друга вечно.
    lock_products(conn, branch_id, [req.from_product_id, req.to_product_id]).await?;
    let archived = sqlx::query_scalar!(
        r#"select count(*) as "n!" from products where id = any($1) and archived"#,
        &[req.from_product_id, req.to_product_id][..]
    )
    .fetch_one(&mut *conn)
    .await?;
    if archived > 0 {
        return Err(invalid("товар в архиве"));
    }
    let source = ops::lock_pool(conn, branch_id, req.from_product_id).await?;
    if req.qty_ml > source.qty {
        return Err(invalid(format!(
            "в источнике только {} мл",
            source.qty.max(0)
        )));
    }
    // Стоимость уходит по средней источника; весь остаток — ровно вся его стоимость (ADR-011).
    let value = cost_of(req.qty_ml, &source).ok_or_else(overflow)?;
    let number = ops::next_counter(conn, branch_id, "oil_transfer").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into oil_transfers (id, branch_id, number, from_product_id, to_product_id, qty_ml,
                                      value_tyiyn, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        req.from_product_id,
        req.to_product_id,
        req.qty_ml,
        value,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: req.from_product_id,
            qty_delta: -req.qty_ml,
            value_delta: -value,
            doc_type: "oil_transfer",
            doc_id: id,
        },
    )
    .await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: req.to_product_id,
            qty_delta: req.qty_ml,
            value_delta: value,
            doc_type: "oil_transfer",
            doc_id: id,
        },
    )
    .await?;
    let from = oil_state(conn, branch_id, req.from_product_id).await?;
    let to = oil_state(conn, branch_id, req.to_product_id).await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "product",
        Some(req.to_product_id),
        json!({
            "number": number,
            "from": req.from_product_id,
            "from_name": from.name,
            "to_name": to.name,
            "qty_ml": req.qty_ml,
            "value": value,
            "avg_per_l": to.avg_per_l_tyiyn,
        }),
    )
    .await?;
    let out = TransferOut {
        id,
        number,
        qty_ml: req.qty_ml,
        value_tyiyn: value,
        from,
        to,
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<TransferReq>,
) -> AppResult<Json<TransferOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_transfer_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Отмена перелива: масло возвращается в источник той же стоимостью, с какой ушло (ADR-052).
/// Если перелитое уже продано и в получателе меньше, чем переливали, — отменить нельзя.
pub async fn reverse_transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<TransferOut> {
    const KIND: &str = "oil.transfer_reverse";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину отмены"));
    }
    let branch_id = ctx.user.branch_id;
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let t = sqlx::query!(
        r#"select number, from_product_id, to_product_id, qty_ml, value_tyiyn,
                  exists (select 1 from oil_transfer_reversals r where r.transfer_id = t.id) as "reversed!"
           from oil_transfers t where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if t.reversed {
        return Err(AppError::Conflict("перелив уже отменён".into()));
    }
    // Карточку могли объединить с другой: масло лежит на основной (ADR-044).
    let from_id = ops::live_product(conn, t.from_product_id).await?;
    let to_id = ops::live_product(conn, t.to_product_id).await?;
    if from_id == to_id {
        return Err(invalid(
            "масла объединены в одну карточку — отменять нечего",
        ));
    }
    lock_products(conn, branch_id, [from_id, to_id]).await?;
    let target = ops::lock_pool(conn, branch_id, to_id).await?;
    if target.qty < t.qty_ml {
        return Err(invalid(format!(
            "в получателе осталось {} мл из {} — перелитое уже продано, отмена невозможна",
            target.qty.max(0),
            t.qty_ml
        )));
    }
    let rid = new_id();
    sqlx::query!(
        r#"insert into oil_transfer_reversals (id, branch_id, transfer_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        rid,
        branch_id,
        id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (product_id, sign) in [(to_id, -1i64), (from_id, 1)] {
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id,
                qty_delta: sign * t.qty_ml,
                value_delta: sign * t.value_tyiyn,
                doc_type: "oil_transfer_reversal",
                doc_id: rid,
            },
        )
        .await?;
    }
    let from = oil_state(conn, branch_id, from_id).await?;
    let to = oil_state(conn, branch_id, to_id).await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "product",
        Some(from_id),
        json!({
            "number": t.number,
            "from_name": from.name,
            "to_name": to.name,
            "qty_ml": t.qty_ml,
            "value": t.value_tyiyn,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    let out = TransferOut {
        id,
        number: t.number,
        qty_ml: t.qty_ml,
        value_tyiyn: t.value_tyiyn,
        from,
        to,
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn reverse_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<TransferOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_transfer_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct TransferRow {
    id: Uuid,
    number: i64,
    from_name: String,
    to_name: String,
    qty_ml: i64,
    value_tyiyn: i64,
    comment: String,
    user_name: String,
    created_at: DateTime<Utc>,
    reversed: bool,
}

async fn list(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<Vec<TransferRow>>> {
    let rows = sqlx::query_as!(
        TransferRow,
        r#"select t.id, t.number, f.name as from_name, d.name as to_name, t.qty_ml, t.value_tyiyn,
                  t.comment, u.full_name as user_name, t.created_at,
                  exists (select 1 from oil_transfer_reversals r where r.transfer_id = t.id) as "reversed!"
           from oil_transfers t
           join products f on f.id = t.from_product_id
           join products d on d.id = t.to_product_id
           join users u on u.id = t.user_id
           where t.branch_id = $1
           order by t.created_at desc limit 100"#,
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}
