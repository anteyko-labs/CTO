//! Контрагенты, их работники и машины, долги и погашения (SPEC-10).

use axum::extract::{Path, Query, State};
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
    Router::new()
        .route("/parties", routing::get(list_parties).post(create_party))
        .route("/parties/{id}", routing::get(get_party).patch(update_party))
        .route("/parties/{id}/card", routing::get(party_card))
        .route("/parties/{id}/contacts", routing::post(create_contact))
        .route(
            "/parties/{id}/contacts/{contact_id}",
            routing::patch(update_contact),
        )
        .route("/parties/{id}/vehicles", routing::post(create_vehicle))
        .route(
            "/parties/{id}/vehicles/{vehicle_id}",
            routing::patch(update_vehicle),
        )
        .route("/parties/{id}/limit-request", routing::post(limit_request))
        .route("/debts/repayments", routing::post(post_repayment))
        .route("/debts/adjust", routing::post(post_adjust))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PartyOut {
    pub id: Uuid,
    pub role: String,
    pub kind: String,
    pub name: String,
    pub phone: String,
    pub inn: String,
    pub comment: String,
    pub credit_limit_tyiyn: Option<i64>,
    pub due_days: Option<i32>,
    pub active: bool,
    pub balance_tyiyn: i64,
}

#[derive(Serialize)]
struct ContactOut {
    id: Uuid,
    full_name: String,
    phone: String,
    position: String,
    inn: String,
    active: bool,
}

#[derive(Serialize)]
struct VehicleOut {
    id: Uuid,
    plate: String,
    brand: String,
    model: String,
    comment: String,
    active: bool,
}

#[derive(Deserialize)]
struct ListQuery {
    role: Option<String>,
    kind: Option<String>,
    q: Option<String>,
    #[serde(default)]
    only_debtors: bool,
}

async fn list_parties(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(f): Query<ListQuery>,
) -> AppResult<Json<Vec<PartyOut>>> {
    let q =
        f.q.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            });
    let rows = sqlx::query_as!(
        PartyOut,
        r#"select id, role, kind, name, phone, inn, comment,
                  credit_limit_tyiyn as "credit_limit_tyiyn?", due_days as "due_days?",
                  active, balance_tyiyn
           from parties
           where branch_id = $1
             and ($2::text is null or role = $2)
             and ($3::text is null or kind = $3)
             and ($4::text is null
                  or name ilike '%' || $4 || '%' or phone ilike '%' || $4 || '%' or inn ilike $4 || '%')
             and (not $5 or balance_tyiyn <> 0)
           order by (balance_tyiyn <> 0) desc, name
           limit 500"#,
        user.branch_id,
        f.role,
        f.kind,
        q,
        f.only_debtors
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct PartyReq {
    role: Option<String>,
    kind: Option<String>,
    name: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    inn: String,
    #[serde(default)]
    comment: String,
    credit_limit_tyiyn: Option<i64>,
    due_days: Option<i32>,
}

async fn create_party(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<PartyReq>,
) -> AppResult<Json<PartyOut>> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("укажите название или ФИО"));
    }
    let role = req.role.unwrap_or_else(|| "customer".into());
    let kind = req.kind.unwrap_or_else(|| "person".into());
    if !matches!(role.as_str(), "customer" | "supplier") {
        return Err(invalid("роль: клиент или поставщик"));
    }
    if !matches!(kind.as_str(), "person" | "company") {
        return Err(invalid("вид: физлицо или юрлицо"));
    }
    if req.credit_limit_tyiyn.is_some_and(|v| v < 0) || req.due_days.is_some_and(|v| v < 0) {
        return Err(invalid("лимит и срок оплаты не отрицательны"));
    }
    // Лимит долга и срок оплаты ставит владелец (SPEC-10).
    if (req.credit_limit_tyiyn.is_some() || req.due_days.is_some()) && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // У юрлица срок оплаты по умолчанию 14 дней, дальше правится в карточке.
    let due_days = req
        .due_days
        .or_else(|| (kind == "company" && role == "customer").then_some(14));
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into parties (id, branch_id, role, kind, name, phone, inn, comment,
                                credit_limit_tyiyn, due_days)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        ctx.user.branch_id,
        role,
        kind,
        name,
        req.phone.trim(),
        req.inn.trim(),
        req.comment.trim(),
        req.credit_limit_tyiyn,
        due_days
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.create",
        "party",
        Some(id),
        json!({ "name": name, "role": role, "kind": kind }),
    )
    .await?;
    let out = load_party(&mut tx, ctx.user.branch_id, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn load_party(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<PartyOut> {
    sqlx::query_as!(
        PartyOut,
        r#"select id, role, kind, name, phone, inn, comment,
                  credit_limit_tyiyn as "credit_limit_tyiyn?", due_days as "due_days?",
                  active, balance_tyiyn
           from parties where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

async fn get_party(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<PartyOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_party(&mut conn, user.branch_id, id).await?))
}

#[derive(Deserialize)]
struct PartyPatch {
    name: Option<String>,
    phone: Option<String>,
    inn: Option<String>,
    comment: Option<String>,
    kind: Option<String>,
    credit_limit_tyiyn: Option<i64>,
    due_days: Option<i32>,
    active: Option<bool>,
}

async fn update_party(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PartyPatch>,
) -> AppResult<Json<PartyOut>> {
    if (req.credit_limit_tyiyn.is_some() || req.due_days.is_some()) && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    let found = sqlx::query!(
        r#"update parties
           set name = coalesce(nullif(trim($3), ''), name),
               phone = coalesce($4, phone),
               inn = coalesce($5, inn),
               comment = coalesce($6, comment),
               kind = coalesce($7, kind),
               credit_limit_tyiyn = coalesce($8, credit_limit_tyiyn),
               due_days = coalesce($9, due_days),
               active = coalesce($10, active)
           where id = $1 and branch_id = $2
           returning id"#,
        id,
        ctx.user.branch_id,
        req.name,
        req.phone,
        req.inn,
        req.comment,
        req.kind,
        req.credit_limit_tyiyn,
        req.due_days,
        req.active
    )
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Err(AppError::NotFound);
    }
    ops::audit(&mut tx, &ctx, "party.update", "party", Some(id), json!({})).await?;
    let out = load_party(&mut tx, ctx.user.branch_id, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Работники и машины ----------

#[derive(Deserialize)]
struct ContactReq {
    full_name: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    position: String,
    #[serde(default)]
    inn: String,
    active: Option<bool>,
}

async fn create_contact(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ContactReq>,
) -> AppResult<Json<ContactOut>> {
    let name = req.full_name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("укажите ФИО работника"));
    }
    let mut tx = state.pool.begin().await?;
    load_party(&mut tx, ctx.user.branch_id, id).await?;
    let cid = new_id();
    sqlx::query!(
        r#"insert into party_contacts (id, branch_id, party_id, full_name, phone, position, inn)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        cid,
        ctx.user.branch_id,
        id,
        name,
        req.phone.trim(),
        req.position.trim(),
        req.inn.trim()
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.contact",
        "party",
        Some(id),
        json!({ "full_name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ContactOut {
        id: cid,
        full_name: name,
        phone: req.phone.trim().into(),
        position: req.position.trim().into(),
        inn: req.inn.trim().into(),
        active: true,
    }))
}

async fn update_contact(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, contact_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ContactReq>,
) -> AppResult<Json<ContactOut>> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as!(
        ContactOut,
        r#"update party_contacts
           set full_name = coalesce(nullif(trim($3), ''), full_name),
               phone = $4, position = $5, inn = $6,
               active = coalesce($7, active)
           where id = $1 and party_id = $2
           returning id, full_name, phone, position, inn, active"#,
        contact_id,
        id,
        req.full_name,
        req.phone.trim(),
        req.position.trim(),
        req.inn.trim(),
        req.active
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.contact_update",
        "party",
        Some(id),
        json!({ "contact_id": contact_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}

#[derive(Deserialize)]
struct VehicleReq {
    plate: String,
    #[serde(default)]
    brand: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    comment: String,
    active: Option<bool>,
}

async fn create_vehicle(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<VehicleReq>,
) -> AppResult<Json<VehicleOut>> {
    let plate = req.plate.trim().to_uppercase();
    if plate.is_empty() {
        return Err(invalid("укажите госномер"));
    }
    let mut tx = state.pool.begin().await?;
    load_party(&mut tx, ctx.user.branch_id, id).await?;
    let vid = new_id();
    sqlx::query!(
        r#"insert into party_vehicles (id, branch_id, party_id, plate, brand, model, comment)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        vid,
        ctx.user.branch_id,
        id,
        plate,
        req.brand.trim(),
        req.model.trim(),
        req.comment.trim()
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.vehicle",
        "party",
        Some(id),
        json!({ "plate": plate }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(VehicleOut {
        id: vid,
        plate,
        brand: req.brand.trim().into(),
        model: req.model.trim().into(),
        comment: req.comment.trim().into(),
        active: true,
    }))
}

async fn update_vehicle(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, vehicle_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<VehicleReq>,
) -> AppResult<Json<VehicleOut>> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as!(
        VehicleOut,
        r#"update party_vehicles
           set plate = coalesce(nullif(upper(trim($3)), ''), plate),
               brand = $4, model = $5, comment = $6,
               active = coalesce($7, active)
           where id = $1 and party_id = $2
           returning id, plate, brand, model, comment, active"#,
        vehicle_id,
        id,
        req.plate,
        req.brand.trim(),
        req.model.trim(),
        req.comment.trim(),
        req.active
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.vehicle_update",
        "party",
        Some(id),
        json!({ "vehicle_id": vehicle_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(row))
}

// ---------- Долг ----------

/// Движение по долгу контрагента.
pub struct LedgerEntry<'a> {
    pub party_id: Uuid,
    pub kind: &'a str,
    pub amount: i64,
    pub doc_type: &'a str,
    pub doc_id: Option<Uuid>,
    pub comment: &'a str,
}

/// Записывает долг или погашение и пересчитывает баланс контрагента.
/// Вызывается из кассы в той же транзакции, что и чек (инвариант 7).
pub async fn add_ledger(conn: &mut PgConnection, ctx: &Ctx, e: LedgerEntry<'_>) -> AppResult<i64> {
    let LedgerEntry {
        party_id,
        kind,
        amount,
        doc_type,
        doc_id,
        comment,
    } = e;
    let balance = sqlx::query_scalar!(
        "select balance_tyiyn from parties where id = $1 and branch_id = $2 for update",
        party_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let next = balance
        .checked_add(amount)
        .ok_or_else(|| AppError::Validation("слишком большая сумма долга".into()))?;
    sqlx::query!(
        r#"insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date,
                                     doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, (now() at time zone 'Asia/Bishkek')::date, $6, $7, $8, $9, $10)"#,
        new_id(),
        ctx.user.branch_id,
        party_id,
        kind,
        amount,
        doc_type,
        doc_id,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "update parties set balance_tyiyn = $2 where id = $1",
        party_id,
        next
    )
    .execute(&mut *conn)
    .await?;
    Ok(next)
}

/// Проверяет, что клиент может брать в долг, и что работник с машиной — его.
pub async fn check_sale_party(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Uuid,
    contact_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
) -> AppResult<()> {
    let p = sqlx::query!(
        "select role, active from parties where id = $1 and branch_id = $2",
        party_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("клиент не найден"))?;
    if !p.active {
        return Err(invalid("клиент отключён"));
    }
    if p.role != "customer" {
        return Err(invalid("это поставщик, а не клиент"));
    }
    if let Some(cid) = contact_id {
        let ok = sqlx::query_scalar!(
            "select count(*) as \"n!\" from party_contacts where id = $1 and party_id = $2",
            cid,
            party_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if ok == 0 {
            return Err(invalid("работник не из этой фирмы"));
        }
    }
    if let Some(vid) = vehicle_id {
        let ok = sqlx::query_scalar!(
            "select count(*) as \"n!\" from party_vehicles where id = $1 and party_id = $2",
            vid,
            party_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if ok == 0 {
            return Err(invalid("машина не из этой фирмы"));
        }
    }
    Ok(())
}

/// Остаток лимита: сколько ещё можно отдать в долг. `None` — лимит не задан.
pub async fn credit_limit_left(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Option<i64>> {
    let r = sqlx::query!(
        "select credit_limit_tyiyn, balance_tyiyn from parties where id = $1",
        party_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(r.credit_limit_tyiyn.map(|lim| lim - r.balance_tyiyn))
}

#[derive(Deserialize)]
struct RepaymentReq {
    op_id: Uuid,
    party_id: Uuid,
    amount_tyiyn: i64,
    #[serde(default)]
    comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LedgerResult {
    pub party_id: Uuid,
    pub balance_tyiyn: i64,
}

/// Погашение долга. Наличные попадут в кассу вместе с этапом 2 (SPEC-05);
/// пока операция меняет только баланс контрагента.
async fn post_repayment(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RepaymentReq>,
) -> AppResult<Json<LedgerResult>> {
    const KIND: &str = "debt.repayment";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма погашения больше нуля"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    // Клиент гасит свой долг, мы — свой перед поставщиком: знак разный.
    let role = sqlx::query_scalar!(
        "select role from parties where id = $1 and branch_id = $2",
        req.party_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let signed = if role == "supplier" {
        req.amount_tyiyn
    } else {
        -req.amount_tyiyn
    };
    let balance = add_ledger(
        &mut tx,
        &ctx,
        LedgerEntry {
            party_id: req.party_id,
            kind: "repayment",
            amount: signed,
            doc_type: "repayment",
            doc_id: None,
            comment: req.comment.trim(),
        },
    )
    .await?;
    crate::api::payroll::accrue_on_repayment(&mut tx, &ctx, req.party_id, req.amount_tyiyn).await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "party",
        Some(req.party_id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "balance_tyiyn": balance }),
    )
    .await?;
    let out = LedgerResult {
        party_id: req.party_id,
        balance_tyiyn: balance,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct AdjustReq {
    op_id: Uuid,
    party_id: Uuid,
    amount_tyiyn: i64,
    comment: String,
}

/// Правка баланса владельцем: прощение долга или перенос старого долга из тетради.
async fn post_adjust(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<AdjustReq>,
) -> AppResult<Json<LedgerResult>> {
    const KIND: &str = "debt.adjust";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.amount_tyiyn == 0 {
        return Err(invalid("сумма правки не ноль"));
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину правки"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let balance = add_ledger(
        &mut tx,
        &ctx,
        LedgerEntry {
            party_id: req.party_id,
            kind: "adjust",
            amount: req.amount_tyiyn,
            doc_type: "adjust",
            doc_id: None,
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "party",
        Some(req.party_id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = LedgerResult {
        party_id: req.party_id,
        balance_tyiyn: balance,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Карточка ----------

#[derive(Serialize)]
struct TimelineItem {
    at: DateTime<Utc>,
    kind: String,
    title: String,
    amount_tyiyn: i64,
    number: Option<i64>,
    doc_id: Option<Uuid>,
    comment: String,
}

#[derive(Serialize)]
struct PartyCard {
    party: PartyOut,
    contacts: Vec<ContactOut>,
    vehicles: Vec<VehicleOut>,
    purchases: i64,
    purchases_tyiyn: i64,
    debt_taken_tyiyn: i64,
    repaid_tyiyn: i64,
    last_at: Option<DateTime<Utc>>,
    timeline: Vec<TimelineItem>,
}

#[derive(Deserialize)]
struct CardQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn party_card(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(q): Query<CardQuery>,
) -> AppResult<Json<PartyCard>> {
    let mut conn = state.pool.acquire().await?;
    let party = load_party(&mut conn, user.branch_id, id).await?;
    let contacts = sqlx::query_as!(
        ContactOut,
        "select id, full_name, phone, position, inn, active from party_contacts
         where party_id = $1 order by active desc, full_name",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let vehicles = sqlx::query_as!(
        VehicleOut,
        "select id, plate, brand, model, comment, active from party_vehicles
         where party_id = $1 order by active desc, plate",
        id
    )
    .fetch_all(&mut *conn)
    .await?;

    let sales = sqlx::query!(
        r#"select s.id, s.number, s.kind, s.total_tyiyn, s.created_at, s.comment,
                  c.full_name as "contact?", v.plate as "plate?"
           from sales s
           left join party_contacts c on c.id = s.contact_id
           left join party_vehicles v on v.id = s.vehicle_id
           where s.party_id = $1 and s.branch_id = $2
             and ($3::date is null or s.created_at >= ($3::date)::timestamp at time zone 'Asia/Bishkek')
             and ($4::date is null or s.created_at < ($4::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by s.created_at desc
           limit 200"#,
        id,
        user.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let ledger = sqlx::query!(
        r#"select kind, amount_tyiyn, created_at, comment, doc_id
           from party_ledger where party_id = $1 and branch_id = $2
             and ($3::date is null or created_at >= ($3::date)::timestamp at time zone 'Asia/Bishkek')
             and ($4::date is null or created_at < ($4::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by created_at desc limit 200"#,
        id,
        user.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;

    // Что именно брали: состав чеков короткой строкой.
    let sale_ids: Vec<Uuid> = sales.iter().map(|s| s.id).collect();
    let line_rows = sqlx::query!(
        r#"select l.sale_id as "sale_id!", coalesce(p.name, sv.name) as "name!", l.qty as "qty!",
                  l.kind as "kind!", p.container_ml as "container_ml?"
           from sale_lines l
           left join products p on p.id = l.product_id
           left join services sv on sv.id = l.service_id
           where l.sale_id = any($1)
           order by l.line_no"#,
        &sale_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut goods: std::collections::HashMap<Uuid, Vec<String>> = std::collections::HashMap::new();
    for r in line_rows {
        let qty = if r.kind == "pour" {
            format!("{} мл", r.qty)
        } else {
            format!("{} шт", r.qty)
        };
        goods
            .entry(r.sale_id)
            .or_default()
            .push(format!("{} × {}", r.name, qty));
    }

    let purchases = sales.iter().filter(|s| s.kind == "sale").count() as i64;
    let purchases_tyiyn = sales.iter().map(|s| s.total_tyiyn).sum();
    let debt_taken_tyiyn = ledger
        .iter()
        .filter(|l| l.kind == "debt" && l.amount_tyiyn > 0)
        .map(|l| l.amount_tyiyn)
        .sum();
    let repaid_tyiyn = ledger
        .iter()
        .filter(|l| l.kind == "repayment")
        .map(|l| -l.amount_tyiyn)
        .sum();

    let mut timeline: Vec<TimelineItem> = Vec::with_capacity(sales.len() + ledger.len());
    for s in &sales {
        let who = match (&s.contact, &s.plate) {
            (Some(c), Some(p)) => format!("{c}, {p}"),
            (Some(c), None) => c.clone(),
            (None, Some(p)) => p.clone(),
            (None, None) => String::new(),
        };
        timeline.push(TimelineItem {
            at: s.created_at,
            kind: if s.kind == "return" {
                "sale_return".into()
            } else {
                "sale".into()
            },
            title: if s.kind == "return" {
                "Возврат".into()
            } else {
                "Покупка".into()
            },
            amount_tyiyn: s.total_tyiyn,
            number: Some(s.number),
            doc_id: Some(s.id),
            comment: {
                let what = goods.get(&s.id).map(|g| g.join(", ")).unwrap_or_default();
                [what, who, s.comment.clone()]
                    .into_iter()
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
            },
        });
    }
    for l in &ledger {
        timeline.push(TimelineItem {
            at: l.created_at,
            kind: l.kind.clone(),
            title: match l.kind.as_str() {
                "debt" if l.amount_tyiyn > 0 => "Взял в долг".into(),
                "debt" => "Долг уменьшен возвратом".into(),
                "repayment" => "Погашение".into(),
                _ => "Правка баланса".into(),
            },
            amount_tyiyn: l.amount_tyiyn,
            number: None,
            doc_id: l.doc_id,
            comment: l.comment.clone(),
        });
    }
    timeline.sort_by_key(|t| std::cmp::Reverse(t.at));
    let last_at = timeline.first().map(|t| t.at);

    Ok(Json(PartyCard {
        party,
        contacts,
        vehicles,
        purchases,
        purchases_tyiyn,
        debt_taken_tyiyn,
        repaid_tyiyn,
        last_at,
        timeline,
    }))
}

#[derive(Deserialize)]
struct LimitRequestReq {
    amount_tyiyn: i64,
    #[serde(default)]
    comment: String,
}

/// Долг упёрся в лимит: касса просит владельца поднять его (SPEC-10).
/// Сам лимит меняет только владелец, в карточке клиента.
async fn limit_request(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LimitRequestReq>,
) -> AppResult<Json<serde_json::Value>> {
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма повышения больше нуля"));
    }
    let mut tx = state.pool.begin().await?;
    let party = load_party(&mut tx, ctx.user.branch_id, id).await?;
    ops::audit(
        &mut tx,
        &ctx,
        "party.limit_request",
        "party",
        Some(id),
        json!({
            "name": party.name,
            "amount_tyiyn": req.amount_tyiyn,
            "limit_tyiyn": party.credit_limit_tyiyn,
            "balance_tyiyn": party.balance_tyiyn,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
