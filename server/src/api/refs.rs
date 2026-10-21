//! Справочники сотрудников, услуг, поставщиков и настройки этикеток (SPEC-02).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/employees",
            routing::get(list_employees).post(create_employee),
        )
        .route("/employees/{id}", routing::patch(update_employee))
        .route("/employees/{id}/pin", routing::post(set_pin))
        .route(
            "/services",
            routing::get(list_services).post(create_service),
        )
        .route("/services/import", routing::post(import_services))
        .route("/services/{id}", routing::patch(update_service))
        .route(
            "/suppliers",
            routing::get(list_suppliers).post(create_supplier),
        )
        .route(
            "/suppliers/{id}",
            routing::get(get_supplier).patch(update_supplier),
        )
        .route("/suppliers/{id}/supplies", routing::get(supplier_supplies))
        .route("/settings/labels", routing::get(get_labels).put(put_labels))
        .route(
            "/settings/debt-docs",
            routing::get(get_debt_docs).put(put_debt_docs),
        )
        .route(
            "/settings/sales",
            routing::get(get_sales_settings).put(put_sales_settings),
        )
}

fn required(s: &str, what: &str) -> AppResult<String> {
    let t = s.trim();
    if t.is_empty() {
        Err(invalid(format!("{what} обязательно")))
    } else {
        Ok(t.to_string())
    }
}

fn optional_required(s: Option<String>, what: &str) -> AppResult<Option<String>> {
    s.map(|v| required(&v, what)).transpose()
}

// ---------- Сотрудники ----------

#[derive(Serialize)]
struct EmployeeOut {
    id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
    active: bool,
    /// Телефон для кабинета в боте (SPEC-23) — только владельцу: он открывает доступ к начислениям.
    #[serde(skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    /// PIN кассира задан (ADR-081); сам PIN не отдаётся никому.
    has_pin: bool,
}

async fn list_employees(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<EmployeeOut>>> {
    let owner = user.is_owner();
    let rows = sqlx::query!(
        "select id, full_name, is_cashier, is_master, active, phone, pin_hash is not null as \"has_pin!\" from employees where branch_id = $1 order by full_name",
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|r| EmployeeOut {
        id: r.id,
        full_name: r.full_name,
        is_cashier: r.is_cashier,
        is_master: r.is_master,
        active: r.active,
        phone: owner.then_some(r.phone),
        has_pin: r.has_pin,
    })
    .collect();
    Ok(Json(rows))
}

/// Телефон сотрудника: пусто или номер, в котором не меньше 9 цифр (сверяются последние 9).
fn employee_phone(s: &str) -> AppResult<String> {
    let t = s.trim();
    if !t.is_empty() && crate::api::telegram::phone_key(t).is_none() {
        return Err(invalid("телефон: не меньше 9 цифр"));
    }
    if t.chars().count() > 40 {
        return Err(invalid("телефон не длиннее 40 знаков"));
    }
    Ok(t.to_string())
}

#[derive(Deserialize)]
struct EmployeeReq {
    full_name: String,
    #[serde(default)]
    is_cashier: bool,
    #[serde(default)]
    is_master: bool,
    /// Телефон для бота можно указать сразу — только владелец (SPEC-23, SPEC-27).
    #[serde(default)]
    phone: String,
    /// PIN кассира (4 цифры) можно задать сразу — только владелец (ADR-081).
    #[serde(default)]
    pin: Option<String>,
}

/// PIN кассира — ровно 4 цифры (ADR-081).
pub(crate) fn check_pin(pin: &str) -> AppResult<()> {
    if pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(invalid("PIN — 4 цифры"))
    }
}

async fn create_employee(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<EmployeeReq>,
) -> AppResult<Json<EmployeeOut>> {
    let full_name = required(&req.full_name, "ФИО")?;
    // Кассир сразу получает оплату (2 % и оклад) — ставки назначает только владелец (SPEC-07, ADR-059).
    if req.is_cashier && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let phone = employee_phone(&req.phone)?;
    if !phone.is_empty() && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let pin_hash = match req.pin.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(pin) => {
            if !ctx.user.is_owner() {
                return Err(AppError::Forbidden);
            }
            check_pin(pin)?;
            Some(crate::auth::hash_password(pin.to_string()).await?)
        }
        None => None,
    };
    let has_pin = pin_hash.is_some();
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into employees (id, branch_id, full_name, is_cashier, is_master, phone, pin_hash, pin_set_at)
           values ($1, $2, $3, $4, $5, $6, $7, case when $7::text is null then null else now() end)"#,
        id,
        ctx.user.branch_id,
        full_name,
        req.is_cashier,
        req.is_master,
        phone,
        pin_hash
    )
    .execute(&mut *tx)
    .await?;
    let rules_added = if req.is_cashier {
        crate::api::payroll::ensure_cashier_rules(&mut tx, &ctx, id).await?
    } else {
        false
    };
    ops::audit(
        &mut tx,
        &ctx,
        "employee.create",
        "employee",
        Some(id),
        json!({
            "full_name": full_name,
            "is_cashier": req.is_cashier,
            "is_master": req.is_master,
            "phone": phone,
            "pay_rules_added": rules_added,
            "pin_set": has_pin,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(EmployeeOut {
        id,
        full_name,
        is_cashier: req.is_cashier,
        is_master: req.is_master,
        active: true,
        phone: ctx.user.is_owner().then_some(phone),
        has_pin,
    }))
}

#[derive(Deserialize)]
struct EmployeePatch {
    full_name: Option<String>,
    is_cashier: Option<bool>,
    is_master: Option<bool>,
    active: Option<bool>,
    /// Телефон для кабинета в боте — меняет только владелец (SPEC-23).
    phone: Option<String>,
}

async fn update_employee(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<EmployeePatch>,
) -> AppResult<Json<EmployeeOut>> {
    let full_name = optional_required(req.full_name, "ФИО")?;
    let phone = req.phone.as_deref().map(employee_phone).transpose()?;
    let mut tx = state.pool.begin().await?;
    let prev = sqlx::query!(
        "select full_name, is_cashier, is_master, active, phone from employees where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let was_cashier = prev.is_cashier;
    let before = json!({
        "full_name": prev.full_name,
        "is_cashier": prev.is_cashier,
        "is_master": prev.is_master,
        "active": prev.active,
        "phone": prev.phone,
    });
    // Признак «кассир» меняет оплату (процент с чеков, оклад) — только владелец (SPEC-07, ADR-059).
    // Клиент шлёт признак и без изменения — это не правка.
    let becomes_cashier = req.is_cashier == Some(true) && !was_cashier;
    if req.is_cashier.is_some_and(|c| c != was_cashier) && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // Телефон открывает работнику его начисления в боте — задаёт и меняет только владелец (SPEC-23).
    let phone_changed = phone.as_ref().is_some_and(|p| *p != prev.phone);
    if phone_changed && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // Включённый снова сотрудник с телефоном сразу видит свой заработок в боте (SPEC-23) —
    // это доступ, его открывает только владелец (ADR-070).
    let has_phone = !phone
        .as_deref()
        .unwrap_or(prev.phone.as_str())
        .trim()
        .is_empty();
    if req.active == Some(true) && !prev.active && has_phone && !ctx.user.is_owner() {
        return Err(AppError::ForbiddenWith(
            "сотрудника с телефоном включает владелец: телефон открывает ему кабинет в боте".into(),
        ));
    }
    let r = sqlx::query!(
        r#"update employees set full_name = coalesce($3, full_name), is_cashier = coalesce($4, is_cashier),
             is_master = coalesce($5, is_master), active = coalesce($6, active), phone = coalesce($7, phone)
           where id = $1 and branch_id = $2
           returning id, full_name, is_cashier, is_master, active, phone, pin_hash is not null as "has_pin!""#,
        id,
        ctx.user.branch_id,
        full_name,
        req.is_cashier,
        req.is_master,
        req.active,
        phone
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Отключили или сменили номер (по последним 9 цифрам) — чат работника отвязывается сразу.
    let new_key = crate::api::telegram::phone_key(&r.phone);
    if prev.active && !r.active {
        crate::api::bot_staff::drop_for_employee(&mut tx, &ctx, id, "сотрудник отключён").await?;
    } else if new_key.is_none() || new_key != crate::api::telegram::phone_key(&prev.phone) {
        crate::api::bot_staff::drop_for_employee(&mut tx, &ctx, id, "телефон в карточке изменён")
            .await?;
    }
    let out = EmployeeOut {
        id: r.id,
        full_name: r.full_name,
        is_cashier: r.is_cashier,
        is_master: r.is_master,
        active: r.active,
        phone: ctx.user.is_owner().then(|| r.phone.clone()),
        has_pin: r.has_pin,
    };
    // Правила по умолчанию — только когда сотрудник становится кассиром и правил кассира у него
    // никогда не было: правка имени и повторное назначение не возвращают закрытый оклад (ADR-062).
    let rules_added = if becomes_cashier {
        crate::api::payroll::ensure_cashier_rules(&mut tx, &ctx, id).await?
    } else {
        false
    };
    ops::audit(
        &mut tx,
        &ctx,
        "employee.update",
        "employee",
        Some(id),
        json!({
            "full_name": out.full_name,
            "is_cashier": out.is_cashier,
            "is_master": out.is_master,
            "active": out.active,
            "phone": r.phone,
            "phone_changed": phone_changed,
            "pay_rules_added": rules_added,
            "before": before,
            "after": {
                "full_name": out.full_name,
                "is_cashier": out.is_cashier,
                "is_master": out.is_master,
                "active": out.active,
                "phone": r.phone,
            },
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct PinReq {
    /// Новый PIN, 4 цифры; `null` — снять PIN.
    pin: Option<String>,
}

/// PIN кассира задаёт, меняет и снимает только владелец (ADR-081). Хранится хеш argon2, как у
/// паролей; новый PIN снимает блокировку после неверных попыток.
async fn set_pin(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PinReq>,
) -> AppResult<Json<EmployeeOut>> {
    ctx.user.require_owner()?;
    let pin = req.pin.map(|p| p.trim().to_string());
    let hash = match &pin {
        Some(p) => {
            check_pin(p)?;
            Some(crate::auth::hash_password(p.clone()).await?)
        }
        None => None,
    };
    let mut tx = state.pool.begin().await?;
    let r = sqlx::query!(
        r#"update employees
           set pin_hash = $3, pin_set_at = case when $3::text is null then null else now() end
           where id = $1 and branch_id = $2
           returning id, full_name, is_cashier, is_master, active, phone"#,
        id,
        ctx.user.branch_id,
        hash
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "employee.pin",
        "employee",
        Some(id),
        json!({ "full_name": r.full_name, "set": hash.is_some() }),
    )
    .await?;
    tx.commit().await?;
    crate::api::auth::clear_attempts(&state, &pin_key(id)).await?;
    Ok(Json(EmployeeOut {
        id: r.id,
        full_name: r.full_name,
        is_cashier: r.is_cashier,
        is_master: r.is_master,
        active: r.active,
        phone: Some(r.phone),
        has_pin: hash.is_some(),
    }))
}

/// Ключ счётчика неверных PIN кассира (та же таблица, что у попыток входа).
pub(crate) fn pin_key(employee_id: Uuid) -> String {
    format!("pin|{employee_id}")
}

// ---------- Услуги ----------

/// Услуга (ADR-096): цена клиенту (со своим маслом), «бесплатно с нашим маслом» и ставка
/// мастеру за одну услугу.
#[derive(Serialize)]
struct ServiceOut {
    id: Uuid,
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: i64,
    free_with_oil: bool,
    active: bool,
}

async fn list_services(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<ServiceOut>>> {
    let rows = sqlx::query_as!(
        ServiceOut,
        "select id, name, price_tyiyn, master_fee_tyiyn, free_with_oil, active from services where branch_id = $1 order by name",
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ServiceReq {
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: Option<i64>,
    #[serde(default)]
    free_with_oil: bool,
}

async fn create_service(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ServiceReq>,
) -> AppResult<Json<ServiceOut>> {
    let name = required(&req.name, "название")?;
    // Без ставки по умолчанию: её задаёт владелец, как и остальную оплату труда (ADR-019, ADR-043).
    let fee = req.master_fee_tyiyn.unwrap_or(0);
    if req.price_tyiyn < 0 || fee < 0 {
        return Err(invalid("суммы не отрицательны"));
    }
    ops::check_amount(req.price_tyiyn)?;
    ops::check_amount(fee)?;
    // Ставку мастера и «бесплатно с нашим маслом» задаёт владелец: это цена и оплата труда.
    if (fee != 0 || req.free_with_oil) && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, free_with_oil) values ($1, $2, $3, $4, $5, $6)",
        id,
        ctx.user.branch_id,
        name,
        req.price_tyiyn,
        fee,
        req.free_with_oil
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "service.create",
        "service",
        Some(id),
        json!({ "name": name, "price": req.price_tyiyn, "fee": fee, "free_with_oil": req.free_with_oil }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ServiceOut {
        id,
        name,
        price_tyiyn: req.price_tyiyn,
        master_fee_tyiyn: fee,
        free_with_oil: req.free_with_oil,
        active: true,
    }))
}

/// Строка загрузки услуг из выгрузки старой программы: название и цена.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ServiceImportItem {
    pub name: String,
    pub price_tyiyn: i64,
}

/// Загрузка услуг списком (SPEC-02 §услуги, проверка 20.10.2026): одна операция с `op_id`.
#[derive(Deserialize, Debug)]
pub struct ServicesImportReq {
    pub op_id: Uuid,
    pub items: Vec<ServiceImportItem>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ServicesImportOut {
    /// Заведено новых услуг.
    pub created: i64,
    /// Пропущено: такое название уже есть (без учёта регистра) или повтор в самом списке.
    pub skipped: Vec<String>,
}

async fn import_services(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ServicesImportReq>,
) -> AppResult<Json<ServicesImportOut>> {
    let mut tx = state.pool.begin().await?;
    let out = import_services_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Услуги из выгрузки старой программы на новой точке: только владелец, одной транзакцией.
/// Названия, которые уже есть (без учёта регистра и пробелов по краям), пропускаются; ставка
/// мастера — 0, её задаёт владелец в карточке услуги.
pub async fn import_services_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ServicesImportReq,
) -> AppResult<ServicesImportOut> {
    const KIND: &str = "service.import";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.items.is_empty() {
        return Err(invalid("отметьте хотя бы одну услугу"));
    }
    if req.items.len() > 1000 {
        return Err(invalid("за один раз — не больше 1000 услуг"));
    }
    let mut items = Vec::with_capacity(req.items.len());
    for i in &req.items {
        let name = required(&i.name, "название услуги")?;
        if name.chars().count() > 200 {
            return Err(invalid(format!("название «{name}» длиннее 200 знаков")));
        }
        if i.price_tyiyn < 0 {
            return Err(invalid("суммы не отрицательны"));
        }
        ops::check_amount(i.price_tyiyn)?;
        items.push((name, i.price_tyiyn));
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let existing = sqlx::query_scalar!(
        r#"select lower(btrim(name)) as "name!" from services where branch_id = $1"#,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut seen: std::collections::HashSet<String> = existing.into_iter().collect();
    let mut created = 0i64;
    let mut skipped = Vec::new();
    for (name, price) in items {
        if !seen.insert(name.to_lowercase()) {
            skipped.push(name);
            continue;
        }
        let id = new_id();
        sqlx::query!(
            "insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn) values ($1, $2, $3, $4, 0)",
            id,
            ctx.user.branch_id,
            name,
            price
        )
        .execute(&mut *conn)
        .await?;
        ops::audit(
            &mut *conn,
            ctx,
            "service.create",
            "service",
            Some(id),
            json!({ "name": name, "price": price, "fee": 0, "import": true }),
        )
        .await?;
        created += 1;
    }
    let out = ServicesImportOut { created, skipped };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize)]
struct ServicePatch {
    name: Option<String>,
    price_tyiyn: Option<i64>,
    master_fee_tyiyn: Option<i64>,
    free_with_oil: Option<bool>,
    active: Option<bool>,
}

async fn update_service(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ServicePatch>,
) -> AppResult<Json<ServiceOut>> {
    let name = optional_required(req.name, "название")?;
    if [req.price_tyiyn, req.master_fee_tyiyn]
        .iter()
        .flatten()
        .any(|v| *v < 0)
    {
        return Err(invalid("суммы не отрицательны"));
    }
    for v in [req.price_tyiyn, req.master_fee_tyiyn]
        .into_iter()
        .flatten()
    {
        ops::check_amount(v)?;
    }
    let mut tx = state.pool.begin().await?;
    // Цену и ставку мастера меняет только владелец (решение заказчика, ADR-047); форма
    // администратора присылает их без изменений.
    if !ctx.user.is_owner()
        && (req.master_fee_tyiyn.is_some()
            || req.price_tyiyn.is_some()
            || req.free_with_oil.is_some())
    {
        let cur = sqlx::query!(
            "select price_tyiyn, master_fee_tyiyn, free_with_oil from services where id = $1 and branch_id = $2",
            id,
            ctx.user.branch_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
        if req
            .master_fee_tyiyn
            .is_some_and(|f| f != cur.master_fee_tyiyn)
            || req.price_tyiyn.is_some_and(|p| p != cur.price_tyiyn)
            || req.free_with_oil.is_some_and(|f| f != cur.free_with_oil)
        {
            return Err(AppError::Forbidden);
        }
    }
    let out = sqlx::query_as!(
        ServiceOut,
        r#"update services set name = coalesce($3, name), price_tyiyn = coalesce($4, price_tyiyn),
             master_fee_tyiyn = coalesce($5, master_fee_tyiyn), active = coalesce($6, active),
             free_with_oil = coalesce($7, free_with_oil)
           where id = $1 and branch_id = $2
           returning id, name, price_tyiyn, master_fee_tyiyn, free_with_oil, active"#,
        id,
        ctx.user.branch_id,
        name,
        req.price_tyiyn,
        req.master_fee_tyiyn,
        req.active,
        req.free_with_oil
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "service.update",
        "service",
        Some(id),
        json!({ "price": out.price_tyiyn, "fee": out.master_fee_tyiyn, "free_with_oil": out.free_with_oil, "active": out.active }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Поставщики ----------

#[derive(Serialize)]
struct SupplierOut {
    id: Uuid,
    name: String,
    phone: String,
    comment: String,
    active: bool,
}

async fn list_suppliers(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> AppResult<Json<Vec<SupplierOut>>> {
    let rows = sqlx::query_as!(
        SupplierOut,
        "select id, name, phone, comment, active from suppliers order by name"
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct SupplierReq {
    name: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    comment: String,
}

async fn create_supplier(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SupplierReq>,
) -> AppResult<Json<SupplierOut>> {
    let name = required(&req.name, "название")?;
    let id = new_id();
    let (phone, comment) = (req.phone.trim().to_string(), req.comment.trim().to_string());
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into suppliers (id, name, phone, comment) values ($1, $2, $3, $4)",
        id,
        name,
        phone,
        comment
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into parties (id, branch_id, role, kind, name, phone, comment)
           values ($1, $2, 'supplier', 'company', $3, $4, $5)"#,
        id,
        ctx.user.branch_id,
        name,
        phone,
        comment
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "supplier.create",
        "supplier",
        Some(id),
        json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(SupplierOut {
        id,
        name,
        phone,
        comment,
        active: true,
    }))
}

#[derive(Deserialize)]
struct SupplierPatch {
    name: Option<String>,
    phone: Option<String>,
    comment: Option<String>,
    active: Option<bool>,
}

async fn update_supplier(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<SupplierPatch>,
) -> AppResult<Json<SupplierOut>> {
    let name = optional_required(req.name, "название")?;
    let mut tx = state.pool.begin().await?;
    // Поставщик общий для филиалов, без branch_id (отступление записано в ADR-062): журнал
    // правки пишется в филиал того, кто правил.
    let before = sqlx::query!(
        "select name, phone, comment, active from suppliers where id = $1 for update",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let out = sqlx::query_as!(
        SupplierOut,
        r#"update suppliers set name = coalesce($2, name), phone = coalesce($3, phone),
             comment = coalesce($4, comment), active = coalesce($5, active)
           where id = $1 returning id, name, phone, comment, active"#,
        id,
        name,
        req.phone.map(|s| s.trim().to_string()),
        req.comment.map(|s| s.trim().to_string()),
        req.active
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    sqlx::query!(
        "update parties set name = $2, phone = $3, comment = $4, active = $5 where id = $1",
        id,
        out.name,
        out.phone,
        out.comment,
        out.active
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "supplier.update",
        "supplier",
        Some(id),
        json!({
            "name": out.name,
            "active": out.active,
            "changes": ops::changes(
                &json!({ "name": before.name, "phone": before.phone, "comment": before.comment, "active": before.active }),
                &json!({ "name": out.name, "phone": out.phone, "comment": out.comment, "active": out.active }),
            ),
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Этикетки ----------

#[derive(Serialize, Deserialize)]
struct LabelSettings {
    width_mm: i64,
    height_mm: i64,
    show_name: bool,
    show_price: bool,
    show_article: bool,
    /// Бренд отдельной строкой над названием (SPEC-29 §6). У прежних настроек — да.
    #[serde(default = "yes")]
    show_brand: bool,
    /// Характеристики: вязкость и объём масла, тип фильтра, ёмкость и ток аккумулятора.
    #[serde(default = "yes")]
    show_spec: bool,
}

fn yes() -> bool {
    true
}

impl Default for LabelSettings {
    fn default() -> Self {
        Self {
            width_mm: 40,
            height_mm: 30,
            show_name: true,
            show_price: true,
            show_article: false,
            show_brand: true,
            show_spec: true,
        }
    }
}

/// Реквизиты точки для документов: продавец в расписке, накладной и акте сверки.
#[derive(Serialize, Deserialize, Default, Clone)]
struct Seller {
    #[serde(default)]
    name: String,
    #[serde(default)]
    inn: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    director: String,
    #[serde(default)]
    bank: String,
    #[serde(default)]
    city: String,
}

/// Тексты шаблонов: пустой — касса берёт текст по умолчанию из SPEC-10.
#[derive(Serialize, Deserialize, Default, Clone)]
struct DebtDocs {
    #[serde(default)]
    seller: Seller,
    #[serde(default)]
    person: Option<String>,
    #[serde(default)]
    company: Option<String>,
}

async fn get_debt_docs(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Value>> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'debt_docs'",
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?;
    let docs = v
        .and_then(|v| serde_json::from_value::<DebtDocs>(v).ok())
        .unwrap_or_default();
    Ok(Json(json!(docs)))
}

/// Реквизиты и тексты правит владелец (SPEC-10, права).
async fn put_debt_docs(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<DebtDocs>,
) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let too_long = [&req.person, &req.company]
        .into_iter()
        .flatten()
        .any(|t| t.chars().count() > 20_000);
    if too_long {
        return Err(invalid("текст шаблона не длиннее 20 000 знаков"));
    }
    let value = json!(req);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'debt_docs', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.debt_docs",
        "settings",
        None,
        json!({ "seller": req.seller.name }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}

async fn get_labels(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Value>> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'labels'",
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?;
    let settings = v
        .and_then(|v| serde_json::from_value::<LabelSettings>(v).ok())
        .unwrap_or_default();
    Ok(Json(json!(settings)))
}

async fn put_labels(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<LabelSettings>,
) -> AppResult<Json<Value>> {
    if !(10..=200).contains(&req.width_mm) || !(10..=200).contains(&req.height_mm) {
        return Err(invalid("размер этикетки от 10 до 200 мм"));
    }
    let value = json!(req);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'labels', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.labels",
        "settings",
        None,
        value.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}

// ---------- Настройки кассы ----------

#[derive(Serialize, Deserialize, Clone, Copy)]
struct SalesSettings {
    /// Ставка мастера за чек с заменой (ADR-027).
    oil_change_master_fee_tyiyn: i64,
}

impl Default for SalesSettings {
    fn default() -> Self {
        Self {
            oil_change_master_fee_tyiyn: crate::api::sales::DEFAULT_OIL_CHANGE_FEE,
        }
    }
}

async fn get_sales_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Value>> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'sales'",
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?;
    let settings = v
        .and_then(|v| serde_json::from_value::<SalesSettings>(v).ok())
        .unwrap_or_default();
    Ok(Json(json!(settings)))
}

async fn put_sales_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SalesSettings>,
) -> AppResult<Json<Value>> {
    // Ставку оплаты задаёт только владелец (ADR-019).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !(0..=100_000).contains(&req.oil_change_master_fee_tyiyn) {
        return Err(invalid("ставка за замену от 0 до 1000 сом"));
    }
    let value = json!(req);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'sales', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.sales",
        "settings",
        None,
        value.clone(),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(value))
}

// ---------- Карточка поставщика ----------

#[derive(Serialize)]
struct SupplyRow {
    product_id: Uuid,
    name: String,
    unit: String,
    container_ml: Option<i64>,
    receipts: i64,
    qty: i64,
    amount_tyiyn: i64,
    /// Цена последней поставки за штуку или канистру.
    last_price_tyiyn: i64,
    last_at: DateTime<Utc>,
}

async fn get_supplier(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<SupplierOut>> {
    sqlx::query_as!(
        SupplierOut,
        "select id, name, phone, comment, active from suppliers where id = $1",
        id
    )
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or(AppError::NotFound)
}

/// Что поставщик привозил: по товарам, с количеством, суммой и последней ценой.
/// Сторно накладных входит со своим знаком, поэтому отменённая поставка сама себя гасит.
async fn supplier_supplies(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<SupplyRow>>> {
    let rows = sqlx::query!(
        r#"select l.product_id as "product_id!", p.name as "name!", p.unit as "unit!",
                  p.container_ml as "container_ml?",
                  count(distinct r.id) as "receipts!",
                  sum(l.qty)::bigint as "qty!",
                  sum(l.cost_tyiyn)::bigint as "amount_tyiyn!",
                  (array_agg(l.cost_tyiyn order by r.created_at desc))[1] as "last_cost!",
                  (array_agg(l.qty order by r.created_at desc))[1] as "last_qty!",
                  max(r.created_at) as "last_at!"
           from receipt_lines l
           join receipts r on r.id = l.receipt_id
           join products p on p.id = l.product_id
           where r.branch_id = $1 and r.supplier_id = $2
           group by l.product_id, p.name, p.unit, p.container_ml
           order by max(r.created_at) desc"#,
        user.branch_id,
        id
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|r| SupplyRow {
        product_id: r.product_id,
        name: r.name,
        unit: r.unit,
        container_ml: r.container_ml,
        receipts: r.receipts,
        qty: r.qty,
        amount_tyiyn: r.amount_tyiyn,
        last_price_tyiyn: if r.last_qty > 0 {
            div_round(i128::from(r.last_cost), i128::from(r.last_qty)).unwrap_or(0)
        } else {
            0
        },
        last_at: r.last_at,
    })
    .collect();
    Ok(Json(rows))
}
