//! Сообщения клиентам (SPEC-32, ADR-107): напоминание о замене масла по среднему пробегу и
//! сообщение юрлицу о закупе — в Телеграм (клиент подключён к боту) или в WhatsApp Business
//! Platform (Cloud API Meta) по одобренному шаблону. Нет ни того ни другого — запись «не
//! отправлено» в журнале сообщений.
//!
//! Сообщение сначала составляется (строка `client_messages` и ключ повода в
//! `client_message_keys` — одной транзакцией: один повод — один раз), потом отправляется. И то и
//! другое — только в окно 09:00–20:00 по Бишкеку (ADR-063): чек фирмы ночью — сообщение утром.

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use crate::api::oil_book::VehicleBook;
use crate::api::paging::{PageQuery, Paged, paged};
use crate::api::telegram::{self, Delivery};
use crate::auth::{Ctx, Owner};
use crate::config::SecretKey;
use crate::domain::mileage;
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings/client-messages",
            routing::get(get_settings).put(put_settings),
        )
        .route("/client-messages", routing::get(journal))
        .route("/client-messages/test", routing::post(test_send))
        .route("/parties/{id}/bot-link", routing::get(bot_link))
        .route(
            "/parties/{id}/bot-link/reset",
            routing::post(reset_bot_link),
        )
}

/// Адрес Graph API по умолчанию (версия — как в документации Meta на 24.10.2026).
pub const DEFAULT_WHATSAPP_URL: &str = "https://graph.facebook.com/v26.0";

/// Напоминание о замене — за столько дней до ожидаемой даты (SPEC-32 §3).
pub const REMIND_DAYS_BEFORE: u64 = 3;
/// Другие строки книжки, срок которых подходит в эти дни после главной, — в том же сообщении.
pub const TOGETHER_DAYS: u64 = 14;
/// Пропущенное напоминание (сервер был выключен) догоняется ещё столько дней после срока.
pub const LATE_DAYS: u64 = 3;
/// Средний пробег — по записям за последние два года, первая и последняя — не ближе недели.
pub const RATE_WINDOW_DAYS: u64 = 730;
pub const RATE_MIN_SPAN_DAYS: i64 = 7;
/// Пробег в день по умолчанию (одна запись в книжке), км.
pub const DEFAULT_KM_PER_DAY: i32 = 50;
/// Сколько раз пробовать отправить при временной ошибке, потом — «ошибка».
const MAX_ATTEMPTS: i32 = 30;
/// Сообщений за один проход отправки.
const SEND_BATCH: i64 = 50;
/// Чем подменить пустую переменную шаблона: Meta не принимает пустые значения.
const EMPTY_PARAM: &str = "—";

// ---------- WhatsApp Business Platform ----------

/// Подключение к Cloud API Meta: `POST {url}/{phone_id}/messages` с ключом в заголовке
/// `Authorization: Bearer`. Ключ не печатается и не пишется в журнал.
#[derive(Clone)]
pub struct WhatsApp {
    http: reqwest::Client,
    url: String,
    phone_id: String,
    token: SecretKey,
}

impl std::fmt::Debug for WhatsApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WhatsApp")
            .field("url", &self.url)
            .field("phone_id", &self.phone_id)
            .finish_non_exhaustive()
    }
}

/// Что ответил канал на одно сообщение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Принято; номер сообщения у Meta (у Телеграма — пусто).
    Sent(String),
    /// Не примут никогда (неверный номер, шаблон не одобрен, бот заблокирован) — «ошибка».
    Rejected(String),
    /// Попробовать позже: перегрузка, сбой, нет связи, ключ не подходит.
    Retry(Duration, String),
}

/// Значение переменной шаблона: одной строкой (Meta не принимает переводы строк, табуляцию и
/// больше четырёх пробелов подряд), не пустое.
pub fn template_param(s: &str) -> String {
    let line = crate::api::bot_drafts::one_line(s);
    let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if squeezed.is_empty() {
        EMPTY_PARAM.to_string()
    } else {
        squeezed.chars().take(900).collect()
    }
}

/// Тело запроса Cloud API на сообщение по шаблону.
pub fn template_body(to: &str, name: &str, language: &str, params: &[String]) -> Value {
    let mut template = json!({ "name": name, "language": { "code": language } });
    if !params.is_empty() {
        template["components"] = json!([{
            "type": "body",
            "parameters": params
                .iter()
                .map(|p| json!({ "type": "text", "text": template_param(p) }))
                .collect::<Vec<_>>()
        }]);
    }
    json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": to,
        "type": "template",
        "template": template
    })
}

/// Ответ Cloud API → итог. Ошибка Meta: `{"error":{"message","code","error_data":{"details"}}}`.
pub fn classify_whatsapp(status: u16, body: &Value) -> Outcome {
    if (200..300).contains(&status) {
        return match body
            .pointer("/messages/0/id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(id) => Outcome::Sent(id.to_string()),
            None => Outcome::Rejected("непонятный ответ WhatsApp: нет номера сообщения".into()),
        };
    }
    let err = body.get("error");
    let code = err.and_then(|e| e.get("code")).and_then(Value::as_i64);
    let message = err
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let details = err
        .and_then(|e| e.pointer("/error_data/details"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut why = match (message.is_empty(), details.is_empty()) {
        (true, true) => format!("WhatsApp ответил {status}"),
        (false, true) => message.to_string(),
        (true, false) => details.to_string(),
        (false, false) => format!("{message}: {details}"),
    };
    if let Some(c) = code {
        why = format!("{why} (код {c})");
    }
    let why: String = crate::api::bot_drafts::one_line(&why)
        .chars()
        .take(300)
        .collect();
    // Ключ не подходит или истёк (190, 401) — ждём исправления настройки, сообщение не теряем.
    if status == 401 || code == Some(190) {
        return Outcome::Retry(
            Duration::from_secs(600),
            format!("ключ WhatsApp не подходит: {why}"),
        );
    }
    // Пределы отправки и временные сбои Meta.
    let temporary = matches!(
        code,
        Some(1 | 2 | 4 | 80007 | 130429 | 131000 | 131016 | 131048 | 131056 | 133004)
    );
    if status == 429 || status >= 500 || temporary {
        return Outcome::Retry(Duration::from_secs(60), why);
    }
    Outcome::Rejected(why)
}

impl WhatsApp {
    pub fn new(token: SecretKey, phone_id: String, url: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url
                .unwrap_or_else(|| DEFAULT_WHATSAPP_URL.into())
                .trim_end_matches('/')
                .to_string(),
            phone_id,
            token,
        }
    }

    /// Сообщение по шаблону на номер `to` («+996…»).
    pub async fn send_template(
        &self,
        to: &str,
        name: &str,
        language: &str,
        params: &[String],
    ) -> Outcome {
        let body = template_body(to, name, language, params);
        let r = self
            .http
            .post(format!("{}/{}/messages", self.url, self.phone_id))
            .bearer_auth(&self.token.0)
            .json(&body)
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        match r {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let v = resp.json::<Value>().await.unwrap_or(Value::Null);
                let out = classify_whatsapp(status, &v);
                if !matches!(out, Outcome::Sent(_)) {
                    tracing::warn!(status, "whatsapp: сообщение не принято");
                }
                out
            }
            Err(e) => {
                let kind = if e.is_timeout() {
                    "таймаут"
                } else if e.is_connect() {
                    "нет соединения"
                } else {
                    "ошибка запроса"
                };
                tracing::warn!(error = kind, "whatsapp: нет связи");
                Outcome::Retry(
                    Duration::from_secs(60),
                    format!("нет связи с WhatsApp: {kind}"),
                )
            }
        }
    }
}

// ---------- Настройка «Сообщения клиентам» ----------

/// Настройка филиала (ключ `client_messages` в `settings`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MessageSettings {
    /// WhatsApp включён владельцем (при заданных на сервере ключе и номере).
    #[serde(default = "yes")]
    pub whatsapp_enabled: bool,
    /// Шаблон Meta «замена масла»: имя и язык.
    #[serde(default)]
    pub oil_template: String,
    #[serde(default = "ru")]
    pub oil_language: String,
    /// Шаблон Meta «закуп юрлица».
    #[serde(default)]
    pub purchase_template: String,
    #[serde(default = "ru")]
    pub purchase_language: String,
    /// Пробег в день, если в книжке одна запись, — км.
    #[serde(default = "default_km")]
    pub km_per_day: i32,
}

fn yes() -> bool {
    true
}
fn ru() -> String {
    "ru".into()
}
fn default_km() -> i32 {
    DEFAULT_KM_PER_DAY
}

impl Default for MessageSettings {
    fn default() -> Self {
        Self {
            whatsapp_enabled: true,
            oil_template: String::new(),
            oil_language: ru(),
            purchase_template: String::new(),
            purchase_language: ru(),
            km_per_day: DEFAULT_KM_PER_DAY,
        }
    }
}

impl MessageSettings {
    /// Шаблон и язык для вида сообщения; `None` — шаблон не задан.
    fn template(&self, kind: Kind) -> Option<(&str, &str)> {
        let (name, lang) = match kind {
            Kind::Oil => (&self.oil_template, &self.oil_language),
            Kind::Purchase => (&self.purchase_template, &self.purchase_language),
        };
        (!name.is_empty()).then_some((name.as_str(), lang.as_str()))
    }
}

pub async fn load_settings(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<MessageSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'client_messages'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

fn valid_template_name(s: &str) -> bool {
    s.len() <= 512
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_language(s: &str) -> bool {
    (2..=10).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_')
}

// ---------- Средний пробег и срок напоминания ----------

/// Заезд из книжки: дата и пробег в км.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visit {
    pub date: NaiveDate,
    pub km: i32,
}

/// Пробег в день дробью «км за дней» — чтобы считать в целых (инвариант 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub km: i64,
    pub days: i64,
    /// Посчитан по книжке, а не взят по умолчанию.
    pub measured: bool,
}

/// Средний пробег машины в день: по заездам за последние два года, если их не меньше двух и
/// между первым и последним не меньше 7 дней; иначе — по умолчанию.
pub fn daily_rate(visits: &[Visit], today: NaiveDate, default_km_per_day: i32) -> Rate {
    let fallback = Rate {
        km: i64::from(default_km_per_day.max(1)),
        days: 1,
        measured: false,
    };
    let since = today
        .checked_sub_days(Days::new(RATE_WINDOW_DAYS))
        .unwrap_or(today);
    let recent: Vec<&Visit> = visits.iter().filter(|v| v.date >= since).collect();
    if recent.len() < 2 {
        return fallback;
    }
    let first = recent.iter().min_by_key(|v| (v.date, v.km));
    let last = recent.iter().max_by_key(|v| (v.date, v.km));
    let (Some(first), Some(last)) = (first, last) else {
        return fallback;
    };
    let days = (last.date - first.date).num_days();
    let km = i64::from(last.km) - i64::from(first.km);
    if days < RATE_MIN_SPAN_DAYS || km <= 0 {
        return fallback;
    }
    Rate {
        km,
        days,
        measured: true,
    }
}

/// Последний заезд с пробегом: самый поздний, при равной дате — с большим пробегом.
pub fn last_visit(visits: &[Visit]) -> Option<Visit> {
    visits.iter().copied().max_by_key(|v| (v.date, v.km))
}

/// Когда машина дойдёт до пробега `target`: дата заезда + (цель − пробег) / пробег в день,
/// с округлением вверх до целого дня. Дальше 10 лет не считаем.
pub fn expected_date(last: Visit, target_km: i32, rate: Rate) -> NaiveDate {
    let left = (i64::from(target_km) - i64::from(last.km)).max(0);
    let km = rate.km.max(1);
    let days = (left.saturating_mul(rate.days.max(1)) + km - 1) / km;
    let days = u64::try_from(days.clamp(0, 3650)).unwrap_or(0);
    last.date
        .checked_add_days(Days::new(days))
        .unwrap_or(last.date)
}

/// Срок строки книжки по пробегу.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDue {
    pub slot: String,
    /// Цель в км (ключ повода) и в единице машины (текст).
    pub target_km: i32,
    pub target: i32,
    pub unit: String,
    /// Ожидаемая дата по пробегу или срок по дням — что раньше.
    pub due: NaiveDate,
}

/// В книжке есть пробег: напоминание считается по нему (SPEC-32 §3), иначе — по дням (ADR-063).
pub fn has_mileage(b: &VehicleBook) -> bool {
    b.records.iter().any(|r| r.mileage_km.is_some())
}

/// Сроки строк книжки машины по среднему пробегу: у каждой строки с целевым пробегом.
pub fn slot_dues(b: &VehicleBook, today: NaiveDate, default_km_per_day: i32) -> Vec<SlotDue> {
    let visits: Vec<Visit> = b
        .records
        .iter()
        .filter_map(|r| {
            r.mileage_km.map(|km| Visit {
                date: r.change_date,
                km,
            })
        })
        .collect();
    let Some(last) = last_visit(&visits) else {
        return Vec::new();
    };
    let rate = daily_rate(&visits, today, default_km_per_day);
    b.slots
        .iter()
        .filter_map(|n| {
            let target_km = n.next_km?;
            let by_km = expected_date(last, target_km, rate);
            let due = n.next_date.map_or(by_km, |d| d.min(by_km));
            Some(SlotDue {
                slot: n.slot.clone(),
                target_km,
                target: n.next_mileage.unwrap_or(target_km),
                unit: n.unit.clone(),
                due,
            })
        })
        .collect()
}

/// Что напомнить: главная строка (масло двигателя, если его срок подошёл, иначе ближайшая) и
/// строки, чей срок в те же 14 дней.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OilPick {
    pub primary: SlotDue,
    pub extras: Vec<SlotDue>,
}

/// Подошёл ли срок напоминания: за 3 дня до даты и ещё 3 дня после (пропущенное догоняется).
fn remind_now(d: &SlotDue, today: NaiveDate) -> bool {
    let from = d
        .due
        .checked_sub_days(Days::new(REMIND_DAYS_BEFORE))
        .unwrap_or(d.due);
    let until = d
        .due
        .checked_add_days(Days::new(LATE_DAYS))
        .unwrap_or(d.due);
    (from..=until).contains(&today)
}

pub fn pick(
    dues: &[SlotDue],
    today: NaiveDate,
    sent: impl Fn(&SlotDue) -> bool,
) -> Option<OilPick> {
    let open: Vec<&SlotDue> = dues.iter().filter(|d| !sent(d)).collect();
    let ready: Vec<&SlotDue> = open
        .iter()
        .copied()
        .filter(|d| remind_now(d, today))
        .collect();
    let primary = ready
        .iter()
        .find(|d| d.slot == "oil")
        .or_else(|| ready.iter().min_by_key(|d| d.due))
        .copied()?
        .clone();
    let late_limit = today
        .checked_sub_days(Days::new(LATE_DAYS))
        .unwrap_or(today);
    let together = primary
        .due
        .checked_add_days(Days::new(TOGETHER_DAYS))
        .unwrap_or(primary.due);
    let extras = open
        .into_iter()
        .filter(|d| d.slot != primary.slot && d.due <= together && d.due >= late_limit)
        .cloned()
        .collect();
    Some(OilPick { primary, extras })
}

/// Ключ повода «замена»: вид + клиент + машина + строка + цель.
pub fn oil_key(party_id: Uuid, vehicle_id: Uuid, d: &SlotDue) -> String {
    format!("oil:{party_id}:{vehicle_id}:{}:{}", d.slot, d.target_km)
}

// ---------- Тексты ----------

/// Что менять — в напоминании: «масло», «масло в коробке», «воздушный фильтр».
pub fn slot_noun(slot: &str) -> &'static str {
    match slot {
        "oil" => "масло",
        "gear_oil" => "масло в коробке",
        "oil_filter" => "масляный фильтр",
        "air_filter" => "воздушный фильтр",
        "cabin_filter" => "салонный фильтр",
        "fuel_filter" => "топливный фильтр",
        _ => "прочие работы",
    }
}

/// «Toyota Camry 01KG123ABC».
pub fn car_text(brand: &str, model: &str, plate: &str) -> String {
    [brand, model, plate]
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Сумма для клиента: «1 000 сом», «1 234,50 сом».
pub fn som(t: i64) -> String {
    let s = crate::domain::money::format_som(t);
    format!("{} сом", s.trim_end_matches(" с"))
}

/// Переменные шаблона «замена масла» — по порядку (экран настройки показывает тот же порядок).
pub const OIL_PARAMS: [&str; 5] = [
    "имя клиента",
    "что менять (масло)",
    "машина: марка, модель, госномер",
    "пробег и остальное («201 748 км. А масло в коробке — на 230 748 км»)",
    "ссылка на бот",
];

/// Переменные шаблона «закуп юрлица» — по порядку.
pub const PURCHASE_PARAMS: [&str; 7] = [
    "имя и фирма («Азиз (ОсОО Бишкек такси)»)",
    "когда и что («Сегодня был закуп», «Вчера был возврат»)",
    "сумма («1 000 сом»)",
    "водитель",
    "машина и госномер",
    "общий долг («6 240 сом»)",
    "ссылка на кабинет или на бот",
];

/// Напоминание о замене: части текста.
#[derive(Debug, Clone)]
pub struct OilText {
    pub name: String,
    pub car: String,
    pub pick: OilPick,
}

impl OilText {
    fn what(&self) -> &'static str {
        slot_noun(&self.pick.primary.slot)
    }

    /// «201 748 км. А масло в коробке — на 230 748 км».
    fn tail(&self) -> String {
        let p = &self.pick.primary;
        let mut out = mileage::text(p.target, &p.unit);
        if !self.pick.extras.is_empty() {
            let rest = self
                .pick
                .extras
                .iter()
                .map(|e| {
                    format!(
                        "{} — на {}",
                        slot_noun(&e.slot),
                        mileage::text(e.target, &e.unit)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(". А {rest}"));
        }
        out
    }

    /// Текст для Телеграма: без ссылки — клиент уже в боте.
    pub fn telegram(&self) -> String {
        format!(
            "{}, здравствуйте! Магазин Avtodom. Не забудьте поменять {} на {} на пробеге {}.",
            self.name,
            self.what(),
            self.car,
            self.tail()
        )
    }

    pub fn params(&self, link: &str) -> Vec<String> {
        vec![
            self.name.clone(),
            self.what().to_string(),
            self.car.clone(),
            self.tail(),
            link.to_string(),
        ]
    }
}

/// Сообщение фирме о закупе: части текста.
#[derive(Debug, Clone)]
pub struct PurchaseText {
    /// «Азиз (ОсОО Бишкек такси)» или название фирмы.
    pub name: String,
    pub is_return: bool,
    /// Чек был сегодня, вчера или раньше (дата).
    pub day: DayWord,
    pub amount_tyiyn: i64,
    pub driver: String,
    pub car: String,
    pub debt_tyiyn: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DayWord {
    Today,
    Yesterday,
    On(NaiveDate),
}

impl DayWord {
    pub fn of(day: NaiveDate, today: NaiveDate) -> Self {
        match (today - day).num_days() {
            0 => Self::Today,
            1 => Self::Yesterday,
            _ => Self::On(day),
        }
    }
}

impl PurchaseText {
    /// «Сегодня был закуп», «Вчера был возврат», «22.10.2026 был закуп».
    fn event(&self) -> String {
        let when = match &self.day {
            DayWord::Today => "Сегодня".to_string(),
            DayWord::Yesterday => "Вчера".to_string(),
            DayWord::On(d) => d.format("%d.%m.%Y").to_string(),
        };
        format!(
            "{when} был {}",
            if self.is_return {
                "возврат"
            } else {
                "закуп"
            }
        )
    }

    fn debt(&self) -> String {
        som(self.debt_tyiyn.max(0))
    }

    pub fn telegram(&self, link: Option<&str>) -> String {
        let mut s = format!(
            "{}, здравствуйте! {} на сумму {}.",
            self.name,
            self.event(),
            som(self.amount_tyiyn.abs())
        );
        match (self.driver.is_empty(), self.car.is_empty()) {
            (false, false) => {
                s.push_str(&format!(" Водитель {}, машина {}.", self.driver, self.car))
            }
            (false, true) => s.push_str(&format!(" Водитель {}.", self.driver)),
            (true, false) => s.push_str(&format!(" Машина {}.", self.car)),
            (true, true) => {}
        }
        if self.debt_tyiyn > 0 {
            s.push_str(&format!(" Общий долг на сегодня — {}.", self.debt()));
        } else if self.debt_tyiyn < 0 {
            s.push_str(&format!(" Долга нет, аванс {}.", som(-self.debt_tyiyn)));
        } else {
            s.push_str(" Долга на сегодня нет.");
        }
        if let Some(l) = link {
            s.push_str(&format!(
                " Для полного ознакомления пройдите в личный кабинет: {l}"
            ));
        }
        s
    }

    pub fn params(&self, link: &str) -> Vec<String> {
        vec![
            self.name.clone(),
            self.event(),
            som(self.amount_tyiyn.abs()),
            if self.driver.is_empty() {
                "не указан".into()
            } else {
                self.driver.clone()
            },
            if self.car.is_empty() {
                "не указана".into()
            } else {
                self.car.clone()
            },
            self.debt(),
            link.to_string(),
        ]
    }
}

/// Текст шаблона с подставленными переменными — для журнала: что увидел клиент. Сами шаблоны
/// подаются в Meta этим же текстом с `{{1}}…` (экран настройки, `web/src/lib/clientMessages.ts`).
fn preview(kind: Kind, params: &[String]) -> String {
    let p = |i: usize| params.get(i).map(String::as_str).unwrap_or(EMPTY_PARAM);
    match kind {
        Kind::Oil => format!(
            "Здравствуйте, {}! Магазин Avtodom. Не забудьте поменять {} на {} на пробеге {}. Бонусы нашего магазина и информация по ТО вашего авто — по ссылке: {}\nЖдём вас в Avtodom!",
            p(0),
            p(1),
            p(2),
            p(3),
            p(4)
        ),
        Kind::Purchase => format!(
            "Здравствуйте, {}! {} на сумму {}. Водитель {}, машина {}. Общий долг на сегодня — {}. Для полного ознакомления пройдите в личный кабинет: {}\nСпасибо, что вы с нами!",
            p(0),
            p(1),
            p(2),
            p(3),
            p(4),
            p(5),
            p(6)
        ),
    }
}

// ---------- Кому и каким каналом ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Oil,
    Purchase,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Oil => "oil",
            Self::Purchase => "purchase",
        }
    }
}

/// Каналы отправки, которые есть на сервере.
#[derive(Clone, Default)]
pub struct Channels {
    pub telegram: Option<telegram::Api>,
    pub whatsapp: Option<WhatsApp>,
    pub public_url: Option<String>,
}

impl Channels {
    pub fn from_state(state: &AppState) -> Self {
        Self {
            telegram: state.telegram_api.clone(),
            whatsapp: state.whatsapp.clone(),
            public_url: state.public_url.clone(),
        }
    }
}

/// Куда уходит сообщение.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Telegram(Vec<i64>),
    WhatsApp {
        phone: String,
        template: String,
        language: String,
    },
    /// Не отправляется: причина.
    None(String),
}

/// Канал: подключён к боту — Телеграм; иначе телефон и WhatsApp — WhatsApp; иначе ничего.
pub fn choose(
    ch: &Channels,
    settings: &MessageSettings,
    kind: Kind,
    chats: Vec<i64>,
    phone: Option<String>,
) -> Route {
    if ch.telegram.is_some() && !chats.is_empty() {
        return Route::Telegram(chats);
    }
    let wa = ch.whatsapp.is_some() && settings.whatsapp_enabled;
    let template = settings.template(kind);
    match (phone, wa, template) {
        (Some(phone), true, Some((name, lang))) => Route::WhatsApp {
            phone,
            template: name.to_string(),
            language: lang.to_string(),
        },
        (None, _, _) => Route::None(if chats.is_empty() {
            "клиент не подключён к боту, телефона нет".into()
        } else {
            "Телеграм-бот выключен на сервере, телефона нет".into()
        }),
        (Some(_), false, _) => Route::None(if chats.is_empty() {
            "клиент не подключён к боту, WhatsApp выключен".into()
        } else {
            "Телеграм-бот выключен на сервере, WhatsApp выключен".into()
        }),
        (Some(_), true, None) => Route::None(format!(
            "клиент не подключён к боту, шаблон WhatsApp «{}» не задан",
            match kind {
                Kind::Oil => "замена масла",
                Kind::Purchase => "закуп юрлица",
            }
        )),
    }
}

/// Телефон для WhatsApp: «+996555123456»; не номер — `None`.
pub fn wa_phone(phone: &str) -> Option<String> {
    crate::domain::phone::key(phone).map(|k| format!("+{k}"))
}

/// Ссылка на бот с кодом клиента; без имени бота — `None`.
pub async fn bot_url(conn: &mut PgConnection, code: &str) -> AppResult<Option<String>> {
    let username = sqlx::query_scalar!("select value from bot_state where key = 'username'")
        .fetch_optional(&mut *conn)
        .await?
        .and_then(|v| v.as_str().map(str::to_string));
    Ok(username.map(|u| format!("https://t.me/{u}?start={code}")))
}

/// Ссылки в сообщении нет — что подставить в шаблон.
const NO_LINK: &str = "спросите на точке";

/// Чаты бота клиента.
async fn party_chats(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Vec<i64>> {
    Ok(sqlx::query_scalar!(
        "select chat_id from telegram_customers where party_id = $1 order by linked_at",
        party_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Готовое сообщение для записи в журнал.
struct Draft {
    branch_id: Uuid,
    party_id: Uuid,
    vehicle_id: Option<Uuid>,
    sale_id: Option<Uuid>,
    kind: Kind,
    test: bool,
    user_id: Option<Uuid>,
    keys: Vec<String>,
    telegram_text: String,
    params: Vec<String>,
}

/// Записать сообщение (по строке на получателя) и ключи повода одной транзакцией. Повод уже
/// составлен (ключ занят) — `None`.
async fn store(conn: &mut PgConnection, d: &Draft, route: &Route) -> AppResult<Option<Vec<Uuid>>> {
    let mut tx = conn.begin().await?;
    let rows: Vec<(String, String, String, String, Value, &str, String)> = match route {
        Route::Telegram(chats) => chats
            .iter()
            .map(|c| {
                (
                    "telegram".to_string(),
                    c.to_string(),
                    d.telegram_text.clone(),
                    String::new(),
                    json!([]),
                    "pending",
                    String::new(),
                )
            })
            .collect(),
        Route::WhatsApp {
            phone,
            template,
            language,
        } => vec![(
            "whatsapp".to_string(),
            phone.clone(),
            preview(d.kind, &d.params),
            template.clone(),
            json!(
                d.params
                    .iter()
                    .map(|p| template_param(p))
                    .collect::<Vec<_>>()
            ),
            "pending",
            language.clone(),
        )],
        Route::None(why) => vec![(
            "none".to_string(),
            String::new(),
            d.telegram_text.clone(),
            String::new(),
            json!([]),
            "not_sent",
            why.clone(),
        )],
    };
    let mut ids = Vec::with_capacity(rows.len());
    for (channel, recipient, text, template, params, status, extra) in rows {
        let id = ops::new_id();
        let (language, error) = if channel == "whatsapp" {
            (extra, String::new())
        } else {
            (String::new(), extra)
        };
        sqlx::query!(
            r#"insert into client_messages (id, branch_id, party_id, vehicle_id, sale_id, kind, test,
                                            channel, recipient, text, template, language, params,
                                            status, error, user_id)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"#,
            id,
            d.branch_id,
            d.party_id,
            d.vehicle_id,
            d.sale_id,
            d.kind.as_str(),
            d.test,
            channel,
            recipient,
            text,
            template,
            language,
            params,
            status,
            error,
            d.user_id
        )
        .execute(&mut *tx)
        .await?;
        ids.push(id);
    }
    let Some(first) = ids.first().copied() else {
        return Ok(None);
    };
    for key in &d.keys {
        let taken = sqlx::query!(
            "insert into client_message_keys (key, message_id) values ($1, $2) on conflict do nothing",
            key,
            first
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 0;
        if taken {
            // Повод составили параллельно: ничего не записываем.
            return Ok(None);
        }
    }
    tx.commit().await?;
    Ok(Some(ids))
}

// ---------- Составление: замена масла ----------

/// Сегодня по Бишкеку.
async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Клиент машины: имя в обращении, телефон, код ссылки.
struct VehicleOwner {
    party_id: Uuid,
    name: String,
    phone: String,
    bot_code: String,
}

fn greeting_name(first_name: &str, name: &str) -> String {
    if first_name.trim().is_empty() {
        name.trim().to_string()
    } else {
        first_name.trim().to_string()
    }
}

/// Напоминания о замене по среднему пробегу: что пора — составить (SPEC-32 §3).
pub async fn plan_oil(conn: &mut PgConnection, ch: &Channels) -> AppResult<usize> {
    let today = today(conn).await?;
    let branches = sqlx::query_scalar!("select id from branches order by created_at")
        .fetch_all(&mut *conn)
        .await?;
    let mut made = 0;
    // Машины и книжки общие (SPEC-33): книжки читаются один раз, а настройки напоминаний — того
    // филиала, где машину записали; у машины один такой филиал, поэтому повторов нет.
    let all_books = crate::api::oil_book::books(conn, None, None).await?;
    for branch_id in branches {
        let settings = load_settings(conn, branch_id).await?;
        let owners = sqlx::query!(
            r#"select v.id as vehicle_id, p.id, p.name, p.first_name, p.phone, p.bot_code
               from party_vehicles v join parties p on p.id = v.party_id
               where v.branch_id = $1 and v.active and p.active and p.role = 'customer'"#,
            branch_id
        )
        .fetch_all(&mut *conn)
        .await?;
        if owners.is_empty() {
            continue;
        }
        for b in &all_books {
            let Some(o) = owners.iter().find(|o| o.vehicle_id == b.vehicle_id) else {
                continue;
            };
            let owner = VehicleOwner {
                party_id: o.id,
                name: greeting_name(&o.first_name, &o.name),
                phone: o.phone.clone(),
                bot_code: o.bot_code.clone(),
            };
            if oil_for_vehicle(conn, ch, &settings, branch_id, &owner, b, today).await? {
                made += 1;
            }
        }
    }
    Ok(made)
}

async fn oil_for_vehicle(
    conn: &mut PgConnection,
    ch: &Channels,
    settings: &MessageSettings,
    branch_id: Uuid,
    o: &VehicleOwner,
    b: &VehicleBook,
    today: NaiveDate,
) -> AppResult<bool> {
    let dues = slot_dues(b, today, settings.km_per_day);
    if dues.is_empty() {
        return Ok(false);
    }
    let keys: Vec<String> = dues
        .iter()
        .map(|d| oil_key(o.party_id, b.vehicle_id, d))
        .collect();
    let sent: Vec<String> = sqlx::query_scalar!(
        "select key from client_message_keys where key = any($1)",
        &keys
    )
    .fetch_all(&mut *conn)
    .await?;
    let Some(p) = pick(&dues, today, |d| {
        sent.contains(&oil_key(o.party_id, b.vehicle_id, d))
    }) else {
        return Ok(false);
    };
    let text = OilText {
        name: o.name.clone(),
        car: car_text(&b.brand, &b.model, &b.plate),
        pick: p.clone(),
    };
    let link = bot_url(conn, &o.bot_code).await?;
    let chats = party_chats(conn, o.party_id).await?;
    let route = choose(ch, settings, Kind::Oil, chats, wa_phone(&o.phone));
    let draft = Draft {
        branch_id,
        party_id: o.party_id,
        vehicle_id: Some(b.vehicle_id),
        sale_id: None,
        kind: Kind::Oil,
        test: false,
        user_id: None,
        keys: std::iter::once(&p.primary)
            .chain(p.extras.iter())
            .map(|d| oil_key(o.party_id, b.vehicle_id, d))
            .collect(),
        telegram_text: text.telegram(),
        params: text.params(link.as_deref().unwrap_or(NO_LINK)),
    };
    Ok(store(conn, &draft, &route).await?.is_some())
}

// ---------- Составление: закуп юрлица ----------

/// Руководитель фирмы: имя и телефон.
async fn firm_head(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Option<(String, String)>> {
    Ok(sqlx::query!(
        r#"select full_name, phone from party_contacts
           where party_id = $1 and active and is_head order by created_at limit 1"#,
        party_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|h| (h.full_name, h.phone)))
}

/// Чаты фирмы и её руководителя (его своя карточка клиента подключена к боту).
async fn firm_chats(
    conn: &mut PgConnection,
    party_id: Uuid,
    head_phone: Option<&str>,
) -> AppResult<Vec<i64>> {
    let key = head_phone.and_then(crate::domain::phone::key);
    Ok(sqlx::query_scalar!(
        r#"select distinct t.chat_id as "chat_id!" from telegram_customers t
           join parties p on p.id = t.party_id
           where p.active and p.role = 'customer'
             and (t.party_id = $1 or ($2::text is not null and phone_norm(p.phone) = $2))"#,
        party_id,
        key
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Ссылка в сообщении фирме: кабинет при `PUBLIC_URL`, иначе бот с кодом фирмы.
async fn firm_link(
    conn: &mut PgConnection,
    ch: &Channels,
    code: &str,
) -> AppResult<Option<String>> {
    match &ch.public_url {
        Some(u) => Ok(Some(format!("{u}/cabinet"))),
        None => bot_url(conn, code).await,
    }
}

/// Данные чека фирмы для сообщения.
struct FirmSale {
    sale_id: Uuid,
    branch_id: Uuid,
    party_id: Uuid,
    firm: String,
    phone: String,
    bot_code: String,
    is_return: bool,
    total: i64,
    day: NaiveDate,
    driver: String,
    car: String,
    debt: i64,
}

async fn firm_message(
    conn: &mut PgConnection,
    ch: &Channels,
    s: &FirmSale,
    today: NaiveDate,
) -> AppResult<(PurchaseText, Vec<i64>, Option<String>, Option<String>)> {
    let head = firm_head(conn, s.party_id).await?;
    let name = match &head {
        Some((h, _)) if !h.trim().is_empty() => format!("{} ({})", h.trim(), s.firm.trim()),
        _ => s.firm.trim().to_string(),
    };
    let text = PurchaseText {
        name,
        is_return: s.is_return,
        day: DayWord::of(s.day, today),
        amount_tyiyn: s.total,
        driver: s.driver.clone(),
        car: s.car.clone(),
        debt_tyiyn: s.debt,
    };
    let head_phone = head.as_ref().map(|(_, p)| p.as_str());
    let chats = firm_chats(conn, s.party_id, head_phone).await?;
    let phone = wa_phone(&s.phone).or_else(|| head_phone.and_then(wa_phone));
    let link = firm_link(conn, ch, &s.bot_code).await?;
    Ok((text, chats, phone, link))
}

/// Новые чеки фирм (продажа или возврат) — сообщения о закупе (SPEC-32 §4).
pub async fn plan_purchases(conn: &mut PgConnection, ch: &Channels) -> AppResult<usize> {
    let since =
        sqlx::query_scalar!("select value from bot_state where key = 'client_messages_since'")
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|v| v.as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()))
            .unwrap_or_else(Utc::now);
    let rows = sqlx::query!(
        r#"select s.id, s.branch_id, p.id as party_id, p.name, p.phone, p.bot_code, s.kind,
                  s.total_tyiyn, (s.created_at at time zone 'Asia/Bishkek')::date as "day!",
                  c.full_name as "driver?", v.brand as "brand?", v.model as "model?", v.plate as "plate?",
                  (select coalesce(sum(l.amount_tyiyn), 0) from party_ledger l
                   where l.party_id = p.id and l.created_at <= s.created_at)::bigint as "debt!"
           from sales s
           join parties p on p.id = s.party_id
           left join party_contacts c on c.id = s.contact_id
           left join party_vehicles v on v.id = s.vehicle_id
           where p.kind = 'company' and p.role = 'customer' and p.active and not p.no_purchase_messages
             and s.created_at > $1 and s.created_at > now() - interval '2 days'
             and not exists (select 1 from client_message_keys k where k.key = 'purchase:' || s.id::text)
           order by s.created_at
           limit 200"#,
        since
    )
    .fetch_all(&mut *conn)
    .await?;
    let today = today(conn).await?;
    let mut made = 0;
    for r in rows {
        let s = FirmSale {
            sale_id: r.id,
            branch_id: r.branch_id,
            party_id: r.party_id,
            firm: r.name,
            phone: r.phone,
            bot_code: r.bot_code,
            is_return: r.kind == "return",
            total: r.total_tyiyn,
            day: r.day,
            driver: r.driver.unwrap_or_default(),
            car: car_text(
                r.brand.as_deref().unwrap_or(""),
                r.model.as_deref().unwrap_or(""),
                r.plate.as_deref().unwrap_or(""),
            ),
            debt: r.debt,
        };
        let settings = load_settings(conn, s.branch_id).await?;
        let (text, chats, phone, link) = firm_message(conn, ch, &s, today).await?;
        let route = choose(ch, &settings, Kind::Purchase, chats, phone);
        let draft = Draft {
            branch_id: s.branch_id,
            party_id: s.party_id,
            vehicle_id: None,
            sale_id: Some(s.sale_id),
            kind: Kind::Purchase,
            test: false,
            user_id: None,
            keys: vec![format!("purchase:{}", s.sale_id)],
            telegram_text: text.telegram(ch.public_url.as_ref().and(link.as_deref())),
            params: text.params(link.as_deref().unwrap_or(NO_LINK)),
        };
        if store(conn, &draft, &route).await?.is_some() {
            made += 1;
        }
    }
    Ok(made)
}

// ---------- Отправка ----------

struct Pending {
    id: Uuid,
    channel: String,
    recipient: String,
    text: String,
    template: String,
    language: String,
    params: Value,
    attempts: i32,
}

async fn deliver(ch: &Channels, m: &Pending) -> Outcome {
    match m.channel.as_str() {
        "telegram" => {
            let Some(api) = &ch.telegram else {
                return Outcome::Rejected("Телеграм-бот выключен на сервере".into());
            };
            let Ok(chat) = m.recipient.parse::<i64>() else {
                return Outcome::Rejected("неверный чат".into());
            };
            match api.push(chat, &m.text).await {
                Delivery::Sent => Outcome::Sent(String::new()),
                Delivery::Skip => Outcome::Rejected(
                    "Телеграм не принял: клиент заблокировал бота или чат не найден".into(),
                ),
                Delivery::Retry { after, .. } => {
                    Outcome::Retry(after, "Телеграм временно недоступен".into())
                }
            }
        }
        "whatsapp" => {
            let Some(wa) = &ch.whatsapp else {
                return Outcome::Rejected("WhatsApp выключен на сервере".into());
            };
            let params: Vec<String> = m
                .params
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            wa.send_template(&m.recipient, &m.template, &m.language, &params)
                .await
        }
        _ => Outcome::Rejected("нет канала".into()),
    }
}

/// Записать итог отправки. `Retry` — следующая попытка позже, после 30-й — «ошибка».
async fn record(conn: &mut PgConnection, id: Uuid, attempts: i32, out: &Outcome) -> AppResult<()> {
    match out {
        Outcome::Sent(ext) => {
            sqlx::query!(
                r#"update client_messages set status = 'sent', sent_at = now(), external_id = $2,
                          error = '', attempts = attempts + 1
                   where id = $1"#,
                id,
                ext
            )
            .execute(&mut *conn)
            .await?;
        }
        Outcome::Rejected(why) => {
            sqlx::query!(
                "update client_messages set status = 'error', error = $2, attempts = attempts + 1 where id = $1",
                id,
                why
            )
            .execute(&mut *conn)
            .await?;
        }
        Outcome::Retry(after, why) => {
            let secs = i64::try_from(after.as_secs().max(60)).unwrap_or(60);
            let status = if attempts + 1 >= MAX_ATTEMPTS {
                "error"
            } else {
                "pending"
            };
            sqlx::query!(
                r#"update client_messages
                   set status = $2, error = $3, attempts = attempts + 1,
                       next_try_at = now() + $4::bigint * interval '1 second'
                   where id = $1"#,
                id,
                status,
                why,
                secs
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Отправить ждущие сообщения. Канал с временной ошибкой до следующего прохода не трогаем.
pub async fn send_pass(conn: &mut PgConnection, ch: &Channels) -> AppResult<usize> {
    let rows = sqlx::query!(
        r#"select id, channel, recipient, text, template, language, params, attempts
           from client_messages where status = 'pending' and next_try_at <= now()
           order by created_at limit $1"#,
        SEND_BATCH
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut paused: Vec<String> = Vec::new();
    let mut sent = 0;
    for r in rows {
        if paused.contains(&r.channel) {
            continue;
        }
        let m = Pending {
            id: r.id,
            channel: r.channel,
            recipient: r.recipient,
            text: r.text,
            template: r.template,
            language: r.language,
            params: r.params,
            attempts: r.attempts,
        };
        let out = deliver(ch, &m).await;
        if matches!(out, Outcome::Retry(..)) {
            paused.push(m.channel.clone());
        }
        if matches!(out, Outcome::Sent(_)) {
            sent += 1;
        }
        record(conn, m.id, m.attempts, &out).await?;
    }
    Ok(sent)
}

/// Один проход: в окно 09:00–20:00 — составить закупы (каждый раз), напоминания о замене
/// (если `oil`) и отправить ждущие.
pub async fn pass(conn: &mut PgConnection, ch: &Channels, hour: u32, oil: bool) -> AppResult<()> {
    if !telegram::in_send_window(hour) {
        return Ok(());
    }
    plan_purchases(conn, ch).await?;
    if oil {
        plan_oil(conn, ch).await?;
    }
    send_pass(conn, ch).await?;
    Ok(())
}

async fn bishkek_hour(conn: &mut PgConnection) -> AppResult<u32> {
    let h = sqlx::query_scalar!(
        r#"select extract(hour from now() at time zone 'Asia/Bishkek')::int as "h!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(u32::try_from(h).unwrap_or(0))
}

/// Фоновая отправка — если есть хоть один канал. Закупы — каждые 30 секунд, замена — раз в час.
pub fn spawn(pool: PgPool, state: &AppState) {
    let ch = Channels::from_state(state);
    if ch.telegram.is_none() && ch.whatsapp.is_none() {
        return;
    }
    tokio::spawn(run(pool, ch));
    tracing::info!("сообщения клиентам включены");
}

async fn run(pool: PgPool, ch: Channels) {
    let mut last_oil: Option<Instant> = None;
    loop {
        if let Ok(mut conn) = pool.acquire().await {
            let oil = last_oil.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600));
            match bishkek_hour(&mut conn).await {
                Ok(hour) => match pass(&mut conn, &ch, hour, oil).await {
                    Ok(()) => {
                        if oil && telegram::in_send_window(hour) {
                            last_oil = Some(Instant::now());
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "сообщения клиентам: проход не удался"),
                },
                Err(e) => tracing::warn!(error = %e, "сообщения клиентам: нет времени базы"),
            }
        }
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

// ---------- Экран владельца ----------

#[derive(Serialize)]
struct SettingsOut {
    #[serde(flatten)]
    settings: MessageSettings,
    /// На сервере заданы `WHATSAPP_TOKEN` и `WHATSAPP_PHONE_ID`.
    whatsapp_configured: bool,
    /// На сервере задан ключ бота.
    telegram: bool,
    /// Внешний адрес программы для ссылки на кабинет.
    public_url: Option<String>,
    /// Переменные шаблонов по порядку.
    oil_params: Vec<&'static str>,
    purchase_params: Vec<&'static str>,
}

fn settings_out(state: &AppState, settings: MessageSettings) -> SettingsOut {
    SettingsOut {
        settings,
        whatsapp_configured: state.whatsapp.is_some(),
        telegram: state.telegram_api.is_some(),
        public_url: state.public_url.clone(),
        oil_params: OIL_PARAMS.to_vec(),
        purchase_params: PURCHASE_PARAMS.to_vec(),
    }
}

async fn get_settings(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<SettingsOut>> {
    let mut conn = state.pool.acquire().await?;
    let s = load_settings(&mut conn, user.branch_id).await?;
    Ok(Json(settings_out(&state, s)))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(mut s): Json<MessageSettings>,
) -> AppResult<Json<SettingsOut>> {
    ctx.user.require_owner()?;
    s.oil_template = s.oil_template.trim().to_string();
    s.purchase_template = s.purchase_template.trim().to_string();
    s.oil_language = s.oil_language.trim().to_string();
    s.purchase_language = s.purchase_language.trim().to_string();
    if !valid_template_name(&s.oil_template) || !valid_template_name(&s.purchase_template) {
        return Err(invalid(
            "имя шаблона — как в WhatsApp Manager: строчные латинские буквы, цифры и «_»",
        ));
    }
    if !valid_language(&s.oil_language) || !valid_language(&s.purchase_language) {
        return Err(invalid("язык шаблона — код Meta, например ru или en_US"));
    }
    if !(1..=1000).contains(&s.km_per_day) {
        return Err(invalid("пробег в день — от 1 до 1000 км"));
    }
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    let before = load_settings(&mut tx, branch_id).await?;
    let value = serde_json::to_value(&s).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'client_messages', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != s {
        let old = serde_json::to_value(&before).map_err(|e| AppError::Internal(e.to_string()))?;
        ops::audit(
            &mut tx,
            &ctx,
            "client_message.settings",
            "settings",
            None,
            json!({ "changes": ops::changes(&old, &value) }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(settings_out(&state, s)))
}

#[derive(Serialize)]
pub struct MessageOut {
    id: Uuid,
    created_at: DateTime<Utc>,
    sent_at: Option<DateTime<Utc>>,
    kind: String,
    test: bool,
    channel: String,
    recipient: String,
    party_id: Option<Uuid>,
    party_name: Option<String>,
    sale_number: Option<i64>,
    text: String,
    template: String,
    params: Value,
    status: String,
    error: String,
}

#[derive(Deserialize)]
struct JournalQuery {
    limit: Option<i64>,
    offset: Option<i64>,
    party_id: Option<Uuid>,
}

async fn journal(
    State(state): State<AppState>,
    Owner(user): Owner,
    Query(q): Query<JournalQuery>,
) -> AppResult<Paged<MessageOut>> {
    let page = PageQuery {
        limit: q.limit,
        offset: q.offset,
    };
    let mut conn = state.pool.acquire().await?;
    let total = sqlx::query_scalar!(
        r#"select count(*) as "n!" from client_messages
           where branch_id = $1 and ($2::uuid is null or party_id = $2)"#,
        user.branch_id,
        q.party_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query_as!(
        MessageOut,
        r#"select m.id, m.created_at, m.sent_at, m.kind, m.test, m.channel, m.recipient,
                  m.party_id, p.name as "party_name?", s.number as "sale_number?", m.text,
                  m.template, m.params, m.status, m.error
           from client_messages m
           left join parties p on p.id = m.party_id
           left join sales s on s.id = m.sale_id
           where m.branch_id = $1 and ($2::uuid is null or m.party_id = $2)
           order by m.created_at desc, m.id desc
           limit $3 offset $4"#,
        user.branch_id,
        q.party_id,
        page.size(100),
        page.skip()
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(paged(rows, total))
}

#[derive(Deserialize)]
struct TestReq {
    party_id: Uuid,
    kind: String,
}

/// «Отправить пробное»: сообщение выбранному клиенту сейчас, без окна и без ключа повода —
/// по его книжке или последнему чеку, а если их нет — с примером.
async fn test_send(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<TestReq>,
) -> AppResult<Json<Vec<MessageOut>>> {
    ctx.user.require_owner()?;
    let kind = match req.kind.as_str() {
        "oil" => Kind::Oil,
        "purchase" => Kind::Purchase,
        _ => return Err(invalid("вид сообщения: oil или purchase")),
    };
    let ch = Channels::from_state(&state);
    let branch_id = ctx.user.branch_id;
    let mut conn = state.pool.acquire().await?;
    let p = sqlx::query!(
        r#"select id, kind, name, first_name, phone, bot_code from parties
           where id = $1 and role = 'customer'"#,
        req.party_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let settings = load_settings(&mut conn, branch_id).await?;
    let today = today(&mut conn).await?;
    let (route, telegram_text, params, vehicle_id) = match kind {
        Kind::Oil => {
            let books = crate::api::oil_book::books(&mut conn, Some(p.id), None).await?;
            let found = books.iter().find_map(|b| {
                let dues = slot_dues(b, today, settings.km_per_day);
                let primary = dues
                    .iter()
                    .find(|d| d.slot == "oil")
                    .or_else(|| dues.first())?
                    .clone();
                Some((b, primary))
            });
            let (car, primary, vehicle_id) = match found {
                Some((b, d)) => (
                    car_text(&b.brand, &b.model, &b.plate),
                    d,
                    Some(b.vehicle_id),
                ),
                None => (
                    "Toyota Camry 01KG123ABC".to_string(),
                    SlotDue {
                        slot: "oil".into(),
                        target_km: 201_748,
                        target: 201_748,
                        unit: "km".into(),
                        due: today,
                    },
                    None,
                ),
            };
            let text = OilText {
                name: greeting_name(&p.first_name, &p.name),
                car,
                pick: OilPick {
                    primary,
                    extras: Vec::new(),
                },
            };
            let link = bot_url(&mut conn, &p.bot_code).await?;
            let chats = party_chats(&mut conn, p.id).await?;
            (
                choose(&ch, &settings, kind, chats, wa_phone(&p.phone)),
                text.telegram(),
                text.params(link.as_deref().unwrap_or(NO_LINK)),
                vehicle_id,
            )
        }
        Kind::Purchase => {
            let last = sqlx::query!(
                r#"select s.id, s.kind, s.total_tyiyn, (s.created_at at time zone 'Asia/Bishkek')::date as "day!",
                          c.full_name as "driver?", v.brand as "brand?", v.model as "model?", v.plate as "plate?"
                   from sales s
                   left join party_contacts c on c.id = s.contact_id
                   left join party_vehicles v on v.id = s.vehicle_id
                   where s.party_id = $1 order by s.created_at desc limit 1"#,
                p.id
            )
            .fetch_optional(&mut *conn)
            .await?;
            let debt = sqlx::query_scalar!("select balance_tyiyn from parties where id = $1", p.id)
                .fetch_one(&mut *conn)
                .await?;
            let s = FirmSale {
                sale_id: last.as_ref().map_or_else(Uuid::nil, |l| l.id),
                branch_id,
                party_id: p.id,
                firm: p.name.clone(),
                phone: p.phone.clone(),
                bot_code: p.bot_code.clone(),
                is_return: last.as_ref().is_some_and(|l| l.kind == "return"),
                total: last.as_ref().map_or(100_000, |l| l.total_tyiyn),
                day: last.as_ref().map_or(today, |l| l.day),
                driver: last
                    .as_ref()
                    .and_then(|l| l.driver.clone())
                    .unwrap_or_default(),
                car: last
                    .as_ref()
                    .map(|l| {
                        car_text(
                            l.brand.as_deref().unwrap_or(""),
                            l.model.as_deref().unwrap_or(""),
                            l.plate.as_deref().unwrap_or(""),
                        )
                    })
                    .unwrap_or_default(),
                debt,
            };
            let (text, chats, phone, link) = firm_message(&mut conn, &ch, &s, today).await?;
            (
                choose(&ch, &settings, kind, chats, phone),
                text.telegram(ch.public_url.as_ref().and(link.as_deref())),
                text.params(link.as_deref().unwrap_or(NO_LINK)),
                None,
            )
        }
    };
    let draft = Draft {
        branch_id,
        party_id: p.id,
        vehicle_id,
        sale_id: None,
        kind,
        test: true,
        user_id: Some(ctx.user.id),
        keys: vec![format!("test:{}", ops::new_id())],
        telegram_text,
        params,
    };
    let ids = store(&mut conn, &draft, &route).await?.unwrap_or_default();
    // Пробное уходит сразу, без окна отправки.
    for id in &ids {
        let m = sqlx::query!(
            r#"select id, channel, recipient, text, template, language, params, attempts, status
               from client_messages where id = $1"#,
            id
        )
        .fetch_one(&mut *conn)
        .await?;
        if m.status != "pending" {
            continue;
        }
        let pending = Pending {
            id: m.id,
            channel: m.channel,
            recipient: m.recipient,
            text: m.text,
            template: m.template,
            language: m.language,
            params: m.params,
            attempts: m.attempts,
        };
        let out = deliver(&ch, &pending).await;
        // Пробное не повторяется: временная ошибка — тоже ошибка, владелец видит причину.
        let out = match out {
            Outcome::Retry(_, why) => Outcome::Rejected(why),
            o => o,
        };
        record(&mut conn, pending.id, pending.attempts, &out).await?;
    }
    let rows = sqlx::query_as!(
        MessageOut,
        r#"select m.id, m.created_at, m.sent_at, m.kind, m.test, m.channel, m.recipient,
                  m.party_id, p.name as "party_name?", s.number as "sale_number?", m.text,
                  m.template, m.params, m.status, m.error
           from client_messages m
           left join parties p on p.id = m.party_id
           left join sales s on s.id = m.sale_id
           where m.id = any($1) order by m.created_at, m.id"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut tx = conn.begin().await?;
    for r in &rows {
        ops::audit(
            &mut tx,
            &ctx,
            "client_message.send",
            "party",
            Some(p.id),
            json!({ "name": p.name, "kind": r.kind, "channel": r.channel, "status": r.status,
                    "error": r.error, "test": true }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Serialize)]
struct BotLinkOut {
    /// Ссылка на бот с кодом клиента; `None` — бот ещё не запускался (имя бота неизвестно).
    url: Option<String>,
}

async fn bot_link(
    State(state): State<AppState>,
    Owner(_user): Owner,
    Path(id): Path<Uuid>,
) -> AppResult<Json<BotLinkOut>> {
    let mut conn = state.pool.acquire().await?;
    let code = sqlx::query_scalar!(
        "select bot_code from parties where id = $1 and role = 'customer'",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(BotLinkOut {
        url: bot_url(&mut conn, &code).await?,
    }))
}

/// «Новая ссылка»: новый код, старая ссылка перестаёт работать (подключённые чаты остаются).
async fn reset_bot_link(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<BotLinkOut>> {
    ctx.user.require_owner()?;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query!(
        r#"update parties set bot_code = replace(gen_random_uuid()::text, '-', '')
           where id = $1 and role = 'customer'
           returning name, bot_code"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "client.bot_link_reset",
        "party",
        Some(id),
        json!({ "name": row.name }),
    )
    .await?;
    let url = bot_url(&mut tx, &row.bot_code).await?;
    tx.commit().await?;
    Ok(Json(BotLinkOut { url }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap_or_default()
    }

    fn due(slot: &str, target: i32, on: &str) -> SlotDue {
        SlotDue {
            slot: slot.into(),
            target_km: target,
            target,
            unit: "km".into(),
            due: d(on),
        }
    }

    #[test]
    fn average_mileage_from_book() {
        let today = d("2026-10-24");
        let visits = [
            Visit {
                date: d("2026-04-01"),
                km: 190_000,
            },
            Visit {
                date: d("2026-07-01"),
                km: 194_550,
            },
            Visit {
                date: d("2026-09-29"),
                km: 199_100,
            },
        ];
        let r = daily_rate(&visits, today, 50);
        assert_eq!((r.km, r.days, r.measured), (9_100, 181, true));
        // До 201 748 от 199 100 — 2 648 км, ≈ 50,3 км в день → 53 дня (вверх).
        let last = last_visit(&visits).unwrap_or(visits[0]);
        assert_eq!(expected_date(last, 201_748, r), d("2026-11-21"));
        // Одна запись или записи ближе недели — по умолчанию.
        assert!(!daily_rate(&visits[..1], today, 50).measured);
        let close = [
            visits[2],
            Visit {
                date: d("2026-10-02"),
                km: 199_400,
            },
        ];
        assert_eq!(
            daily_rate(&close, today, 40),
            Rate {
                km: 40,
                days: 1,
                measured: false
            }
        );
        // Записи старше двух лет не считаются.
        let old = [
            Visit {
                date: d("2023-01-01"),
                km: 100_000,
            },
            visits[2],
        ];
        assert!(!daily_rate(&old, today, 50).measured);
        // Цель уже пройдена — дата заезда.
        assert_eq!(expected_date(last, 150_000, r), d("2026-09-29"));
    }

    #[test]
    fn motor_and_gearbox_in_one_message() {
        let today = d("2026-10-24");
        let dues = [
            due("oil", 201_748, "2026-10-26"),
            due("gear_oil", 202_300, "2026-11-05"),
            due("air_filter", 230_000, "2027-06-01"),
        ];
        let p = pick(&dues, today, |_| false).unwrap_or(OilPick {
            primary: dues[2].clone(),
            extras: vec![],
        });
        assert_eq!(p.primary.slot, "oil");
        assert_eq!(p.extras.len(), 1);
        assert_eq!(p.extras[0].slot, "gear_oil");
        let t = OilText {
            name: "Азиз".into(),
            car: car_text("Toyota", "Camry", "01KG123ABC"),
            pick: p,
        };
        assert_eq!(
            t.telegram(),
            "Азиз, здравствуйте! Магазин Avtodom. Не забудьте поменять масло на Toyota Camry 01KG123ABC на пробеге 201 748 км. А масло в коробке — на 202 300 км."
        );
        let params = t.params("https://t.me/avtodom_bot?start=abc");
        assert_eq!(params.len(), OIL_PARAMS.len());
        assert_eq!(params[3], "201 748 км. А масло в коробке — на 202 300 км");
        // Рано — ничего; уже отправлено — ничего.
        assert!(pick(&dues, d("2026-10-20"), |_| false).is_none());
        assert!(pick(&dues[..1], today, |_| true).is_none());
        // Срок масла прошёл больше 3 дней назад — не догоняем.
        assert!(pick(&dues[..1], d("2026-10-30"), |_| false).is_none());
    }

    #[test]
    fn miles_in_text() {
        let p = OilPick {
            primary: SlotDue {
                slot: "oil".into(),
                target_km: 96_561,
                target: 60_000,
                unit: "mi".into(),
                due: d("2026-10-26"),
            },
            extras: vec![],
        };
        let t = OilText {
            name: "Азиз".into(),
            car: "Ford F-150 01KG777AAA".into(),
            pick: p,
        };
        assert!(t.telegram().ends_with("на пробеге 60 000 миль."));
    }

    #[test]
    fn purchase_text_like_customer_sample() {
        let t = PurchaseText {
            name: "Азиз (ОсОО Бишкек такси)".into(),
            is_return: false,
            day: DayWord::Today,
            amount_tyiyn: 100_000,
            driver: "Чынтемир".into(),
            car: "Sprinter 09KG111JJJ".into(),
            debt_tyiyn: 624_000,
        };
        assert_eq!(
            t.telegram(Some("https://avtodom.example/cabinet")),
            "Азиз (ОсОО Бишкек такси), здравствуйте! Сегодня был закуп на сумму 1 000 сом. Водитель Чынтемир, машина Sprinter 09KG111JJJ. Общий долг на сегодня — 6 240 сом. Для полного ознакомления пройдите в личный кабинет: https://avtodom.example/cabinet"
        );
        let back = PurchaseText {
            is_return: true,
            day: DayWord::of(d("2026-10-23"), d("2026-10-24")),
            amount_tyiyn: -50_050,
            driver: String::new(),
            car: String::new(),
            debt_tyiyn: 0,
            ..t
        };
        assert_eq!(
            back.telegram(None),
            "Азиз (ОсОО Бишкек такси), здравствуйте! Вчера был возврат на сумму 500,50 сом. Долга на сегодня нет."
        );
        let params = back.params("x");
        assert_eq!(params.len(), PURCHASE_PARAMS.len());
        assert_eq!(params[1], "Вчера был возврат");
        assert_eq!(params[3], "не указан");
    }

    #[test]
    fn whatsapp_body_and_answers() {
        let b = template_body(
            "+996555123456",
            "avtodom_oil",
            "ru",
            &["Азиз\nстрока".into(), String::new()],
        );
        assert_eq!(b["messaging_product"], "whatsapp");
        assert_eq!(b["type"], "template");
        assert_eq!(b["template"]["language"]["code"], "ru");
        assert_eq!(b["template"]["components"][0]["type"], "body");
        assert_eq!(
            b["template"]["components"][0]["parameters"][0]["text"],
            "Азиз строка"
        );
        assert_eq!(b["template"]["components"][0]["parameters"][1]["text"], "—");
        assert_eq!(
            classify_whatsapp(200, &json!({ "messages": [{ "id": "wamid.1" }] })),
            Outcome::Sent("wamid.1".into())
        );
        let bad = json!({ "error": { "message": "Template name does not exist", "code": 132001,
                                     "error_data": { "details": "template name (x) does not exist in ru" } } });
        assert!(
            matches!(classify_whatsapp(400, &bad), Outcome::Rejected(w) if w.contains("132001"))
        );
        let limit = json!({ "error": { "message": "rate", "code": 130429 } });
        assert!(matches!(classify_whatsapp(400, &limit), Outcome::Retry(..)));
        assert!(matches!(
            classify_whatsapp(401, &Value::Null),
            Outcome::Retry(..)
        ));
        assert_eq!(wa_phone("0555 12 34 56").as_deref(), Some("+996555123456"));
        assert_eq!(wa_phone(""), None);
    }

    #[test]
    fn screen_shows_the_same_template_text() {
        // Текст шаблона на экране (его владелец подаёт в Meta) и текст журнала — одни и те же.
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../web/src/lib/clientMessages.ts"
        );
        let Ok(ts) = std::fs::read_to_string(path) else {
            return;
        };
        for (kind, n) in [
            (Kind::Oil, OIL_PARAMS.len()),
            (Kind::Purchase, PURCHASE_PARAMS.len()),
        ] {
            let marks: Vec<String> = (1..=n).map(|i| format!("{{{{{i}}}}}")).collect();
            let text = preview(kind, &marks).replace('\n', "\\n");
            assert!(ts.contains(&format!("'{text}'")), "{text}");
        }
    }

    #[test]
    fn template_names_and_languages() {
        assert!(valid_template_name("avtodom_oil_change") && valid_template_name(""));
        assert!(!valid_template_name("Замена") && !valid_template_name("oil change"));
        assert!(valid_language("ru") && valid_language("en_US") && !valid_language("r"));
    }
}
