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

/// Короткая пометка строки книжки — в скобках после того, что залито или поставлено, когда без неё
/// непонятно (задача 003 Н5): «масло клиента (коробка)», «W712 (масляный) · 1 шт».
fn slot_note(slot: &str) -> &'static str {
    match slot {
        "oil" => "двигатель",
        "gear_oil" => "коробка",
        "oil_filter" => "масляный",
        "air_filter" => "воздушный",
        "cabin_filter" => "салонный",
        "fuel_filter" => "топливный",
        "antifreeze" => "антифриз",
        "brake_fluid" => "тормозная",
        _ => "",
    }
}

/// Вид фильтра в названии товара или услуги: «Фильтр топливный WK 820», «Замена топливного
/// фильтра» — пометка не нужна.
fn names_filter_kind(slot: &str, text: &str) -> bool {
    let stem = match slot {
        "oil_filter" => "масл",
        "air_filter" => "воздуш",
        "cabin_filter" => "салон",
        "fuel_filter" => "топлив",
        _ => return true,
    };
    text.to_lowercase().contains(stem)
}

/// Подпись строки книжки — что залито или поставлено (задача 003 Н5), одно правило для книжки,
/// чеков, бота и напоминаний: товар с количеством («Mobil ATF 3309 · 1 л»); фильтр, в названии
/// которого нет вида, — с видом в скобках («W712 (масляный) · 1 шт»); жидкость по услуге без товара —
/// «масло клиента (коробка)»; работа без товара — название услуги. Название строки книжки («Масло в
/// коробке») вместо товара не пишется. Записанное не меняется (I4): подпись строится при показе.
/// Касса — то же правило (`oilBook.ts`, `titleOf`).
pub fn title_of(slot: &str, text: &str, service: Option<&str>) -> String {
    if is_fluid(slot) && service == Some(text) {
        return format!("{} ({})", own_fluid(slot), slot_note(slot));
    }
    if !names_filter_kind(slot, text) {
        let note = slot_note(slot);
        return match text.rsplit_once(" · ") {
            Some((name, qty)) => format!("{name} ({note}) · {qty}"),
            None => format!("{text} ({note})"),
        };
    }
    text.to_string()
}

/// Подпись строки замены (`title_of`).
pub fn line_title(l: &BookLine) -> String {
    title_of(&l.slot, &l.text, l.service.as_deref())
}

/// Подпись нескольких строк одной строки книжки (два товара — оба): «Mobil ATF 3309 · 1 л, Shell
/// ATF · 1 л».
fn titles<'a>(lines: impl Iterator<Item = &'a BookLine>) -> String {
    let mut out: Vec<String> = Vec::new();
    for t in lines.map(line_title) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out.join(", ")
}

/// Товар строки замены без количества: «Mobil ATF 3309»; строка без товара (услуга, запись из
/// тетради) — `None`.
fn product_name(l: &BookLine) -> Option<String> {
    l.category_id?;
    let name = l.text.rsplit_once(" · ").map_or(l.text.as_str(), |x| x.0);
    (!name.trim().is_empty()).then(|| name.trim().to_string())
}

/// Вид замены строки — под ним она в чеке книжки и в блоке книжки товарного чека (задача 003 Н5):
/// услуга строки; масляный, воздушный и салонный фильтр без своей услуги — услуга масла двигателя
/// той же записи (их меняют с маслом, ADR-117). Нет услуги — без вида.
fn line_group(l: &BookLine, all: &[BookLine]) -> Option<String> {
    if let Some(s) = &l.service {
        return Some(s.clone());
    }
    if follows_oil(&l.slot) {
        return all
            .iter()
            .find(|o| o.slot == "oil" && o.service.is_some())
            .and_then(|o| o.service.clone());
    }
    None
}

/// Подпись и вид замены у каждой строки записи — для показа (задача 003 Н5).
fn with_titles(mut lines: Vec<BookLine>) -> Vec<BookLine> {
    let groups: Vec<Option<String>> = lines.iter().map(|l| line_group(l, &lines)).collect();
    for (l, group) in lines.iter_mut().zip(groups) {
        l.title = line_title(l);
        l.group = group;
    }
    lines
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

/// Мост, редуктор или раздатка в названии услуги (ADR-117, ADR-131): «мост» — с начала слова
/// («самостоятельно» — не мост), «редукт», «раздатк». Их масло — не масло двигателя и не коробки:
/// строка книжки «прочее», «бесплатно с нашим маслом» — от трансмиссионных масел (как в миграции
/// 0071).
pub fn axle_or_transfer_by_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with("мост"))
        || ["редукт", "раздатк"].iter().any(|w| n.contains(w))
}

/// Строка книжки услуги по названию (SPEC-34 §1, как в миграции 0064, с уточнениями ADR-117 и
/// ADR-131): сначала фильтры (в «масляном фильтре» есть «масл»), потом антифриз, тормозная
/// жидкость, коробка и масло мотора; остальное — «прочее» (в книжку не пишется и без интервала
/// владельца не напоминает).
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
    // Доливка — не замена (ADR-117); масло моста, редуктора и раздатки — не масло двигателя и не
    // коробка: «прочее» (в «Замене масла мех. моста» есть «масл»).
    if axle_or_transfer_by_name(&n) || has(&["долив"]) {
        return "other";
    }
    // Радиатор охлаждения — не замена антифриза, тормозные колодки — не тормозная жидкость
    // (ADR-131): «охлажд…» с «радиатор…» и «тормоз…» без «жидк…» — дальше по названию.
    if has(&["антифриз", "тосол"]) || (has(&["охлажд"]) && !has(&["радиатор"]))
    {
        "antifreeze"
    } else if has(&["тормоз"]) && has(&["жидк"]) {
        "brake_fluid"
    } else if has(&["коробк", "акпп", "мкпп", "трансмис"]) {
        "gear_oil"
    } else if has(&["масл"]) {
        "oil"
    } else {
        "other"
    }
}

/// Жидкости книжки (ADR-117): пишутся заменой, только если в чеке есть услуга той же строки или
/// цель кассира по ней; иначе это доливка.
pub fn is_fluid(slot: &str) -> bool {
    matches!(slot, "oil" | "gear_oil" | "antifreeze" | "brake_fluid")
}

/// Масла, проданные в чеке «В сервис», — замена и без услуги своей строки (ADR-136): моторное и
/// трансмиссионное. Антифриз и тормозная жидкость — только с услугой или целью (ADR-117).
pub fn sold_is_change(slot: &str) -> bool {
    matches!(slot, "oil" | "gear_oil")
}

/// Строка масла, которую гасит услуга-доливка (ADR-136): своя строка книжки услуги, если владелец
/// дал ей масло, иначе по названию — коробка («кпп», «коробк», «трансмис», «atf»), иначе двигатель.
pub fn topup_slot(name: &str, book_slot: &str) -> &'static str {
    if book_slot == "gear_oil" {
        return "gear_oil";
    }
    let n = name.to_lowercase();
    if ["кпп", "коробк", "трансмис", "atf"]
        .iter()
        .any(|w| n.contains(w))
    {
        "gear_oil"
    } else {
        "oil"
    }
}

/// Услуга-доливка (ADR-118): флажок «Доливка — на вынос» или слово «долив» в названии.
pub fn is_topup(name: &str, takeaway: bool) -> bool {
    takeaway || name.to_lowercase().contains("долив")
}

/// ГУР, мост, редуктор или раздатка в названии услуги (ADR-131, ADR-136): ATF в таком чеке — не
/// замена масла коробки. «гур» — с начала слова.
pub fn steering_or_axle_by_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.starts_with("гур"))
        || axle_or_transfer_by_name(&n)
}

/// Фильтры, которые меняют вместе с маслом двигателя (ADR-117): в чеке с целью по маслу получают её.
fn follows_oil(slot: &str) -> bool {
    matches!(slot, "oil_filter" | "air_filter" | "cabin_filter")
}

/// Вид фильтра по названию — категории (`catalog::book_slot_by_name`, миграция 0085) или товара:
/// воздушный, салонный, топливный, масляный; без подсказки — `None`. Касса — то же правило
/// (`nextChange.ts`, `filterSlotByName`).
pub fn filter_slot_by_name(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    [
        ("воздуш", "air_filter"),
        ("салон", "cabin_filter"),
        ("топлив", "fuel_filter"),
        ("масл", "oil_filter"),
    ]
    .into_iter()
    .find(|(word, _)| n.contains(word))
    .map(|(_, slot)| slot)
}

/// Строка книжки товара: по категории, а у фильтров — по типу фильтра товара, если он указан
/// (одна категория «Фильтры» на все виды). Типа нет (или «Тип» выключен), а строку книжки категории
/// фильтров владелец не выбирал — по названию товара (`name`): «Фильтр топливный WK 820» в общей
/// категории «Фильтры» — топливный, не масляный на срок масла (задача 003 Н5).
pub fn product_slot(
    category_slot: &str,
    filter_type: Option<&str>,
    name: Option<&str>,
) -> Option<&'static str> {
    let by_category = SLOTS.iter().copied().find(|s| *s == category_slot)?;
    if by_category.ends_with("_filter") {
        let by_type = match filter_type.map(str::trim) {
            Some("масляный") => Some("oil_filter"),
            Some("воздушный") => Some("air_filter"),
            Some("салонный") => Some("cabin_filter"),
            Some("топливный") => Some("fuel_filter"),
            _ => None,
        };
        return Some(
            by_type
                .or_else(|| name.and_then(filter_slot_by_name))
                .unwrap_or(by_category),
        );
    }
    Some(by_category)
}

/// Строка книжки жидкости по названию товара (ADR-130, ADR-131): у товара категории без строки
/// книжки (и не масла, не фильтра) «антифриз», «тосол», «охлажда…» и G11 / G12 / G12+ / G13 —
/// антифриз; «тормозн…» и DOT 3 / 4 / 5 (5.1, «ROSDOT 4», «ДОТ-4») — тормозная жидкость; иначе
/// `None`. Касса — то же правило (`nextChange.ts`, `nameSlot`).
pub fn slot_by_product_name(name: &str) -> Option<&'static str> {
    let n = name.to_lowercase();
    if ["антифриз", "тосол", "охлажда"]
        .iter()
        .any(|w| n.contains(w))
        || has_class_token(&n, &["g"], &["11", "12", "13"])
    {
        Some("antifreeze")
    } else if n.contains("тормозн")
        || has_class_token(&n, &["dot", "дот", "rosdot", "росдот"], &["3", "4", "5"])
    {
        Some("brake_fluid")
    } else {
        None
    }
}

/// Класс жидкости в названии (`n` — в нижнем регистре): слово из `words` с начала слова, после
/// него через пробелы или дефис — число из `numbers`, за которым не идёт цифра: «dot 4», «dot-4»,
/// «dot4», «dot 5.1», «g12+», «g-12». «g-force», «g 10w-40», «sg12», «g120» — не класс.
fn has_class_token(n: &str, words: &[&str], numbers: &[&str]) -> bool {
    n.char_indices()
        .filter(|&(i, _)| {
            n[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
        })
        .filter_map(|(i, _)| words.iter().find_map(|w| n[i..].strip_prefix(w)))
        .any(|rest| {
            let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '-');
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            numbers.contains(&&rest[..end])
        })
}

/// Строка замены: что сделали по одной строке книжки.
#[derive(Serialize, Clone, Default)]
pub struct BookLine {
    pub slot: String,
    pub text: String,
    #[serde(skip)]
    pub category_id: Option<Uuid>,
    /// Услуга чека по этой строке (SPEC-34): её название — в печати чека; у «прочего» — своя
    /// строка у каждой услуги (ADR-131).
    pub service_id: Option<Uuid>,
    pub service: Option<String>,
    /// Следующую замену задал кассир (SPEC-34): пробег в единице машины, в км и дата по интервалу
    /// в днях. Задал пустой — по строке не напоминать; не задавал — по интервалу.
    pub target_set: bool,
    /// Цель поставил сервер: кассир её не прислал (ADR-144); «Исправить» её не переносит.
    #[serde(skip)]
    pub target_auto: bool,
    pub next_mileage: Option<i32>,
    pub next_unit: Option<String>,
    pub next_km: Option<i32>,
    pub next_date: Option<NaiveDate>,
    /// Подпись строки — что залито или поставлено (`line_title`, задача 003 Н5); строится при показе.
    pub title: String,
    /// Вид замены строки (`line_group`): под ним она в чеке книжки.
    pub group: Option<String>,
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

/// Следующая замена из чека (SPEC-34 §2): строка книжки, услуга этого чека и пробег в единице
/// машины; без пробега — не напоминать по этой строке.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextTarget {
    pub slot: String,
    pub service_id: Option<Uuid>,
    pub service_name: Option<String>,
    /// Пробег в единице машины и он же в км.
    pub next: Option<(i32, i32)>,
    pub unit: String,
    /// Дни интервала, заданные у услуги: дата цели у услуги «прочее» (ADR-131).
    pub service_days: Option<i32>,
}

/// Следующая замена по строке книжки: последняя замена и интервал (машины, категории или
/// по умолчанию). У «прочего» — по каждой услуге отдельно, с её названием (ADR-131).
#[derive(Serialize, Clone)]
pub struct SlotNext {
    pub slot: String,
    /// Услуга «прочее», чья это следующая замена; у остальных строк — нет.
    pub service_id: Option<Uuid>,
    /// Название строки книжки («Масло в коробке»), у «прочего» — услуги: для интервала и выбора строки.
    pub label: String,
    /// Что залито или поставлено в последнюю замену строки (задача 003 Н5): «Mobil ATF 3309 · 1 л»;
    /// под этой подписью — следующая замена в книжке, чеках и боте. Замен не было — `label`.
    pub title: String,
    /// Товары последней замены строки без количества: «Mobil ATF 3309» — в напоминании.
    pub products: Vec<String>,
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

/// Количество строки для книжки: «1 кан. × 4 л», «1,5 л», «2,5 кг», «1,25 м», «2 шт».
fn qty_text(kind: &str, qty: i64, container_ml: Option<i64>) -> String {
    crate::domain::qty::sale_qty_text(kind, qty, container_ml)
}

/// Одна строка чека товара книжки: вид, количество (канистры, мл розлива или штуки) и канистра.
#[derive(Clone, Debug)]
pub struct QtyPart {
    pub kind: String,
    pub qty: i64,
    pub container_ml: Option<i64>,
}

/// Количество товара книжки по всем его строкам чека (ADR-144): одна строка — как было
/// («1 кан. × 4 л»); несколько — итог: масло в литрах («4,5 л»), штуки — штуками («2 шт»); `split` —
/// ещё и разбивка: «4,5 л (1 кан. × 4 л + 0,5 л)». Всё в целых мл (I1), литры — только в тексте.
pub fn merged_qty_text(parts: &[QtyPart], style: &str) -> String {
    let each = |p: &QtyPart| qty_text(&p.kind, p.qty, p.container_ml);
    if let [one] = parts {
        return each(one);
    }
    // Единица учёта строки и количество в ней: мл, граммы, миллиметры или штуки.
    let units = |p: &QtyPart| match (p.kind.as_str(), p.container_ml) {
        ("container", Some(c)) => ("ml", p.qty.saturating_mul(c)),
        ("pour", _) => ("ml", p.qty),
        ("weight", _) => ("g", p.qty),
        ("length", _) => ("mm", p.qty),
        _ => ("piece", p.qty),
    };
    let unit = parts.first().map_or("ml", |p| units(p).0);
    if parts.iter().any(|p| units(p).0 != unit) {
        // Разных единиц у одного товара не бывает (кроме смены единицы) — строки как есть.
        return parts.iter().map(each).collect::<Vec<_>>().join(" + ");
    }
    let sum = parts
        .iter()
        .map(|p| units(p).1)
        .fold(0i64, i64::saturating_add);
    let total = crate::domain::qty::qty_text(sum, unit);
    if style == "split" {
        let split = parts.iter().map(each).collect::<Vec<_>>().join(" + ");
        return format!("{total} ({split})");
    }
    total
}

/// Запись из чека «в сервис» с машиной: строки книжки по категориям товаров (SPEC-27), по
/// услугам со строкой книжки и следующие замены, заданные кассиром (SPEC-34); услуга «прочее»
/// с целью — своей строкой (ADR-131). Вызывается в транзакции чека.
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
        // Строка книжки категории; не задана — по виду: масла, фильтры (как было); категория без
        // строки — по названию товара (ADR-130).
        // «Тип» фильтра выключен у категории (задача 002 П10) — строка от категории, а не от типа.
        r#"select p.id as product_id, p.name, c.id as category_id,
                  coalesce(c.book_slot, case c.kind when 'oil' then 'oil' when 'filter' then 'oil_filter' else 'none' end) as "book_slot!",
                  case when category_by_type(c.attributes) then p.attrs ->> 'filter_type' end as filter_type,
                  (c.kind = 'filter' and c.book_slot is null) as "slot_by_name!",
                  l.kind, l.qty, p.container_ml
           from sale_lines l
           join products p on p.id = l.product_id
           join categories c on c.id = p.category_id
           where l.sale_id = $1 and not l.gift
           order by l.line_no"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let settings = crate::api::book_settings::load(conn, ctx.user.branch_id).await?;
    // Строки одного товара одной строки книжки (канистра и разлив, две строки штук) — одной
    // строкой книжки с итогом (ADR-144).
    let mut goods: Vec<(&'static str, Uuid, Uuid, &str, Vec<QtyPart>)> = Vec::new();
    for r in &rows {
        let slot = if r.book_slot == "none" {
            slot_by_product_name(&r.name)
        } else {
            product_slot(
                &r.book_slot,
                r.filter_type.as_deref(),
                r.slot_by_name.then_some(r.name.as_str()),
            )
        };
        let Some(slot) = slot else {
            continue;
        };
        let part = QtyPart {
            kind: r.kind.clone(),
            qty: r.qty,
            container_ml: r.container_ml,
        };
        match goods
            .iter_mut()
            .find(|g| g.0 == slot && g.1 == r.product_id)
        {
            Some(g) => g.4.push(part),
            None => goods.push((slot, r.product_id, r.category_id, &r.name, vec![part])),
        }
    }
    let mut lines: Vec<BookLine> = goods
        .iter()
        .map(|(slot, _, category_id, name, parts)| {
            BookLine::plain(
                slot,
                format!("{name} · {}", merged_qty_text(parts, &settings.qty_style)),
                Some(*category_id),
            )
        })
        .collect();
    // Услуги со строкой книжки (SPEC-34): масло клиента — товара-масла в чеке нет, но замена
    // масла — это замена (SPEC-16); антифриз, тормозная жидкость — так же. Товар той же строки
    // уже есть — услуга только подписывает его.
    // Услуга-доливка (ADR-118) — не замена, даже если владелец дал ей строку книжки.
    let services = sqlx::query!(
        r#"select s.id, s.name, s.book_slot from sale_lines l join services s on s.id = l.service_id
           where l.sale_id = $1 and s.book_slot <> 'other' and not s.takeaway order by l.line_no"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Строки книжки, по которым в чеке замена: услуга той же строки или цель кассира (ADR-117),
    // проданное масло двигателя или коробки (ADR-136).
    let mut changed: Vec<String> = services.iter().map(|s| s.book_slot.clone()).collect();
    // Масло по товару — замена, если в чеке нет доливки этой строки; ATF при ГУР, мосте,
    // редукторе, раздатке — не замена коробки (ADR-131, ADR-136). Объём не важен.
    let all_services = sqlx::query!(
        r#"select s.name, s.book_slot, s.takeaway from sale_lines l join services s on s.id = l.service_id
           where l.sale_id = $1"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let blocked: Vec<&str> = all_services
        .iter()
        .filter_map(|s| {
            if is_topup(&s.name, s.takeaway) {
                Some(topup_slot(&s.name, &s.book_slot))
            } else if s.book_slot == "other" && steering_or_axle_by_name(&s.name) {
                Some("gear_oil")
            } else {
                None
            }
        })
        .collect();
    let sold: Vec<String> = lines
        .iter()
        .map(|l| l.slot.clone())
        .filter(|slot| sold_is_change(slot) && !blocked.contains(&slot.as_str()))
        .collect();
    changed.extend(sold.iter().cloned());
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
    let own = own_intervals(conn, vehicle_id).await?;
    let mut seen: Vec<&str> = Vec::new();
    let mut seen_services: Vec<Uuid> = Vec::new();
    for t in targets {
        // Цель — по услуге этого чека (ADR-131; `resolve_target` проверил, что услуга в чеке и не
        // доливка на вынос) или по проданному маслу без доливки этой строки (ADR-136).
        if t.service_id.is_none() && !sold.contains(&t.slot) {
            continue;
        }
        if t.slot == "other" {
            let Some(service_id) = t.service_id else {
                continue;
            };
            // Услуга «прочее» с целью кассира — своя строка книжки с её названием и своё
            // напоминание; очищенное поле строки не даёт: по «прочему» и так не напоминают.
            if seen_services.contains(&service_id) {
                continue;
            }
            seen_services.push(service_id);
            let Some((value, km)) = t.next else {
                continue;
            };
            let name = t.service_name.clone().unwrap_or_default();
            let mut l = BookLine::plain("other", name.clone(), None);
            l.service_id = Some(service_id);
            l.service = Some(name);
            l.target_set = true;
            l.next_mileage = Some(value);
            l.next_unit = Some(t.unit.clone());
            l.next_km = Some(km);
            // Дата — только по дням, заданным у услуги: у «прочего» срока по умолчанию нет.
            l.next_date = t.service_days.and_then(|d| {
                change_date.checked_add_days(chrono::Days::new(u64::try_from(d).unwrap_or(0)))
            });
            lines.push(l);
            continue;
        }
        // Цель — на строку книжки этого чека (товар или услуга той же строки); такой строки в
        // чеке нет — цель не пишется (ADR-130).
        let Some(at) = lines.iter().position(|l| l.slot == t.slot) else {
            continue;
        };
        if seen.contains(&t.slot.as_str()) {
            continue;
        }
        seen.push(&t.slot);
        changed.push(t.slot.clone());
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
    // Жидкость без услуги своей строки и без цели кассира — доливка: в книжку не пишется, срок не
    // сдвигает (ADR-117).
    lines.retain(|l| !is_fluid(&l.slot) || changed.contains(&l.slot));
    // Цель по умолчанию (ADR-144): у жидкости-замены, по которой кассир цель не прислал и не
    // очищал, при известном пробеге — пробег сейчас + интервал строки (машины, услуги, категории,
    // по умолчанию), как заполняет касса. Тогда печатный чек, чек книжки, бот и книжка показывают
    // одно число и фильтры получают срок масла (ADR-117). Без пробега — без цели, по интервалу.
    if let Some(now) = &mileage {
        let mut done: Vec<String> = Vec::new();
        for at in 0..lines.len() {
            let Some(l) = lines.get(at) else {
                continue;
            };
            let slot = l.slot.clone();
            if !is_fluid(&slot)
                || done.contains(&slot)
                || lines.iter().any(|x| x.slot == slot && x.target_set)
            {
                continue;
            }
            done.push(slot.clone());
            let Some((km, days)) =
                line_interval(conn, &own, &slot, l.service_id, l.category_id).await?
            else {
                continue;
            };
            let Some(value) = crate::domain::mileage::from_km(km, &now.unit)
                .and_then(|add| now.value.checked_add(add))
            else {
                continue;
            };
            let Some(next_km) = crate::domain::mileage::to_km(value, &now.unit) else {
                continue;
            };
            if let Some(l) = lines.get_mut(at) {
                l.target_set = true;
                l.target_auto = true;
                l.next_mileage = Some(value);
                l.next_unit = Some(now.unit.clone());
                l.next_km = Some(next_km);
                l.next_date = change_date
                    .checked_add_days(chrono::Days::new(u64::try_from(days).unwrap_or(0)));
            }
        }
    }
    // Масляный, воздушный и салонный фильтры с заменой масла — на пробег и дату масла; у масла
    // «не напоминать» — и у них (ADR-117). Топливный — по своему интервалу. Владелец может
    // выключить: тогда каждый фильтр — по своему интервалу.
    if let Some(oil) = lines
        .iter()
        .find(|l| l.slot == "oil" && l.target_set)
        .filter(|_| settings.filters_follow_oil)
        .cloned()
    {
        for l in lines.iter_mut().filter(|l| follows_oil(&l.slot)) {
            // Цель масла поставил сервер, а у машины свой интервал этого фильтра — фильтр по нему:
            // свой интервал машины точнее (как в книжке, `fill_auto_follow`).
            if oil.target_auto
                && let (Some(o), Some(now)) = (own.iter().find(|o| o.slot == l.slot), &mileage)
            {
                let value = crate::domain::mileage::from_km(o.interval_km, &now.unit)
                    .and_then(|add| now.value.checked_add(add));
                l.target_set = true;
                l.target_auto = true;
                l.next_mileage = value;
                l.next_unit = value.map(|_| now.unit.clone());
                l.next_km = value.and_then(|v| crate::domain::mileage::to_km(v, &now.unit));
                l.next_date = change_date.checked_add_days(chrono::Days::new(
                    u64::try_from(o.interval_days).unwrap_or(0),
                ));
                continue;
            }
            l.target_set = true;
            l.target_auto = oil.target_auto;
            l.next_mileage = oil.next_mileage;
            l.next_unit = oil.next_unit.clone();
            l.next_km = oil.next_km;
            l.next_date = oil.next_date;
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
    interval_km: i32,
    interval_days: i32,
}

/// Следующая замена из чека, как прислала касса (SPEC-34 §2): услуга этого чека (по ней — строка
/// книжки) и пробег в единице пробега чека; пусто — не напоминать. `slot` — для справки: строку
/// решает услуга; строка без услуги (по товару, касса до 27.10.2026) не пишется (ADR-131).
#[derive(Deserialize, Clone, Debug)]
pub struct NextChangeReq {
    #[serde(default)]
    pub service_id: Option<Uuid>,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default, deserialize_with = "lenient_mileage")]
    pub next_mileage: Option<i64>,
}

/// Проверяет следующую замену из чека `sale_id`: услуга — строка этого чека и не доливка на вынос
/// (иначе цели нет, ADR-131), пробег не меньше пробега сейчас. Услуга «прочее» — своя цель.
pub async fn resolve_target(
    conn: &mut PgConnection,
    sale_id: Uuid,
    r: &NextChangeReq,
    now: Option<&Mileage>,
    unit: &str,
) -> AppResult<Option<NextTarget>> {
    let Some(id) = r.service_id else {
        return sold_target(r, now, unit);
    };
    let Some(sv) = sqlx::query!(
        r#"select s.name, s.book_slot, s.book_interval_days from services s
           where s.id = $1 and not s.takeaway
             and exists (select 1 from sale_lines l where l.sale_id = $2 and l.service_id = s.id)"#,
        id,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    if !is_slot(&sv.book_slot) {
        return Err(invalid("неизвестная строка книжки"));
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
                    sv.name,
                    crate::domain::mileage::text(now.value, &now.unit)
                )));
            }
            Some((m.value, m.km))
        }
    };
    Ok(Some(NextTarget {
        slot: sv.book_slot,
        service_id: Some(id),
        service_name: Some(sv.name),
        next,
        unit: unit.to_string(),
        service_days: sv.book_interval_days,
    }))
}

/// Цель по проданному маслу без услуги своей строки (ADR-136): строка `oil` или `gear_oil`;
/// пишется, только если такое масло в чеке и доливки этой строки нет (`record_from_sale`).
fn sold_target(
    r: &NextChangeReq,
    now: Option<&Mileage>,
    unit: &str,
) -> AppResult<Option<NextTarget>> {
    let Some(slot) = r.slot.as_deref().filter(|s| sold_is_change(s)) else {
        return Ok(None);
    };
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
                    slot_label(slot),
                    crate::domain::mileage::text(now.value, &now.unit)
                )));
            }
            Some((m.value, m.km))
        }
    };
    Ok(Some(NextTarget {
        slot: slot.to_string(),
        service_id: None,
        service_name: None,
        next,
        unit: unit.to_string(),
        service_days: None,
    }))
}

/// Следующая замена чека, как её задал кассир, — для черновика «Исправить» (ADR-131): строка
/// книжки (у «прочего» — услуга) и пробег; пусто — «не напоминать».
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct NextChangeOut {
    pub service_id: Option<Uuid>,
    pub slot: String,
    pub next_mileage: Option<i32>,
}

/// Цели кассира в записи книжки чека: по строке одна, у «прочего» — по услуге; фильтры, которые
/// идут за маслом (ADR-117), своей цели не имеют и не возвращаются.
pub async fn sale_targets(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<Vec<NextChangeOut>> {
    let items = sqlx::query!(
        r#"select i.slot, i.service_id, i.next_value from oil_change_items i
           join oil_changes o on o.id = i.oil_change_id
           where o.sale_id = $1 and i.target_set and not i.target_auto
           order by i.position"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let oil = items.iter().any(|i| i.slot == "oil");
    let mut out: Vec<NextChangeOut> = Vec::new();
    for i in items {
        let dup = out
            .iter()
            .any(|o| o.slot == i.slot && (i.slot != "other" || o.service_id == i.service_id));
        if dup || (oil && follows_oil(&i.slot)) {
            continue;
        }
        out.push(NextChangeOut {
            service_id: i.service_id,
            slot: i.slot,
            next_mileage: i.next_value,
        });
    }
    Ok(out)
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

/// Интервалы строк книжки, заданные у машины (SPEC-27).
async fn own_intervals(conn: &mut PgConnection, vehicle_id: Uuid) -> AppResult<Vec<OwnDays>> {
    Ok(sqlx::query!(
        "select slot, interval_km, interval_days from vehicle_book_intervals where vehicle_id = $1",
        vehicle_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| OwnDays {
        slot: r.slot,
        interval_km: r.interval_km,
        interval_days: r.interval_days,
    })
    .collect())
}

/// Интервал строки (км и дни) — та же цепочка, что в книжке (`slot_next`): машины, иначе услуги
/// строки (если у неё заданы оба числа), иначе категории строки, иначе по умолчанию. Им сервер
/// ставит цель, которую кассир не прислал (ADR-144), и считает срок старой записи без цели.
async fn line_interval(
    conn: &mut PgConnection,
    own: &[OwnDays],
    slot: &str,
    service_id: Option<Uuid>,
    category_id: Option<Uuid>,
) -> AppResult<Option<(i32, i32)>> {
    if let Some(o) = own.iter().find(|o| o.slot == slot) {
        return Ok(Some((o.interval_km, o.interval_days)));
    }
    if let Some(id) = service_id
        && let Some(r) = sqlx::query!(
            "select book_interval_km, book_interval_days from services where id = $1",
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        && let (Some(km), Some(days)) = (r.book_interval_km, r.book_interval_days)
    {
        return Ok(Some((km, days)));
    }
    let categories = category_intervals(conn).await?;
    Ok(category_interval(&categories, slot, category_id).or_else(|| slot_default(slot)))
}

/// Масляная книжка в печатном чеке (SPEC-34 §4): «чек книжки» — только этот чек: жидкости,
/// фильтры (по настройке, по умолчанию — с ними) и работы «прочее» со сроком, по видам замены
/// (задача 003 Н5).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SaleBook {
    pub brand: String,
    pub model: String,
    pub plate: String,
    /// Пробег сейчас, как вписали, и единица.
    pub mileage: Option<i32>,
    pub mileage_unit: String,
    /// Следующие замены строк чека; пустые (без напоминания) не печатаются.
    pub lines: Vec<SaleBookLine>,
    /// Что залито и поставлено: товар с количеством, у масла клиента — «масло клиента» (ADR-116).
    #[serde(default)]
    pub filled: Vec<SaleBookFill>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SaleBookLine {
    /// Что залито или поставлено по строке (`title_of`): «Mobil ATF 3309 · 1 л» (задача 003 Н5).
    pub label: String,
    pub next_mileage: i32,
    pub unit: String,
    /// Срок в данных остаётся, в тексте — только пробег (ADR-115).
    pub next_date: Option<NaiveDate>,
    #[serde(default)]
    pub slot: String,
    /// Услуга «прочее» строки: у каждой своя следующая замена (ADR-131).
    #[serde(default)]
    pub service_id: Option<Uuid>,
    /// Вид замены (`line_group`): «Замена масла в моторе».
    #[serde(default)]
    pub group: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SaleBookFill {
    pub slot: String,
    /// Подпись (`title_of`): «Mobil ATF 3309 · 1 л», «W712 (масляный) · 1 шт», «масло клиента
    /// (коробка)» (задача 003 Н5).
    pub label: String,
    /// Товар и количество, как записано: «Sintec G12 красный 5 кг · 2 шт».
    pub product: String,
    #[serde(default)]
    pub service_id: Option<Uuid>,
    /// Вид замены (`line_group`).
    #[serde(default)]
    pub group: Option<String>,
}

/// Чем залили строку, когда товара в чеке нет: своё у клиента.
fn own_fluid(slot: &str) -> &'static str {
    match slot {
        "oil" | "gear_oil" => "масло клиента",
        _ => "жидкость клиента",
    }
}

/// Книжка чека для печати: запись замены этого чека; у чека без записи — `None`. Жидкости, фильтры
/// (настройка «с фильтрами», ADR-144; по умолчанию — да) и работы «прочее» со сроком; у каждой
/// строки — подпись товаром и вид замены (задача 003 Н5). Строка без цели (запись до 31.10.2026) —
/// срок по интервалу, как в книжке клиента (`slot_next`): записанное не меняется (I4).
pub async fn sale_book(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<Option<SaleBook>> {
    let Some(h) = sqlx::query!(
        r#"select o.id, o.branch_id, o.vehicle_id, v.brand, v.model, v.plate, o.mileage_value,
                  o.mileage_km, o.mileage_unit
           from oil_changes o join party_vehicles v on v.id = o.vehicle_id
           where o.sale_id = $1"#,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let settings = crate::api::book_settings::load(conn, h.branch_id).await?;
    let items = sqlx::query!(
        r#"select i.slot, i.text, i.category_id, i.service_id, s.name as "service?", i.target_set,
                  i.next_value, i.next_unit, i.next_km, i.next_date
           from oil_change_items i left join services s on s.id = i.service_id
           where i.oil_change_id = $1
           order by i.position"#,
        h.id
    )
    .fetch_all(&mut *conn)
    .await?;
    let all = with_titles(
        items
            .into_iter()
            .map(|i| BookLine {
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
                ..BookLine::default()
            })
            .collect(),
    );
    // Что идёт в чек: жидкости, фильтры — по настройке, «прочее» — по услуге (срок у него всегда от
    // кассира, ADR-131).
    let printed = |l: &BookLine| {
        is_fluid(&l.slot)
            || (settings.with_filters && l.slot.ends_with("_filter"))
            || (l.slot == "other" && l.service_id.is_some())
    };
    let own = own_intervals(conn, h.vehicle_id).await?;
    // Пробег сейчас в единице записи: от него срок строки без цели.
    let now = h.mileage_value.or(h.mileage_km);
    let mut lines: Vec<SaleBookLine> = Vec::new();
    let mut filled: Vec<SaleBookFill> = Vec::new();
    let mut decided: Vec<(String, Option<Uuid>)> = Vec::new();
    for i in all.iter().filter(|l| printed(l)) {
        // У «прочего» — своя строка у каждой услуги (ADR-131).
        let other = i.service_id.filter(|_| i.slot == "other");
        // Строка товара — «название · количество»; строка услуги без товара — масло клиента. Услуга
        // при товаре той же строки строки не даёт (товар её подписывает).
        filled.push(SaleBookFill {
            slot: i.slot.clone(),
            label: i.title.clone(),
            product: if i.category_id.is_some() || !is_fluid(&i.slot) {
                i.text.clone()
            } else {
                own_fluid(&i.slot).to_string()
            },
            service_id: other,
            group: i.group.clone(),
        });
        let key = (i.slot.clone(), other);
        if decided.contains(&key) {
            continue;
        }
        decided.push(key.clone());
        let next = if i.target_set {
            i.next_mileage.map(|v| {
                (
                    v,
                    i.next_unit
                        .clone()
                        .unwrap_or_else(|| h.mileage_unit.clone()),
                    i.next_date,
                )
            })
        } else {
            match now {
                None => None,
                Some(now) => line_interval(conn, &own, &i.slot, i.service_id, i.category_id)
                    .await?
                    .and_then(|(km, _)| {
                        crate::domain::mileage::from_km(km, &h.mileage_unit)
                            .and_then(|add| now.checked_add(add))
                    })
                    .map(|v| (v, h.mileage_unit.clone(), None)),
            }
        };
        if let Some((next_mileage, unit, next_date)) = next {
            // Подпись — все товары этой строки чека: «Mobil ATF 3309 · 1 л, Shell ATF · 1 л».
            let same = all.iter().filter(|l| {
                printed(l) && l.slot == key.0 && (other.is_none() || l.service_id == other)
            });
            lines.push(SaleBookLine {
                label: titles(same),
                next_mileage,
                unit,
                next_date,
                slot: key.0,
                service_id: other,
                group: i.group.clone(),
            });
        }
    }
    Ok(Some(SaleBook {
        brand: h.brand,
        model: h.model,
        plate: h.plate,
        mileage: now,
        mileage_unit: h.mileage_unit,
        lines,
        filled,
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
                                             service_id, target_set, next_value, next_unit, next_km, next_date,
                                             target_auto)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
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
            l.next_date,
            l.target_auto
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
                  i.target_set, i.target_auto, i.next_value, i.next_unit, i.next_km, i.next_date
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
            target_auto: i.target_auto,
            next_mileage: i.next_value,
            next_unit: i.next_unit,
            next_km: i.next_km,
            next_date: i.next_date,
            ..BookLine::default()
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
                lines: with_titles(
                    items_by
                        .get(&r.id)
                        .cloned()
                        .unwrap_or_else(|| legacy_lines(&r.oil_text, &r.filter_text)),
                ),
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
        let mut slots: Vec<SlotNext> = SLOTS
            .iter()
            .map(|slot| {
                slot_next(
                    (*slot, None),
                    &recs,
                    own.get(slot).copied(),
                    &categories,
                    &services,
                    &v.mileage_unit,
                )
            })
            .filter(|n| n.slot == "oil" || n.last_date.is_some() || n.own_interval)
            .collect();
        fill_auto_follow(&mut slots, &recs, &v.mileage_unit);
        // «Прочее» по услугам (ADR-131): у каждой услуги — своя следующая замена с её названием,
        // свежие сверху.
        let mut other: Vec<Uuid> = Vec::new();
        for l in recs.iter().flat_map(|r| r.lines.iter()) {
            if l.slot == "other"
                && let Some(id) = l.service_id
                && !other.contains(&id)
            {
                other.push(id);
            }
        }
        for id in other {
            slots.push(slot_next(
                ("other", Some(id)),
                &recs,
                None,
                &categories,
                &services,
                &v.mileage_unit,
            ));
        }
        let oil = slots
            .iter()
            .find(|n| n.slot == "oil")
            .cloned()
            .unwrap_or_else(|| {
                slot_next(
                    ("oil", None),
                    &recs,
                    None,
                    &categories,
                    &services,
                    &v.mileage_unit,
                )
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

/// Когда следующая замена — только пробег (ADR-115): «на 93 000 км» (у машины в милях — мили);
/// дата — только у строки без пробега: «по сроку до 12.01.2027». Пусто — срока нет.
fn when_text(mileage: Option<i32>, unit: &str, date: Option<NaiveDate>) -> String {
    match (mileage, date) {
        (Some(v), _) => format!("на {}", crate::domain::mileage::text(v, unit)),
        (None, Some(d)) => format!("по сроку до {}", d.format("%d.%m.%Y")),
        (None, None) => String::new(),
    }
}

/// «на 93 000 км» — следующая замена строки для бота; без пробега — «по сроку до 12.01.2027»;
/// пусто — срока нет (ADR-115).
pub fn next_text(n: &SlotNext) -> String {
    when_text(n.next_mileage, &n.unit, n.next_date)
}

/// Следующая замена по строкам книжки для бота: по строке на каждую, у которой есть срок.
pub fn next_lines(b: &VehicleBook, indent: &str) -> Vec<String> {
    b.slots
        .iter()
        .filter_map(|n| {
            let t = next_text(n);
            (!t.is_empty()).then(|| format!("{indent}{}: {t}", n.title))
        })
        .collect()
}

/// Строка книжки и услуга «прочее» (ADR-131): у «прочего» следующая замена своя у каждой услуги,
/// а строки без услуги (из тетради) — общие «Прочие работы».
pub type SlotKey<'a> = (&'a str, Option<Uuid>);

fn line_of(l: &BookLine, (slot, service): SlotKey) -> bool {
    l.slot == slot && (slot != "other" || l.service_id == service)
}

/// Ключ строки в напоминаниях: строка книжки, у услуги «прочее» — «other:<услуга>».
pub fn slot_key(n: &SlotNext) -> String {
    match n.service_id {
        Some(id) => format!("{}:{id}", n.slot),
        None => n.slot.clone(),
    }
}

/// Последняя замена строки (как её выбирает `slot_next`): свежая запись с этой строкой, в ней —
/// строка с целью кассира, иначе первая.
fn last_line<'a>(b: &'a VehicleBook, key: SlotKey) -> Option<(&'a OilRecord, &'a BookLine)> {
    b.records.iter().find_map(|r| {
        let mine = || r.lines.iter().filter(|l| line_of(l, key));
        mine()
            .find(|l| l.target_set)
            .or_else(|| mine().next())
            .map(|l| (r, l))
    })
}

/// «Сейчас в машине» для бота (SPEC-36 §6): по каждой строке книжки, где уже была замена, —
/// что залито или поставлено (подпись товаром, задача 003 Н5), дата, пробег и следующая замена
/// (только пробег, ADR-115).
/// «  Mobil ATF 3309 · 1 л — 26.10.2026, 250 000 км → следующая на 290 000 км»; услуга «прочее» —
/// своим названием: «  Замена ГУР — 27.10.2026, 217 000 км → следующая на 150 000 км».
pub fn now_lines(b: &VehicleBook, indent: &str) -> Vec<String> {
    b.slots
        .iter()
        .filter_map(|n| {
            let date = n.last_date?;
            let last = last_line(b, (&n.slot, n.service_id));
            let mut when = vec![date.format("%d.%m.%Y").to_string()];
            if let Some((r, _)) = last
                && let Some(m) = r.mileage
            {
                when.push(crate::domain::mileage::text(m, &r.mileage_unit));
            }
            let mut line = format!("{indent}{} — {}", n.title, when.join(", "));
            let next = next_text(n);
            if !next.is_empty() {
                line.push_str(&format!(" → следующая {next}"));
            }
            Some(line)
        })
        .collect()
}

/// Строки замены для бота: что залито или поставлено (подпись товаром, задача 003 Н5) и цель.
pub fn record_lines(r: &OilRecord, indent: &str) -> Vec<String> {
    r.lines
        .iter()
        .map(|l| {
            let next = line_next_text(l);
            if next.is_empty() {
                format!("{indent}{}", line_title(l))
            } else {
                format!("{indent}{} — {next}", line_title(l))
            }
        })
        .collect()
}

/// «следующая замена на 225 000 км» — цель кассира у строки замены (SPEC-34), только пробег
/// (ADR-115); без пробега — «по сроку до …»; без цели — пусто.
pub fn line_next_text(l: &BookLine) -> String {
    let when = when_text(
        l.next_mileage,
        l.next_unit.as_deref().unwrap_or("km"),
        l.next_date,
    );
    if when.is_empty() {
        return when;
    }
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
/// любой категории с этой строкой, иначе по умолчанию. У услуги «прочее» (ADR-131) — только её
/// замены и её интервал, подпись — её название.
fn slot_next(
    key: SlotKey,
    recs: &[OilRecord],
    own: Option<(i32, i32)>,
    categories: &[CategoryInterval],
    services: &HashMap<Uuid, (i32, i32)>,
    unit: &str,
) -> SlotNext {
    let (slot, service) = key;
    // Записи свежие сверху: первая со строкой — последняя замена; в ней строка с целью — первой.
    let last = recs.iter().find_map(|r| {
        let mine = || r.lines.iter().filter(|l| line_of(l, key));
        mine()
            .find(|l| l.target_set)
            .or_else(|| mine().next())
            .map(|l| (r, l.clone()))
    });
    let last_km = recs
        .iter()
        .filter(|r| r.lines.iter().any(|l| line_of(l, key)))
        .find_map(|r| r.mileage_km);
    let by_service = last
        .as_ref()
        .and_then(|(_, l)| l.service_id)
        .and_then(|id| services.get(&id).copied());
    let interval = match service {
        Some(_) => by_service,
        None => own
            .or(by_service)
            .or_else(|| {
                category_interval(
                    categories,
                    slot,
                    last.as_ref().and_then(|(_, l)| l.category_id),
                )
            })
            .or_else(|| slot_default(slot)),
    };
    let label = match (service, &last) {
        (Some(_), Some((_, l))) => l.service.clone().unwrap_or_else(|| l.text.clone()),
        _ => slot_label(slot).to_string(),
    };
    // Что залито или поставлено в последнюю замену строки — все её товары (задача 003 Н5).
    let (title, products) = match &last {
        Some((r, _)) if service.is_none() => {
            let mine = || r.lines.iter().filter(|l| line_of(l, key));
            let mut products: Vec<String> = Vec::new();
            for p in mine().filter_map(product_name) {
                if !products.contains(&p) {
                    products.push(p);
                }
            }
            (titles(mine()), products)
        }
        _ => (label.clone(), Vec::new()),
    };
    let in_unit = |k: Option<i32>| k.and_then(|k| crate::domain::mileage::from_km(k, unit));
    // Цель, которую поставил сервер (ADR-144), — тот же расчёт по интервалу: владелец поменял
    // интервал машины или категории — следующая замена пересчитывается, как без цели.
    let target = last
        .as_ref()
        .map(|(_, l)| l)
        .filter(|l| l.target_set && !l.target_auto);
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
        service_id: service,
        label,
        title,
        products,
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

/// Масляный, воздушный и салонный фильтр, поставленный с заменой масла по серверной цели
/// (ADR-117, ADR-144), — на интервал масла от своей замены; свой интервал машины у фильтра —
/// по нему (`slot_next` уже посчитал).
fn fill_auto_follow(slots: &mut [SlotNext], recs: &[OilRecord], unit: &str) {
    let Some(oil) = slots
        .iter()
        .find(|n| n.slot == "oil" && n.service_id.is_none())
        .map(|n| (n.interval_km, n.interval_days))
    else {
        return;
    };
    for n in slots
        .iter_mut()
        .filter(|n| follows_oil(&n.slot) && !n.own_interval)
    {
        let auto = recs.iter().find_map(|r| {
            let mine = || r.lines.iter().filter(|l| l.slot == n.slot);
            mine()
                .find(|l| l.target_set)
                .or_else(|| mine().next())
                .map(|l| l.target_set && l.target_auto)
        });
        if auto != Some(true) {
            continue;
        }
        let (Some(km), Some(days)) = oil else {
            continue;
        };
        n.interval_km = Some(km);
        n.interval_days = Some(days);
        n.next_km = n.last_km.map(|m| m.saturating_add(km));
        n.next_date = n
            .last_date
            .and_then(|d| d.checked_add_days(chrono::Days::new(u64::try_from(days).unwrap_or(31))));
        n.next_mileage = n
            .next_km
            .and_then(|k| crate::domain::mileage::from_km(k, unit));
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
    fn filter_slot_by_type_then_product_name() {
        // Тип решает; без типа в категории без выбранной строки — название товара (задача 003 Н5).
        let fuel = Some("Фильтр топливный WK 820");
        assert_eq!(
            product_slot("oil_filter", Some("воздушный"), fuel),
            Some("air_filter")
        );
        assert_eq!(product_slot("oil_filter", None, fuel), Some("fuel_filter"));
        assert_eq!(
            product_slot("oil_filter", None, Some("Фильтр W712")),
            Some("oil_filter")
        );
        // Строку выбрал владелец — название не передаётся.
        assert_eq!(product_slot("oil_filter", None, None), Some("oil_filter"));
        assert_eq!(product_slot("oil", None, fuel), Some("oil"));
        assert_eq!(product_slot("none", None, fuel), None);
    }

    #[test]
    fn fluid_slot_by_product_name() {
        for (name, slot) in [
            ("Антифриз Sintec G12 красный 5 кг", Some("antifreeze")),
            ("ТОСОЛ А-40М 1 кг", Some("antifreeze")),
            ("Охлаждающая жидкость Felix", Some("antifreeze")),
            ("Тормозная жидкость DOT-4 0,5 л", Some("brake_fluid")),
            ("Омыватель стекла -20", None),
            ("Щётка стеклоочистителя", None),
            // ADR-131: класс жидкости без слова «тормозная» / «антифриз».
            ("FENIX DOT 4", Some("brake_fluid")),
            ("DOT-4 MANNOL 0.5", Some("brake_fluid")),
            ("Unix Dot4", Some("brake_fluid")),
            ("ABRO dot3 354 мл", Some("brake_fluid")),
            ("Motul DOT 5.1 0,5 л", Some("brake_fluid")),
            ("ROSDOT 4 455 г", Some("brake_fluid")),
            ("Жидкость ДОТ-4 Felix", Some("brake_fluid")),
            ("SIBIRIA GREEN G11", Some("antifreeze")),
            ("Sintec G12+", Some("antifreeze")),
            ("Felix Carbox g12++ 5 кг", Some("antifreeze")),
            ("Coolstream G13 1 кг", Some("antifreeze")),
            ("Hepu P999-G12", Some("antifreeze")),
            ("Щётка G-Force 600 мм", None),
            ("G-Energy Expert G 10W-40 4 л", None),
            ("G-Box ATF DX III", None),
            ("Лампа G4 12V", None),
            ("Кольцо SG12", None),
            ("Артикул G1205", None),
            ("Набор Dotted 3 шт", None),
            ("DOT 45 мм", None),
            ("Свеча NGK", None),
        ] {
            assert_eq!(slot_by_product_name(name), slot, "{name}");
        }
    }

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
            // ADR-117: мост, редуктор, раздатка и доливка — «прочее».
            ("Замена масла мех. моста", "other"),
            ("Замена масла в редукторе", "other"),
            ("Масло в раздатку", "other"),
            ("Доливка масла в мотор", "other"),
            ("Доливка антифриза", "other"),
            ("Замена тосола", "antifreeze"),
            ("Замена масла самостоятельно", "oil"),
            // ADR-131: тормозная — только со словом «жидк…», радиатор охлаждения — не антифриз.
            ("Замена тормозной жидкости", "brake_fluid"),
            ("Замена тормозных колодок", "other"),
            ("Прокачка тормозов", "other"),
            ("Замена радиатора охлаждения", "other"),
            ("Замена антифриза и радиатора", "antifreeze"),
            ("ЗАМЕНА МАСЛА В МОСТУ", "other"),
            ("Масло заднего моста и редуктора", "other"),
        ] {
            assert_eq!(service_slot_by_name(name), slot, "{name}");
        }
        for (name, axle) in [
            ("Замена масла мех. моста", true),
            ("Мостовой редуктор", true),
            ("Масло в раздатку", true),
            ("Замена масла самостоятельно", false),
            ("Замена масла в коробке", false),
        ] {
            assert_eq!(axle_or_transfer_by_name(name), axle, "{name}");
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
        // Только пробег (ADR-115): дата — только у строки без пробега.
        assert_eq!(line_next_text(&l), "следующая замена на 225 000 км");
        l.next_unit = Some("mi".into());
        l.next_date = None;
        assert_eq!(line_next_text(&l), "следующая замена на 225 000 миль");
        l.next_mileage = None;
        l.next_date = NaiveDate::from_ymd_opt(2027, 1, 5);
        assert_eq!(
            line_next_text(&l),
            "следующая замена по сроку до 05.01.2027"
        );
    }

    #[test]
    fn weight_and_length_are_not_pieces() {
        let part = |kind: &str, qty| QtyPart {
            kind: kind.into(),
            qty,
            container_ml: None,
        };
        assert_eq!(merged_qty_text(&[part("weight", 2_500)], "total"), "2,5 кг");
        assert_eq!(
            merged_qty_text(&[part("length", 1_250), part("length", 750)], "total"),
            "2 м"
        );
        assert_eq!(
            merged_qty_text(&[part("piece", 1), part("piece", 2)], "split"),
            "3 шт (1 шт + 2 шт)"
        );
    }
}
