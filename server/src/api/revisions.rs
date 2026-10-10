//! Ревизия склада: пересчёт по категориям и выравнивание остатков (SPEC-15).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::receipts::lock_products;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/revisions", routing::get(list).post(create))
        .route("/revisions/{id}", routing::get(get_one))
        .route("/revisions/{id}/lines", routing::put(put_line))
        .route(
            "/revisions/{id}/lines/{product_id}",
            routing::delete(delete_line),
        )
        .route("/revisions/{id}/post", routing::post(post_revision))
        .route("/revisions/{id}/cancel", routing::post(cancel))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionHead {
    pub id: Uuid,
    pub number: i64,
    pub category_id: Option<Uuid>,
    pub category_name: Option<String>,
    pub status: String,
    pub comment: String,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub posted_at: Option<DateTime<Utc>>,
    pub counted: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionLine {
    pub product_id: Uuid,
    pub name: String,
    pub article: String,
    pub barcodes: Vec<String>,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// Остаток на момент пересчёта строки (не пересчитана — сейчас); проведённая — тот же, из итога.
    pub expected_qty: i64,
    pub counted_qty: Option<i64>,
    /// Стоимость расхождения — только владельцу (инвариант 13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_delta_tyiyn: Option<i64>,
    /// Масло: расхождение больше нормы (вопрос 32, ADR-052).
    #[serde(default)]
    pub over_norm: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionOut {
    pub head: RevisionHead,
    pub lines: Vec<RevisionLine>,
    /// Норма расхождения по маслу, сотые доли процента от «должно быть».
    #[serde(default)]
    pub oil_norm_bp: i32,
}

/// Расхождение по маслу больше нормы: |пересчитано − должно| > должно × норма.
pub fn over_norm(unit: &str, expected: i64, counted: Option<i64>, norm_bp: i32) -> bool {
    let Some(counted) = counted else { return false };
    if unit != "ml" {
        return false;
    }
    let diff = i128::from(counted) - i128::from(expected);
    diff.abs() * 10_000 > i128::from(expected.max(0)) * i128::from(norm_bp)
}

async fn head(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<RevisionHead> {
    sqlx::query_as!(
        RevisionHead,
        r#"select r.id, r.number, r.category_id, c.name as "category_name?", r.status, r.comment,
                  u.full_name as user_name, r.created_at, r.posted_at,
                  (select count(*) from revision_lines l where l.revision_id = r.id) as "counted!"
           from revisions r
           join users u on u.id = r.user_id
           left join categories c on c.id = r.category_id
           where r.id = $1 and r.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

async fn load(conn: &mut PgConnection, user: &CurrentUser, id: Uuid) -> AppResult<RevisionOut> {
    let h = head(conn, user.branch_id, id).await?;
    let owner = user.is_owner();
    let norm = crate::api::receipts::oil_norm_bp(conn, user.branch_id).await?;
    let lines = if h.status == "posted" {
        sqlx::query!(
            r#"select p.id, p.name, p.article, p.unit, p.container_ml,
                      coalesce((select array_agg(b.code order by b.code) from product_barcodes b
                                where b.product_id = p.id), '{}') as "barcodes!",
                      x.expected_qty, x.counted_qty, x.value_delta_tyiyn
               from revision_results x join products p on p.id = x.product_id
               where x.revision_id = $1 order by p.name"#,
            id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| RevisionLine {
            product_id: r.id,
            name: r.name,
            article: r.article,
            barcodes: r.barcodes,
            container_ml: r.container_ml,
            expected_qty: r.expected_qty,
            over_norm: over_norm(&r.unit, r.expected_qty, Some(r.counted_qty), norm),
            counted_qty: Some(r.counted_qty),
            value_delta_tyiyn: owner.then_some(r.value_delta_tyiyn),
            unit: r.unit,
        })
        .collect()
    } else {
        // Черновик: товары категории (или все) и пересчитанное, сверху — уже посчитанные.
        sqlx::query!(
            r#"select p.id, p.name, p.article, p.unit, p.container_ml,
                      coalesce((select array_agg(b.code order by b.code) from product_barcodes b
                                where b.product_id = p.id), '{}') as "barcodes!",
                      coalesce(l.stock_at_count, bp.stock_qty, 0) as "expected!", l.counted_qty as "counted?"
               from products p
               left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               left join revision_lines l on l.product_id = p.id and l.revision_id = $1
               where not p.archived
                 and ($3::uuid is null or p.category_id = $3)
                 and (l.product_id is not null or coalesce(bp.stock_qty, 0) <> 0 or $3::uuid is not null)
               order by (l.product_id is null), p.name
               limit 3000"#,
            id,
            user.branch_id,
            h.category_id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| RevisionLine {
            product_id: r.id,
            name: r.name,
            article: r.article,
            barcodes: r.barcodes,
            over_norm: over_norm(&r.unit, r.expected, r.counted, norm),
            unit: r.unit,
            container_ml: r.container_ml,
            expected_qty: r.expected,
            counted_qty: r.counted,
            value_delta_tyiyn: None,
        })
        .collect()
    };
    Ok(RevisionOut {
        head: h,
        lines,
        oil_norm_bp: norm,
    })
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<RevisionHead>>> {
    let rows = sqlx::query_as!(
        RevisionHead,
        r#"select r.id, r.number, r.category_id, c.name as "category_name?", r.status, r.comment,
                  u.full_name as user_name, r.created_at, r.posted_at,
                  (select count(*) from revision_lines l where l.revision_id = r.id) as "counted!"
           from revisions r
           join users u on u.id = r.user_id
           left join categories c on c.id = r.category_id
           where r.branch_id = $1
           order by r.created_at desc limit 100"#,
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<RevisionOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, &user, id).await?))
}

#[derive(Deserialize)]
struct CreateReq {
    category_id: Option<Uuid>,
    #[serde(default)]
    comment: String,
}

async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<RevisionOut>> {
    let mut tx = state.pool.begin().await?;
    if let Some(cid) = req.category_id {
        sqlx::query_scalar!("select id from categories where id = $1", cid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| invalid("категория не найдена"))?;
    }
    let number = ops::next_counter(&mut tx, ctx.user.branch_id, "revision").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into revisions (id, branch_id, number, category_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        id,
        ctx.user.branch_id,
        number,
        req.category_id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "revision.create",
        "revision",
        Some(id),
        json!({ "number": number }),
    )
    .await?;
    let out = load(&mut tx, &ctx.user, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Черновик ревизии этого филиала, заблокированный для правки.
async fn lock_draft(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<()> {
    let status = sqlx::query_scalar!(
        "select status from revisions where id = $1 and branch_id = $2 for update",
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if status != "draft" {
        return Err(AppError::Conflict(
            "ревизия уже проведена или отменена".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct LineReq {
    product_id: Uuid,
    counted_qty: i64,
}

async fn put_line(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LineReq>,
) -> AppResult<Json<serde_json::Value>> {
    if !(0..=1_000_000_000).contains(&req.counted_qty) {
        return Err(invalid("пересчитано: от 0"));
    }
    let mut tx = state.pool.begin().await?;
    lock_draft(&mut tx, ctx.user.branch_id, id).await?;
    let archived = sqlx::query_scalar!(
        "select archived from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    if archived {
        return Err(invalid("товар в архиве: считайте основную карточку"));
    }
    // Остаток в момент пересчёта: продажи между пересчётом и проведением не станут «излишком».
    let stock_now = sqlx::query_scalar!(
        "select stock_qty from branch_products where branch_id = $1 and product_id = $2",
        ctx.user.branch_id,
        req.product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(0);
    sqlx::query!(
        r#"insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count)
           values ($1, $2, $3, $4, $5)
           on conflict (revision_id, product_id)
           do update set counted_qty = excluded.counted_qty, user_id = excluded.user_id, updated_at = now(),
                         stock_at_count = excluded.stock_at_count"#,
        id,
        req.product_id,
        req.counted_qty,
        ctx.user.id,
        stock_now
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

async fn delete_line(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, product_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await?;
    lock_draft(&mut tx, ctx.user.branch_id, id).await?;
    sqlx::query!(
        "delete from revision_lines where revision_id = $1 and product_id = $2",
        id,
        product_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct PostReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Проведение: остатки выравниваются по пересчитанному, расхождение — в себестоимость (SPEC-15).
pub async fn post_revision_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: PostReq,
) -> AppResult<RevisionOut> {
    const KIND: &str = "revision.post";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину или кто пересчитывал"));
    }
    let branch_id = ctx.user.branch_id;
    lock_draft(conn, branch_id, id).await?;
    let lines = sqlx::query!(
        "select product_id, counted_qty, stock_at_count from revision_lines where revision_id = $1",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    if lines.is_empty() {
        return Err(invalid("ничего не пересчитано"));
    }
    lock_products(conn, branch_id, lines.iter().map(|l| l.product_id)).await?;
    // Карточку могли объединить или убрать в архив после пересчёта: её остаток уже на основной,
    // и «излишек» по дублю удвоил бы склад.
    let gone = sqlx::query_scalar!(
        r#"select name from products where id = any($1) and archived order by name limit 1"#,
        &lines.iter().map(|l| l.product_id).collect::<Vec<_>>()[..]
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(name) = gone {
        return Err(AppError::Conflict(format!(
            "«{name}» объединили или убрали в архив после пересчёта — удалите строку и пересчитайте основную карточку"
        )));
    }
    let (mut shortage, mut surplus) = (0i64, 0i64);
    for l in &lines {
        let pool = ops::lock_pool(conn, branch_id, l.product_id).await?;
        let delta = l
            .counted_qty
            .checked_sub(l.stock_at_count.unwrap_or(pool.qty))
            .ok_or_else(overflow)?;
        let value = if delta < 0 {
            // Недостача уходит по средней, как продажа (ADR-011).
            -cost_of(-delta, &pool).ok_or_else(overflow)?
        } else if delta > 0 {
            // Излишек — по средней, а при пустом остатке — по последней закупке.
            let per = if pool.qty > 0 && pool.value > 0 {
                (pool.value, pool.qty)
            } else {
                (pool.last_cost, pool.last_qty.max(1))
            };
            div_round(i128::from(delta) * i128::from(per.0), i128::from(per.1))
                .ok_or_else(overflow)?
        } else {
            0
        };
        if delta != 0 {
            ops::apply_movement(
                conn,
                Movement {
                    branch_id,
                    product_id: l.product_id,
                    qty_delta: delta,
                    value_delta: value,
                    doc_type: "revision",
                    doc_id: id,
                },
            )
            .await?;
        }
        if value < 0 {
            shortage = shortage.saturating_sub(value);
        } else {
            surplus = surplus.saturating_add(value);
        }
        sqlx::query!(
            r#"insert into revision_results (revision_id, product_id, expected_qty, counted_qty,
                                             qty_delta, value_delta_tyiyn)
               values ($1, $2, $3, $4, $5, $6)"#,
            id,
            l.product_id,
            l.stock_at_count.unwrap_or(pool.qty),
            l.counted_qty,
            delta,
            value
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        r#"update revisions set status = 'posted', posted_by = $2, posted_at = now(),
             comment = case when comment = '' then $3 else comment || ' · ' || $3 end
           where id = $1"#,
        id,
        ctx.user.id,
        req.comment.trim()
    )
    .execute(&mut *conn)
    .await?;
    let number = sqlx::query_scalar!("select number from revisions where id = $1", id)
        .fetch_one(&mut *conn)
        .await?;
    let out = load(conn, &ctx.user, id).await?;
    let over = out.lines.iter().filter(|l| l.over_norm).count();
    ops::audit(
        conn,
        ctx,
        KIND,
        "revision",
        Some(id),
        json!({ "number": number, "lines": lines.len(), "shortage": shortage, "surplus": surplus, "over_norm": over }),
    )
    .await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_revision(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PostReq>,
) -> AppResult<Json<RevisionOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_revision_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn cancel(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await?;
    lock_draft(&mut tx, ctx.user.branch_id, id).await?;
    sqlx::query!(
        "update revisions set status = 'cancelled' where id = $1",
        id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "revision.cancel",
        "revision",
        Some(id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
