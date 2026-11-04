//! Незавершённые приход и оприходование (задача 030, ADR-190): поля формы «Новый приход», как их
//! ввели, — чтобы случайный переход или закрытие вкладки ничего не стёрли. Это не учётная запись:
//! остаток, деньги и себестоимость не меняются, суммы считаются при проведении. Черновик свой у
//! каждого входа в филиале, один на вид; проведение с его `op_id` удаляет его в своей транзакции.

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::new_id;
use crate::state::AppState;

/// С запасом на 500 строк документа.
const MAX_BODY_BYTES: usize = 512 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/entry-drafts/{kind}", routing::get(get_one).put(save))
        .route("/entry-drafts/{kind}/discard", routing::post(discard))
}

#[derive(Serialize)]
struct EntryDraftOut {
    kind: String,
    op_id: Uuid,
    seq: i64,
    body: Value,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct SaveReq {
    op_id: Uuid,
    seq: i64,
    body: Value,
}

/// Вид документа и его право: тот же раздел, что у проведения (ADR-097).
async fn require_kind(conn: &mut PgConnection, user: &CurrentUser, kind: &str) -> AppResult<()> {
    let permission = match kind {
        "stock_in" => "stock_in",
        "receipt" => "receipts",
        _ => return Err(AppError::NotFound),
    };
    crate::auth::require_permission(conn, user, permission).await
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(kind): Path<String>,
) -> AppResult<Json<Option<EntryDraftOut>>> {
    let mut conn = state.pool.acquire().await?;
    require_kind(&mut conn, &user, &kind).await?;
    let row = sqlx::query!(
        "select op_id, seq, body, updated_at from entry_drafts
         where branch_id = $1 and user_id = $2 and kind = $3",
        user.branch_id,
        user.id,
        kind
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(Json(row.map(|r| EntryDraftOut {
        kind,
        op_id: r.op_id,
        seq: r.seq,
        body: r.body,
        updated_at: r.updated_at,
    })))
}

/// Записывает форму. Устаревшая запись (`seq` не больше сохранённого) не применяется; документ с
/// этим `op_id` уже проведён — 409: запоздавшее сохранение не вернёт черновик после проведения
/// (та же блокировка, что у `ops::begin_op`).
async fn save(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(kind): Path<String>,
    Json(req): Json<SaveReq>,
) -> AppResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    require_kind(&mut tx, &ctx.user, &kind).await?;
    if !req.body.is_object() || req.body.to_string().len() > MAX_BODY_BYTES {
        return Err(invalid("черновик слишком большой или неверного вида"));
    }
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        req.op_id.to_string()
    )
    .execute(&mut *tx)
    .await?;
    let posted = sqlx::query_scalar!(
        r#"select exists (select 1 from operations where op_id = $1) as "posted!""#,
        req.op_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if posted {
        return Err(AppError::Conflict("Этот документ уже проведён".into()));
    }
    let saved = sqlx::query!(
        "insert into entry_drafts (id, branch_id, user_id, kind, op_id, seq, body)
         values ($1, $2, $3, $4, $5, $6, $7)
         on conflict (branch_id, user_id, kind) do update
           set op_id = excluded.op_id, seq = excluded.seq, body = excluded.body, updated_at = now()
           where entry_drafts.seq < excluded.seq",
        new_id(),
        ctx.user.branch_id,
        ctx.user.id,
        kind,
        req.op_id,
        req.seq,
        req.body
    )
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    tx.commit().await?;
    Ok(Json(json!({ "saved": saved })))
}

async fn discard(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(kind): Path<String>,
) -> AppResult<Json<Value>> {
    let mut conn = state.pool.acquire().await?;
    require_kind(&mut conn, &ctx.user, &kind).await?;
    sqlx::query!(
        "delete from entry_drafts where branch_id = $1 and user_id = $2 and kind = $3",
        ctx.user.branch_id,
        ctx.user.id,
        kind
    )
    .execute(&mut *conn)
    .await?;
    Ok(Json(json!({ "ok": true })))
}

/// Проведение документа: черновик с его `op_id` исчезает в той же транзакции (I7).
pub async fn clear_posted(conn: &mut PgConnection, ctx: &Ctx, op_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "delete from entry_drafts where user_id = $1 and op_id = $2",
        ctx.user.id,
        op_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}
