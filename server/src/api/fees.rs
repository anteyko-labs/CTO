//! Ставки комиссии банка по безналичным счетам с историей (SPEC-08, ADR-022, ADR-065, ADR-077).
//! Ставку ставит владелец в карточке счёта: комиссию берёт банк, куда пришли деньги (терминал
//! «Оптимы» — с комиссией, перевод на карту владельца — без неё).

use axum::extract::{Path, State};
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
    Router::new().route("/cash/accounts/{id}/fees", routing::get(list))
}

/// Начальная ставка безналичного счёта нового филиала — 0,5 % (ADR-022), действует «всегда».
// Два первых запроса филиала одновременно: второй не падает на уникальной ставке.
pub async fn ensure_default_fee(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        r#"insert into cash_account_fees (id, branch_id, account_id, rate_bp, active_from)
           select $1, a.branch_id, a.id, 50, date '2000-01-01' from cash_accounts a
           where a.branch_id = $2 and a.kind = 'bank' and a.default_card
             and not exists (select 1 from cash_account_fees f where f.account_id = a.id)
           on conflict do nothing"#,
        new_id(),
        branch_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Сегодня по времени точки.
pub async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Ставка счёта, действовавшая в учётный день документа: чек без сети вчерашнего дня получает
/// вчерашнюю ставку, а не ту, что владелец поставил сегодня (SPEC-08). Нет ставки — 0.
pub async fn rate_on(conn: &mut PgConnection, account_id: Uuid, on: NaiveDate) -> AppResult<i64> {
    let rate = sqlx::query_scalar!(r#"select account_fee_on($1, $2) as "r!""#, account_id, on)
        .fetch_one(&mut *conn)
        .await?;
    Ok(i64::from(rate))
}

/// Комиссия с суммы по ставке счёта в день `on`.
pub async fn fee_on(
    conn: &mut PgConnection,
    account_id: Uuid,
    on: NaiveDate,
    amount: i64,
) -> AppResult<i64> {
    let rate = rate_on(conn, account_id, on).await?;
    Ok(crate::domain::money::div_round(i128::from(amount) * i128::from(rate), 10_000).unwrap_or(0))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeeRow {
    pub id: Uuid,
    pub rate_bp: i32,
    pub active_from: NaiveDate,
    pub active_to: Option<NaiveDate>,
    pub user_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FeesOut {
    /// Ставка на сегодня (нет ставки — 0).
    pub current_bp: i32,
    /// Вся история счёта, новые сверху.
    pub history: Vec<FeeRow>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<FeesOut>> {
    // Ставки банка — настройка денег точки, только владельцу (SPEC-08).
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    let kind = sqlx::query_scalar!(
        "select kind from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if kind != "bank" {
        return Err(invalid("комиссия банка — только у безналичного счёта"));
    }
    let history = sqlx::query_as!(
        FeeRow,
        r#"select f.id, f.rate_bp, f.active_from, f.active_to as "active_to?",
                  u.full_name as "user_name?", f.created_at
           from cash_account_fees f left join users u on u.id = f.user_id
           where f.account_id = $1
           order by f.active_from desc, f.created_at desc"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let on = today(&mut conn).await?;
    let current_bp = i32::try_from(rate_on(&mut conn, id, on).await?).unwrap_or(0);
    Ok(Json(FeesOut {
        current_bp,
        history,
    }))
}

/// Новая ставка счёта с даты (не раньше сегодня) — из правки счёта владельцем: прежняя
/// закрывается этой датой, прошлые чеки свою комиссию сохраняют. Та же ставка с той же или
/// более ранней даты — ничего не меняет (повтор правки не множит историю).
pub async fn set_rate(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account_id: Uuid,
    account_name: &str,
    rate_bp: i32,
    active_from: NaiveDate,
) -> AppResult<()> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !(0..=10_000).contains(&rate_bp) {
        return Err(invalid("ставка комиссии от 0 до 100 %"));
    }
    let branch_id = ctx.user.branch_id;
    let on = today(&mut *conn).await?;
    // Задним числом ставку не поставить: комиссия уже проведённых чеков не меняется, а возврат
    // берёт комиссию исходного чека (ADR-070).
    if active_from < on {
        return Err(invalid(
            "ставка действует с сегодняшнего дня или позже: задним числом не меняется",
        ));
    }
    if active_from > on + chrono::Duration::days(366) {
        return Err(invalid("дата начала — не дальше года вперёд"));
    }
    let open = sqlx::query!(
        r#"select id, rate_bp, active_from from cash_account_fees
           where account_id = $1 and active_to is null for update"#,
        account_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if open
        .as_ref()
        .is_some_and(|o| o.rate_bp == rate_bp && o.active_from <= active_from)
        || (open.is_none() && rate_bp == 0)
    {
        return Ok(());
    }
    // История только дописывается: раньше действующей ставки новую не поставить.
    if let Some(o) = &open
        && active_from < o.active_from
    {
        return Err(invalid(format!(
            "действующая ставка начала действовать {}: новая — не раньше",
            o.active_from.format("%d.%m.%Y")
        )));
    }
    if let Some(o) = &open {
        sqlx::query!(
            "update cash_account_fees set active_to = $2 where id = $1",
            o.id,
            active_from
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        r#"insert into cash_account_fees (id, branch_id, account_id, rate_bp, active_from, user_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        new_id(),
        branch_id,
        account_id,
        rate_bp,
        active_from,
        ctx.user.id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        "settings.payment_fee",
        "cash_account",
        Some(account_id),
        json!({
            "account": account_name,
            "before": open.as_ref().map(|o| json!({ "rate_bp": o.rate_bp, "active_from": o.active_from })),
            "after": { "rate_bp": rate_bp, "active_from": active_from },
        }),
    )
    .await?;
    Ok(())
}
