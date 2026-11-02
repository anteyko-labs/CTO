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
            "/settings/debt-docs/signature",
            routing::get(get_owner_signature)
                .put(put_owner_signature)
                .delete(delete_owner_signature),
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
    /// Вход в программу этого сотрудника (задача 005 Н1, ADR-170) — только владельцу: он заводит
    /// и меняет вход.
    #[serde(skip_serializing_if = "Option::is_none")]
    login: Option<EmployeeLogin>,
}

/// Вход сотрудника в карточке: логин и включён ли он.
#[derive(Serialize)]
struct EmployeeLogin {
    user_id: Uuid,
    login: String,
    active: bool,
}

/// Вход сотрудника для карточки владельца; у других — `None`.
async fn login_of(
    conn: &mut PgConnection,
    owner: bool,
    employee_id: Uuid,
) -> AppResult<Option<EmployeeLogin>> {
    if !owner {
        return Ok(None);
    }
    Ok(sqlx::query_as!(
        EmployeeLogin,
        "select id as user_id, login, active from users where employee_id = $1",
        employee_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

async fn list_employees(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<EmployeeOut>>> {
    let owner = user.is_owner();
    let rows = sqlx::query!(
        r#"select e.id, e.full_name, e.is_cashier, e.is_master, e.active, e.phone,
                  e.pin_hash is not null as "has_pin!",
                  u.id as "user_id?", u.login as "login?", u.active as "login_active?"
           from employees e left join users u on u.employee_id = e.id
           where e.branch_id = $1 order by e.full_name"#,
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
        login: match (owner, r.user_id, r.login, r.login_active) {
            (true, Some(user_id), Some(login), Some(active)) => Some(EmployeeLogin {
                user_id,
                login,
                active,
            }),
            _ => None,
        },
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
    // Заводить и править сотрудников — по праву «Сотрудники» (задача 003 Н8, ADR-151).
    {
        let mut conn = state.pool.acquire().await?;
        crate::auth::require_permission(&mut conn, &ctx.user, "employees").await?;
    }
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
        login: None,
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
    crate::auth::require_permission(&mut tx, &ctx.user, "employees").await?;
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
    // Отключение сотрудника выключает и его вход в программу, а вход заводит и меняет только
    // владелец (задача 005, ADR-170): иначе любой с правом «Сотрудники» выключал бы коллегу.
    if req.active == Some(false) && prev.active && !ctx.user.is_owner() {
        let has_login = sqlx::query_scalar!(
            r#"select exists(select 1 from users where employee_id = $1 and active) as "e!""#,
            id
        )
        .fetch_one(&mut *tx)
        .await?;
        if has_login {
            return Err(AppError::ForbiddenWith(
                "сотрудника со входом в программу отключает владелец: вместе с ним выключается вход"
                    .into(),
            ));
        }
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
        // Отключили сотрудника — его вход в программу тоже, сессии сброшены (ADR-170).
        crate::api::auth::disable_employee_login(&mut tx, &ctx, id).await?;
    } else if new_key.is_none() || new_key != crate::api::telegram::phone_key(&prev.phone) {
        crate::api::bot_staff::drop_for_employee(&mut tx, &ctx, id, "телефон в карточке изменён")
            .await?;
    }
    // Имя входа сотрудника — как в карточке: журнал и списки показывают одно имя (ADR-170).
    if r.full_name != prev.full_name {
        sqlx::query!(
            "update users set full_name = $2 where employee_id = $1",
            id,
            r.full_name
        )
        .execute(&mut *tx)
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
        login: login_of(&mut tx, ctx.user.is_owner(), id).await?,
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
        login: None,
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
    /// Категория масла, с которым услуга бесплатна (ADR-104); без неё флажок не действует.
    oil_category_id: Option<Uuid>,
    active: bool,
    /// Строка книжки услуги (SPEC-34): что она меняет; «прочее» — без напоминания.
    book_slot: String,
    /// Интервал, заданный владельцем; пусто — по строке книжки.
    book_interval_km: Option<i32>,
    book_interval_days: Option<i32>,
    /// Интервал для кассы: заданный или по строке книжки (у «прочего» — пусто).
    interval_km: Option<i32>,
    interval_days: Option<i32>,
    /// Доливка — продаётся на вынос, без клиента и книжки (ADR-118).
    takeaway: bool,
}

impl ServiceOut {
    /// Интервал для кассы — заданный владельцем или по умолчанию строки книжки.
    fn with_default_interval(mut self) -> Self {
        let (km, days) = service_interval(
            &self.book_slot,
            self.book_interval_km,
            self.book_interval_days,
        );
        self.interval_km = km;
        self.interval_days = days;
        self
    }
}

/// Интервал услуги для кассы (SPEC-34 §2): свой, иначе по строке книжки.
pub(crate) fn service_interval(
    slot: &str,
    km: Option<i32>,
    days: Option<i32>,
) -> (Option<i32>, Option<i32>) {
    match km.zip(days) {
        Some((k, d)) => (Some(k), Some(d)),
        None => crate::api::oil_book::slot_default(slot)
            .map_or((None, None), |(k, d)| (Some(k), Some(d))),
    }
}

/// Строка книжки и интервал услуги из запроса: строка известна, интервал — оба числа или ни
/// одного.
fn check_service_book(slot: Option<&str>, km: Option<i32>, days: Option<i32>) -> AppResult<()> {
    if slot.is_some_and(|s| !crate::api::oil_book::is_slot(s)) {
        return Err(invalid("неизвестная строка книжки"));
    }
    match (km, days) {
        (Some(k), Some(d)) => crate::api::oil_book::check_interval(k, d),
        (None, None) => Ok(()),
        _ => Err(invalid("интервал: укажите и км, и дни")),
    }
}

/// Категория масла по названию услуги (ADR-104), как в миграции 0059: «АКПП», «короб»,
/// «трансмис», а также мост, редуктор и раздатка (ADR-131, как в миграции 0071) —
/// трансмиссионная (строка книжки «Масло в коробке» или «трансмис» в названии, первая по времени
/// заведения), иначе моторная (без такого названия — единственная нетрансмиссионная категория
/// масла).
pub(crate) async fn guess_oil_category(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> AppResult<Option<Uuid>> {
    let low = name.to_lowercase();
    let gear = ["акпп", "короб", "трансмис"]
        .iter()
        .any(|w| low.contains(w))
        || crate::api::oil_book::axle_or_transfer_by_name(&low);
    let cats = sqlx::query!(
        "select id, name, book_slot from categories where kind = 'oil' order by created_at, id"
    )
    .fetch_all(&mut *conn)
    .await?;
    let is_gear = |slot: Option<&str>, n: &str| {
        slot == Some("gear_oil") || n.to_lowercase().contains("трансмис")
    };
    if gear {
        return Ok(cats
            .iter()
            .find(|c| is_gear(c.book_slot.as_deref(), &c.name))
            .map(|c| c.id));
    }
    if let Some(c) = cats
        .iter()
        .find(|c| c.name.to_lowercase().contains("мотор"))
    {
        return Ok(Some(c.id));
    }
    let other: Vec<_> = cats
        .iter()
        .filter(|c| !is_gear(c.book_slot.as_deref(), &c.name))
        .collect();
    Ok(match other.as_slice() {
        [one] => Some(one.id),
        _ => None,
    })
}

/// Категория масла услуги — категория вида «масло».
async fn check_oil_category(conn: &mut sqlx::PgConnection, id: Uuid) -> AppResult<()> {
    let ok = sqlx::query_scalar!(
        r#"select exists (select 1 from categories where id = $1 and kind = 'oil') as "e!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if ok {
        Ok(())
    } else {
        Err(invalid("категория масла не найдена"))
    }
}

async fn list_services(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> AppResult<Json<Vec<ServiceOut>>> {
    let rows = sqlx::query_as!(
        ServiceOut,
        "select id, name, price_tyiyn, master_fee_tyiyn, free_with_oil, oil_category_id, active, book_slot, book_interval_km, book_interval_days, book_interval_km as interval_km, book_interval_days as interval_days, takeaway from services order by name"
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(ServiceOut::with_default_interval)
            .collect(),
    ))
}

#[derive(Deserialize)]
struct ServiceReq {
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: Option<i64>,
    #[serde(default)]
    free_with_oil: bool,
    /// Категория масла (ADR-104); без неё у «бесплатно с нашим маслом» — по названию услуги.
    #[serde(default)]
    oil_category_id: Option<Uuid>,
    /// Строка книжки (SPEC-34); не прислана — по названию услуги.
    #[serde(default)]
    book_slot: Option<String>,
    #[serde(default)]
    book_interval_km: Option<i32>,
    #[serde(default)]
    book_interval_days: Option<i32>,
    /// Доливка на вынос (ADR-118); не прислан — по названию («долив»).
    #[serde(default)]
    takeaway: Option<bool>,
}

/// Услуга-доливка по названию (ADR-118, как в миграции 0070): слово «долив».
pub(crate) fn takeaway_by_name(name: &str) -> bool {
    name.to_lowercase().contains("долив")
}

async fn create_service(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ServiceReq>,
) -> AppResult<Json<ServiceOut>> {
    // Заводить и править услуги — по праву «Услуги» (задача 003 Н8, ADR-151).
    {
        let mut conn = state.pool.acquire().await?;
        crate::auth::require_permission(&mut conn, &ctx.user, "services").await?;
    }
    let name = required(&req.name, "название")?;
    // Ставка мастеру по умолчанию — 30 сом за каждую услугу (ADR-112, замечание заказчика
    // 26.10.2026); владелец меняет её в карточке услуги.
    let fee = match req.master_fee_tyiyn {
        Some(f) => f,
        None => {
            let mut c = state.pool.acquire().await?;
            crate::api::sales::oil_change_fee(&mut c, ctx.user.branch_id).await?
        }
    };
    if req.price_tyiyn < 0 || fee < 0 {
        return Err(invalid("суммы не отрицательны"));
    }
    ops::check_amount(req.price_tyiyn)?;
    ops::check_amount(fee)?;
    // Ставку мастера и «бесплатно с нашим маслом» задаёт владелец: это цена и оплата труда.
    // Ставка по умолчанию (не задана в запросе) — правило владельца, её может поставить и администратор.
    if (req.master_fee_tyiyn.is_some_and(|f| f != 0)
        || req.free_with_oil
        || req.oil_category_id.is_some()
        || req.book_slot.is_some()
        || req.book_interval_km.is_some()
        || req.book_interval_days.is_some()
        || req.takeaway.is_some_and(|t| t != takeaway_by_name(&name)))
        && !ctx.user.is_owner()
    {
        return Err(AppError::Forbidden);
    }
    // «Цену новой услуги задаёт только владелец» (настройка «Продажи», задача 003): цену
    // ставит владелец или администратор с правом «Цены товаров и услуг».
    if !ctx.user.is_owner() && req.price_tyiyn != 0 {
        let mut c = state.pool.acquire().await?;
        if sales_settings(&mut c, ctx.user.branch_id)
            .await?
            .service_price_owner_only
            && !crate::auth::has_permission(&mut c, &ctx.user, "prices").await?
        {
            return Err(AppError::ForbiddenWith(
                "цену новой услуги задаёт владелец: заведите услугу с ценой 0".into(),
            ));
        }
    }
    let takeaway = req.takeaway.unwrap_or_else(|| takeaway_by_name(&name));
    check_service_book(
        req.book_slot.as_deref(),
        req.book_interval_km,
        req.book_interval_days,
    )?;
    let book_slot = req
        .book_slot
        .clone()
        .unwrap_or_else(|| crate::api::oil_book::service_slot_by_name(&name).to_string());
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    let oil_category_id = match req.oil_category_id {
        Some(c) => {
            check_oil_category(&mut tx, c).await?;
            Some(c)
        }
        None if req.free_with_oil => guess_oil_category(&mut tx, &name).await?,
        None => None,
    };
    sqlx::query!(
        r#"insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, free_with_oil, oil_category_id,
                                 book_slot, book_interval_km, book_interval_days, takeaway)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        id,
        ctx.user.branch_id,
        name,
        req.price_tyiyn,
        fee,
        req.free_with_oil,
        oil_category_id,
        book_slot,
        req.book_interval_km,
        req.book_interval_days,
        takeaway
    )
    .execute(&mut *tx)
    .await?;
    // Услугу завёл администратор — отдельным событием: владелец узнаёт о каждой с ценой
    // (задача 003, как перелив администратора, ADR-082).
    let action = if ctx.user.is_owner() {
        "service.create"
    } else {
        "service.create_admin"
    };
    ops::audit(
        &mut tx,
        &ctx,
        action,
        "service",
        Some(id),
        json!({ "name": name, "price": req.price_tyiyn, "fee": fee, "free_with_oil": req.free_with_oil, "oil_category_id": oil_category_id,
                "book_slot": book_slot, "book_km": req.book_interval_km, "book_days": req.book_interval_days, "takeaway": takeaway }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(
        ServiceOut {
            id,
            name,
            price_tyiyn: req.price_tyiyn,
            master_fee_tyiyn: fee,
            free_with_oil: req.free_with_oil,
            oil_category_id,
            active: true,
            book_slot,
            book_interval_km: req.book_interval_km,
            book_interval_days: req.book_interval_days,
            interval_km: None,
            interval_days: None,
            takeaway,
        }
        .with_default_interval(),
    ))
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
/// мастера — по умолчанию, 30 сом (ADR-112), владелец меняет её в карточке услуги.
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
    let existing = sqlx::query_scalar!(r#"select lower(btrim(name)) as "name!" from services"#)
        .fetch_all(&mut *conn)
        .await?;
    let mut seen: std::collections::HashSet<String> = existing.into_iter().collect();
    // Ставка мастеру по умолчанию — 30 сом за каждую услугу (ADR-112).
    let default_fee = crate::api::sales::oil_change_fee(&mut *conn, ctx.user.branch_id).await?;
    let mut created = 0i64;
    let mut skipped = Vec::new();
    for (name, price) in items {
        if !seen.insert(name.to_lowercase()) {
            skipped.push(name);
            continue;
        }
        let id = new_id();
        sqlx::query!(
            "insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, book_slot, takeaway) values ($1, $2, $3, $4, $5, $6, $7)",
            id,
            ctx.user.branch_id,
            name,
            price,
            default_fee,
            crate::api::oil_book::service_slot_by_name(&name),
            takeaway_by_name(&name)
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
    /// Категория масла (ADR-104); не прислана — прежняя.
    oil_category_id: Option<Uuid>,
    active: Option<bool>,
    /// Строка книжки и интервал (SPEC-34): меняет владелец. `book_interval_reset` — интервал
    /// снова по строке книжки.
    book_slot: Option<String>,
    book_interval_km: Option<i32>,
    book_interval_days: Option<i32>,
    #[serde(default)]
    book_interval_reset: bool,
    /// Доливка на вынос (ADR-118): меняет владелец.
    takeaway: Option<bool>,
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
    check_service_book(
        req.book_slot.as_deref(),
        req.book_interval_km,
        req.book_interval_days,
    )?;
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "services").await?;
    // Строку книжки и интервал услуги меняет владелец (SPEC-34 §1).
    if !ctx.user.is_owner() {
        let cur = sqlx::query!(
            "select book_slot, book_interval_km, book_interval_days, takeaway from services where id = $1",
            id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
        if req.book_slot.as_ref().is_some_and(|s| *s != cur.book_slot)
            || req.takeaway.is_some_and(|t| t != cur.takeaway)
            || req.book_interval_reset
            || req
                .book_interval_km
                .is_some_and(|k| Some(k) != cur.book_interval_km)
            || req
                .book_interval_days
                .is_some_and(|d| Some(d) != cur.book_interval_days)
        {
            return Err(AppError::Forbidden);
        }
    }
    // Ставку мастера меняет только владелец (решение заказчика, ADR-047), цену — владелец или
    // администратор с правом «Цены товаров и услуг» (ADR-151); форма администратора присылает
    // их без изменений.
    if !ctx.user.is_owner()
        && (req.master_fee_tyiyn.is_some()
            || req.price_tyiyn.is_some()
            || req.free_with_oil.is_some()
            || req.oil_category_id.is_some())
    {
        let cur = sqlx::query!(
            "select price_tyiyn, master_fee_tyiyn, free_with_oil, oil_category_id from services where id = $1",
            id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
        if req.price_tyiyn.is_some_and(|p| p != cur.price_tyiyn) {
            crate::auth::require_permission(&mut tx, &ctx.user, "prices").await?;
        }
        if req
            .master_fee_tyiyn
            .is_some_and(|f| f != cur.master_fee_tyiyn)
            || req.free_with_oil.is_some_and(|f| f != cur.free_with_oil)
            || req
                .oil_category_id
                .is_some_and(|c| Some(c) != cur.oil_category_id)
        {
            return Err(AppError::Forbidden);
        }
    }
    if let Some(c) = req.oil_category_id {
        check_oil_category(&mut tx, c).await?;
    }
    let mut out = sqlx::query_as!(
        ServiceOut,
        r#"update services set name = coalesce($2, name), price_tyiyn = coalesce($3, price_tyiyn),
             master_fee_tyiyn = coalesce($4, master_fee_tyiyn), active = coalesce($5, active),
             free_with_oil = coalesce($6, free_with_oil),
             oil_category_id = coalesce($7, oil_category_id),
             book_slot = coalesce($8, book_slot),
             book_interval_km = case when $11 then null else coalesce($9, book_interval_km) end,
             book_interval_days = case when $11 then null else coalesce($10, book_interval_days) end,
             takeaway = coalesce($12, takeaway)
           where id = $1
           returning id, name, price_tyiyn, master_fee_tyiyn, free_with_oil, oil_category_id, active, book_slot, book_interval_km, book_interval_days, book_interval_km as interval_km, book_interval_days as interval_days, takeaway"#,
        id,
        name,
        req.price_tyiyn,
        req.master_fee_tyiyn,
        req.active,
        req.free_with_oil,
        req.oil_category_id,
        req.book_slot,
        req.book_interval_km,
        req.book_interval_days,
        req.book_interval_reset,
        req.takeaway
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Флажок поставили без категории — категория по названию услуги (ADR-104).
    if out.free_with_oil && out.oil_category_id.is_none() {
        out.oil_category_id = guess_oil_category(&mut tx, &out.name).await?;
        sqlx::query!(
            "update services set oil_category_id = $2 where id = $1",
            id,
            out.oil_category_id
        )
        .execute(&mut *tx)
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "service.update",
        "service",
        Some(id),
        json!({ "price": out.price_tyiyn, "fee": out.master_fee_tyiyn, "free_with_oil": out.free_with_oil, "oil_category_id": out.oil_category_id, "active": out.active,
                "book_slot": out.book_slot, "book_km": out.book_interval_km, "book_days": out.book_interval_days, "takeaway": out.takeaway }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out.with_default_interval()))
}

// ---------- Поставщики ----------

/// Поставщик с реквизитами (задача 003 Н10): вид, ИНН, адрес, банк и счёт, срок оплаты.
/// Имя, телефон, комментарий и активность — в `suppliers` (за ним приход) и в `parties`,
/// реквизиты — в `parties` (там же их видит акт сверки).
#[derive(Serialize, Deserialize)]
struct SupplierOut {
    id: Uuid,
    name: String,
    phone: String,
    comment: String,
    active: bool,
    /// `company` — фирма, `person` — человек.
    kind: String,
    inn: String,
    address: String,
    /// Банк (название, БИК) и расчётный счёт.
    bank: String,
    bank_account: String,
    /// Срок оплаты поставщику в днях; пусто — не задан.
    due_days: Option<i32>,
}

/// Срок оплаты — не больше 10 лет, как у клиентов (SPEC-10).
const MAX_SUPPLIER_DUE_DAYS: i32 = 3650;

async fn list_suppliers(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> AppResult<Json<Vec<SupplierOut>>> {
    let rows = sqlx::query_as!(
        SupplierOut,
        r#"select s.id, s.name, s.phone, s.comment, s.active,
                  coalesce(p.kind, 'company') as "kind!", coalesce(p.inn, '') as "inn!",
                  coalesce(p.address, '') as "address!", coalesce(p.bank, '') as "bank!",
                  coalesce(p.bank_account, '') as "bank_account!", p.due_days as "due_days?"
           from suppliers s left join parties p on p.id = s.id
           order by s.name"#
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Реквизиты из формы: вид, ИНН, адрес, банк, счёт. Проверка — здесь, а не ограничением базы:
/// иначе человек увидит «внутреннюю ошибку».
struct SupplierDetails {
    kind: Option<String>,
    inn: Option<String>,
    address: Option<String>,
    bank: Option<String>,
    bank_account: Option<String>,
}

fn supplier_details(
    kind: Option<String>,
    inn: Option<String>,
    address: Option<String>,
    bank: Option<String>,
    bank_account: Option<String>,
    due_days: Option<i32>,
) -> AppResult<SupplierDetails> {
    if kind
        .as_deref()
        .is_some_and(|k| !matches!(k, "company" | "person"))
    {
        return Err(invalid("вид поставщика: фирма или человек"));
    }
    if due_days.is_some_and(|d| !(0..=MAX_SUPPLIER_DUE_DAYS).contains(&d)) {
        return Err(invalid("срок оплаты — от 0 до 3650 дней"));
    }
    let inn = inn.map(|s| s.trim().to_string());
    if inn
        .as_deref()
        .is_some_and(|i| !i.is_empty() && (i.len() > 20 || !i.bytes().all(|b| b.is_ascii_digit())))
    {
        return Err(invalid("ИНН — только цифры, не больше 20"));
    }
    let text = |v: Option<String>, what: &str, max: usize| -> AppResult<Option<String>> {
        let v = v.map(|s| s.trim().to_string());
        if v.as_deref().is_some_and(|t| t.chars().count() > max) {
            return Err(invalid(format!("{what} — не длиннее {max} знаков")));
        }
        Ok(v)
    };
    Ok(SupplierDetails {
        kind,
        inn,
        address: text(address, "адрес", 300)?,
        bank: text(bank, "банк", 200)?,
        bank_account: text(bank_account, "счёт", 60)?,
    })
}

async fn supplier_out(conn: &mut PgConnection, id: Uuid) -> AppResult<SupplierOut> {
    sqlx::query_as!(
        SupplierOut,
        r#"select s.id, s.name, s.phone, s.comment, s.active,
                  coalesce(p.kind, 'company') as "kind!", coalesce(p.inn, '') as "inn!",
                  coalesce(p.address, '') as "address!", coalesce(p.bank, '') as "bank!",
                  coalesce(p.bank_account, '') as "bank_account!", p.due_days as "due_days?"
           from suppliers s left join parties p on p.id = s.id
           where s.id = $1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

/// Поля поставщика для журнала: было → стало (I5).
fn supplier_fields(s: &SupplierOut) -> Value {
    json!({
        "name": s.name, "phone": s.phone, "comment": s.comment, "active": s.active,
        "kind": s.kind, "inn": s.inn, "address": s.address, "bank": s.bank,
        "bank_account": s.bank_account, "due_days": s.due_days,
    })
}

#[derive(Deserialize)]
struct SupplierReq {
    name: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    comment: String,
    kind: Option<String>,
    inn: Option<String>,
    address: Option<String>,
    bank: Option<String>,
    bank_account: Option<String>,
    due_days: Option<i32>,
}

async fn create_supplier(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SupplierReq>,
) -> AppResult<Json<SupplierOut>> {
    let name = required(&req.name, "название")?;
    let d = supplier_details(
        req.kind,
        req.inn,
        req.address,
        req.bank,
        req.bank_account,
        req.due_days,
    )?;
    let id = new_id();
    let (phone, comment) = (req.phone.trim().to_string(), req.comment.trim().to_string());
    let mut tx = state.pool.begin().await?;
    // Нового поставщика заводят при приходе: право «Приход товара» (ADR-097).
    crate::auth::require_permission(&mut tx, &ctx.user, "receipts").await?;
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
        r#"insert into parties (id, branch_id, role, kind, name, phone, comment, inn, address, bank,
                                bank_account, due_days)
           values ($1, $2, 'supplier', $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        id,
        ctx.user.branch_id,
        d.kind.as_deref().unwrap_or("company"),
        name,
        phone,
        comment,
        d.inn.as_deref().unwrap_or(""),
        d.address.as_deref().unwrap_or(""),
        d.bank.as_deref().unwrap_or(""),
        d.bank_account.as_deref().unwrap_or(""),
        req.due_days
    )
    .execute(&mut *tx)
    .await?;
    let out = supplier_out(&mut tx, id).await?;
    ops::audit(
        &mut tx,
        &ctx,
        "supplier.create",
        "supplier",
        Some(id),
        json!({ "name": name, "fields": supplier_fields(&out) }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct SupplierPatch {
    name: Option<String>,
    phone: Option<String>,
    comment: Option<String>,
    active: Option<bool>,
    kind: Option<String>,
    inn: Option<String>,
    address: Option<String>,
    bank: Option<String>,
    bank_account: Option<String>,
    /// Срок оплаты в днях; без поля — не менять, `clear_due_days` — снять срок.
    due_days: Option<i32>,
    #[serde(default)]
    clear_due_days: bool,
}

async fn update_supplier(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<SupplierPatch>,
) -> AppResult<Json<SupplierOut>> {
    let name = optional_required(req.name, "название")?;
    let d = supplier_details(
        req.kind,
        req.inn,
        req.address,
        req.bank,
        req.bank_account,
        req.due_days,
    )?;
    let mut tx = state.pool.begin().await?;
    // Править поставщика — то же право, что заводить: «Приход товара» (ADR-097, ADR-128);
    // реквизиты и срок оплаты — тоже (задача 003 Н10): это сведения для документов, не деньги.
    crate::auth::require_permission(&mut tx, &ctx.user, "receipts").await?;
    // Поставщик общий для филиалов, без branch_id (отступление записано в ADR-062): журнал
    // правки пишется в филиал того, кто правил.
    sqlx::query_scalar!("select id from suppliers where id = $1 for update", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let before = supplier_out(&mut tx, id).await?;
    sqlx::query!(
        r#"update suppliers set name = coalesce($2, name), phone = coalesce($3, phone),
             comment = coalesce($4, comment), active = coalesce($5, active)
           where id = $1"#,
        id,
        name,
        req.phone.map(|s| s.trim().to_string()),
        req.comment.map(|s| s.trim().to_string()),
        req.active
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"update parties p set name = s.name, phone = s.phone, comment = s.comment, active = s.active,
             kind = coalesce($2, p.kind), inn = coalesce($3, p.inn),
             address = coalesce($4, p.address), bank = coalesce($5, p.bank),
             bank_account = coalesce($6, p.bank_account),
             due_days = case when $8 then null else coalesce($7, p.due_days) end
           from suppliers s
           where p.id = $1 and s.id = p.id"#,
        id,
        d.kind,
        d.inn,
        d.address,
        d.bank,
        d.bank_account,
        req.due_days,
        req.clear_due_days
    )
    .execute(&mut *tx)
    .await?;
    let out = supplier_out(&mut tx, id).await?;
    ops::audit(
        &mut tx,
        &ctx,
        "supplier.update",
        "supplier",
        Some(id),
        json!({
            "name": out.name,
            "active": out.active,
            "changes": ops::changes(&supplier_fields(&before), &supplier_fields(&out)),
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
    /// Кнопка по умолчанию на телефоне: `station` — «На складе», `here` — «Здесь» (задача 006 Н5).
    /// По умолчанию «На складе» — прямая просьба владельца (I21).
    #[serde(default = "station")]
    remote_default: String,
}

fn station() -> String {
    "station".into()
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
            remote_default: station(),
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
    /// Период акта сверки по умолчанию на экране (задача 003 Н9): `all` — всё время (как было),
    /// `month`, `last_month`, `year`.
    #[serde(default)]
    act_period: Option<String>,
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
    let has_owner_signature = sqlx::query_scalar!(
        r#"select exists (select 1 from debt_doc_signatures where branch_id = $1) as "e!""#,
        user.branch_id
    )
    .fetch_one(&state.pool)
    .await?;
    let mut out = json!(docs);
    out["has_owner_signature"] = json!(has_owner_signature);
    Ok(Json(out))
}

/// Подпись руководителя точки в документах о долге (задача 008): видна всем вошедшим точки —
/// она печатается в документах.
async fn get_owner_signature(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<axum::response::Response> {
    let png = sqlx::query_scalar!(
        "select signature_png from debt_doc_signatures where branch_id = $1",
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(crate::api::acceptances::file_response(png, "image/png"))
}

#[derive(Deserialize)]
struct OwnerSignatureReq {
    signature_png: String,
}

/// Подпись руководителя ставит и меняет владелец.
async fn put_owner_signature(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OwnerSignatureReq>,
) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let png = crate::api::acceptances::signature_png_or(&req.signature_png, "нужна подпись")?;
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into debt_doc_signatures (branch_id, signature_png, updated_by)
           values ($1, $2, $3)
           on conflict (branch_id) do update
             set signature_png = excluded.signature_png, updated_at = now(),
                 updated_by = excluded.updated_by"#,
        ctx.user.branch_id,
        png,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.debt_docs_signature",
        "settings",
        None,
        json!({ "set": true }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "has_owner_signature": true })))
}

async fn delete_owner_signature(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "delete from debt_doc_signatures where branch_id = $1",
        ctx.user.branch_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.debt_docs_signature",
        "settings",
        None,
        json!({ "set": false }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "has_owner_signature": false })))
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
    if req
        .act_period
        .as_deref()
        .is_some_and(|p| !matches!(p, "all" | "month" | "last_month" | "year"))
    {
        return Err(invalid(
            "период акта: всё время, месяц, прошлый месяц или год",
        ));
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
        json!({ "seller": req.seller.name, "act_period": req.act_period }),
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
    if !matches!(req.remote_default.as_str(), "station" | "here") {
        return Err(invalid("кнопка по умолчанию — «Здесь» или «На складе»"));
    }
    let value = json!(req);
    let mut tx = state.pool.begin().await?;
    // Этикетки — часть «Товары и штрихкоды» (ADR-128).
    crate::auth::require_permission(&mut tx, &ctx.user, "products").await?;
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

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
struct SalesSettings {
    /// Ставка мастера за чек с заменой (ADR-027).
    #[serde(default = "default_oil_change_fee")]
    oil_change_master_fee_tyiyn: i64,
    /// «Цену новой услуги задаёт только владелец» (задача 003): администратор без права «Цены
    /// товаров и услуг» заводит услугу с ценой 0, цену ставит владелец. По умолчанию — нет.
    #[serde(default)]
    service_price_owner_only: bool,
}

fn default_oil_change_fee() -> i64 {
    crate::api::sales::DEFAULT_OIL_CHANGE_FEE
}

impl Default for SalesSettings {
    fn default() -> Self {
        Self {
            oil_change_master_fee_tyiyn: default_oil_change_fee(),
            service_price_owner_only: false,
        }
    }
}

/// Правка настроек продаж: неприсланное поле остаётся прежним.
#[derive(Deserialize)]
struct SalesSettingsPatch {
    #[serde(default)]
    oil_change_master_fee_tyiyn: Option<i64>,
    #[serde(default)]
    service_price_owner_only: Option<bool>,
}

async fn sales_settings(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<SalesSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'sales'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| serde_json::from_value::<SalesSettings>(v).ok())
            .unwrap_or_default(),
    )
}

async fn get_sales_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Value>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(json!(
        sales_settings(&mut conn, user.branch_id).await?
    )))
}

async fn put_sales_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SalesSettingsPatch>,
) -> AppResult<Json<Value>> {
    // Ставку оплаты задаёт только владелец (ADR-019).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req
        .oil_change_master_fee_tyiyn
        .is_some_and(|f| !(0..=100_000).contains(&f))
    {
        return Err(invalid("ставка за замену от 0 до 1000 сом"));
    }
    let mut tx = state.pool.begin().await?;
    let before = sales_settings(&mut tx, ctx.user.branch_id).await?;
    let after = SalesSettings {
        oil_change_master_fee_tyiyn: req
            .oil_change_master_fee_tyiyn
            .unwrap_or(before.oil_change_master_fee_tyiyn),
        service_price_owner_only: req
            .service_price_owner_only
            .unwrap_or(before.service_price_owner_only),
    };
    let value = json!(after);
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'sales', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != after {
        ops::audit(
            &mut tx,
            &ctx,
            "settings.sales",
            "settings",
            None,
            json!({ "before": before, "after": after }),
        )
        .await?;
    }
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
    /// Цена последней поставки за единицу цены: штуку, канистру, кг или м.
    last_price_tyiyn: i64,
    last_at: DateTime<Utc>,
}

async fn get_supplier(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<SupplierOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(supplier_out(&mut conn, id).await?))
}

/// Что поставщик привозил: по товарам, с количеством, суммой и последней ценой.
/// Сторно накладных входит со своим знаком, поэтому отменённая поставка сама себя гасит.
async fn supplier_supplies(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<SupplyRow>>> {
    // Раздел «Поставщики» — по праву (задача 003 Н8, ADR-151).
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "view_suppliers").await?;
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
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| {
        // За единицу цены: штуку, канистру, кг или м (ADR-178).
        let per = crate::api::catalog::price_unit(&r.unit, r.container_ml);
        SupplyRow {
            product_id: r.product_id,
            name: r.name,
            unit: r.unit,
            container_ml: r.container_ml,
            receipts: r.receipts,
            qty: r.qty,
            amount_tyiyn: r.amount_tyiyn,
            last_price_tyiyn: if r.last_qty > 0 {
                div_round(
                    i128::from(r.last_cost) * i128::from(per),
                    i128::from(r.last_qty),
                )
                .unwrap_or(0)
            } else {
                0
            },
            last_at: r.last_at,
        }
    })
    .collect();
    Ok(Json(rows))
}
