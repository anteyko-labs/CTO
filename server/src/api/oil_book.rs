//! Масляная книжка: замены по машинам клиента и следующая замена (SPEC-16).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::NaiveDate;
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
        .route("/vehicles/{id}/oil-book", routing::get(vehicle_book))
        .route("/vehicles/{id}/oil-settings", routing::patch(settings))
        .route("/vehicles/{id}/oil-changes", routing::post(manual))
        .route("/oil-changes/{id}", routing::patch(edit))
        .route("/parties/{id}/oil-book", routing::get(party_book))
}

#[derive(Serialize)]
pub struct OilRecord {
    pub id: Uuid,
    pub change_date: NaiveDate,
    pub mileage_km: Option<i32>,
    pub oil_text: String,
    pub filter_text: String,
    pub comment: String,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
}

#[derive(Serialize)]
pub struct VehicleBook {
    pub vehicle_id: Uuid,
    pub plate: String,
    pub brand: String,
    pub model: String,
    pub interval_km: i32,
    pub interval_days: i32,
    /// Следующая замена: последний известный пробег + интервал, дата последней замены + дни.
    pub next_km: Option<i32>,
    pub next_date: Option<NaiveDate>,
    pub records: Vec<OilRecord>,
}

/// Количество строки для книжки: «1 кан. × 4 л», «1,5 л», «2 шт».
fn qty_text(kind: &str, qty: i64, container_ml: Option<i64>) -> String {
    let liters = |ml: i64| {
        let frac = format!("{:03}", ml % 1000);
        let frac = frac.trim_end_matches('0');
        if frac.is_empty() {
            format!("{} л", ml / 1000)
        } else {
            format!("{},{} л", ml / 1000, frac)
        }
    };
    match (kind, container_ml) {
        ("container", Some(c)) => format!("{qty} кан. × {}", liters(c)),
        ("pour", _) => liters(qty),
        _ => format!("{qty} шт"),
    }
}

/// Запись из чека «в сервис» с машиной: что залили и что поставили. Вызывается в транзакции чека.
pub async fn record_from_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    party_id: Uuid,
    vehicle_id: Uuid,
    change_date: NaiveDate,
    mileage_km: Option<i32>,
) -> AppResult<()> {
    let lines = sqlx::query!(
        r#"select p.name, c.kind as cat, l.kind, l.qty, p.container_ml
           from sale_lines l
           join products p on p.id = l.product_id
           join categories c on c.id = p.category_id
           where l.sale_id = $1 and not l.gift and c.kind in ('oil', 'filter')
           order by l.line_no"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Масло клиента: товара-масла в чеке нет, но услуга замены масла — это замена (SPEC-16).
    let own_oil = if lines.iter().any(|l| l.cat == "oil") {
        None
    } else {
        sqlx::query_scalar!(
            r#"select s.name from sale_lines l join services s on s.id = l.service_id
               where l.sale_id = $1 and s.name ilike '%масл%' order by l.line_no limit 1"#,
            sale_id
        )
        .fetch_optional(&mut *conn)
        .await?
    };
    if lines.is_empty() && own_oil.is_none() {
        return Ok(());
    }
    let join = |cat: &str| {
        lines
            .iter()
            .filter(|l| l.cat == cat)
            .map(|l| format!("{} · {}", l.name, qty_text(&l.kind, l.qty, l.container_ml)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    sqlx::query!(
        r#"insert into oil_changes (id, branch_id, vehicle_id, party_id, sale_id, change_date,
                                    mileage_km, oil_text, filter_text, user_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
           on conflict (sale_id) where sale_id is not null do nothing"#,
        new_id(),
        ctx.user.branch_id,
        vehicle_id,
        party_id,
        sale_id,
        change_date,
        mileage_km,
        own_oil.unwrap_or_else(|| join("oil")),
        join("filter"),
        ctx.user.id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Книжки машин: одной машины или всех машин клиента. Полностью возвращённые чеки скрыты.
pub async fn books(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
) -> AppResult<Vec<VehicleBook>> {
    let vehicles = sqlx::query!(
        r#"select id, plate, brand, model, interval_km, interval_days
           from party_vehicles
           where branch_id = $1 and ($2::uuid is null or party_id = $2) and ($3::uuid is null or id = $3)
             and (active or $3::uuid is not null)
           order by plate"#,
        branch_id,
        party_id,
        vehicle_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = vehicles.iter().map(|v| v.id).collect();
    let records = sqlx::query!(
        r#"select o.id, o.vehicle_id, o.change_date, o.mileage_km, o.oil_text, o.filter_text, o.comment,
                  o.sale_id, s.number as "sale_number?"
           from oil_changes o
           left join sales s on s.id = o.sale_id
           where o.vehicle_id = any($1)
             and (s.id is null or coalesce((select sum(r.total_tyiyn) from sales r where r.reversal_of = s.id), 0) <> -s.total_tyiyn)
           order by o.change_date desc, o.created_at desc"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(vehicles.len());
    for v in vehicles {
        let recs: Vec<OilRecord> = records
            .iter()
            .filter(|r| r.vehicle_id == v.id)
            .map(|r| OilRecord {
                id: r.id,
                change_date: r.change_date,
                mileage_km: r.mileage_km,
                oil_text: r.oil_text.clone(),
                filter_text: r.filter_text.clone(),
                comment: r.comment.clone(),
                sale_id: r.sale_id,
                sale_number: r.sale_number,
            })
            .collect();
        let next_km = recs
            .iter()
            .find_map(|r| r.mileage_km)
            .map(|m| m.saturating_add(v.interval_km));
        let next_date = recs.first().and_then(|r| {
            r.change_date.checked_add_days(chrono::Days::new(
                u64::try_from(v.interval_days).unwrap_or(31),
            ))
        });
        out.push(VehicleBook {
            vehicle_id: v.id,
            plate: v.plate,
            brand: v.brand,
            model: v.model,
            interval_km: v.interval_km,
            interval_days: v.interval_days,
            next_km,
            next_date,
            records: recs,
        });
    }
    Ok(out)
}

async fn vehicle_book(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<VehicleBook>> {
    let mut conn = state.pool.acquire().await?;
    books(&mut conn, user.branch_id, None, Some(id))
        .await?
        .pop()
        .map(Json)
        .ok_or(AppError::NotFound)
}

async fn party_book(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<VehicleBook>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(
        books(&mut conn, user.branch_id, Some(id), None).await?,
    ))
}

#[derive(Deserialize)]
struct SettingsReq {
    interval_km: i32,
    interval_days: i32,
}

async fn settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<SettingsReq>,
) -> AppResult<Json<VehicleBook>> {
    if !(500..=100_000).contains(&req.interval_km) || !(1..=730).contains(&req.interval_days) {
        return Err(invalid("интервал: от 500 до 100 000 км и от 1 до 730 дней"));
    }
    let mut tx = state.pool.begin().await?;
    let found = sqlx::query!(
        "update party_vehicles set interval_km = $3, interval_days = $4 where id = $1 and branch_id = $2 returning plate",
        id,
        ctx.user.branch_id,
        req.interval_km,
        req.interval_days
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "vehicle.oil_interval",
        "vehicle",
        Some(id),
        json!({ "plate": found.plate, "km": req.interval_km, "days": req.interval_days }),
    )
    .await?;
    let out = books(&mut tx, ctx.user.branch_id, None, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ManualReq {
    change_date: NaiveDate,
    mileage_km: Option<i32>,
    oil_text: String,
    #[serde(default)]
    filter_text: String,
    #[serde(default)]
    comment: String,
}

fn check_mileage(m: Option<i32>) -> AppResult<()> {
    if m.is_some_and(|m| !(0..=5_000_000).contains(&m)) {
        return Err(invalid("пробег от 0 до 5 000 000 км"));
    }
    Ok(())
}

/// Запись из тетради: замены до появления системы.
async fn manual(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ManualReq>,
) -> AppResult<Json<VehicleBook>> {
    check_mileage(req.mileage_km)?;
    if req.oil_text.trim().is_empty() {
        return Err(invalid("укажите, какое масло залили"));
    }
    let mut tx = state.pool.begin().await?;
    let party_id = sqlx::query_scalar!(
        "select party_id from party_vehicles where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km,
                                    oil_text, filter_text, comment, user_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        rid,
        ctx.user.branch_id,
        id,
        party_id,
        req.change_date,
        req.mileage_km,
        req.oil_text.trim(),
        req.filter_text.trim(),
        req.comment.trim(),
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "vehicle.oil_manual",
        "vehicle",
        Some(id),
        json!({ "record": rid, "date": req.change_date, "mileage": req.mileage_km }),
    )
    .await?;
    let out = books(&mut tx, ctx.user.branch_id, None, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct EditReq {
    mileage_km: Option<i32>,
    comment: Option<String>,
}

/// Поправить пробег или комментарий записи: книжка — справка, а не учёт денег (SPEC-16).
async fn edit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<EditReq>,
) -> AppResult<Json<VehicleBook>> {
    check_mileage(req.mileage_km)?;
    let mut tx = state.pool.begin().await?;
    let r = sqlx::query!(
        r#"update oil_changes set mileage_km = coalesce($3, mileage_km),
             comment = coalesce($4, comment), updated_at = now()
           where id = $1 and branch_id = $2 returning vehicle_id"#,
        id,
        ctx.user.branch_id,
        req.mileage_km,
        req.comment.as_deref().map(str::trim)
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "vehicle.oil_edit",
        "vehicle",
        Some(r.vehicle_id),
        json!({ "record": id, "mileage": req.mileage_km }),
    )
    .await?;
    let out = books(&mut tx, ctx.user.branch_id, None, Some(r.vehicle_id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}
