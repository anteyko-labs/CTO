//! Гаражи юрлица и подтверждение машин, пересчёт долга клиента (задача 037 п. 1–4, ADR-199).
//!
//! Гараж фирмы — руководитель добавляет машину в кабинете; личный гараж — работник фирмы в боте
//! добавляет свою машину «Оплачивает фирма» (`party_vehicles.paid_by_party_id`). Обе ждут
//! подтверждения (`status = 'pending'`): в долг фирмы их не провести, пока не подтвердит
//! руководитель (кабинет) или владелец/администратор с правом «Клиенты и машины». Долг за личную
//! машину ложится на фирму: касса указывает работника, сервер переносит чек на фирму
//! ([`payer_rewrite`]) — дальше работает подтверждение заправки фирмой (задача 039 C).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cabinet::Client;
use crate::api::sales::SaleReq;
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/vehicles", routing::post(client_add))
        .route("/client/requests", routing::get(client_list))
        .route(
            "/client/requests/{id}/approve",
            routing::post(client_approve),
        )
        .route("/client/requests/{id}/reject", routing::post(client_reject))
        .route("/parties/{id}/requests", routing::get(staff_list))
        .route(
            "/parties/{id}/requests/{rid}/approve",
            routing::post(staff_approve),
        )
        .route(
            "/parties/{id}/requests/{rid}/reject",
            routing::post(staff_reject),
        )
        .route("/parties/{id}/recount", routing::post(recount))
}

const REASON_LEN: usize = 300;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RequestOut {
    pub id: Uuid,
    pub vehicle_id: Uuid,
    pub plate: String,
    pub brand: String,
    pub model: String,
    /// Личная машина работника — его имя; машина фирмы — `None`.
    pub personal_of: Option<String>,
    pub source: String,
    pub author: String,
    pub status: String,
    pub reason: String,
    pub decided_by: String,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

async fn load(conn: &mut PgConnection, firm: Uuid, id: Option<Uuid>) -> AppResult<Vec<RequestOut>> {
    Ok(sqlx::query_as!(
        RequestOut,
        r#"select r.id, r.vehicle_id, v.plate, v.brand, v.model,
                  (select o.name from parties o where o.id = v.party_id and v.party_id <> r.party_id)
                      as "personal_of?",
                  r.source, r.author, r.status, r.reason, r.decided_by, r.created_at, r.decided_at
           from vehicle_requests r join party_vehicles v on v.id = r.vehicle_id
           where r.party_id = $1 and ($2::uuid is null or r.id = $2)
           order by (r.status = 'pending') desc, r.created_at desc limit 200"#,
        firm,
        id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Событие владельцу `party.vehicle_request`: новая машина ждёт подтверждения или решение.
async fn journal(conn: &mut PgConnection, request_id: Uuid, decision: &str) -> AppResult<()> {
    let r = sqlx::query!(
        r#"select r.branch_id, r.party_id, p.name, v.plate, r.source, r.author, r.reason, r.decided_by,
                  (select o.name from parties o where o.id = v.party_id and v.party_id <> r.party_id)
                      as "personal_of?"
           from vehicle_requests r join parties p on p.id = r.party_id
           join party_vehicles v on v.id = r.vehicle_id where r.id = $1"#,
        request_id
    )
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, 'party.vehicle_request', 'party', $3, $4)"#,
        ops::new_id(),
        r.branch_id,
        r.party_id,
        json!({
            "request_id": request_id, "name": r.name, "plate": r.plate, "decision": decision,
            "source": r.source, "author": r.author, "reason": r.reason, "by": r.decided_by,
            "personal_of": r.personal_of,
        })
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Новый запрос на подтверждение машины фирмой `firm`. Машина уже записана со `status = 'pending'`.
#[allow(clippy::too_many_arguments)]
pub async fn open_request(
    conn: &mut PgConnection,
    branch_id: Uuid,
    vehicle_id: Uuid,
    firm: Uuid,
    source: &str,
    author: &str,
    chat_id: Option<i64>,
    op_id: Option<Uuid>,
) -> AppResult<Uuid> {
    let id = ops::new_id();
    sqlx::query!(
        r#"insert into vehicle_requests (id, branch_id, vehicle_id, party_id, source, author, chat_id, op_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        id,
        branch_id,
        vehicle_id,
        firm,
        source,
        author,
        chat_id,
        op_id
    )
    .execute(&mut *conn)
    .await?;
    journal(conn, id, "new").await?;
    Ok(id)
}

/// Решение по запросу фирмы `firm`: подтвердить — машина `active`, отклонить — `rejected`.
pub async fn decide(
    conn: &mut PgConnection,
    firm: Uuid,
    id: Uuid,
    approve: bool,
    reason: &str,
    by: &str,
) -> AppResult<RequestOut> {
    let reason = reason.trim();
    if !approve && reason.is_empty() {
        return Err(invalid("укажите причину отказа"));
    }
    if reason.chars().count() > REASON_LEN {
        return Err(invalid("причина — до 300 знаков"));
    }
    let r = sqlx::query!(
        "select status, vehicle_id from vehicle_requests where id = $1 and party_id = $2 for update",
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
        r#"update vehicle_requests set status = $2, reason = $3, decided_by = $4, decided_at = now()
           where id = $1"#,
        id,
        status,
        if approve { "" } else { reason },
        by
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "update party_vehicles set status = $2 where id = $1",
        r.vehicle_id,
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

/// Фирмы, где работает владелец чата: его номер записан у работника фирмы-клиента.
pub async fn employer_firms(
    conn: &mut PgConnection,
    person: Uuid,
) -> AppResult<Vec<(Uuid, String)>> {
    let phone = sqlx::query_scalar!(
        "select phone from parties where id = $1 and kind = 'person'",
        person
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(key) = phone.as_deref().and_then(crate::api::telegram::phone_key) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query!(
        r#"select distinct p.id, p.name, c.phone from party_contacts c join parties p on p.id = c.party_id
           where c.active and c.phone <> '' and p.active and p.kind = 'company' and p.role = 'customer'
           order by p.name"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<(Uuid, String)> = Vec::new();
    for r in rows {
        if crate::api::telegram::phone_key(&r.phone).as_deref() == Some(key.as_str())
            && !out.iter().any(|(id, _)| *id == r.id)
        {
            out.push((r.id, r.name));
        }
    }
    Ok(out)
}

/// Личная машина работника, которую оплачивает фирма: чек с долгом переносится на фирму
/// (ответ 5а). Касса может указать и работника, и сразу фирму — итог один.
pub async fn payer_rewrite(conn: &mut PgConnection, req: &mut SaleReq) -> AppResult<()> {
    let Some(vid) = req.vehicle_id else {
        return Ok(());
    };
    let debt = req.payments.iter().any(|p| p.method == "debt");
    if !debt {
        return Ok(());
    }
    let v = sqlx::query!(
        "select party_id, paid_by_party_id, status from party_vehicles where id = $1",
        vid
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(v) = v
        && let Some(firm) = v.paid_by_party_id
        && req.party_id == Some(v.party_id)
        // Фирма отказала — работник платит сам.
        && v.status != "rejected"
    {
        req.party_id = Some(firm);
        // Работник чека — его карточка в фирме, если касса указала работника не из фирмы.
        if let Some(cid) = req.contact_id {
            let of_firm = sqlx::query_scalar!(
                r#"select exists (select 1 from party_contacts where id = $1 and party_id = $2) as "e!""#,
                cid,
                firm
            )
            .fetch_one(&mut *conn)
            .await?;
            if !of_firm {
                req.contact_id = None;
            }
        }
    }
    Ok(())
}

// ---------- Кабинет фирмы ----------

#[derive(Deserialize)]
struct AddIn {
    op_id: Uuid,
    plate: String,
    #[serde(default)]
    brand: String,
    #[serde(default)]
    model: String,
}

/// Гараж фирмы: руководитель добавляет машину — она ждёт подтверждения (п. 1, 3).
async fn client_add(
    State(state): State<AppState>,
    client: Client,
    Json(req): Json<AddIn>,
) -> AppResult<Json<RequestOut>> {
    client.customer()?;
    let norm = crate::domain::plate::parse(&req.plate)
        .ok_or_else(|| invalid("не похоже на госномер, пример: 01KG123ABC"))?;
    let (brand, model) = (req.brand.trim(), req.model.trim());
    if brand.chars().count() > 60 || model.chars().count() > 60 {
        return Err(invalid("марка и модель — до 60 знаков"));
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
        "select id, party_id from vehicle_requests where op_id = $1",
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
    let own = sqlx::query!(
        r#"select id, status from party_vehicles
           where party_id = $1 and plate_norm = $2 and active order by created_at limit 1"#,
        client.party_id,
        norm
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(v) = own {
        return Err(AppError::Conflict(if v.status == "pending" {
            "эта машина уже ждёт подтверждения".into()
        } else {
            "эта машина уже есть в гараже фирмы".into()
        }));
    }
    let vid = ops::new_id();
    sqlx::query!(
        r#"insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, brand, model, added_by, status)
           values ($1, $2, $3, $4, $4, $5, $6, 'client', 'pending')"#,
        vid,
        client.branch_id,
        client.party_id,
        norm,
        brand,
        model
    )
    .execute(&mut *tx)
    .await?;
    let id = open_request(
        &mut tx,
        client.branch_id,
        vid,
        client.party_id,
        "cabinet",
        "Кабинет",
        None,
        Some(req.op_id),
    )
    .await?;
    let out = load(&mut tx, client.party_id, Some(id)).await?.pop();
    tx.commit().await?;
    out.map(Json).ok_or(AppError::NotFound)
}

async fn client_list(
    State(state): State<AppState>,
    client: Client,
) -> AppResult<Json<Vec<RequestOut>>> {
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
) -> AppResult<Json<RequestOut>> {
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
) -> AppResult<Json<RequestOut>> {
    client.customer()?;
    let mut tx = state.pool.begin().await?;
    let out = decide(&mut tx, client.party_id, id, false, &req.reason, "Кабинет").await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Карточка фирмы на точке ----------

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
) -> AppResult<Json<Vec<RequestOut>>> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "clients").await?;
    firm_of(&mut conn, id).await?;
    Ok(Json(load(&mut conn, id, None).await?))
}

fn staff_name(user: &CurrentUser) -> String {
    let name = user.full_name.trim();
    if name.is_empty() {
        user.login.clone()
    } else {
        name.to_string()
    }
}

async fn staff_decide(
    state: AppState,
    ctx: Ctx,
    id: Uuid,
    rid: Uuid,
    approve: bool,
    req: DecideIn,
) -> AppResult<Json<RequestOut>> {
    let kind = if approve {
        "party.vehicle_request.approve"
    } else {
        "party.vehicle_request.reject"
    };
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "clients").await?;
    if let Some(op) = req.op_id
        && let Some(done) = ops::begin_op::<RequestOut>(&mut tx, &ctx, op, kind).await?
    {
        return Ok(Json(done));
    }
    firm_of(&mut tx, id).await?;
    let out = decide(
        &mut tx,
        id,
        rid,
        approve,
        &req.reason,
        &staff_name(&ctx.user),
    )
    .await?;
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
) -> AppResult<Json<RequestOut>> {
    staff_decide(state, ctx, id, rid, true, req).await
}

async fn staff_reject(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, rid)): Path<(Uuid, Uuid)>,
    Json(req): Json<DecideIn>,
) -> AppResult<Json<RequestOut>> {
    staff_decide(state, ctx, id, rid, false, req).await
}

// ---------- Сообщение автору в бот ----------

pub struct Notice {
    pub request_id: Uuid,
    pub chat_id: i64,
    pub text: String,
}

/// Решения по запросам из бота, о которых автору ещё не сказали (за неделю).
pub async fn notices(conn: &mut PgConnection) -> AppResult<Vec<Notice>> {
    let rows = sqlx::query!(
        r#"select r.id, r.chat_id as "chat_id!", r.status, r.reason, v.plate, p.name
           from vehicle_requests r join party_vehicles v on v.id = r.vehicle_id
           join parties p on p.id = r.party_id
           where r.chat_id is not null and r.notified_at is null and r.status <> 'pending'
             and r.decided_at > now() - interval '7 days'
           order by r.decided_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Notice {
            request_id: r.id,
            chat_id: r.chat_id,
            text: notice_text(&r.plate, &r.name, &r.status, &r.reason),
        })
        .collect())
}

pub fn notice_text(plate: &str, firm: &str, status: &str, reason: &str) -> String {
    if status == "approved" {
        format!("✅ Машина {plate}: фирма {firm} подтвердила оплату обслуживания.")
    } else {
        format!("✖ Машина {plate}: фирма {firm} не подтвердила оплату — {reason}.")
    }
}

pub async fn mark_notice(conn: &mut PgConnection, n: &Notice) -> AppResult<()> {
    sqlx::query!(
        "update vehicle_requests set notified_at = now() where id = $1 and chat_id = $2",
        n.request_id,
        n.chat_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------- Пересчёт долга клиента (п. 4) ----------

#[derive(Deserialize)]
struct RecountIn {
    op_id: Uuid,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BranchDiff {
    pub branch_id: Uuid,
    pub branch_name: String,
    pub before_tyiyn: i64,
    pub after_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecountOut {
    /// Баланс совпал с журналом долга — ничего не исправлено.
    pub matched: bool,
    pub before_tyiyn: i64,
    pub after_tyiyn: i64,
    pub branches: Vec<BranchDiff>,
}

/// «Пересчитать долг»: баланс клиента и его части по филиалам — по журналу долга (I6).
async fn recount(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<RecountIn>,
) -> AppResult<Json<RecountOut>> {
    ctx.user.require_owner()?;
    const KIND: &str = "party.recount";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<RecountOut>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = sqlx::query_scalar!(
        "select balance_tyiyn from parties where id = $1 for update",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let rows = sqlx::query!(
        r#"select b.id, b.name,
                  coalesce((select pb.balance_tyiyn from party_balances pb
                            where pb.party_id = $1 and pb.branch_id = b.id), 0)::bigint as "cached!",
                  coalesce((select sum(l.amount_tyiyn) from party_ledger l
                            where l.party_id = $1 and l.branch_id = b.id), 0)::bigint as "ledger!"
           from branches b order by b.created_at"#,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    let after: i64 = rows.iter().map(|r| r.ledger).sum();
    let branches: Vec<BranchDiff> = rows
        .iter()
        .filter(|r| r.cached != r.ledger)
        .map(|r| BranchDiff {
            branch_id: r.id,
            branch_name: r.name.clone(),
            before_tyiyn: r.cached,
            after_tyiyn: r.ledger,
        })
        .collect();
    let matched = before == after && branches.is_empty();
    let out = RecountOut {
        matched,
        before_tyiyn: before,
        after_tyiyn: after,
        branches,
    };
    if !matched {
        sqlx::query!(
            "update parties set balance_tyiyn = $2 where id = $1",
            id,
            after
        )
        .execute(&mut *tx)
        .await?;
        for b in &out.branches {
            sqlx::query!(
                r#"insert into party_balances (party_id, branch_id, balance_tyiyn) values ($1, $2, $3)
                   on conflict (party_id, branch_id) do update set balance_tyiyn = excluded.balance_tyiyn"#,
                id,
                b.branch_id,
                b.after_tyiyn
            )
            .execute(&mut *tx)
            .await?;
        }
        let name = sqlx::query_scalar!("select name from parties where id = $1", id)
            .fetch_one(&mut *tx)
            .await?;
        let data: Value = json!({
            "name": name, "before_tyiyn": before, "after_tyiyn": after,
            "branches": out.branches,
        });
        ops::audit(&mut tx, &ctx, KIND, "party", Some(id), data).await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
