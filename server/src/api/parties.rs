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
        .route("/debts/ledger/{id}/reverse", routing::post(reverse_ledger))
        .route("/parties/{id}/reconciliation", routing::get(reconciliation))
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
        .route(
            "/parties/{id}/vehicles/{vehicle_id}/restore",
            routing::post(restore_vehicle),
        )
        .route("/parties/{id}/limit-request", routing::post(limit_request))
        .route("/debts/repayments", routing::post(post_repayment))
        .route("/debts/adjust", routing::post(post_adjust))
        .route("/debts/list", routing::get(debts_list))
        .route("/parties/{id}/open-debts", routing::get(open_debts))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PartyOut {
    pub id: Uuid,
    pub role: String,
    pub kind: String,
    pub name: String,
    /// Имя и фамилия клиента-человека (ADR-083); у прежних записей — пусто, полное имя в `name`.
    #[serde(default)]
    pub first_name: String,
    #[serde(default)]
    pub last_name: String,
    pub phone: String,
    pub inn: String,
    pub comment: String,
    pub credit_limit_tyiyn: Option<i64>,
    pub due_days: Option<i32>,
    pub active: bool,
    pub balance_tyiyn: i64,
    /// Просрочено: долг старше срока оплаты, ещё не погашенный. Старый долг гасится первым,
    /// поэтому просрочка — то, что не покрыто долгами за последние `due_days` дней.
    #[serde(default)]
    pub overdue_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct ContactOut {
    pub(crate) id: Uuid,
    pub(crate) full_name: String,
    pub(crate) phone: String,
    pub(crate) position: String,
    pub(crate) inn: String,
    pub(crate) active: bool,
}

#[derive(Serialize)]
pub(crate) struct VehicleOut {
    pub(crate) id: Uuid,
    pub(crate) plate: String,
    pub(crate) brand: String,
    pub(crate) model: String,
    pub(crate) comment: String,
    pub(crate) active: bool,
    /// Единица пробега машины: `km` или `mi` (ADR-084).
    pub(crate) mileage_unit: String,
}

#[derive(Deserialize, Default)]
pub(crate) struct ListQuery {
    pub(crate) role: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) q: Option<String>,
    #[serde(default)]
    pub(crate) only_debtors: bool,
}

async fn list_parties(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(f): Query<ListQuery>,
    Query(page): Query<crate::api::paging::PageQuery>,
) -> AppResult<crate::api::paging::Paged<PartyOut>> {
    let mut conn = state.pool.acquire().await?;
    let (items, total) = page.slice(search_parties(&mut conn, user.branch_id, f).await?, 500);
    Ok(crate::api::paging::paged(items, total))
}

/// Поиск контрагентов филиала с долгом и просрочкой — для экрана и для вопросов боту (SPEC-21).
pub(crate) async fn search_parties(
    conn: &mut PgConnection,
    branch_id: Uuid,
    f: ListQuery,
) -> AppResult<Vec<PartyOut>> {
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
        r#"select id, role, kind, name, first_name, last_name, phone, inn, comment,
                  credit_limit_tyiyn as "credit_limit_tyiyn?", due_days as "due_days?",
                  active, balance_tyiyn,
                  case when parties.balance_tyiyn > 0 and parties.due_days is not null then
                    greatest(0, parties.balance_tyiyn - coalesce((
                      select sum(l.amount_tyiyn) from party_ledger l
                      where l.party_id = parties.id and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
                        and l.business_date > (now() at time zone 'Asia/Bishkek')::date - parties.due_days), 0))
                  else 0 end::bigint as "overdue_tyiyn!"
           from parties
           where branch_id = $1
             and ($2::text is null or role = $2)
             and ($3::text is null or kind = $3)
             and ($4::text is null
                  or name ilike '%' || $4 || '%' or phone ilike '%' || $4 || '%' or inn ilike $4 || '%'
                  -- «Имя Фамилия» тоже находит «Фамилия Имя» (ADR-083).
                  or (first_name <> '' and (first_name || ' ' || last_name) ilike '%' || $4 || '%')
                  -- Госномер машины: на замене кассир видит машину, а не паспорт (SPEC-16).
                  or exists (select 1 from party_vehicles v
                             where v.party_id = parties.id and v.active
                               and upper(replace(v.plate, ' ', '')) like '%' || upper(replace($4, ' ', '')) || '%'))
             and (not $5 or balance_tyiyn <> 0)
           order by (balance_tyiyn <> 0) desc, name
           limit $6"#,
        branch_id,
        f.role,
        f.kind,
        q,
        f.only_debtors,
        crate::api::paging::MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

#[derive(Deserialize)]
struct PartyReq {
    role: Option<String>,
    kind: Option<String>,
    /// Название фирмы или полное имя; у человека с `first_name`/`last_name` — собирается из них.
    #[serde(default)]
    name: String,
    /// Клиент-человек: имя и фамилия отдельно, обе обязательны (ADR-083).
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
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
    let role = req.role.unwrap_or_else(|| "customer".into());
    let kind = req.kind.unwrap_or_else(|| "person".into());
    let person = person_name(&kind, req.first_name.as_deref(), req.last_name.as_deref())?;
    let name = match &person {
        Some((_, _, full)) => full.clone(),
        None => req.name.trim().to_string(),
    };
    if name.is_empty() {
        return Err(invalid("укажите название или ФИО"));
    }
    let (first_name, last_name) = person.map(|(f, l, _)| (f, l)).unwrap_or_default();
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
                                credit_limit_tyiyn, due_days, first_name, last_name)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        id,
        ctx.user.branch_id,
        role,
        kind,
        name,
        req.phone.trim(),
        req.inn.trim(),
        req.comment.trim(),
        req.credit_limit_tyiyn,
        due_days,
        first_name,
        last_name
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

/// Полное имя человека — «Фамилия Имя» (ADR-083): так его ищут и видят в списках, чеках и
/// документах о долге. Имя и фамилия обязательны, если прислано хоть одно из них; у фирмы их нет.
pub(crate) fn person_name(
    kind: &str,
    first: Option<&str>,
    last: Option<&str>,
) -> AppResult<Option<(String, String, String)>> {
    if first.is_none() && last.is_none() {
        return Ok(None);
    }
    if kind != "person" {
        return Err(invalid("имя и фамилия — только у физлица"));
    }
    let f = first
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let l = last
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if f.is_empty() || l.is_empty() {
        return Err(invalid("укажите имя и фамилию клиента"));
    }
    if f.chars().count() > 60 || l.chars().count() > 60 {
        return Err(invalid("имя и фамилия — не длиннее 60 знаков"));
    }
    let full = format!("{l} {f}");
    Ok(Some((f, l, full)))
}

pub(crate) async fn load_party(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<PartyOut> {
    sqlx::query_as!(
        PartyOut,
        r#"select id, role, kind, name, first_name, last_name, phone, inn, comment,
                  credit_limit_tyiyn as "credit_limit_tyiyn?", due_days as "due_days?",
                  active, balance_tyiyn,
                  case when parties.balance_tyiyn > 0 and parties.due_days is not null then
                    greatest(0, parties.balance_tyiyn - coalesce((
                      select sum(l.amount_tyiyn) from party_ledger l
                      where l.party_id = parties.id and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
                        and l.business_date > (now() at time zone 'Asia/Bishkek')::date - parties.due_days), 0))
                  else 0 end::bigint as "overdue_tyiyn!"
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

/// Поля карточки контрагента, которые правятся запросом, — для журнала «было/стало».
async fn party_fields(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<serde_json::Value> {
    sqlx::query_scalar!(
        r#"select jsonb_build_object('name', name, 'first_name', first_name, 'last_name', last_name,
                                     'phone', phone, 'inn', inn, 'comment', comment,
                                     'kind', kind, 'credit_limit_tyiyn', credit_limit_tyiyn,
                                     'due_days', due_days, 'active', active) as "v!"
           from parties where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct PartyPatch {
    name: Option<String>,
    /// Имя и фамилия человека (ADR-083): присылаются вместе, `name` собирается из них.
    first_name: Option<String>,
    last_name: Option<String>,
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
    // Проверка здесь, а не ограничением базы: иначе пользователь увидит «внутреннюю ошибку».
    if req.credit_limit_tyiyn.is_some_and(|v| v < 0) || req.due_days.is_some_and(|v| v < 0) {
        return Err(invalid("лимит и срок не могут быть меньше нуля"));
    }
    if req
        .kind
        .as_deref()
        .is_some_and(|k| !matches!(k, "person" | "company"))
    {
        return Err(invalid("вид клиента: физлицо или юрлицо"));
    }
    let mut tx = state.pool.begin().await?;
    let before = party_fields(&mut tx, ctx.user.branch_id, id).await?;
    let kind_now = req
        .kind
        .clone()
        .or_else(|| {
            before
                .get("kind")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let person = person_name(
        &kind_now,
        req.first_name.as_deref(),
        req.last_name.as_deref(),
    )?;
    // Стал фирмой — имя и фамилия человека не нужны.
    let clear_person = kind_now == "company";
    let name = person.as_ref().map(|p| p.2.clone()).or(req.name);
    let found = sqlx::query!(
        r#"update parties
           set name = coalesce(nullif(trim($3), ''), name),
               phone = coalesce($4, phone),
               inn = coalesce($5, inn),
               comment = coalesce($6, comment),
               kind = coalesce($7, kind),
               credit_limit_tyiyn = coalesce($8, credit_limit_tyiyn),
               due_days = coalesce($9, due_days),
               active = coalesce($10, active),
               first_name = case when $13 then '' else coalesce($11, first_name) end,
               last_name = case when $13 then '' else coalesce($12, last_name) end
           where id = $1 and branch_id = $2
           returning id"#,
        id,
        ctx.user.branch_id,
        name,
        req.phone,
        req.inn,
        req.comment,
        req.kind,
        req.credit_limit_tyiyn,
        req.due_days,
        req.active,
        person.as_ref().map(|p| p.0.clone()),
        person.as_ref().map(|p| p.1.clone()),
        clear_person
    )
    .fetch_optional(&mut *tx)
    .await?;
    if found.is_none() {
        return Err(AppError::NotFound);
    }
    let after = party_fields(&mut tx, ctx.user.branch_id, id).await?;
    let changes = ops::changes(&before, &after);
    ops::audit(
        &mut tx,
        &ctx,
        "party.update",
        "party",
        Some(id),
        json!({ "name": after.get("name"), "changes": changes }),
    )
    .await?;
    // Лимит долга и срок оплаты — деньги, которые точка отдаёт в долг: владелец видит правку
    // отдельным уведомлением, остальные правки карточки — только в журнале (ADR-062).
    let limit_changed = ["credit_limit_tyiyn", "due_days"]
        .iter()
        .any(|k| changes.get(*k).is_some());
    if limit_changed {
        ops::audit(
            &mut tx,
            &ctx,
            "party.limit_change",
            "party",
            Some(id),
            json!({
                "name": after.get("name"),
                "old_limit": before.get("credit_limit_tyiyn"),
                "new_limit": after.get("credit_limit_tyiyn"),
                "old_days": before.get("due_days"),
                "new_days": after.get("due_days"),
            }),
        )
        .await?;
    }
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
    let before = sqlx::query!(
        r#"select c.full_name, c.phone, c.position, c.inn, c.active from party_contacts c
           join parties p on p.id = c.party_id
           where c.id = $1 and c.party_id = $2 and p.branch_id = $3
           for update of c"#,
        contact_id,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let row = sqlx::query_as!(
        ContactOut,
        r#"update party_contacts
           set full_name = coalesce(nullif(trim($3), ''), full_name),
               phone = $4, position = $5, inn = $6,
               active = coalesce($7, active)
           where id = $1 and party_id = $2
             and party_id in (select p.id from parties p where p.branch_id = $8)
           returning id, full_name, phone, position, inn, active"#,
        contact_id,
        id,
        req.full_name,
        req.phone.trim(),
        req.position.trim(),
        req.inn.trim(),
        req.active,
        ctx.user.branch_id
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
        json!({
            "contact_id": contact_id,
            "changes": ops::changes(
                &json!({ "full_name": before.full_name, "phone": before.phone, "position": before.position,
                         "inn": before.inn, "active": before.active }),
                &json!({ "full_name": row.full_name, "phone": row.phone, "position": row.position,
                         "inn": row.inn, "active": row.active }),
            ),
        }),
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
    /// Пробег в км или милях (ADR-084); без него — км у новой, прежняя у правки.
    #[serde(default)]
    mileage_unit: Option<String>,
}

fn check_unit(unit: Option<&str>) -> AppResult<()> {
    if unit.is_some_and(|u| !crate::domain::mileage::is_unit(u)) {
        return Err(invalid("пробег — в км или милях"));
    }
    Ok(())
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
    check_unit(req.mileage_unit.as_deref())?;
    // Номер в одном виде (SPEC-22): «01 kg-123 АВС» и «01KG123ABC» — одна машина.
    let norm = crate::domain::plate::normalize(&plate);
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    // Марка и модель — из справочника; новую справочник запоминает (ADR-083).
    let (brand, model) =
        crate::api::cars::remember(&mut tx, &ctx.user, &req.brand, &req.model).await?;
    let unit = req.mileage_unit.clone().unwrap_or_else(|| "km".into());
    // Блокировка клиента: два одновременных добавления одного номера не заведут две машины.
    let party_name = sqlx::query_scalar!(
        "select name from parties where id = $1 and branch_id = $2 for update",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Этот номер у клиента уже есть — возвращаем его машину с историей, а не заводим дубль.
    // Убранную (клиентом в боте или точкой) — возвращаем в список (ADR-062).
    let own = sqlx::query_as!(
        VehicleOut,
        r#"select id, plate, brand, model, comment, active, mileage_unit from party_vehicles
           where party_id = $1 and branch_id = $2 and plate_norm = $3
           order by active desc, created_at limit 1"#,
        id,
        branch_id,
        norm
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(mut v) = own {
        if !v.active {
            restore_vehicle_tx(&mut tx, &ctx, id, v.id, "add").await?;
            v.active = true;
        }
        tx.commit().await?;
        return Ok(Json(v));
    }
    let vid = new_id();
    sqlx::query!(
        r#"insert into party_vehicles (id, branch_id, party_id, plate, brand, model, comment, plate_norm,
                                       mileage_unit)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        vid,
        branch_id,
        id,
        plate,
        brand,
        model,
        req.comment.trim(),
        norm,
        unit
    )
    .execute(&mut *tx)
    .await?;
    // Номер записан на другого клиента точки: новая машина без чужой истории, разбирается
    // владелец — так же, как при добавлении в боте (ADR-061).
    let others = sqlx::query_scalar!(
        r#"select distinct p.name from party_vehicles v join parties p on p.id = v.party_id
           where v.branch_id = $1 and v.plate_norm = $2 and v.party_id <> $3
           order by p.name"#,
        branch_id,
        norm,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    if !others.is_empty() {
        ops::audit(
            &mut tx,
            &ctx,
            "vehicle.plate_conflict",
            "vehicle",
            Some(vid),
            json!({
                "plate": norm,
                "client": party_name,
                "other": others.join(", "),
                "party_id": id,
                "source": "staff",
            }),
        )
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "party.vehicle",
        "party",
        Some(id),
        json!({ "plate": plate, "vehicle_id": vid }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(VehicleOut {
        id: vid,
        plate,
        brand,
        model,
        comment: req.comment.trim().into(),
        active: true,
        mileage_unit: unit,
    }))
}

/// Вернуть машину, которую убрали (клиент в боте или точка): история замен и чеки при ней
/// остались, машина снова в списке клиента и в кассе (ADR-062). `via` — откуда вернули.
pub(crate) async fn restore_vehicle_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    vehicle_id: Uuid,
    via: &str,
) -> AppResult<()> {
    let plate = sqlx::query_scalar!(
        r#"update party_vehicles set active = true
           where id = $1 and party_id = $2 and branch_id = $3 and not active
           returning plate"#,
        vehicle_id,
        party_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(plate) = plate {
        ops::audit(
            conn,
            ctx,
            "vehicle.restore",
            "vehicle",
            Some(vehicle_id),
            json!({ "plate": plate, "party_id": party_id, "via": via }),
        )
        .await?;
    }
    Ok(())
}

/// «Вернуть» в карточке клиента: машину, убранную клиентом, видит и возвращает точка.
async fn restore_vehicle(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, vehicle_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<VehicleOut>> {
    let mut tx = state.pool.begin().await?;
    restore_vehicle_tx(&mut tx, &ctx, id, vehicle_id, "card").await?;
    let row = sqlx::query_as!(
        VehicleOut,
        r#"select id, plate, brand, model, comment, active, mileage_unit from party_vehicles
           where id = $1 and party_id = $2 and branch_id = $3"#,
        vehicle_id,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(row))
}

async fn update_vehicle(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, vehicle_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<VehicleReq>,
) -> AppResult<Json<VehicleOut>> {
    // Номер в одном виде (SPEC-22) — по тому же правилу, что в боте.
    let plate_norm = Some(req.plate.trim())
        .filter(|p| !p.is_empty())
        .map(crate::domain::plate::normalize);
    check_unit(req.mileage_unit.as_deref())?;
    let mut tx = state.pool.begin().await?;
    let (brand, model) =
        crate::api::cars::remember(&mut tx, &ctx.user, &req.brand, &req.model).await?;
    let before = sqlx::query!(
        r#"select v.plate, v.brand, v.model, v.comment, v.active, v.mileage_unit from party_vehicles v
           where v.id = $1 and v.party_id = $2 and v.branch_id = $3
           for update"#,
        vehicle_id,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let row = sqlx::query_as!(
        VehicleOut,
        r#"update party_vehicles
           set plate = coalesce(nullif(upper(trim($3)), ''), plate),
               plate_norm = coalesce($9, plate_norm),
               brand = $4, model = $5, comment = $6,
               active = coalesce($7, active),
               mileage_unit = coalesce($10, mileage_unit)
           where id = $1 and party_id = $2
             and party_id in (select p.id from parties p where p.branch_id = $8)
           returning id, plate, brand, model, comment, active, mileage_unit"#,
        vehicle_id,
        id,
        req.plate,
        brand,
        model,
        req.comment.trim(),
        req.active,
        ctx.user.branch_id,
        plate_norm,
        req.mileage_unit
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
        json!({
            "vehicle_id": vehicle_id,
            "plate": row.plate,
            "changes": ops::changes(
                &json!({ "plate": before.plate, "brand": before.brand, "model": before.model,
                         "comment": before.comment, "active": before.active,
                         "mileage_unit": before.mileage_unit }),
                &json!({ "plate": row.plate, "brand": row.brand, "model": row.model,
                         "comment": row.comment, "active": row.active,
                         "mileage_unit": row.mileage_unit }),
            ),
        }),
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
    add_ledger_to(conn, ctx, e, None).await.map(|(b, _)| b)
}

/// То же, что [`add_ledger`], и возвращает id записи. У поставщика запись сразу распределяется
/// (ADR-073): оплата с `target` сначала ложится на эту запись долга (накладную), остальное —
/// на самые старые долги.
pub async fn add_ledger_to(
    conn: &mut PgConnection,
    ctx: &Ctx,
    e: LedgerEntry<'_>,
    target: Option<(Uuid, i64)>,
) -> AppResult<(i64, Uuid)> {
    let LedgerEntry {
        party_id,
        kind,
        amount,
        doc_type,
        doc_id,
        comment,
    } = e;
    ops::check_amount(amount)?;
    let party = sqlx::query!(
        "select balance_tyiyn, role from parties where id = $1 and branch_id = $2 for update",
        party_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let next = party
        .balance_tyiyn
        .checked_add(amount)
        .ok_or_else(|| AppError::Validation("слишком большая сумма долга".into()))?;
    let id = new_id();
    sqlx::query!(
        r#"insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date,
                                     doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5,
                   -- Долг по чеку — в учётный день чека: офлайн-чек вчерашнего дня в акте сверки вчера.
                   coalesce((select s.business_date from sales s
                             where s.id = $7 and $6 in ('sale', 'sale_return')),
                            (now() at time zone 'Asia/Bishkek')::date),
                   $6, $7, $8, $9, $10)"#,
        id,
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
    if party.role == "supplier" {
        if let Some((debit_id, amount)) = target {
            crate::api::supplier_debts::allocate_to(conn, ctx, party_id, id, debit_id, amount)
                .await?;
        }
        crate::api::supplier_debts::allocate(conn, party_id).await?;
    }
    Ok((next, id))
}

/// Проверяет, что клиент может брать в долг, и что работник с машиной — его.
/// Чек без сети уже состоялся: отключённого клиента и убранную машину он не отклоняет —
/// машина возвращается в список, а причины уходят владельцу (ADR-062). Возвращает эти причины.
pub async fn check_sale_party(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    contact_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
    offline: bool,
    on_debt: bool,
) -> AppResult<Vec<String>> {
    let branch_id = ctx.user.branch_id;
    let mut bypass = Vec::new();
    let p = sqlx::query!(
        "select role, active, name from parties where id = $1 and branch_id = $2",
        party_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("клиент не найден"))?;
    if !p.active {
        if !offline {
            return Err(invalid("клиент отключён"));
        }
        bypass.push(if on_debt {
            format!("в долг отключённому клиенту «{}»", p.name)
        } else {
            format!("клиент «{}» отключён", p.name)
        });
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
        let v = sqlx::query!(
            "select plate, active from party_vehicles where id = $1 and party_id = $2",
            vid,
            party_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("машина не из этой фирмы"))?;
        if !v.active {
            // Клиент убрал машину в боте (или её убрали в карточке): замена на ней — повод
            // вернуть её осознанно, а не молча (ADR-062).
            if !offline {
                return Err(AppError::Conflict(format!(
                    "машина {} убрана клиентом — верните её в карточке клиента",
                    v.plate
                )));
            }
            restore_vehicle_tx(conn, ctx, party_id, vid, "offline_sale").await?;
            bypass.push(format!(
                "машина {} была убрана клиентом — возвращена",
                v.plate
            ));
        }
    }
    Ok(bypass)
}

/// Остаток лимита: сколько ещё можно отдать в долг. `None` — лимит не задан.
pub async fn credit_limit_left(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Option<i64>> {
    let r = sqlx::query!(
        "select credit_limit_tyiyn, balance_tyiyn from parties where id = $1",
        party_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(r.credit_limit_tyiyn
        .map(|lim| lim.saturating_sub(r.balance_tyiyn)))
}

#[derive(Deserialize)]
struct ReconQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Serialize)]
pub(crate) struct ReconRow {
    pub(crate) date: NaiveDate,
    pub(crate) document: String,
    /// Документ-основание: по нему касса находит свой чек в истории долга.
    pub(crate) doc_type: String,
    pub(crate) doc_id: Option<Uuid>,
    /// В пользу точки: отгрузили клиенту / оплатили поставщику.
    pub(crate) debit_tyiyn: i64,
    /// В пользу контрагента: клиент оплатил / поставщик привёз.
    pub(crate) credit_tyiyn: i64,
    pub(crate) comment: String,
}

#[derive(Serialize)]
pub(crate) struct ReconOut {
    pub(crate) party_id: Uuid,
    pub(crate) name: String,
    pub(crate) kind: String,
    pub(crate) role: String,
    pub(crate) inn: String,
    pub(crate) phone: String,
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
    /// Сальдо в пользу точки: больше нуля — контрагент должен нам, меньше — мы ему.
    pub(crate) opening_tyiyn: i64,
    pub(crate) rows: Vec<ReconRow>,
    pub(crate) debit_total_tyiyn: i64,
    pub(crate) credit_total_tyiyn: i64,
    pub(crate) closing_tyiyn: i64,
}

/// Акт сверки: все движения долга контрагента за период с номерами документов.
/// По умолчанию — с первой операции по сегодня.
async fn reconciliation(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(q): Query<ReconQuery>,
) -> AppResult<Json<ReconOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(
        reconciliation_data(&mut conn, user.branch_id, id, q.from, q.to).await?,
    ))
}

/// Данные акта сверки: тот же акт видят владелец и юрлицо в своём кабинете (SPEC-10, SPEC-12).
pub(crate) async fn reconciliation_data(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<ReconOut> {
    let p = sqlx::query!(
        "select name, kind, role, inn, phone from parties where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let bounds = sqlx::query!(
        r#"select coalesce(min(business_date), (now() at time zone 'Asia/Bishkek')::date) as "first!",
                  (now() at time zone 'Asia/Bishkek')::date as "today!"
           from party_ledger where party_id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let from = from.unwrap_or(bounds.first);
    let to = to.unwrap_or(bounds.today);
    if from > to {
        return Err(invalid("начало периода позже конца"));
    }
    let opening = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from party_ledger
           where party_id = $1 and business_date < $2"#,
        id,
        from
    )
    .fetch_one(&mut *conn)
    .await?;
    let raw = sqlx::query!(
        r#"select l.business_date, l.kind, l.amount_tyiyn, l.doc_type, l.doc_id, l.comment,
                  s.number as "sale_number?", r.number as "receipt_number?",
                  r.supplier_doc as "supplier_doc?",
                  (select a.kind from cash_movements m join cash_accounts a on a.id = m.account_id
                    where m.doc_type = 'repayment' and m.doc_id = l.doc_id
                      and m.kind in ('debt_repayment', 'supplier_payment') limit 1) as "paid_via?"
           from party_ledger l
           left join sales s on s.id = l.doc_id and l.doc_type in ('sale', 'sale_return')
           left join receipts r on r.id = l.doc_id and l.doc_type in ('receipt', 'receipt_reversal')
           where l.party_id = $1 and l.business_date between $2 and $3
           order by l.business_date, l.created_at"#,
        id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut rows = Vec::with_capacity(raw.len());
    let (mut debit, mut credit) = (0i64, 0i64);
    for r in raw {
        let num = |n: Option<i64>| n.map(|n| format!(" № {n}")).unwrap_or_default();
        let document = match (r.doc_type.as_str(), r.kind.as_str()) {
            ("sale", _) => format!("Отгрузка товара, чек{}", num(r.sale_number)),
            ("sale_return", _) => format!("Возврат товара, чек{}", num(r.sale_number)),
            ("receipt", _) => format!(
                "Поступление товара, накладная{}{}",
                num(r.receipt_number),
                r.supplier_doc
                    .as_deref()
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" (док. поставщика {d})"))
                    .unwrap_or_default()
            ),
            ("receipt_reversal", _) => format!("Сторно накладной{}", num(r.receipt_number)),
            // Возврат брака со скидкой на следующий приход (SPEC-25).
            ("defect", _) => "Возврат брака поставщику".to_string(),
            ("defect_reversal", _) => "Отмена возврата брака".to_string(),
            (_, "repayment") => match r.paid_via.as_deref() {
                Some("register") => "Оплата наличными".to_string(),
                Some("bank") => "Оплата безналичная".to_string(),
                _ => "Оплата".to_string(),
            },
            (_, "adjust") => "Корректировка долга".to_string(),
            _ => "Операция".to_string(),
        };
        let (d, c) = if r.amount_tyiyn >= 0 {
            (r.amount_tyiyn, 0)
        } else {
            (0, r.amount_tyiyn.saturating_neg())
        };
        debit = debit.saturating_add(d);
        credit = credit.saturating_add(c);
        rows.push(ReconRow {
            date: r.business_date,
            document,
            doc_type: r.doc_type,
            doc_id: r.doc_id,
            debit_tyiyn: d,
            credit_tyiyn: c,
            comment: r.comment,
        });
    }
    Ok(ReconOut {
        party_id: id,
        name: p.name,
        kind: p.kind,
        role: p.role,
        inn: p.inn,
        phone: p.phone,
        from,
        to,
        opening_tyiyn: opening,
        rows,
        debit_total_tyiyn: debit,
        credit_total_tyiyn: credit,
        closing_tyiyn: opening.saturating_add(debit).saturating_sub(credit),
    })
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct RepaymentReq {
    pub op_id: Uuid,
    pub party_id: Uuid,
    pub amount_tyiyn: i64,
    /// Чем платят: `cash` — касса смены, `card` / `transfer` — счёт. По умолчанию наличные.
    /// `outside` (мимо денег точки) больше не принимается — 422 (ADR-079).
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub comment: String,
    /// Оплата одной накладной поставщика (SPEC-27): сумма не больше её остатка. Без накладной
    /// оплата закрывает самые старые долги (ADR-073).
    #[serde(default)]
    pub receipt_id: Option<Uuid>,
    /// Касса или счёт (ADR-076): наличные — касса (администратору — только касса смены), карта
    /// и перевод — безналичный счёт. Без него — касса смены или безналичный счёт по умолчанию
    /// для способа (ADR-077).
    #[serde(default)]
    pub account_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct LedgerResult {
    pub party_id: Uuid,
    pub balance_tyiyn: i64,
    /// Проведённая запись долга: погашение, правка или сторно (SPEC-29 §24).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger_id: Option<Uuid>,
}

/// Погашение долга: баланс контрагента и движение денег — в одной транзакции (SPEC-10, SPEC-05).
/// Выручка и валовая прибыль по долговому чеку уже учтены в день продажи (SPEC-08), поэтому
/// погашение — это поступление денег в кассу, а не новая прибыль.
async fn post_repayment(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RepaymentReq>,
) -> AppResult<Json<LedgerResult>> {
    let mut tx = state.pool.begin().await?;
    let out = post_repayment_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Погашение долга клиентом или оплата поставщику.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
/// Касса погашения по способу оплаты: наличные — касса по умолчанию, карта и перевод — счёт по
/// умолчанию для способа (ADR-077),
/// мимо касс — никакой (`None`). Одна функция и для проведения, и для черновика бота (ADR-070).
pub(crate) async fn repayment_account(
    conn: &mut PgConnection,
    user: &crate::auth::CurrentUser,
    method: &str,
    account: Option<Uuid>,
) -> AppResult<Option<Uuid>> {
    let branch_id = user.branch_id;
    match (method, account) {
        ("cash", None) => Ok(Some(
            crate::api::cash::default_account(&mut *conn, branch_id).await?,
        )),
        // Наличные в выбранную кассу: владелец — в любую наличную, администратор — только в
        // кассу смены (ADR-076).
        ("cash", Some(id)) => {
            let kind = crate::api::cash::check_pay_account(&mut *conn, user, id).await?;
            if kind == "bank" {
                return Err(invalid(
                    "наличные — в кассу или сейф, не на безналичный счёт",
                ));
            }
            Ok(Some(id))
        }
        ("card" | "transfer", None) => {
            crate::api::cash::bank_account(&mut *conn, branch_id, method)
                .await?
                .ok_or_else(|| invalid("нет безналичного счёта"))
                .map(Some)
        }
        ("card" | "transfer", Some(id)) => {
            let mut ignored = Vec::new();
            crate::api::cash::sale_bank_account(
                &mut *conn,
                user,
                method,
                Some(id),
                false,
                &mut ignored,
            )
            .await
            .map(Some)
        }
        _ => Ok(None),
    }
}

/// Комиссия банка с погашения клиентом по карте или переводу — по ставке счёта, куда пришли
/// деньги, действующей в день погашения, как с чека (ADR-022, SPEC-08, ADR-077). Оплата
/// поставщику и наличные — без комиссии.
pub(crate) async fn repayment_fee(
    conn: &mut PgConnection,
    account_id: Uuid,
    method: &str,
    supplier: bool,
    amount: i64,
) -> AppResult<i64> {
    if supplier || !matches!(method, "card" | "transfer") {
        return Ok(0);
    }
    let today = crate::api::fees::today(&mut *conn).await?;
    crate::api::fees::fee_on(&mut *conn, account_id, today, amount).await
}

pub async fn post_repayment_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: RepaymentReq,
) -> AppResult<LedgerResult> {
    const KIND: &str = "debt.repayment";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма погашения больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    let method = req.method.clone().unwrap_or_else(|| "cash".into());
    if method == "outside" {
        return Err(invalid(crate::api::receipts::NO_OUTSIDE));
    }
    if !matches!(method.as_str(), "cash" | "card" | "transfer") {
        return Err(invalid("чем платят: наличные, карта или перевод"));
    }
    let branch_id = ctx.user.branch_id;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    // Клиент гасит свой долг, мы — свой перед поставщиком: знак разный.
    let party = sqlx::query!(
        "select role, name from parties where id = $1 and branch_id = $2",
        req.party_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let supplier = party.role == "supplier";
    // Со счёта поставщику платит владелец (SPEC-10, права).
    if !ctx.user.is_owner() && supplier && method != "cash" {
        return Err(AppError::Forbidden);
    }
    // Оплата накладной: остаток считается под блокировкой поставщика, переплатить её нельзя.
    let target = match req.receipt_id {
        None => None,
        Some(_) if !supplier => {
            return Err(invalid("накладную оплачивают только поставщику"));
        }
        Some(receipt_id) => {
            sqlx::query!(
                "select id from parties where id = $1 for update",
                req.party_id
            )
            .fetch_one(&mut *conn)
            .await?;
            let inv = crate::api::supplier_debts::invoice_open(
                &mut *conn,
                branch_id,
                req.party_id,
                receipt_id,
            )
            .await?;
            if req.amount_tyiyn > inv.open_tyiyn {
                return Err(invalid(format!(
                    "по накладной № {} осталось {} — больше заплатить нельзя",
                    inv.number,
                    crate::domain::money::format_som(inv.open_tyiyn)
                )));
            }
            Some((inv.ledger_id, req.amount_tyiyn))
        }
    };
    let rid = new_id();
    let signed = if supplier {
        req.amount_tyiyn
    } else {
        -req.amount_tyiyn
    };
    let money = if supplier {
        -req.amount_tyiyn
    } else {
        req.amount_tyiyn
    };
    let comment = if req.comment.trim().is_empty() {
        party.name.clone()
    } else {
        format!("{}: {}", party.name, req.comment.trim())
    };
    let kind = if supplier {
        "supplier_payment"
    } else {
        "debt_repayment"
    };
    // Касса и комиссия — те же функции, что показывает черновик бота (SPEC-24, ADR-070).
    if let Some(account_id) =
        repayment_account(&mut *conn, &ctx.user, &method, req.account_id).await?
    {
        if method == "cash" {
            crate::api::cash::require_open_shift(&mut *conn, branch_id, account_id).await?;
        }
        crate::api::cash::add_movement(
            &mut *conn,
            ctx,
            crate::api::cash::CashEntry {
                account_id,
                kind,
                amount: money,
                doc_type: "repayment",
                doc_id: Some(rid),
                comment: &comment,
            },
        )
        .await?;
        let fee =
            repayment_fee(&mut *conn, account_id, &method, supplier, req.amount_tyiyn).await?;
        if fee != 0 {
            crate::api::cash::add_movement(
                &mut *conn,
                ctx,
                crate::api::cash::CashEntry {
                    account_id,
                    kind: "bank_fee",
                    amount: -fee,
                    doc_type: "repayment",
                    doc_id: Some(rid),
                    comment: &comment,
                },
            )
            .await?;
        }
    }
    let (balance, ledger_id) = add_ledger_to(
        &mut *conn,
        ctx,
        LedgerEntry {
            party_id: req.party_id,
            kind: "repayment",
            amount: signed,
            doc_type: "repayment",
            doc_id: Some(rid),
            comment: req.comment.trim(),
        },
        target,
    )
    .await?;
    if !supplier {
        crate::api::payroll::accrue_on_repayment(
            &mut *conn,
            ctx,
            req.party_id,
            req.amount_tyiyn,
            ledger_id,
        )
        .await?;
    }
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "party",
        Some(req.party_id),
        json!({
            "amount_tyiyn": req.amount_tyiyn,
            "method": method,
            "balance_tyiyn": balance,
            "repayment_id": rid,
            "receipt_id": req.receipt_id,
        }),
    )
    .await?;
    let out = LedgerResult {
        party_id: req.party_id,
        balance_tyiyn: balance,
        ledger_id: Some(ledger_id),
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AdjustReq {
    pub op_id: Uuid,
    pub party_id: Uuid,
    pub amount_tyiyn: i64,
    pub comment: String,
}

/// Правка баланса владельцем: прощение долга или перенос старого долга из тетради.
async fn post_adjust(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<AdjustReq>,
) -> AppResult<Json<LedgerResult>> {
    let mut tx = state.pool.begin().await?;
    let out = post_adjust_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Правка долга владельцем.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_adjust_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: AdjustReq,
) -> AppResult<LedgerResult> {
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
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let (balance, ledger_id) = add_ledger_to(
        &mut *conn,
        ctx,
        LedgerEntry {
            party_id: req.party_id,
            kind: "adjust",
            amount: req.amount_tyiyn,
            doc_type: "adjust",
            doc_id: None,
            comment: req.comment.trim(),
        },
        None,
    )
    .await?;
    // Прощённый долг не будет погашен: процент кассира по нему не ждём.
    if req.amount_tyiyn < 0 {
        crate::api::payroll::forgive_pending(
            &mut *conn,
            ctx,
            req.party_id,
            -req.amount_tyiyn,
            ledger_id,
        )
        .await?;
    }
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "party",
        Some(req.party_id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = LedgerResult {
        party_id: req.party_id,
        balance_tyiyn: balance,
        ledger_id: Some(ledger_id),
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

// ---------- Карточка ----------

#[derive(Serialize)]
pub(crate) struct TimelineItem {
    pub(crate) at: DateTime<Utc>,
    /// Запись долга: её можно сторнировать, если это погашение или правка (SPEC-10).
    pub(crate) ledger_id: Option<Uuid>,
    pub(crate) reversible: bool,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) amount_tyiyn: i64,
    pub(crate) number: Option<i64>,
    pub(crate) doc_id: Option<Uuid>,
    pub(crate) comment: String,
}

#[derive(Serialize)]
pub(crate) struct PartyCard {
    pub(crate) party: PartyOut,
    pub(crate) contacts: Vec<ContactOut>,
    pub(crate) vehicles: Vec<VehicleOut>,
    pub(crate) purchases: i64,
    pub(crate) purchases_tyiyn: i64,
    pub(crate) debt_taken_tyiyn: i64,
    pub(crate) repaid_tyiyn: i64,
    pub(crate) last_at: Option<DateTime<Utc>>,
    pub(crate) timeline: Vec<TimelineItem>,
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
    Ok(Json(
        party_card_data(&mut conn, &user, id, q.from, q.to).await?,
    ))
}

/// Карточка контрагента: покупки, долг и история — экран и бот владельца (SPEC-24).
pub(crate) async fn party_card_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<PartyCard> {
    let q = CardQuery { from, to };
    let party = load_party(&mut *conn, user.branch_id, id).await?;
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
        "select id, plate, brand, model, comment, active, mileage_unit from party_vehicles
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
        r#"select l.id, l.kind, l.amount_tyiyn, l.created_at, l.comment, l.doc_id, l.reversal_of, l.doc_type,
                  exists (select 1 from party_ledger r where r.reversal_of = l.id) as "reversed!"
           from party_ledger l where l.party_id = $1 and l.branch_id = $2
             and ($3::date is null or l.created_at >= ($3::date)::timestamp at time zone 'Asia/Bishkek')
             and ($4::date is null or l.created_at < ($4::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by l.created_at desc limit 200"#,
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
    let purchases_tyiyn = sales
        .iter()
        .fold(0i64, |acc, s| acc.saturating_add(s.total_tyiyn));
    let debt_taken_tyiyn = ledger
        .iter()
        .filter(|l| l.kind == "debt" && l.amount_tyiyn > 0)
        .map(|l| l.amount_tyiyn)
        .fold(0i64, |acc, v| acc.saturating_add(v));
    let repaid_tyiyn = ledger
        .iter()
        .filter(|l| l.kind == "repayment")
        .map(|l| -l.amount_tyiyn)
        .fold(0i64, |acc, v| acc.saturating_add(v));

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
            ledger_id: None,
            reversible: false,
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
            ledger_id: Some(l.id),
            reversible: l.kind != "debt" && l.reversal_of.is_none() && !l.reversed,
            kind: l.kind.clone(),
            title: match l.doc_type.as_str() {
                // Возмещение за брак: поставщик должен нам (SPEC-25).
                "defect" => "Возврат брака: скидка на приход".into(),
                "defect_reversal" => "Отмена возврата брака".into(),
                _ => ledger_title(&l.kind, l.amount_tyiyn, l.reversal_of.is_some()).into(),
            },
            amount_tyiyn: l.amount_tyiyn,
            number: None,
            doc_id: l.doc_id,
            comment: l.comment.clone(),
        });
    }
    timeline.sort_by_key(|t| std::cmp::Reverse(t.at));
    let last_at = timeline.first().map(|t| t.at);

    Ok(PartyCard {
        party,
        contacts,
        vehicles,
        purchases,
        purchases_tyiyn,
        debt_taken_tyiyn,
        repaid_tyiyn,
        last_at,
        timeline,
    })
}

/// Название записи долга для людей: в карточке клиента и в ответе бота.
pub(crate) fn ledger_title(kind: &str, amount: i64, reversal: bool) -> &'static str {
    match (kind, reversal) {
        (_, true) => "Сторно",
        ("debt", _) if amount > 0 => "Взял в долг",
        ("debt", _) => "Долг уменьшен возвратом",
        ("repayment", _) => "Погашение",
        _ => "Правка баланса",
    }
}

#[derive(Deserialize)]
pub struct LedgerReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Сторно погашения или правки долга владельцем (SPEC-10): новая запись с обратным знаком и
/// ссылкой на исходную; деньги погашения уходят из той кассы, куда пришли. Долг по чеку
/// снимается возвратом чека, по накладной — сторно накладной.
async fn reverse_ledger(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LedgerReverseReq>,
) -> AppResult<Json<LedgerResult>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_ledger_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно записи долга одной транзакцией вызывающего. У поставщика распределения сторнированной
/// оплаты отменяются, освободившиеся суммы ложатся на самые старые долги (ADR-073).
pub async fn reverse_ledger_tx(
    tx: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: LedgerReverseReq,
) -> AppResult<LedgerResult> {
    const KIND: &str = "debt.reverse";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let branch_id = ctx.user.branch_id;
    if let Some(done) = ops::begin_op(&mut *tx, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let l = sqlx::query!(
        r#"select l.party_id, l.kind, l.amount_tyiyn, l.doc_type, l.doc_id, l.reversal_of, p.role
           from party_ledger l join parties p on p.id = l.party_id
           where l.id = $1 and l.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if l.reversal_of.is_some() {
        return Err(invalid("это уже сторно"));
    }
    if l.kind == "debt" && l.doc_type.starts_with("defect") {
        return Err(AppError::Conflict(
            "возврат брака отменяется на экране «Остатки → Брак»".into(),
        ));
    }
    if l.kind == "debt" {
        return Err(AppError::Conflict(
            "долг по чеку снимается возвратом, по накладной — сторно накладной".into(),
        ));
    }
    let reversed = sqlx::query_scalar!(
        r#"select exists (select 1 from party_ledger where reversal_of = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if reversed {
        return Err(AppError::Conflict("запись уже сторнирована".into()));
    }
    // Деньги погашения уходят из той же кассы; для кассы смены нужна открытая смена.
    if l.kind == "repayment" && l.doc_type == "repayment" {
        let moves = sqlx::query!(
            // Комиссия возвращается вместе с деньгами, как при возврате по чеку (ADR-070,
            // ADR-088): иначе «Исправить» погашение взяло бы её второй раз.
            r#"select account_id, amount_tyiyn, kind from cash_movements
               where branch_id = $1 and doc_type = 'repayment' and doc_id = $2"#,
            branch_id,
            l.doc_id
        )
        .fetch_all(&mut *tx)
        .await?;
        for m in moves {
            crate::api::cash::require_open_shift(&mut *tx, branch_id, m.account_id).await?;
            // Возвращённая комиссия — той же статьёй «комиссия банка» по тому же погашению:
            // отчёты считают её по этим движениям (SPEC-08).
            let fee = m.kind == "bank_fee";
            crate::api::cash::add_movement(
                &mut *tx,
                ctx,
                crate::api::cash::CashEntry {
                    account_id: m.account_id,
                    kind: if fee { "bank_fee" } else { "reversal" },
                    amount: -m.amount_tyiyn,
                    doc_type: if fee { "repayment" } else { "ledger_reversal" },
                    doc_id: Some(if fee { l.doc_id.unwrap_or(id) } else { id }),
                    comment: req.comment.trim(),
                },
            )
            .await?;
        }
        // Процент кассира, начисленный за это погашение, тоже откатывается — с тех чеков, по
        // которым он начислен (ADR-035, ADR-088).
        if l.role == "customer" && !crate::api::payroll::reverse_takes(&mut *tx, ctx, id).await? {
            crate::api::payroll::reverse_repayment(&mut *tx, ctx, l.party_id, -l.amount_tyiyn)
                .await?;
        }
    }
    // Прощение долга отменили: процент кассира по этим чекам снова ждёт оплаты (ADR-088).
    if l.kind == "adjust" && l.amount_tyiyn < 0 && l.role == "customer" {
        crate::api::payroll::reverse_takes(&mut *tx, ctx, id).await?;
    }
    let balance = sqlx::query_scalar!(
        "select balance_tyiyn from parties where id = $1 for update",
        l.party_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let next = balance
        .checked_sub(l.amount_tyiyn)
        .ok_or_else(|| invalid("слишком большая сумма долга"))?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date,
                                     doc_type, doc_id, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, (now() at time zone 'Asia/Bishkek')::date,
                   'reversal', $6, $7, $6, $8, $9)"#,
        rid,
        branch_id,
        l.party_id,
        l.kind,
        -l.amount_tyiyn,
        id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("запись уже сторнирована".into())
        } else {
            AppError::from(e)
        }
    })?;
    sqlx::query!(
        "update parties set balance_tyiyn = $2 where id = $1",
        l.party_id,
        next
    )
    .execute(&mut *tx)
    .await?;
    if l.role == "supplier" {
        crate::api::supplier_debts::allocate(&mut *tx, l.party_id).await?;
    }
    ops::audit(
        &mut *tx,
        ctx,
        KIND,
        "party",
        Some(l.party_id),
        json!({ "ledger": id, "kind": l.kind, "amount_tyiyn": -l.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = LedgerResult {
        party_id: l.party_id,
        balance_tyiyn: next,
        ledger_id: Some(rid),
    };
    ops::finish_op(&mut *tx, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
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
    crate::ops::check_amount(req.amount_tyiyn)?;
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

// ---------- Срок долга и экран «Долги» (SPEC-27 §6–7) ----------

/// Срок долга клиента: последний день оплаты самого старого непогашенного долга и дней
/// просрочки. Без срока оплаты у карточки или без долга — пусто.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DebtDue {
    pub due_date: Option<NaiveDate>,
    pub overdue_days: i64,
}

/// Сроки долгов контрагентов: `(id, баланс, срок оплаты в днях)`. Тот же отбор записей, что
/// у просрочки `overdue_tyiyn` (долги и правки в плюс), поэтому просрочка в днях есть ровно
/// тогда, когда есть просроченная сумма.
pub(crate) async fn debt_dues(
    conn: &mut PgConnection,
    parties: &[(Uuid, i64, Option<i32>)],
) -> AppResult<std::collections::HashMap<Uuid, DebtDue>> {
    let ids: Vec<Uuid> = parties
        .iter()
        .filter(|(_, balance, days)| *balance > 0 && days.is_some())
        .map(|(id, _, _)| *id)
        .collect();
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let rows = sqlx::query!(
        r#"select * from (
             select l.party_id, l.business_date, l.created_at, (l.amount_tyiyn
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x join sales r on r.id = x.doc_id
                              where l.doc_type = 'sale' and x.doc_type = 'sale_return'
                                and r.reversal_of = l.doc_id and x.party_id = l.party_id), 0)
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x where x.reversal_of = l.id), 0))::bigint as "amount_tyiyn!"
             from party_ledger l
             where l.party_id = any($1) and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
           ) d where d."amount_tyiyn!" > 0
           order by d.party_id, d.business_date desc, d.created_at desc"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut entries: std::collections::HashMap<Uuid, Vec<(NaiveDate, i64)>> =
        std::collections::HashMap::new();
    for r in rows {
        entries
            .entry(r.party_id)
            .or_default()
            .push((r.business_date, r.amount_tyiyn));
    }
    let mut out = std::collections::HashMap::new();
    for (id, balance, days) in parties {
        let due = match (days, entries.get(id)) {
            (Some(days), Some(list)) => crate::domain::debt::oldest_unpaid(*balance, list)
                .map(|oldest| crate::domain::debt::due_date(oldest, *days)),
            _ => None,
        };
        out.insert(
            *id,
            DebtDue {
                due_date: due,
                overdue_days: due.map_or(0, |d| crate::domain::debt::overdue_days(d, today)),
            },
        );
    }
    Ok(out)
}

/// Строка экрана «Долги»: карточка, как в `GET /parties`, со сроком и последней оплатой.
#[derive(Serialize)]
pub struct DebtRow {
    #[serde(flatten)]
    pub party: PartyOut,
    pub due_date: Option<NaiveDate>,
    pub overdue_days: i64,
    /// Последнее погашение (или оплата поставщику), не сторнированное.
    pub last_payment_at: Option<DateTime<Utc>>,
}

async fn debts_list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<DebtRow>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(debts_list_data(&mut conn, user.branch_id).await?))
}

/// Все должники и кому должны мы — тот же поиск, что `GET /parties?only_debtors=true`, с
/// просрочкой в днях и датой последней оплаты (экран «Долги»).
pub(crate) async fn debts_list_data(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<Vec<DebtRow>> {
    let parties = search_parties(
        &mut *conn,
        branch_id,
        ListQuery {
            only_debtors: true,
            ..ListQuery::default()
        },
    )
    .await?;
    let keys: Vec<(Uuid, i64, Option<i32>)> = parties
        .iter()
        .map(|p| (p.id, p.balance_tyiyn, p.due_days))
        .collect();
    let mut dues = debt_dues(&mut *conn, &keys).await?;
    let ids: Vec<Uuid> = parties.iter().map(|p| p.id).collect();
    let last: std::collections::HashMap<Uuid, DateTime<Utc>> = sqlx::query!(
        r#"select l.party_id, max(l.created_at) as "at!" from party_ledger l
           where l.party_id = any($1) and l.kind = 'repayment' and l.reversal_of is null
             and not exists (select 1 from party_ledger r where r.reversal_of = l.id)
           group by l.party_id"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.party_id, r.at))
    .collect();
    Ok(parties
        .into_iter()
        .map(|p| {
            let due = dues.remove(&p.id).unwrap_or_default();
            DebtRow {
                due_date: due.due_date,
                overdue_days: due.overdue_days,
                last_payment_at: last.get(&p.id).copied(),
                party: p,
            }
        })
        .collect())
}

/// Неоплаченный долг клиента: запись долга (чек в долг или правка) и сколько по ней осталось.
#[derive(Serialize, Deserialize, Debug)]
pub struct OpenDebt {
    pub ledger_id: Uuid,
    pub business_date: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub kind: String,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
    pub comment: String,
    pub amount_tyiyn: i64,
    pub left_tyiyn: i64,
    /// Срок этой записи прошёл (есть срок оплаты у клиента).
    pub overdue: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OpenDebts {
    pub balance_tyiyn: i64,
    pub overdue_tyiyn: i64,
    pub due_date: Option<NaiveDate>,
    pub overdue_days: i64,
    /// Неоплаченные долги, старые сверху.
    pub items: Vec<OpenDebt>,
}

/// Окно приёма долга (SPEC-28 §3): долг клиента, просрочка и неоплаченные чеки с остатком. Те же
/// записи и тот же порядок погашения, что у просрочки `overdue_tyiyn`: оплата гасит старые долги
/// первыми, неоплаченными остаются самые новые на сумму долга.
async fn open_debts(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<OpenDebts>> {
    let mut conn = state.pool.acquire().await?;
    let party = load_party(&mut conn, user.branch_id, id).await?;
    let rows = sqlx::query!(
        // Долг записи — за вычетом возвратов по этому чеку и сторно самой записи: возвращённый
        // чек и отменённая правка не висят «неоплаченными» (ADR-088).
        r#"select * from (
             select l.id, l.business_date, l.created_at, l.kind, (l.amount_tyiyn
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x join sales r on r.id = x.doc_id
                              where l.doc_type = 'sale' and x.doc_type = 'sale_return'
                                and r.reversal_of = l.doc_id and x.party_id = l.party_id), 0)
                  - coalesce((select sum(-x.amount_tyiyn) from party_ledger x where x.reversal_of = l.id), 0))::bigint as "amount_tyiyn!",
                    l.comment, s.id as "sale_id?", s.number as "sale_number?"
             from party_ledger l
             left join sales s on l.doc_type = 'sale' and s.id = l.doc_id
             where l.party_id = $1 and l.branch_id = $2 and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
           ) d where d."amount_tyiyn!" > 0
           order by d.business_date desc, d.created_at desc"#,
        id,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let amounts: Vec<i64> = rows.iter().map(|r| r.amount_tyiyn).collect();
    let left = crate::domain::debt::unpaid_parts(party.balance_tyiyn, &amounts);
    let dues = debt_dues(
        &mut conn,
        &[(party.id, party.balance_tyiyn, party.due_days)],
    )
    .await?;
    let due = dues.get(&party.id);
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let mut items: Vec<OpenDebt> = rows
        .into_iter()
        .zip(left)
        .filter(|(_, left)| *left > 0)
        .map(|(r, left)| OpenDebt {
            ledger_id: r.id,
            business_date: r.business_date,
            created_at: r.created_at,
            kind: r.kind,
            sale_id: r.sale_id,
            sale_number: r.sale_number,
            comment: r.comment,
            amount_tyiyn: r.amount_tyiyn,
            left_tyiyn: left,
            overdue: party.due_days.is_some_and(|d| {
                crate::domain::debt::overdue_days(
                    crate::domain::debt::due_date(r.business_date, d),
                    today,
                ) > 0
            }),
        })
        .collect();
    items.reverse();
    Ok(Json(OpenDebts {
        balance_tyiyn: party.balance_tyiyn,
        overdue_tyiyn: party.overdue_tyiyn,
        due_date: due.and_then(|d| d.due_date),
        overdue_days: due.map_or(0, |d| d.overdue_days),
        items,
    }))
}
