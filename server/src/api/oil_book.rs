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
/// Антифриз и тормозная жидкость — строки услуг (SPEC-34).
pub const SLOTS: [&str; 9] = [
    "oil",
    "gear_oil",
    "oil_filter",
    "air_filter",
    "cabin_filter",
    "fuel_filter",
    "antifreeze",
    "brake_fluid",
    "other",
];

pub fn is_slot(slot: &str) -> bool {
    SLOTS.contains(&slot)
}

/// Название строки книжки для экрана, бота и печати.
pub fn slot_label(slot: &str) -> &'static str {
    match slot {
        "oil" => "Масло двигателя",
        "gear_oil" => "Масло в коробке",
        "oil_filter" => "Фильтр масляный",
        "air_filter" => "Фильтр воздушный",
        "cabin_filter" => "Фильтр салонный",
        "fuel_filter" => "Фильтр топливный",
        "antifreeze" => "Антифриз",
        "brake_fluid" => "Тормозная жидкость",
        _ => "Прочие работы",
    }
}

/// Интервал строки, если его не задали ни у категории, ни у машины: масло и масляный фильтр —
/// 8 000 км или 31 день (решение заказчика), воздушный и салонный — 15 000 км или год,
/// топливный — 30 000 км или два года. У прочих работ интервала по умолчанию нет.
pub fn slot_default(slot: &str) -> Option<(i32, i32)> {
    match slot {
        "oil" | "oil_filter" => Some((8000, 31)),
        // Масло в коробке (SPEC-32): 40 000 км или два года; владелец правит у категории.
        "gear_oil" => Some((40_000, 730)),
        "air_filter" | "cabin_filter" => Some((15_000, 365)),
        "fuel_filter" => Some((30_000, 730)),
        // Антифриз и тормозная жидкость (SPEC-34): владелец правит у услуги и категории.
        "antifreeze" => Some((60_000, 730)),
        "brake_fluid" => Some((40_000, 730)),
        _ => None,
    }
}

/// Строка книжки услуги по названию (SPEC-34 §1, как в миграции 0064): сначала фильтры (в
/// «масляном фильтре» есть «масл»), потом антифриз, тормозная жидкость, коробка и масло мотора;
/// остальное — «прочее» (в книжку не пишется и не напоминает).
pub fn service_slot_by_name(name: &str) -> &'static str {
    let n = name.to_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["фильтр"]) {
        if has(&["воздуш"]) {
            return "air_filter";
        }
        if has(&["салон"]) {
            return "cabin_filter";
        }
        if has(&["топлив"]) {
            return "fuel_filter";
        }
        if has(&["масл"]) {
            return "oil_filter";
        }
        return "other";
    }
    if has(&["антифриз", "охлажд"]) {
        "antifreeze"
    } else if has(&["тормоз"]) {
        "brake_fluid"
    } else if has(&["коробк", "акпп", "мкпп", "трансмис"]) {
        "gear_oil"
    } else if has(&["масл"]) {
        "oil"
    } else {
        "other"
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
#[derive(Serialize, Clone, Default)]
pub struct BookLine {
    pub slot: String,
    pub text: String,
    #[serde(skip)]
    pub category_id: Option<Uuid>,
    /// Услуга чека по этой строке (SPEC-34): её название — в печати чека.
    #[serde(skip)]
    pub service_id: Option<Uuid>,
    pub service: Option<String>,
    /// Следующую замену задал кассир (SPEC-34): пробег в единице машины, в км и дата по интервалу
    /// в днях. Задал пустой — по строке не напоминать; не задавал — по интервалу.
    pub target_set: bool,
    pub next_mileage: Option<i32>,
    pub next_unit: Option<String>,
    pub next_km: Option<i32>,
    pub next_date: Option<NaiveDate>,
}

impl BookLine {
    pub fn plain(slot: &str, text: String, category_id: Option<Uuid>) -> Self {
        BookLine {
            slot: slot.to_string(),
            text,
            category_id,
            ..BookLine::default()
        }
    }
}

/// Следующая замена из чека (SPEC-34 §2): строка книжки, услуга и пробег в единице машины;
/// без пробега — не напоминать по этой строке.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextTarget {
    pub slot: String,
    pub service_id: Option<Uuid>,
    pub service_name: Option<String>,
    /// Пробег в единице машины и он же в км.
    pub next: Option<(i32, i32)>,
    pub unit: String,
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
    /// Единица пробега машины (ADR-084) и пробег последней и следующей замены в ней.
    pub unit: String,
    pub last_mileage: Option<i32>,
    pub next_mileage: Option<i32>,
}

#[derive(Serialize)]
pub struct OilRecord {
    pub id: Uuid,
    pub change_date: NaiveDate,
    pub mileage_km: Option<i32>,
    /// Пробег, как его вписали, и единица (ADR-084).
    pub mileage: Option<i32>,
    pub mileage_unit: String,
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
    /// Пометка точки к машине — чтобы правка машины в книжке её не стёрла.
    pub comment: String,
    /// Машина в списке клиента; убранная (клиентом в боте или точкой) — `false` (ADR-062).
    pub active: bool,
    /// Пробег машины — в км или милях (ADR-084).
    pub mileage_unit: String,
    /// Следующая замена масла в единице машины.
    pub next_mileage: Option<i32>,
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

/// Запись из чека «в сервис» с машиной: строки книжки по категориям товаров (SPEC-27), по
/// услугам со строкой книжки и следующие замены, заданные кассиром (SPEC-34). Вызывается в
/// транзакции чека.
#[allow(clippy::too_many_arguments)]
pub async fn record_from_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    party_id: Uuid,
    vehicle_id: Uuid,
    change_date: NaiveDate,
    mileage: Option<Mileage>,
    declined: &str,
    targets: &[NextTarget],
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
            product_slot(&r.book_slot, r.filter_type.as_deref()).map(|slot| {
                BookLine::plain(
                    slot,
                    format!("{} · {}", r.name, qty_text(&r.kind, r.qty, r.container_ml)),
                    Some(r.category_id),
                )
            })
        })
        .collect();
    // Услуги со строкой книжки (SPEC-34): масло клиента — товара-масла в чеке нет, но замена
    // масла — это замена (SPEC-16); антифриз, тормозная жидкость — так же. Товар той же строки
    // уже есть — услуга только подписывает его.
    let services = sqlx::query!(
        r#"select s.id, s.name, s.book_slot from sale_lines l join services s on s.id = l.service_id
           where l.sale_id = $1 and s.book_slot <> 'other' order by l.line_no"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for sv in services {
        if let Some(l) = lines.iter_mut().find(|l| l.slot == sv.book_slot) {
            if l.service_id.is_none() {
                l.service_id = Some(sv.id);
                l.service = Some(sv.name);
            }
            continue;
        }
        let mut l = BookLine::plain(&sv.book_slot, sv.name.clone(), None);
        l.service_id = Some(sv.id);
        l.service = Some(sv.name);
        // Масло — первой строкой, как раньше.
        if l.slot == "oil" {
            lines.insert(0, l);
        } else {
            lines.push(l);
        }
    }
    let own = sqlx::query!(
        "select slot, interval_days from vehicle_book_intervals where vehicle_id = $1",
        vehicle_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| OwnDays {
        slot: r.slot,
        interval_days: r.interval_days,
    })
    .collect::<Vec<_>>();
    let mut seen: Vec<&str> = Vec::new();
    for t in targets {
        let found = lines.iter().position(|l| l.slot == t.slot);
        // Цель без услуги — строка книжки товара этого чека: «+ ещё строку» предлагает только
        // позиции чека (SPEC-34 §2, уточнение 26.10.2026). Такой строки в чеке нет — цель не
        // пишется: строки, которой не было в чеке, в книжке не появится.
        if seen.contains(&t.slot.as_str()) || (found.is_none() && t.service_id.is_none()) {
            continue;
        }
        seen.push(&t.slot);
        let at = match found {
            Some(i) => i,
            None => {
                // Услуги в чеке нет (касса до 26.10.2026 предлагала «+ ещё» из справочника услуг,
                // такой чек может прийти из очереди) — строка с названием услуги.
                let text = t
                    .service_name
                    .clone()
                    .unwrap_or_else(|| slot_label(&t.slot).to_string());
                lines.push(BookLine {
                    service_id: t.service_id,
                    service: t.service_name.clone(),
                    ..BookLine::plain(&t.slot, text, None)
                });
                lines.len() - 1
            }
        };
        let days = match t.next {
            None => None,
            Some(_) => {
                let category = lines.get(at).and_then(|l| l.category_id);
                interval_days(conn, &own, &t.slot, t.service_id, category).await?
            }
        };
        if let Some(l) = lines.get_mut(at) {
            if t.service_id.is_some() {
                l.service_id = t.service_id;
                l.service = t.service_name.clone();
            }
            l.target_set = true;
            l.next_mileage = t.next.map(|n| n.0);
            l.next_unit = t.next.map(|_| t.unit.clone());
            l.next_km = t.next.map(|n| n.1);
            l.next_date = days.and_then(|d| {
                change_date.checked_add_days(chrono::Days::new(u64::try_from(d).unwrap_or(0)))
            });
        }
    }
    if lines.is_empty() {
        return Ok(());
    }
    let id = new_id();
    let inserted = sqlx::query_scalar!(
        r#"insert into oil_changes (id, branch_id, vehicle_id, party_id, sale_id, change_date,
                                    mileage_km, oil_text, filter_text, user_id, declined,
                                    mileage_value, mileage_unit)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
           on conflict (sale_id) where sale_id is not null do nothing
           returning id"#,
        id,
        ctx.user.branch_id,
        vehicle_id,
        party_id,
        sale_id,
        change_date,
        mileage.as_ref().map(|m| m.km),
        join_slots(&lines, |s| s == "oil"),
        join_slots(&lines, |s| s.ends_with("_filter")),
        ctx.user.id,
        declined.trim(),
        mileage.as_ref().map(|m| m.value),
        mileage.as_ref().map_or("km", |m| m.unit.as_str())
    )
    .fetch_optional(&mut *conn)
    .await?;
    if inserted.is_some() {
        insert_lines(conn, ctx.user.branch_id, id, &lines).await?;
    }
    Ok(())
}

struct OwnDays {
    slot: String,
    interval_days: i32,
}

/// Следующая замена из чека, как прислала касса (SPEC-34 §2): услуга (по ней — строка книжки)
/// или строка книжки товара этого чека и пробег в единице пробега чека; пусто — не напоминать.
#[derive(Deserialize, Clone, Debug)]
pub struct NextChangeReq {
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default, deserialize_with = "lenient_mileage")]
    pub next_mileage: Option<i64>,
}

/// Проверяет следующую замену из чека: услуга есть, строка книжки не «прочее», пробег не меньше
/// пробега сейчас. Услуга «прочее» — `None` (по ней не напоминают).
pub async fn resolve_target(
    conn: &mut PgConnection,
    r: &NextChangeReq,
    now: Option<&Mileage>,
    unit: &str,
) -> AppResult<Option<NextTarget>> {
    let (slot, service_name) = match r.service_id {
        Some(id) => {
            let sv = sqlx::query!("select name, book_slot from services where id = $1", id)
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| invalid("услуга следующей замены не найдена"))?;
            (sv.book_slot, Some(sv.name))
        }
        None => (r.slot.clone().unwrap_or_default(), None),
    };
    if !is_slot(&slot) {
        return Err(invalid("неизвестная строка книжки"));
    }
    if slot == "other" {
        return Ok(None);
    }
    let next = match r.next_mileage {
        None => None,
        Some(v) => {
            let Some(m) = resolve_mileage(Some(v), Some(unit), None, unit)? else {
                return Ok(None);
            };
            if let Some(now) = now
                && now.unit == m.unit
                && m.value < now.value
            {
                return Err(invalid(format!(
                    "{}: следующая замена — не меньше пробега сейчас ({})",
                    service_name.as_deref().unwrap_or(slot_label(&slot)),
                    crate::domain::mileage::text(now.value, &now.unit)
                )));
            }
            Some((m.value, m.km))
        }
    };
    Ok(Some(NextTarget {
        slot,
        service_id: r.service_id,
        service_name,
        next,
        unit: unit.to_string(),
    }))
}

/// Дни до следующей замены строки (SPEC-34 §3): интервал машины, иначе услуги, иначе категории
/// товара строки, иначе по умолчанию. Категория — как в книжке (`slot_next`): своя, только если её
/// строка та же (у общей «Фильтры» строка — масляный фильтр, а воздушный фильтр в ней — по типу),
/// иначе любая категория с этой строкой.
async fn interval_days(
    conn: &mut PgConnection,
    own: &[OwnDays],
    slot: &str,
    service_id: Option<Uuid>,
    category_id: Option<Uuid>,
) -> AppResult<Option<i32>> {
    if let Some(d) = own.iter().find(|o| o.slot == slot) {
        return Ok(Some(d.interval_days));
    }
    if let Some(id) = service_id
        && let Some(Some(d)) =
            sqlx::query_scalar!("select book_interval_days from services where id = $1", id)
                .fetch_optional(&mut *conn)
                .await?
    {
        return Ok(Some(d));
    }
    let categories = category_intervals(conn).await?;
    let by_category = category_interval(&categories, slot, category_id);
    Ok(by_category.or_else(|| slot_default(slot)).map(|i| i.1))
}

/// Масляная книжка в печатном чеке (SPEC-34 §4).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SaleBook {
    pub brand: String,
    pub model: String,
    pub plate: String,
    /// Пробег сейчас, как вписали, и единица.
    pub mileage: Option<i32>,
    pub mileage_unit: String,
    /// Следующие замены, заданные кассиром; пустые (без напоминания) не печатаются.
    pub lines: Vec<SaleBookLine>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SaleBookLine {
    /// Услуга, а без неё — строка книжки.
    pub label: String,
    pub next_mileage: i32,
    pub unit: String,
    pub next_date: Option<NaiveDate>,
}

/// Книжка чека для печати: запись замены этого чека; у чека без записи — `None`.
pub async fn sale_book(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<Option<SaleBook>> {
    let Some(h) = sqlx::query!(
        r#"select o.id, v.brand, v.model, v.plate, o.mileage_value, o.mileage_km, o.mileage_unit
           from oil_changes o join party_vehicles v on v.id = o.vehicle_id
           where o.sale_id = $1"#,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let lines = sqlx::query!(
        r#"select i.slot, s.name as "service?", i.next_value as "next_value!", i.next_unit, i.next_date
           from oil_change_items i left join services s on s.id = i.service_id
           where i.oil_change_id = $1 and i.target_set and i.next_value is not null
           order by i.position"#,
        h.id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|l| SaleBookLine {
        label: l.service.unwrap_or_else(|| slot_label(&l.slot).to_string()),
        next_mileage: l.next_value,
        unit: l.next_unit.unwrap_or_else(|| h.mileage_unit.clone()),
        next_date: l.next_date,
    })
    .collect();
    Ok(Some(SaleBook {
        brand: h.brand,
        model: h.model,
        plate: h.plate,
        mileage: h.mileage_value.or(h.mileage_km),
        mileage_unit: h.mileage_unit,
        lines,
    }))
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
            r#"insert into oil_change_items (id, branch_id, oil_change_id, slot, text, category_id, position,
                                             service_id, target_set, next_value, next_unit, next_km, next_date)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
            new_id(),
            branch_id,
            oil_change_id,
            l.slot,
            l.text,
            l.category_id,
            i32::try_from(i).unwrap_or(i32::MAX),
            l.service_id,
            l.target_set,
            l.next_mileage,
            l.next_unit,
            l.next_km,
            l.next_date
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Книжки машин: одной машины или всех машин клиента. Полностью возвращённые чеки скрыты.
pub async fn books(
    conn: &mut PgConnection,
    party_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
) -> AppResult<Vec<VehicleBook>> {
    books_with_removed(conn, party_id, vehicle_id, false).await
}

/// То же, а с `removed` — и машины, которые клиент убрал: точка видит их историю и может
/// вернуть машину (ADR-062). Убранные идут после действующих.
pub async fn books_with_removed(
    conn: &mut PgConnection,
    party_id: Option<Uuid>,
    vehicle_id: Option<Uuid>,
    removed: bool,
) -> AppResult<Vec<VehicleBook>> {
    // Машины и книжка общие для филиалов (SPEC-33): без отбора по филиалу.
    let vehicles = sqlx::query!(
        r#"select id, plate, brand, model, comment, active, mileage_unit
           from party_vehicles
           where ($1::uuid is null or party_id = $1) and ($2::uuid is null or id = $2)
             and (active or $2::uuid is not null or $3)
           order by active desc, plate"#,
        party_id,
        vehicle_id,
        removed
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = vehicles.iter().map(|v| v.id).collect();
    let records = sqlx::query!(
        r#"select o.id, o.vehicle_id, o.change_date, o.mileage_km, o.mileage_value, o.mileage_unit,
                  o.oil_text, o.filter_text, o.comment,
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
        r#"select i.oil_change_id, i.slot, i.text, i.category_id, i.service_id, s.name as "service?",
                  i.target_set, i.next_value, i.next_unit, i.next_km, i.next_date
           from oil_change_items i left join services s on s.id = i.service_id
           where i.oil_change_id = any($1) order by i.oil_change_id, i.position"#,
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
            service_id: i.service_id,
            service: i.service,
            target_set: i.target_set,
            next_mileage: i.next_value,
            next_unit: i.next_unit,
            next_km: i.next_km,
            next_date: i.next_date,
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
    let services = service_intervals(conn).await?;
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
                mileage: r.mileage_value.or(r.mileage_km),
                mileage_unit: r.mileage_unit.clone(),
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
            .map(|slot| {
                slot_next(
                    slot,
                    &recs,
                    own.get(slot).copied(),
                    &categories,
                    &services,
                    &v.mileage_unit,
                )
            })
            .filter(|n| n.slot == "oil" || n.last_date.is_some() || n.own_interval)
            .collect();
        let oil = slots
            .iter()
            .find(|n| n.slot == "oil")
            .cloned()
            .unwrap_or_else(|| {
                slot_next("oil", &recs, None, &categories, &services, &v.mileage_unit)
            });
        out.push(VehicleBook {
            vehicle_id: v.id,
            plate: v.plate,
            brand: v.brand,
            model: v.model,
            comment: v.comment,
            active: v.active,
            next_mileage: oil.next_mileage,
            mileage_unit: v.mileage_unit,
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
        n.next_mileage
            .map(|k| format!("на {}", crate::domain::mileage::text(k, &n.unit))),
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
        .map(|l| {
            let next = line_next_text(l);
            if next.is_empty() {
                format!("{indent}{}: {}", slot_label(&l.slot), l.text)
            } else {
                format!("{indent}{}: {} — {next}", slot_label(&l.slot), l.text)
            }
        })
        .collect()
}

/// «следующая замена на 225 000 км или до 26.10.2027» — цель кассира у строки замены
/// (SPEC-34); без цели — пусто.
pub fn line_next_text(l: &BookLine) -> String {
    let at = l.next_mileage.map(|v| {
        format!(
            "на {}",
            crate::domain::mileage::text(v, l.next_unit.as_deref().unwrap_or("km"))
        )
    });
    let by = l.next_date.map(|d| format!("до {}", d.format("%d.%m.%Y")));
    if at.is_none() && by.is_none() {
        return String::new();
    }
    let when = [at, by]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" или ");
    format!("следующая замена {when}")
}

/// Строки записи без строк книжки: масло и фильтр из прежних полей.
fn legacy_lines(oil_text: &str, filter_text: &str) -> Vec<BookLine> {
    [("oil", oil_text), ("oil_filter", filter_text)]
        .into_iter()
        .filter(|(_, t)| !t.trim().is_empty())
        .map(|(slot, t)| BookLine::plain(slot, t.to_string(), None))
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

/// Интервал категории для строки книжки: категории товара, если её строка та же, иначе любой
/// категории с этой строкой (по названию); у фильтра строку решает тип, и интервал общей категории
/// «Фильтры» (строка — масляный фильтр) воздушному фильтру не подходит.
fn category_interval(
    categories: &[CategoryInterval],
    slot: &str,
    category_id: Option<Uuid>,
) -> Option<(i32, i32)> {
    category_id
        .and_then(|id| categories.iter().find(|c| c.id == id && c.slot == slot))
        .and_then(|c| c.interval)
        .or_else(|| {
            categories
                .iter()
                .filter(|c| c.slot == slot)
                .find_map(|c| c.interval)
        })
}

/// Интервалы услуг, заданные владельцем (SPEC-34): услуга → (км, дни).
async fn service_intervals(conn: &mut PgConnection) -> AppResult<HashMap<Uuid, (i32, i32)>> {
    Ok(sqlx::query!(
        r#"select id, book_interval_km as "km!", book_interval_days as "days!" from services
           where book_interval_km is not null and book_interval_days is not null"#
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.id, (r.km, r.days)))
    .collect())
}

/// Следующая замена строки. Кассир задал цель в последней замене этой строки (SPEC-34) — она;
/// иначе по интервалу: машины, иначе услуги или категории последней замены этой строки, иначе
/// любой категории с этой строкой, иначе по умолчанию.
fn slot_next(
    slot: &str,
    recs: &[OilRecord],
    own: Option<(i32, i32)>,
    categories: &[CategoryInterval],
    services: &HashMap<Uuid, (i32, i32)>,
    unit: &str,
) -> SlotNext {
    // Записи свежие сверху: первая со строкой — последняя замена; в ней строка с целью — первой.
    let last = recs.iter().find_map(|r| {
        let mine = || r.lines.iter().filter(|l| l.slot == slot);
        mine()
            .find(|l| l.target_set)
            .or_else(|| mine().next())
            .map(|l| (r, l.clone()))
    });
    let last_km = recs
        .iter()
        .filter(|r| r.lines.iter().any(|l| l.slot == slot))
        .find_map(|r| r.mileage_km);
    let by_category = category_interval(
        categories,
        slot,
        last.as_ref().and_then(|(_, l)| l.category_id),
    );
    let by_service = last
        .as_ref()
        .and_then(|(_, l)| l.service_id)
        .and_then(|id| services.get(&id).copied());
    let interval = own
        .or(by_service)
        .or(by_category)
        .or_else(|| slot_default(slot));
    let in_unit = |k: Option<i32>| k.and_then(|k| crate::domain::mileage::from_km(k, unit));
    let target = last.as_ref().map(|(_, l)| l).filter(|l| l.target_set);
    let (next_km, next_date, next_mileage) = match target {
        // Цель кассира: пробег как вписали (в той же единице), дата — по интервалу в днях.
        Some(l) => (
            l.next_km,
            l.next_date,
            if l.next_unit.as_deref() == Some(unit) {
                l.next_mileage
            } else {
                in_unit(l.next_km)
            },
        ),
        None => {
            let next_km = last_km
                .zip(interval)
                .map(|(m, (km, _))| m.saturating_add(km));
            let next_date = last.as_ref().zip(interval).and_then(|((r, _), (_, days))| {
                r.change_date
                    .checked_add_days(chrono::Days::new(u64::try_from(days).unwrap_or(31)))
            });
            (next_km, next_date, in_unit(next_km))
        }
    };
    SlotNext {
        unit: unit.to_string(),
        last_mileage: in_unit(last_km),
        next_mileage,
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
    let mut list = books(&mut conn, None, Some(id)).await?;
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
    let mut list = books_with_removed(&mut conn, Some(id), None, q.removed).await?;
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
    // Книжка машины клиента — право администратора `clients` (ADR-097).
    crate::auth::require_permission(&mut tx, &ctx.user, "clients").await?;
    set_interval_tx(&mut tx, &ctx, id, &req.slot, interval, "card").await?;
    let out = books(&mut tx, None, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Интервал строки книжки у машины: свой (км и дни) или `None` — снова по категории.
/// В транзакции вызывающего: карточка машины и чек замены (SPEC-29 §23). `via` — откуда
/// поменяли, для журнала.
pub async fn set_interval_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    slot: &str,
    interval: Option<(i32, i32)>,
    via: &str,
) -> AppResult<()> {
    if !is_slot(slot) {
        return Err(invalid("неизвестная строка книжки"));
    }
    if let Some((km, days)) = interval {
        check_interval(km, days)?;
    }
    let plate = sqlx::query_scalar!(
        "select plate from party_vehicles where id = $1 for update",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let before = sqlx::query!(
        "select interval_km, interval_days from vehicle_book_intervals where vehicle_id = $1 and slot = $2",
        id,
        slot
    )
    .fetch_optional(&mut *conn)
    .await?;
    if before.as_ref().map(|b| (b.interval_km, b.interval_days)) == interval {
        return Ok(());
    }
    match interval {
        Some((km, days)) => {
            sqlx::query!(
                r#"insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days)
                   values ($1, $2, $3, $4, $5)
                   on conflict (vehicle_id, slot) do update
                   set interval_km = excluded.interval_km, interval_days = excluded.interval_days, updated_at = now()"#,
                id,
                ctx.user.branch_id,
                slot,
                km,
                days
            )
            .execute(&mut *conn)
            .await?;
        }
        None => {
            sqlx::query!(
                "delete from vehicle_book_intervals where vehicle_id = $1 and slot = $2",
                id,
                slot
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    ops::audit(
        &mut *conn,
        ctx,
        "vehicle.oil_interval",
        "vehicle",
        Some(id),
        json!({
            "plate": plate,
            "slot": slot,
            "before": before.map(|b| json!({ "km": b.interval_km, "days": b.interval_days })),
            "km": interval.map(|i| i.0),
            "days": interval.map(|i| i.1),
            "via": via,
        }),
    )
    .await?;
    Ok(())
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
    #[serde(default, deserialize_with = "lenient_mileage")]
    mileage_km: Option<i64>,
    /// Пробег в единице машины или в `mileage_unit` (ADR-084).
    #[serde(default, deserialize_with = "lenient_mileage")]
    mileage: Option<i64>,
    #[serde(default)]
    mileage_unit: Option<String>,
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

/// Пробег из запроса любым числом (ADR-093): слишком большое, дробное или строка не роняют весь
/// запрос ошибкой формата — такое значение вне диапазона, и его отклонит проверка пробега, а чек
/// без сети проведётся без пробега (ADR-090). Пустая строка — пробега нет.
pub fn lenient_mileage<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => Some(n.as_i64().unwrap_or(i64::MAX)),
        Some(serde_json::Value::String(t)) if t.trim().is_empty() => None,
        Some(serde_json::Value::String(t)) => Some(t.trim().parse::<i64>().unwrap_or(i64::MAX)),
        Some(_) => Some(i64::MAX),
    })
}

fn check_mileage(m: Option<i64>) -> AppResult<()> {
    if m.is_some_and(|m| !(0..=5_000_000).contains(&m)) {
        return Err(invalid("пробег от 0 до 5 000 000 км"));
    }
    Ok(())
}

/// Пробег замены (ADR-084): как вписали (число и единица) и он же в км — для интервалов.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mileage {
    pub km: i32,
    pub value: i32,
    pub unit: String,
}

/// Пробег из запроса: `mileage` в единице `mileage_unit` (без неё — в единице машины) или
/// прежнее поле `mileage_km` (в км). Ничего нет — `None`.
pub fn resolve_mileage(
    mileage: Option<i64>,
    unit: Option<&str>,
    legacy_km: Option<i64>,
    car_unit: &str,
) -> AppResult<Option<Mileage>> {
    let (value, unit) = match (mileage, legacy_km) {
        (Some(v), _) => (v, unit.unwrap_or(car_unit)),
        (None, Some(k)) => (k, "km"),
        (None, None) => return Ok(None),
    };
    if !crate::domain::mileage::is_unit(unit) {
        return Err(invalid("пробег — в км или милях"));
    }
    if !(0..=5_000_000).contains(&value) {
        return Err(invalid(if unit == "mi" {
            "пробег от 0 до 5 000 000 миль"
        } else {
            "пробег от 0 до 5 000 000 км"
        }));
    }
    let value = i32::try_from(value).map_err(|_| crate::error::overflow())?;
    let km = crate::domain::mileage::to_km(value, unit).ok_or_else(crate::error::overflow)?;
    Ok(Some(Mileage {
        km,
        value,
        unit: unit.to_string(),
    }))
}

/// Единица пробега машины.
pub async fn vehicle_unit(conn: &mut PgConnection, vehicle_id: Uuid) -> AppResult<String> {
    Ok(sqlx::query_scalar!(
        "select mileage_unit from party_vehicles where id = $1",
        vehicle_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or_else(|| "km".into()))
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
            lines.push(BookLine::plain(&l.slot, l.text.trim().to_string(), None));
        }
    }
    if lines.is_empty() {
        return Err(invalid("укажите, что заменили: масло, фильтр или другое"));
    }
    if lines.iter().any(|l| l.text.chars().count() > 300) {
        return Err(invalid("строка книжки — не длиннее 300 знаков"));
    }
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "clients").await?;
    let v = sqlx::query!(
        "select party_id, plate, active, mileage_unit from party_vehicles where id = $1",
        id
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
    let mileage = resolve_mileage(
        req.mileage,
        req.mileage_unit.as_deref(),
        req.mileage_km,
        &v.mileage_unit,
    )?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km,
                                    oil_text, filter_text, comment, user_id, mileage_value, mileage_unit)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        rid,
        ctx.user.branch_id,
        id,
        party_id,
        req.change_date,
        mileage.as_ref().map(|m| m.km),
        join_slots(&lines, |s| s == "oil"),
        join_slots(&lines, |s| s.ends_with("_filter")),
        req.comment.trim(),
        ctx.user.id,
        mileage.as_ref().map(|m| m.value),
        mileage.as_ref().map_or("km", |m| m.unit.as_str())
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
        json!({
            "record": rid,
            "date": req.change_date,
            "mileage": mileage.as_ref().map(|m| m.km),
            "mileage_value": mileage.as_ref().map(|m| m.value),
            "mileage_unit": mileage.as_ref().map(|m| m.unit.clone()),
        }),
    )
    .await?;
    let out = books(&mut tx, None, Some(id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct EditReq {
    #[serde(default, deserialize_with = "lenient_mileage")]
    mileage_km: Option<i64>,
    /// Пробег в единице машины или в `mileage_unit` (ADR-084).
    #[serde(default, deserialize_with = "lenient_mileage")]
    mileage: Option<i64>,
    #[serde(default)]
    mileage_unit: Option<String>,
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
    crate::auth::require_permission(&mut tx, &ctx.user, "clients").await?;
    let before = sqlx::query!(
        r#"select o.mileage_km, o.comment, o.declined, v.mileage_unit as car_unit
           from oil_changes o join party_vehicles v on v.id = o.vehicle_id
           where o.id = $1 for update of o"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let mileage = resolve_mileage(
        req.mileage,
        req.mileage_unit.as_deref(),
        req.mileage_km,
        &before.car_unit,
    )?;
    let r = sqlx::query!(
        r#"update oil_changes set mileage_km = coalesce($2, mileage_km),
             mileage_value = coalesce($5, mileage_value),
             mileage_unit = coalesce($6, mileage_unit),
             comment = coalesce($3, comment), declined = coalesce($4, declined), updated_at = now()
           where id = $1 returning vehicle_id, mileage_km, comment, declined"#,
        id,
        mileage.as_ref().map(|m| m.km),
        req.comment.as_deref().map(str::trim),
        req.declined.as_deref().map(str::trim),
        mileage.as_ref().map(|m| m.value),
        mileage.as_ref().map(|m| m.unit.clone())
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
    let out = books(&mut tx, None, Some(r.vehicle_id))
        .await?
        .pop()
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_slot_by_its_name() {
        for (name, slot) in [
            ("Замена масла в моторе", "oil"),
            ("Замена масла двигателя", "oil"),
            ("Замена масла в коробке", "gear_oil"),
            ("Замена масла АКПП", "gear_oil"),
            ("МКПП", "gear_oil"),
            ("Трансмиссия: замена", "gear_oil"),
            ("Замена антифриза", "antifreeze"),
            ("Промывка системы охлаждения", "antifreeze"),
            ("Тормозная жидкость", "brake_fluid"),
            ("Замена воздушного фильтра", "air_filter"),
            ("Фильтр салона", "cabin_filter"),
            ("Замена топливного фильтра", "fuel_filter"),
            ("Замена масляного фильтра", "oil_filter"),
            ("Замена фильтра", "other"),
            ("Мойка", "other"),
        ] {
            assert_eq!(service_slot_by_name(name), slot, "{name}");
        }
        assert_eq!(slot_label("antifreeze"), "Антифриз");
        assert!(is_slot("brake_fluid") && slot_default("other").is_none());
    }

    #[test]
    fn line_next_text_in_unit() {
        let mut l = BookLine::plain("oil", "Замена масла".into(), None);
        assert_eq!(line_next_text(&l), "");
        l.target_set = true;
        l.next_mileage = Some(225_000);
        l.next_unit = Some("km".into());
        l.next_date = NaiveDate::from_ymd_opt(2027, 1, 5);
        assert_eq!(
            line_next_text(&l),
            "следующая замена на 225 000 км или до 05.01.2027"
        );
        l.next_unit = Some("mi".into());
        l.next_date = None;
        assert_eq!(line_next_text(&l), "следующая замена на 225 000 миль");
    }
}
