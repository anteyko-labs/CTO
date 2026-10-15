//! Ставки комиссии банка по способам оплаты с историей (SPEC-08, ADR-022, ADR-065).

use axum::extract::State;
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/settings/payment-fees", routing::get(list).post(set_rate))
}

/// Способы оплаты, с которых банк берёт комиссию.
const METHODS: [&str; 2] = ["card", "transfer"];

/// Начальная ставка нового филиала — 0,5 % (ADR-022), действует «всегда».
pub async fn ensure_fees(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    for method in METHODS {
        sqlx::query!(
            r#"insert into payment_method_fees (id, branch_id, method, rate_bp, active_from)
               select $1, $2, $3, 50, date '2000-01-01'
               where not exists (select 1 from payment_method_fees where branch_id = $2 and method = $3)
               on conflict do nothing"#,
            new_id(),
            branch_id,
            method
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Ставка, действовавшая в учётный день документа: чек без сети вчерашнего дня получает
/// вчерашнюю ставку, а не ту, что владелец поставил сегодня (SPEC-08). Нет ставки — 0.
pub async fn rate_on(
    conn: &mut PgConnection,
    branch_id: Uuid,
    method: &str,
    on: NaiveDate,
) -> AppResult<i64> {
    let rate = sqlx::query_scalar!(
        r#"select rate_bp from payment_method_fees
           where branch_id = $1 and method = $2
             and active_from <= $3 and (active_to is null or active_to > $3)
           order by active_from desc, created_at desc limit 1"#,
        branch_id,
        method,
        on
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(rate.map_or(0, i64::from))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeeRow {
    pub id: Uuid,
    pub method: String,
    pub rate_bp: i32,
    pub active_from: NaiveDate,
    pub active_to: Option<NaiveDate>,
    pub user_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeesOut {
    /// Ставка на сегодня по каждому способу.
    pub current: Vec<FeeRow>,
    /// Вся история, новые сверху.
    pub history: Vec<FeeRow>,
}

async fn load(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<FeesOut> {
    let history = sqlx::query_as!(
        FeeRow,
        r#"select f.id, f.method, f.rate_bp, f.active_from, f.active_to as "active_to?",
                  u.full_name as "user_name?", f.created_at
           from payment_method_fees f left join users u on u.id = f.user_id
           where f.branch_id = $1
           order by f.method, f.active_from desc, f.created_at desc"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let current = METHODS
        .iter()
        .filter_map(|m| {
            history
                .iter()
                .find(|f| {
                    f.method == *m
                        && f.active_from <= today
                        && f.active_to.is_none_or(|to| to > today)
                })
                .cloned()
        })
        .collect();
    Ok(FeesOut { current, history })
}

async fn list(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<FeesOut>> {
    // Ставки банка — настройка денег точки, только владельцу (SPEC-08).
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    ensure_fees(&mut conn, user.branch_id).await?;
    Ok(Json(load(&mut conn, user.branch_id).await?))
}

#[derive(Deserialize)]
struct RateReq {
    op_id: Uuid,
    method: String,
    rate_bp: i32,
    active_from: NaiveDate,
}

/// Новая ставка с даты (не раньше сегодня): прежняя закрывается этой датой, прошлые чеки свою
/// комиссию сохраняют.
async fn set_rate(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RateReq>,
) -> AppResult<Json<FeesOut>> {
    const KIND: &str = "settings.payment_fee";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !METHODS.contains(&req.method.as_str()) {
        return Err(invalid("способ оплаты: card или transfer"));
    }
    if !(0..=10_000).contains(&req.rate_bp) {
        return Err(invalid("ставка от 0 до 100 %"));
    }
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    ensure_fees(&mut tx, branch_id).await?;
    // Две правки ставки одного способа идут по очереди: открытой остаётся одна.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 5))",
        format!("{branch_id}:{}", req.method)
    )
    .execute(&mut *tx)
    .await?;
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *tx)
        .await?;
    // Задним числом ставку не поставить: комиссия уже проведённых чеков не меняется, а возврат
    // берёт комиссию исходного чека (ADR-070).
    if req.active_from < today {
        return Err(invalid(
            "ставка действует с сегодняшнего дня или позже: задним числом не меняется",
        ));
    }
    if req.active_from > today + chrono::Duration::days(366) {
        return Err(invalid("дата начала — не дальше года вперёд"));
    }
    let open = sqlx::query!(
        r#"select id, rate_bp, active_from from payment_method_fees
           where branch_id = $1 and method = $2 and active_to is null"#,
        branch_id,
        req.method
    )
    .fetch_optional(&mut *tx)
    .await?;
    // История только дописывается: раньше действующей ставки новую не поставить.
    if let Some(o) = &open
        && req.active_from < o.active_from
    {
        return Err(invalid(format!(
            "действующая ставка начала действовать {}: новая — не раньше",
            o.active_from.format("%d.%m.%Y")
        )));
    }
    if let Some(o) = &open {
        sqlx::query!(
            "update payment_method_fees set active_to = $2 where id = $1",
            o.id,
            req.active_from
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!(
        r#"insert into payment_method_fees (id, branch_id, method, rate_bp, active_from, user_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        new_id(),
        branch_id,
        req.method,
        req.rate_bp,
        req.active_from,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "settings",
        None,
        json!({
            "method": req.method,
            "before": open.as_ref().map(|o| json!({ "rate_bp": o.rate_bp, "active_from": o.active_from })),
            "after": { "rate_bp": req.rate_bp, "active_from": req.active_from },
        }),
    )
    .await?;
    let out = load(&mut tx, branch_id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
