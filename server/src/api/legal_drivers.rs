//! Водители фирмы из кабинета ждут подтверждения, как машины (задача 037 п. 3, ADR-199).
//!
//! Руководитель фирмы в кабинете добавляет водителя — карточка `party_contacts` со
//! `status = 'pending'`: в долг фирмы по нему не провести, пока не подтвердит руководитель
//! (кабинет, «Запросы») или владелец/администратор с правом «Клиенты и машины».

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cabinet::Client;
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/contacts", routing::post(client_add))
        .route("/client/contact-requests", routing::get(client_list))
        .route(
            "/client/contact-requests/{id}/approve",
            routing::post(client_approve),
        )
        .route(
            "/client/contact-requests/{id}/reject",
            routing::post(client_reject),
        )
        .route("/parties/{id}/contact-requests", routing::get(staff_list))
        .route(
            "/parties/{id}/contact-requests/{rid}/approve",
            routing::post(staff_approve),
        )
        .route(
            "/parties/{id}/contact-requests/{rid}/reject",
            routing::post(staff_reject),
        )
}

const REASON_LEN: usize = 300;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ContactRequestOut {
    pub id: Uuid,
    pub contact_id: Uuid,
    pub full_name: String,
    pub phone: String,
    pub source: String,
    pub author: String,
    pub status: String,
    pub reason: String,
    pub decided_by: String,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

async fn load(
    conn: &mut PgConnection,
    firm: Uuid,
    id: Option<Uuid>,
) -> AppResult<Vec<ContactRequestOut>> {
    Ok(sqlx::query_as!(
        ContactRequestOut,
        r#"select r.id, r.contact_id, c.full_name, c.phone, r.source, r.author, r.status, r.reason,
                  r.decided_by, r.created_at, r.decided_at
           from contact_requests r join party_contacts c on c.id = r.contact_id
           where r.party_id = $1 and ($2::uuid is null or r.id = $2)
           order by (r.status = 'pending') desc, r.created_at desc limit 200"#,
        firm,
        id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Событие владельцу `party.contact_request`: водитель ждёт подтверждения или решение.
async fn journal(conn: &mut PgConnection, request_id: Uuid, decision: &str) -> AppResult<()> {
    let r = sqlx::query!(
        r#"select r.branch_id, r.party_id, p.name, c.full_name, r.source, r.author, r.reason,
                  r.decided_by
           from contact_requests r join parties p on p.id = r.party_id
           join party_contacts c on c.id = r.contact_id where r.id = $1"#,
        request_id
    )
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, 'party.contact_request', 'party', $3, $4)"#,
        ops::new_id(),
        r.branch_id,
        r.party_id,
        json!({
            "request_id": request_id, "name": r.name, "contact": r.full_name,
            "decision": decision, "source": r.source, "author": r.author, "reason": r.reason,
            "by": r.decided_by,
        })
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Решение по запросу фирмы `firm`: подтвердить — водитель `active`, отклонить — `rejected`.
pub async fn decide(
    conn: &mut PgConnection,
    firm: Uuid,
    id: Uuid,
    approve: bool,
    reason: &str,
    by: &str,
) -> AppResult<ContactRequestOut> {
    let reason = reason.trim();
    if !approve && reason.is_empty() {
        return Err(invalid("укажите причину отказа"));
    }
    if reason.chars().count() > REASON_LEN {
        return Err(invalid("причина — до 300 знаков"));
    }
    let r = sqlx::query!(
        "select status, contact_id from contact_requests where id = $1 and party_id = $2 for update",
        id,
        firm
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let status = if approve { "approved" } else { "rejected" };
    if r.status != "pending" {
        if r.status == status {
            return load(conn, firm, Some(id))
                .await?
                .pop()
                .ok_or(AppError::NotFound);
        }
        return Err(AppError::Conflict("по запросу уже решено".into()));
    }
    sqlx::query!(
        r#"update contact_requests set status = $2, reason = $3, decided_by = $4, decided_at = now()
           where id = $1"#,
        id,
        status,
        if approve { "" } else { reason },
        by
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "update party_contacts set status = $2 where id = $1",
        r.contact_id,
        if approve { "active" } else { "rejected" }
    )
    .execute(&mut *conn)
    .await?;
    journal(conn, id, status).await?;
    load(conn, firm, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct AddIn {
    op_id: Uuid,
    full_name: String,
    #[serde(default)]
    phone: String,
}

/// Кабинет фирмы: руководитель добавляет водителя — он ждёт подтверждения.
async fn client_add(
    State(state): State<AppState>,
    client: Client,
    Json(req): Json<AddIn>,
) -> AppResult<Json<ContactRequestOut>> {
    client.customer()?;
    let name = req.full_name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(invalid("имя водителя — от 1 до 120 знаков"));
    }
    let phone = req.phone.trim();
    if phone.chars().count() > 30 {
        return Err(invalid("телефон — до 30 знаков"));
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "select id from parties where id = $1 for update",
        client.party_id
    )
    .fetch_one(&mut *tx)
    .await?;
    // Повтор того же запроса (I7, I8).
    if let Some(done) = sqlx::query!(
        "select id, party_id from contact_requests where op_id = $1",
        req.op_id
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        if done.party_id != client.party_id {
            return Err(AppError::Conflict("операция уже использована".into()));
        }
        let out = load(&mut tx, client.party_id, Some(done.id)).await?.pop();
        return out.map(Json).ok_or(AppError::NotFound);
    }
    let pending = sqlx::query_scalar!(
        r#"select exists (select 1 from party_contacts
                          where party_id = $1 and active and status = 'pending'
                            and lower(full_name) = lower($2)) as "e!""#,
        client.party_id,
        name
    )
    .fetch_one(&mut *tx)
    .await?;
    if pending {
        return Err(AppError::Conflict(
            "этот водитель уже ждёт подтверждения".into(),
        ));
    }
    let cid = ops::new_id();
    sqlx::query!(
        r#"insert into party_contacts (id, branch_id, party_id, full_name, phone, status)
           values ($1, $2, $3, $4, $5, 'pending')"#,
        cid,
        client.branch_id,
        client.party_id,
        name,
        phone
    )
    .execute(&mut *tx)
    .await?;
    let id = ops::new_id();
    sqlx::query!(
        r#"insert into contact_requests (id, branch_id, contact_id, party_id, source, author, op_id)
           values ($1, $2, $3, $4, 'cabinet', 'Кабинет', $5)"#,
        id,
        client.branch_id,
        cid,
        client.party_id,
        req.op_id
    )
    .execute(&mut *tx)
    .await?;
    journal(&mut tx, id, "new").await?;
    let out = load(&mut tx, client.party_id, Some(id)).await?.pop();
    tx.commit().await?;
    out.map(Json).ok_or(AppError::NotFound)
}

async fn client_list(
    State(state): State<AppState>,
    client: Client,
) -> AppResult<Json<Vec<ContactRequestOut>>> {
    client.customer()?;
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, client.party_id, None).await?))
}

#[derive(Deserialize)]
struct DecideIn {
    #[serde(default)]
    op_id: Option<Uuid>,
    #[serde(default)]
    reason: String,
}

async fn client_approve(
    State(state): State<AppState>,
    client: Client,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ContactRequestOut>> {
    client.customer()?;
    let mut tx = state.pool.begin().await?;
    let out = decide(&mut tx, client.party_id, id, true, "", "Кабинет").await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn client_reject(
    State(state): State<AppState>,
    client: Client,
    Path(id): Path<Uuid>,
    Json(req): Json<DecideIn>,
) -> AppResult<Json<ContactRequestOut>> {
    client.customer()?;
    let mut tx = state.pool.begin().await?;
    let out = decide(&mut tx, client.party_id, id, false, &req.reason, "Кабинет").await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn firm_of(conn: &mut PgConnection, id: Uuid) -> AppResult<()> {
    let ok = sqlx::query_scalar!(
        r#"select exists (select 1 from parties where id = $1 and role = 'customer') as "e!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !ok {
        return Err(AppError::NotFound);
    }
    Ok(())
}

async fn staff_list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<ContactRequestOut>>> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "clients").await?;
    firm_of(&mut conn, id).await?;
    Ok(Json(load(&mut conn, id, None).await?))
}

async fn staff_decide(
    state: AppState,
    ctx: Ctx,
    id: Uuid,
    rid: Uuid,
    approve: bool,
    req: DecideIn,
) -> AppResult<Json<ContactRequestOut>> {
    let kind = if approve {
        "party.contact_request.approve"
    } else {
        "party.contact_request.reject"
    };
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "clients").await?;
    if let Some(op) = req.op_id
        && let Some(done) = ops::begin_op::<ContactRequestOut>(&mut tx, &ctx, op, kind).await?
    {
        return Ok(Json(done));
    }
    firm_of(&mut tx, id).await?;
    let name = ctx.user.full_name.trim();
    let by = if name.is_empty() {
        ctx.user.login.clone()
    } else {
        name.to_string()
    };
    let out = decide(&mut tx, id, rid, approve, &req.reason, &by).await?;
    if let Some(op) = req.op_id {
        ops::finish_op(&mut tx, &ctx, op, kind, &out).await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

async fn staff_approve(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, rid)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideIn>,
) -> AppResult<Json<ContactRequestOut>> {
    staff_decide(state, ctx, id, rid, true, req).await
}

async fn staff_reject(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, rid)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideIn>,
) -> AppResult<Json<ContactRequestOut>> {
    staff_decide(state, ctx, id, rid, false, req).await
}
