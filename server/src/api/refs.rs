//! Справочники сотрудников, услуг, поставщиков и настройки этикеток (SPEC-02).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
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
        .route("/suppliers/{id}", routing::patch(update_supplier))
        .route("/settings/labels", routing::get(get_labels).put(put_labels))
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
    let fee = req.master_fee_tyiyn.unwrap_or(3000);
    if req.price_tyiyn < 0 || fee < 0 {
        return Err(invalid("суммы не отрицательны"));
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
    let mut tx = state.pool.begin().await?;
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
