//! Свой товар на нужды магазина (задача 038, ADR-198).
//!
//! Товар уходит со склада движением `own_use` по средней себестоимости (инвариант 3), как брак;
//! деньги кассы и смена не участвуют — товар уже оплачен при приходе. Себестоимость — строка
//! прибыли «Товар на нужды магазина» по дню записи. Сторно — новая строка со ссылкой: товар
//! возвращается той же стоимостью.

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/own-use", routing::get(list).post(post_own_use))
        .route("/own-use/{id}/reverse", routing::post(reverse))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OwnUseOut {
    pub id: Uuid,
    pub number: i64,
    pub product_id: Uuid,
    pub product: String,
    pub unit: String,
    pub qty: i64,
    /// Себестоимость — только владельцу.
    pub cost_tyiyn: Option<i64>,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub reversed: bool,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ListQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn list(
    State(state): State<AppState>,
    ctx: Ctx,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<OwnUseOut>>> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &ctx.user, "expenses").await?;
    let today = crate::api::fees::today(&mut conn).await?;
    // Администратор видит последние 7 дней — как расходы из кассы (SPEC-06).
    let floor = if ctx.user.is_owner() {
        None
    } else {
        Some(today - chrono::Days::new(6))
    };
    let from = q.from.unwrap_or(today - chrono::Days::new(6));
    let from = floor.map_or(from, |f| from.max(f));
    let to = q.to.unwrap_or(today);
    Ok(Json(load(&mut conn, &ctx.user, from, to, None).await?))
}

async fn load(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
    id: Option<Uuid>,
) -> AppResult<Vec<OwnUseOut>> {
    let owner = user.is_owner();
    let rows = sqlx::query!(
        r#"select w.id, w.number, w.product_id, p.name as product, p.unit, w.qty, w.cost_tyiyn,
                  w.comment, w.reversal_of, w.created_at, coalesce(u.full_name, u.login) as "user_name!",
                  exists (select 1 from own_use_writeoffs r where r.reversal_of = w.id) as "reversed!"
           from own_use_writeoffs w
           join products p on p.id = w.product_id
           join users u on u.id = w.user_id
           where w.branch_id = $1
             and ($4::uuid is null and (w.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
                  or w.id = $4)
           order by w.created_at desc
           limit 500"#,
        user.branch_id,
        from,
        to,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| OwnUseOut {
            id: r.id,
            number: r.number,
            product_id: r.product_id,
            product: r.product,
            unit: r.unit,
            qty: r.qty,
            cost_tyiyn: owner.then_some(r.cost_tyiyn),
            comment: r.comment,
            reversal_of: r.reversal_of,
            reversed: r.reversed,
            user_name: r.user_name,
            created_at: r.created_at,
        })
        .collect())
}

async fn load_one(conn: &mut PgConnection, user: &CurrentUser, id: Uuid) -> AppResult<OwnUseOut> {
    // По `id` дни не учитываются.
    let d = NaiveDate::default();
    load(conn, user, d, d, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct OwnUseReq {
    pub op_id: Uuid,
    pub product_id: Uuid,
    /// В единицах хранения товара (шт, мл, г, мм — I24).
    pub qty: i64,
    #[serde(default)]
    pub comment: String,
}

async fn post_own_use(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OwnUseReq>,
) -> AppResult<Json<OwnUseOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

fn qty_words(qty: i64, unit: &str) -> String {
    crate::api::bot_drafts::qty_text(qty, unit)
}

/// Списать свой товар на нужды магазина: право «Расходы», как мелкий расход (ответ 1а).
pub async fn post_tx(conn: &mut PgConnection, ctx: &Ctx, req: OwnUseReq) -> AppResult<OwnUseOut> {
    const KIND: &str = "stock.own_use";
    crate::auth::require_permission(&mut *conn, &ctx.user, "expenses").await?;
    if req.qty <= 0 || req.qty > 1_000_000_000 {
        return Err(invalid("количество больше нуля"));
    }
    let comment = req.comment.trim();
    if comment.chars().count() > 500 {
        return Err(invalid("комментарий не длиннее 500 знаков"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let p = sqlx::query!(
        "select name, unit, archived from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.archived {
        return Err(invalid("товар в архиве: выберите основную карточку"));
    }
    crate::api::receipts::require_listed(&mut *conn, branch_id, [req.product_id]).await?;
    let pool = ops::lock_pool(conn, branch_id, req.product_id).await?;
    ops::ensure_not_merged(conn, &[req.product_id]).await?;
    if pool.qty < req.qty {
        return Err(AppError::Conflict(format!(
            "на складе только {} — больше остатка не списать",
            qty_words(pool.qty.max(0), &p.unit)
        )));
    }
    // Остаток без стоимости — по последней цене закупки, как брак (ADR-070).
    let cost = if pool.value <= 0 {
        if pool.last_qty <= 0 {
            return Err(invalid(format!(
                "у «{}» нет стоимости остатка и ни одной закупки — укажите цену закупки, потом спишите",
                p.name
            )));
        }
        div_round(
            i128::from(req.qty) * i128::from(pool.last_cost),
            i128::from(pool.last_qty),
        )
        .ok_or_else(overflow)?
    } else {
        cost_of(req.qty, &pool).ok_or_else(overflow)?
    };
    let number = ops::next_counter(conn, branch_id, "own_use").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into own_use_writeoffs (id, branch_id, number, product_id, qty, cost_tyiyn, comment,
                                          user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        branch_id,
        number,
        req.product_id,
        req.qty,
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
            qty_delta: -req.qty,
            value_delta: -cost,
            doc_type: "own_use",
            doc_id: id,
        },
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "own_use",
        Some(id),
        json!({
            "number": number,
            "name": p.name,
            "qty": req.qty,
            "qty_text": qty_words(req.qty, &p.unit),
            "cost": cost,
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
) -> AppResult<Json<OwnUseOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно: владелец — любое; администратор — своё и в тот же день (как расход, SPEC-06).
pub async fn reverse_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<OwnUseOut> {
    const KIND: &str = "stock.own_use_reverse";
    crate::auth::require_permission(&mut *conn, &ctx.user, "expenses").await?;
    let comment = req.comment.trim();
    if comment.is_empty() {
        return Err(invalid("укажите причину"));
    }
    if comment.chars().count() > 500 {
        return Err(invalid("причина не длиннее 500 знаков"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let w = sqlx::query!(
        r#"select w.number, w.product_id, w.qty, w.cost_tyiyn, w.reversal_of, w.user_id,
                  (w.created_at at time zone 'Asia/Bishkek')::date as "day!", p.name, p.unit,
                  exists (select 1 from own_use_writeoffs r where r.reversal_of = w.id) as "reversed!"
           from own_use_writeoffs w join products p on p.id = w.product_id
           where w.id = $1 and w.branch_id = $2
           for update of w"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if w.reversal_of.is_some() {
        return Err(invalid("это уже сторно"));
    }
    if w.reversed {
        return Err(AppError::Conflict("списание уже сторнировано".into()));
    }
    if !ctx.user.is_owner()
        && (w.user_id != ctx.user.id || w.day != crate::api::fees::today(&mut *conn).await?)
    {
        return Err(AppError::Forbidden);
    }
    // Карточку могли влить в другую: товар возвращается на основную.
    let target = ops::live_product(conn, w.product_id).await?;
    ops::lock_pool(conn, branch_id, target).await?;
    ops::ensure_not_merged(conn, &[target]).await?;
    let number = ops::next_counter(conn, branch_id, "own_use").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into own_use_writeoffs (id, branch_id, number, product_id, qty, cost_tyiyn, comment,
                                          reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        rid,
        branch_id,
        number,
        target,
        -w.qty,
        -w.cost_tyiyn,
        comment,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("списание уже сторнировано".into())
        } else {
            AppError::from(e)
        }
    })?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: target,
            qty_delta: w.qty,
            value_delta: w.cost_tyiyn,
            doc_type: "own_use",
            doc_id: rid,
        },
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "own_use",
        Some(id),
        json!({
            "number": w.number,
            "name": w.name,
            "qty_text": qty_words(w.qty, &w.unit),
            "cost": w.cost_tyiyn,
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
