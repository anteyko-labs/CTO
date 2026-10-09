//! Справочники сотрудников, услуг, поставщиков и настройки этикеток (SPEC-02).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
        .route(
            "/services",
            routing::get(list_services).post(create_service),
        )
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
}

async fn list_employees(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<EmployeeOut>>> {
    let rows = sqlx::query_as!(
        EmployeeOut,
        "select id, full_name, is_cashier, is_master, active from employees where branch_id = $1 order by full_name",
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct EmployeeReq {
    full_name: String,
    #[serde(default)]
    is_cashier: bool,
    #[serde(default)]
    is_master: bool,
}

async fn create_employee(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<EmployeeReq>,
) -> AppResult<Json<EmployeeOut>> {
    let full_name = required(&req.full_name, "ФИО")?;
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into employees (id, branch_id, full_name, is_cashier, is_master) values ($1, $2, $3, $4, $5)",
        id,
        ctx.user.branch_id,
        full_name,
        req.is_cashier,
        req.is_master
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "employee.create",
        "employee",
        Some(id),
        json!({ "full_name": full_name }),
    )
    .await?;
    if req.is_cashier {
        crate::api::payroll::ensure_cashier_rules(&mut tx, &ctx, id).await?;
    }
    tx.commit().await?;
    Ok(Json(EmployeeOut {
        id,
        full_name,
        is_cashier: req.is_cashier,
        is_master: req.is_master,
        active: true,
    }))
}

#[derive(Deserialize)]
struct EmployeePatch {
    full_name: Option<String>,
    is_cashier: Option<bool>,
    is_master: Option<bool>,
    active: Option<bool>,
}

async fn update_employee(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<EmployeePatch>,
) -> AppResult<Json<EmployeeOut>> {
    let full_name = optional_required(req.full_name, "ФИО")?;
    let mut tx = state.pool.begin().await?;
    let out = sqlx::query_as!(
        EmployeeOut,
        r#"update employees set full_name = coalesce($3, full_name), is_cashier = coalesce($4, is_cashier),
             is_master = coalesce($5, is_master), active = coalesce($6, active)
           where id = $1 and branch_id = $2
           returning id, full_name, is_cashier, is_master, active"#,
        id,
        ctx.user.branch_id,
        full_name,
        req.is_cashier,
        req.is_master,
        req.active
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "employee.update",
        "employee",
        Some(id),
        json!({ "active": out.active }),
    )
    .await?;
    if req.is_cashier == Some(true) {
        crate::api::payroll::ensure_cashier_rules(&mut tx, &ctx, id).await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Услуги ----------

#[derive(Serialize)]
struct ServiceOut {
    id: Uuid,
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: i64,
    active: bool,
}

async fn list_services(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<ServiceOut>>> {
    let rows = sqlx::query_as!(
        ServiceOut,
        "select id, name, price_tyiyn, master_fee_tyiyn, active from services where branch_id = $1 order by name",
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
    if fee != 0 && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn) values ($1, $2, $3, $4, $5)",
        id,
        ctx.user.branch_id,
        name,
        req.price_tyiyn,
        fee
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "service.create",
        "service",
        Some(id),
        json!({ "name": name, "price": req.price_tyiyn, "fee": fee }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ServiceOut {
        id,
        name,
        price_tyiyn: req.price_tyiyn,
        master_fee_tyiyn: fee,
        active: true,
    }))
}

#[derive(Deserialize)]
struct ServicePatch {
    name: Option<String>,
    price_tyiyn: Option<i64>,
    master_fee_tyiyn: Option<i64>,
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
    if !ctx.user.is_owner() && (req.master_fee_tyiyn.is_some() || req.price_tyiyn.is_some()) {
        let cur = sqlx::query!(
            "select price_tyiyn, master_fee_tyiyn from services where id = $1 and branch_id = $2",
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
        {
            return Err(AppError::Forbidden);
        }
    }
    let out = sqlx::query_as!(
        ServiceOut,
        r#"update services set name = coalesce($3, name), price_tyiyn = coalesce($4, price_tyiyn),
             master_fee_tyiyn = coalesce($5, master_fee_tyiyn), active = coalesce($6, active)
           where id = $1 and branch_id = $2
           returning id, name, price_tyiyn, master_fee_tyiyn, active"#,
        id,
        ctx.user.branch_id,
        name,
        req.price_tyiyn,
        req.master_fee_tyiyn,
        req.active
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
        json!({ "price": out.price_tyiyn, "fee": out.master_fee_tyiyn, "active": out.active }),
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
        json!({ "active": out.active }),
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
}

impl Default for LabelSettings {
    fn default() -> Self {
        Self {
            width_mm: 40,
            height_mm: 30,
            show_name: true,
            show_price: true,
            show_article: false,
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
