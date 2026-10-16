//! Масляная книжка: замены по машинам клиента и следующая замена (SPEC-16).

use std::collections::HashMap;

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
        .route("/vehicles/{id}/oil-book", routing::get(vehicle_book))
        .route("/vehicles/{id}/oil-settings", routing::patch(settings))
        .route("/vehicles/{id}/oil-changes", routing::post(manual))
        .route("/oil-changes/{id}", routing::patch(edit))
        .route("/parties/{id}/oil-book", routing::get(party_book))
}

/// Строки книжки (SPEC-27): у каждой замены — масло, фильтры по видам и прочие работы.
pub const SLOTS: [&str; 6] = [
    "oil",
    "oil_filter",
    "air_filter",
    "cabin_filter",
    "fuel_filter",
    "other",
];

pub fn is_slot(slot: &str) -> bool {
    SLOTS.contains(&slot)
}

/// Название строки книжки для экрана, бота и печати.
pub fn slot_label(slot: &str) -> &'static str {
    match slot {
        "oil" => "Масло двигателя",
        "oil_filter" => "Фильтр масляный",
        "air_filter" => "Фильтр воздушный",
        "cabin_filter" => "Фильтр салонный",
        "fuel_filter" => "Фильтр топливный",
        _ => "Прочие работы",
    }
}

/// Интервал строки, если его не задали ни у категории, ни у машины: масло и масляный фильтр —
/// 8 000 км или 31 день (решение заказчика), воздушный и салонный — 15 000 км или год,
/// топливный — 30 000 км или два года. У прочих работ интервала по умолчанию нет.
pub fn slot_default(slot: &str) -> Option<(i32, i32)> {
    match slot {
        "oil" | "oil_filter" => Some((8000, 31)),
        "air_filter" | "cabin_filter" => Some((15_000, 365)),
        "fuel_filter" => Some((30_000, 730)),
        _ => None,
    }
}

/// Строка книжки товара: по категории, а у фильтров — по типу фильтра товара, если он указан
/// (одна категория «Фильтры» на все виды).
pub fn product_slot(category_slot: &str, filter_type: Option<&str>) -> Option<&'static str> {
    let by_category = SLOTS.iter().copied().find(|s| *s == category_slot)?;
    if by_category.ends_with("_filter") {
        let by_type = match filter_type.map(str::trim) {
            Some("масляный") => Some("oil_filter"),
            Some("воздушный") => Some("air_filter"),
            Some("салонный") => Some("cabin_filter"),
            Some("топливный") => Some("fuel_filter"),
            _ => None,
        };
        return Some(by_type.unwrap_or(by_category));
    }
    Some(by_category)
}

/// Строка замены: что сделали по одной строке книжки.
#[derive(Serialize, Clone)]
pub struct BookLine {
    pub slot: String,
    pub text: String,
    #[serde(skip)]
    pub category_id: Option<Uuid>,
}

/// Следующая замена по строке книжки: последняя замена и интервал (машины, категории или
/// по умолчанию).
#[derive(Serialize, Clone)]
pub struct SlotNext {
    pub slot: String,
    pub label: String,
    pub interval_km: Option<i32>,
    pub interval_days: Option<i32>,
    /// Интервал задан у этой машины.
    pub own_interval: bool,
    pub last_date: Option<NaiveDate>,
    pub last_km: Option<i32>,
    pub last_text: Option<String>,
    pub next_km: Option<i32>,
    pub next_date: Option<NaiveDate>,
}

#[derive(Serialize)]
pub struct OilRecord {
    pub id: Uuid,
    pub change_date: NaiveDate,
    pub mileage_km: Option<i32>,
    pub oil_text: String,
    pub filter_text: String,
    /// Строки по книжке: масло, фильтры, прочие работы (SPEC-27).
    pub lines: Vec<BookLine>,
    pub comment: String,
    /// Рекомендовали, клиент отказался: «воздушный фильтр».
    #[serde(default)]
    pub declined: String,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
    /// Итог чека за вычетом возвратов по нему; у записи из тетради — пусто (SPEC-22).
    pub sale_total_tyiyn: Option<i64>,
}

/// Пометка клиента к машине из бота (SPEC-22): точка видит, но не правит.
#[derive(Serialize)]
pub struct ClientNote {
    pub id: Uuid,
    pub text: String,
    pub created_at: DateTime<Utc>,
    pub deleted: bool,
}

#[derive(Serialize)]
pub struct VehicleBook {
    pub vehicle_id: Uuid,
    pub plate: String,
    pub brand: String,
    pub model: String,
    /// Машина в списке клиента; убранная (клиентом в боте или точкой) — `false` (ADR-062).
    pub active: bool,
    /// Интервал и следующая замена масла: последний известный пробег + интервал, дата
    /// последней замены масла + дни.
    pub interval_km: i32,
    pub interval_days: i32,
    pub next_km: Option<i32>,
    pub next_date: Option<NaiveDate>,
    /// Следующая замена по каждой строке книжки (SPEC-27): масло всегда, остальные — если
    /// были замены или задан интервал у машины.
    pub slots: Vec<SlotNext>,
    pub records: Vec<OilRecord>,
    /// Пометки клиента, свежие сверху; удалённые клиентом видит только владелец.
    pub client_notes: Vec<ClientNote>,
}

/// Количество строки для книжки: «1 кан. × 4 л», «1,5 л», «2 шт».
fn qty_text(kind: &str, qty: i64, container_ml: Option<i64>) -> String {
    let liters = crate::domain::qty::liters;
    match (kind, container_ml) {
        ("container", Some(c)) => format!("{qty} кан. × {}", liters(c)),
        ("pour", _) => liters(qty),
        _ => format!("{qty} шт"),
    }
}

/// Запись из чека «в сервис» с машиной: строки книжки по категориям товаров (SPEC-27).
/// Вызывается в транзакции чека.
#[allow(clippy::too_many_arguments)]
pub async fn record_from_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    party_id: Uuid,
    vehicle_id: Uuid,
    change_date: NaiveDate,
    mileage_km: Option<i32>,
    declined: &str,
) -> AppResult<()> {
    let rows = sqlx::query!(
        // Строка книжки категории; не задана — по виду: масла, фильтры (как было).
        r#"select p.name, c.id as category_id,
                  coalesce(c.book_slot, case c.kind when 'oil' then 'oil' when 'filter' then 'oil_filter' else 'none' end) as "book_slot!",
                  p.attrs ->> 'filter_type' as filter_type, l.kind, l.qty, p.container_ml
           from sale_lines l
           join products p on p.id = l.product_id
           join categories c on c.id = p.category_id
           where l.sale_id = $1 and not l.gift
             and coalesce(c.book_slot, case c.kind when 'oil' then 'oil' when 'filter' then 'oil_filter' else 'none' end) <> 'none'
           order by l.line_no"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut lines: Vec<BookLine> = rows
        .iter()
        .filter_map(|r| {
            product_slot(&r.book_slot, r.filter_type.as_deref()).map(|slot| BookLine {
                slot: slot.to_string(),
                text: format!("{} · {}", r.name, qty_text(&r.kind, r.qty, r.container_ml)),
                category_id: Some(r.category_id),
            })
        })
        .collect();
    // Масло клиента: товара-масла в чеке нет, но услуга замены масла — это замена (SPEC-16).
    if !lines.iter().any(|l| l.slot == "oil")
        && let Some(name) = sqlx::query_scalar!(
            r#"select s.name from sale_lines l join services s on s.id = l.service_id
               where l.sale_id = $1 and s.name ilike '%масл%' order by l.line_no limit 1"#,
            sale_id
        )
        .fetch_optional(&mut *conn)
        .await?
    {
        lines.insert(
            0,
            BookLine {
                slot: "oil".into(),
                text: name,
                category_id: None,
            },
        );
    }
    if lines.is_empty() {
        return Ok(());
    }
    let id = new_id();
    let inserted = sqlx::query_scalar!(
        r#"insert into oil_changes (id, branch_id, vehicle_id, party_id, sale_id, change_date,
                                    mileage_km, oil_text, filter_text, user_id, declined)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
           on conflict (sale_id) where sale_id is not null do nothing
           returning id"#,
        id,
        ctx.user.branch_id,
        vehicle_id,
        party_id,
        sale_id,
        change_date,
        mileage_km,
        join_slots(&lines, |s| s == "oil"),
        join_slots(&lines, |s| s.ends_with("_filter")),
        ctx.user.id,
        declined.trim()
    )
    .fetch_optional(&mut *conn)
    .await?;
    if inserted.is_some() {
        insert_lines(conn, ctx.user.branch_id, id, &lines).await?;
    }
    Ok(())
}

/// Прежние поля записи (масло и фильтр строкой) — для истории и старых отчётов.
fn join_slots(lines: &[BookLine], pick: impl Fn(&str) -> bool) -> String {
    lines
        .iter()
        .filter(|l| pick(&l.slot))
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

async fn insert_lines(
    conn: &mut PgConnection,
    branch_id: Uuid,
    oil_change_id: Uuid,
    lines: &[BookLine],
) -> AppResult<()> {
    for (i, l) in lines.iter().enumerate() {
        sqlx::query!(
            r#"insert into oil_change_items (id, branch_id, oil_change_id, slot, text, category_id, position)
               values ($1, $2, $3, $4, $5, $6, $7)"#,
            new_id(),
            branch_id,
            oil_change_id,
            l.slot,
            l.text,
            l.category_id,
            i32::try_from(i).unwrap_or(i32::MAX)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Книжки машин: одной машины или всех машин клиента. Полностью возвращённые чеки скрыты.
pub async fn books(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
) -> AppResult<Vec<VehicleBook>> {
    books_with_removed(conn, branch_id, party_id, vehicle_id, false).await
}

/// То же, а с `removed` — и машины, которые клиент убрал: точка видит их историю и может
/// вернуть машину (ADR-062). Убранные идут после действующих.
pub async fn books_with_removed(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
    removed: bool,
) -> AppResult<Vec<VehicleBook>> {
    let vehicles = sqlx::query!(
        r#"select id, plate, brand, model, active
           from party_vehicles
           where branch_id = $1 and ($2::uuid is null or party_id = $2) and ($3::uuid is null or id = $3)
             and (active or $3::uuid is not null or $4)
           order by active desc, plate"#,
        branch_id,
        party_id,
        vehicle_id,
        removed
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = vehicles.iter().map(|v| v.id).collect();
    let records = sqlx::query!(
        r#"select o.id, o.vehicle_id, o.change_date, o.mileage_km, o.oil_text, o.filter_text, o.comment,
                  o.declined, o.sale_id, s.number as "sale_number?",
                  -- Возврат — чек с минусом и ссылкой на исходный: как в фильтре ниже.
                  (s.total_tyiyn + coalesce((select sum(r.total_tyiyn) from sales r where r.reversal_of = s.id), 0))::bigint
                    as "sale_total_tyiyn?"
           from oil_changes o
           left join sales s on s.id = o.sale_id
           where o.vehicle_id = any($1)
             and (s.id is null or coalesce((select sum(r.total_tyiyn) from sales r where r.reversal_of = s.id), 0) <> -s.total_tyiyn)
           order by o.change_date desc, o.created_at desc"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let record_ids: Vec<Uuid> = records.iter().map(|r| r.id).collect();
    let items = sqlx::query!(
        r#"select oil_change_id, slot, text, category_id from oil_change_items
           where oil_change_id = any($1) order by oil_change_id, position"#,
        &record_ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut items_by: HashMap<Uuid, Vec<BookLine>> = HashMap::new();
    for i in items {
        items_by.entry(i.oil_change_id).or_default().push(BookLine {
            slot: i.slot,
            text: i.text,
            category_id: i.category_id,
        });
    }
    let overrides = sqlx::query!(
        r#"select vehicle_id, slot, interval_km, interval_days from vehicle_book_intervals
           where vehicle_id = any($1)"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let categories = category_intervals(conn).await?;
    let notes = sqlx::query!(
        r#"select id, vehicle_id, text, created_at, deleted_at from vehicle_notes
           where vehicle_id = any($1) order by created_at desc"#,
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
                // Запись без строк (заведена в обход книжки) — по прежним полям масла и фильтра.
                lines: items_by
                    .get(&r.id)
                    .cloned()
                    .unwrap_or_else(|| legacy_lines(&r.oil_text, &r.filter_text)),
                comment: r.comment.clone(),
                declined: r.declined.clone(),
                sale_id: r.sale_id,
                sale_number: r.sale_number,
                sale_total_tyiyn: r.sale_id.and(r.sale_total_tyiyn),
            })
            .collect();
        let client_notes = notes
            .iter()
            .filter(|n| n.vehicle_id == v.id)
            .map(|n| ClientNote {
                id: n.id,
                text: n.text.clone(),
                created_at: n.created_at,
                deleted: n.deleted_at.is_some(),
            })
            .collect();
        let own: HashMap<&str, (i32, i32)> = overrides
            .iter()
            .filter(|o| o.vehicle_id == v.id)
            .map(|o| (o.slot.as_str(), (o.interval_km, o.interval_days)))
            .collect();
        let slots: Vec<SlotNext> = SLOTS
            .iter()
            .map(|slot| slot_next(slot, &recs, own.get(slot).copied(), &categories))
            .filter(|n| n.slot == "oil" || n.last_date.is_some() || n.own_interval)
            .collect();
        let oil = slots
            .iter()
            .find(|n| n.slot == "oil")
            .cloned()
            .unwrap_or_else(|| slot_next("oil", &recs, None, &categories));
        out.push(VehicleBook {
            vehicle_id: v.id,
            plate: v.plate,
            brand: v.brand,
            model: v.model,
            active: v.active,
            interval_km: oil.interval_km.unwrap_or(8000),
            interval_days: oil.interval_days.unwrap_or(31),
            next_km: oil.next_km,
            next_date: oil.next_date,
            slots,
            records: recs,
            client_notes,
        });
    }
    Ok(out)
}

/// «на 93 000 км или до 12.01.2027» — следующая замена строки для бота; пусто — срока нет.
pub fn next_text(n: &SlotNext) -> String {
    [
        n.next_km
            .map(|k| format!("на {}", crate::api::telegram::km(k))),
        n.next_date.map(|d| format!("до {}", d.format("%d.%m.%Y"))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" или ")
}

/// Следующая замена по строкам книжки для бота: по строке на каждую, у которой есть срок.
pub fn next_lines(b: &VehicleBook, indent: &str) -> Vec<String> {
    b.slots
        .iter()
        .filter_map(|n| {
            let t = next_text(n);
            (!t.is_empty()).then(|| format!("{indent}{}: {t}", n.label))
        })
        .collect()
}

/// Строки замены для бота: «Масло двигателя: …», «Фильтр воздушный: …».
pub fn record_lines(r: &OilRecord, indent: &str) -> Vec<String> {
    r.lines
        .iter()
        .map(|l| format!("{indent}{}: {}", slot_label(&l.slot), l.text))
        .collect()
}

/// Строки записи без строк книжки: масло и фильтр из прежних полей.
fn legacy_lines(oil_text: &str, filter_text: &str) -> Vec<BookLine> {
    [("oil", oil_text), ("oil_filter", filter_text)]
        .into_iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .map(|(slot, t)| BookLine {
            slot: slot.into(),
            text: t.to_string(),
            category_id: None,
        })
        .collect()
}

/// Категория со строкой книжки и её интервалом по умолчанию.
struct CategoryInterval {
    id: Uuid,
    slot: String,
    interval: Option<(i32, i32)>,
}

async fn category_intervals(conn: &mut PgConnection) -> AppResult<Vec<CategoryInterval>> {
    Ok(sqlx::query!(
        r#"select c.id, coalesce(c.book_slot, case c.kind when 'oil' then 'oil' when 'filter' then 'oil_filter' else 'none' end) as "book_slot!", c.book_interval_km, c.book_interval_days
           from categories c
           where coalesce(c.book_slot, case c.kind when 'oil' then 'oil' when 'filter' then 'oil_filter' else 'none' end) <> 'none' order by c.name"#
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| CategoryInterval {
        id: r.id,
        slot: r.book_slot,
        interval: r.book_interval_km.zip(r.book_interval_days),
    })
    .collect())
}

/// Следующая замена строки: интервал машины, иначе категории последней замены этой строки,
/// иначе любой категории с этой строкой, иначе по умолчанию.
fn slot_next(
    slot: &str,
    recs: &[OilRecord],
    own: Option<(i32, i32)>,
    categories: &[CategoryInterval],
) -> SlotNext {
    // Записи свежие сверху: первая со строкой — последняя замена.
    let last = recs.iter().find_map(|r| {
        r.lines
            .iter()
            .find(|l| l.slot == slot)
            .map(|l| (r, l.clone()))
    });
    let last_km = recs
        .iter()
        .filter(|r| r.lines.iter().any(|l| l.slot == slot))
        .find_map(|r| r.mileage_km);
    let by_category = last
        .as_ref()
        .and_then(|(_, l)| l.category_id)
        .and_then(|id| categories.iter().find(|c| c.id == id && c.slot == slot))
        .and_then(|c| c.interval)
        .or_else(|| {
            categories
                .iter()
                .filter(|c| c.slot == slot)
                .find_map(|c| c.interval)
        });
    let interval = own.or(by_category).or_else(|| slot_default(slot));
    let next_km = last_km
        .zip(interval)
        .map(|(m, (km, _))| m.saturating_add(km));
    let next_date = last.as_ref().zip(interval).and_then(|((r, _), (_, days))| {
        r.change_date
            .checked_add_days(chrono::Days::new(u64::try_from(days).unwrap_or(31)))
    });
    SlotNext {
        slot: slot.to_string(),
        label: slot_label(slot).to_string(),
        interval_km: interval.map(|i| i.0),
        interval_days: interval.map(|i| i.1),
        own_interval: own.is_some(),
        last_date: last.as_ref().map(|(r, _)| r.change_date),
        last_km,
        last_text: last.map(|(_, l)| l.text),
        next_km,
        next_date,
    }
}

/// Удалённые клиентом пометки остаются только владельцу (SPEC-22).
pub fn hide_deleted_notes(list: &mut [VehicleBook]) {
    for b in list {
        b.client_notes.retain(|n| !n.deleted);
    }
}

async fn vehicle_book(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<VehicleBook>> {
    let mut conn = state.pool.acquire().await?;
    let mut list = books(&mut conn, user.branch_id, None, Some(id)).await?;
    if !user.is_owner() {
        hide_deleted_notes(&mut list);
    }
    list.pop().map(Json).ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct PartyBookQuery {
    /// Показать и убранные клиентом машины (ADR-062).
    #[serde(default)]
    removed: bool,
}

async fn party_book(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PartyBookQuery>,
) -> AppResult<Json<Vec<VehicleBook>>> {
    let mut conn = state.pool.acquire().await?;
    let mut list = books_with_removed(&mut conn, user.branch_id, Some(id), None, q.removed).await?;
    if !user.is_owner() {
        hide_deleted_notes(&mut list);
    }
    Ok(Json(list))
}

#[derive(Deserialize)]
struct SettingsReq {
    /// Строка книжки; без неё — масло (как раньше, один интервал на машину).
    #[serde(default = "oil_slot")]
    slot: String,
    interval_km: Option<i32>,
    interval_days: Option<i32>,
    /// Убрать интервал машины: строка возвращается к интервалу категории.
    #[serde(default)]
    reset: bool,
}

fn oil_slot() -> String {
    "oil".into()
}

/// Интервал строки книжки у машины (SPEC-27): правится или сбрасывается к категории.
async fn settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<SettingsReq>,
) -> AppResult<Json<VehicleBook>> {
    if !is_slot(&req.slot) {
        return Err(invalid("неизвестная строка книжки"));
    }
    let interval = if req.reset {
        None
    } else {
        let (Some(km), Some(days)) = (req.interval_km, req.interval_days) else {
            return Err(invalid("укажите интервал в км и днях"));
        };
        check_interval(km, days)?;
        Some((km, days))
    };
    let mut tx = state.pool.begin().await?;
    let plate = sqlx::query_scalar!(
        "select plate from party_vehicles where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let before = sqlx::query!(
        "select interval_km, interval_days from vehicle_book_intervals where vehicle_id = $1 and slot = $2",
        id,
        req.slot
    )
    .fetch_optional(&mut *tx)
    .await?;
    match interval {
        Some((km, days)) => {
            sqlx::query!(
                r#"insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days)
                   values ($1, $2, $3, $4, $5)
                   on conflict (vehicle_id, slot) do update
                   set interval_km = excluded.interval_km, interval_days = excluded.interval_days, updated_at = now()"#,
                id,
                ctx.user.branch_id,
                req.slot,
                km,
                days
            )
            .execute(&mut *tx)
            .await?;
        }
        None => {
            sqlx::query!(
                "delete from vehicle_book_intervals where vehicle_id = $1 and slot = $2",
                id,
                req.slot
            )
            .execute(&mut *tx)
            .await?;
        }
    }
    ops::audit(
        &mut tx,
        &ctx,
        "vehicle.oil_interval",
        "vehicle",
        Some(id),
        json!({
            "plate": plate,
            "slot": req.slot,
            "before": before.map(|b| json!({ "km": b.interval_km, "days": b.interval_days })),
            "km": interval.map(|i| i.0),
            "days": interval.map(|i| i.1),
        }),
    )
    .await?;
    let out = books(&mut tx, ctx.user.branch_id, None, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Интервал строки книжки: от 500 до 200 000 км и от 1 до 1095 дней.
pub fn check_interval(km: i32, days: i32) -> AppResult<()> {
    if !(500..=200_000).contains(&km) || !(1..=1095).contains(&days) {
        return Err(invalid(
            "интервал: от 500 до 200 000 км и от 1 до 1095 дней",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
struct ManualLine {
    slot: String,
    text: String,
}

#[derive(Deserialize)]
struct ManualReq {
    change_date: NaiveDate,
    mileage_km: Option<i32>,
    #[serde(default)]
    oil_text: String,
    #[serde(default)]
    filter_text: String,
    /// Строки книжки по видам: воздушный, салонный фильтр и прочее (SPEC-27).
    #[serde(default)]
    lines: Vec<ManualLine>,
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
    let mut lines = legacy_lines(req.oil_text.trim(), req.filter_text.trim());
    for l in &req.lines {
        if !is_slot(&l.slot) {
            return Err(invalid("неизвестная строка книжки"));
        }
        if !l.text.trim().is_empty() {
            lines.push(BookLine {
                slot: l.slot.clone(),
                text: l.text.trim().to_string(),
                category_id: None,
            });
        }
    }
    if lines.is_empty() {
        return Err(invalid("укажите, что заменили: масло, фильтр или другое"));
    }
    if lines.iter().any(|l| l.text.chars().count() > 300) {
        return Err(invalid("строка книжки — не длиннее 300 знаков"));
    }
    let mut tx = state.pool.begin().await?;
    let v = sqlx::query!(
        "select party_id, plate, active from party_vehicles where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Машину убрал клиент: замену на неё пишем, только когда её осознанно вернули (ADR-062).
    if !v.active {
        return Err(AppError::Conflict(format!(
            "машина {} убрана клиентом — верните её в карточке клиента",
            v.plate
        )));
    }
    let party_id = v.party_id;
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
        join_slots(&lines, |s| s == "oil"),
        join_slots(&lines, |s| s.ends_with("_filter")),
        req.comment.trim(),
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    insert_lines(&mut tx, ctx.user.branch_id, rid, &lines).await?;
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
    declined: Option<String>,
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
    let before = sqlx::query!(
        "select mileage_km, comment, declined from oil_changes where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let r = sqlx::query!(
        r#"update oil_changes set mileage_km = coalesce($3, mileage_km),
             comment = coalesce($4, comment), declined = coalesce($5, declined), updated_at = now()
           where id = $1 and branch_id = $2 returning vehicle_id, mileage_km, comment, declined"#,
        id,
        ctx.user.branch_id,
        req.mileage_km,
        req.comment.as_deref().map(str::trim),
        req.declined.as_deref().map(str::trim)
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
        json!({
            "record": id,
            "mileage": r.mileage_km,
            // Что было и что стало: пробег, комментарий, отказанные работы (инвариант 5).
            "changes": ops::changes(
                &json!({ "mileage_km": before.mileage_km, "comment": before.comment, "declined": before.declined }),
                &json!({ "mileage_km": r.mileage_km, "comment": r.comment, "declined": r.declined }),
            ),
        }),
    )
    .await?;
    let out = books(&mut tx, ctx.user.branch_id, None, Some(r.vehicle_id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}
