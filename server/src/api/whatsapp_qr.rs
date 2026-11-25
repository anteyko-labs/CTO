//! WhatsApp по QR-коду (задача 052, ADR-217): номер магазина подключается как связанное
//! устройство через шлюз `deploy/whatsapp-gate` (слушает только 127.0.0.1). Сообщения клиентам
//! уходят туда тем же текстом, что в Телеграм, с паузой и пределом в день; на входящие бот
//! отвечает меню цифрами (1 — долг, 2 — машины, 3 — баллы, 4 — администратор).

use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::client_messages::{self, Channels, Outcome, load_settings};
use crate::auth::{Ctx, Owner};
use crate::config::SecretKey;
use crate::domain::mileage;
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/whatsapp", routing::get(status))
        .route("/whatsapp/connect", routing::post(connect))
        .route("/whatsapp/logout", routing::post(logout))
        .route("/whatsapp/inbox", routing::get(inbox))
        .route("/whatsapp/gate-event", routing::post(gate_event))
}

/// Сообщений WhatsApp за один проход отправки (раз в 30 с) — с паузой между ними.
const PER_PASS: i64 = 2;

/// Клиент шлюза.
#[derive(Clone)]
pub struct WaGate {
    http: reqwest::Client,
    url: String,
    key: SecretKey,
    /// Пауза между сообщениями рассылки, мс: случайная в пределах (ADR-217: 8–15 с).
    pause_ms: (u64, u64),
}

impl std::fmt::Debug for WaGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaGate").field("url", &self.url).finish()
    }
}

impl WaGate {
    pub fn new(url: String, key: SecretKey) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            url: url.trim_end_matches('/').to_string(),
            key,
            pause_ms: (8_000, 15_000),
        }
    }

    /// Для тестов: без пауз.
    pub fn with_pause(mut self, from_ms: u64, to_ms: u64) -> Self {
        self.pause_ms = (from_ms, to_ms.max(from_ms));
        self
    }

    fn key_matches(&self, got: &str) -> bool {
        let want = self.key.0.as_bytes();
        let got = got.as_bytes();
        got.len() == want.len() && got.iter().zip(want).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value), String> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.url))
            .bearer_auth(&self.key.0);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req
            .send()
            .await
            .map_err(|e| format!("шлюз WhatsApp недоступен: {e}"))?;
        let code = res.status().as_u16();
        let v = res.json::<Value>().await.unwrap_or(Value::Null);
        Ok((code, v))
    }

    /// Текст на номер «+996…»: принято — `Sent`; номера нет в WhatsApp — `Rejected`; не подключён,
    /// сбой — `Retry` через минуту.
    pub async fn send(&self, to: &str, text: &str) -> Outcome {
        match self
            .call(
                reqwest::Method::POST,
                "/send",
                Some(json!({ "to": to, "text": text })),
            )
            .await
        {
            Ok((200, v)) => Outcome::Sent(v["id"].as_str().unwrap_or("").to_string()),
            Ok((_, v)) => {
                let why = v["error"]
                    .as_str()
                    .unwrap_or("шлюз WhatsApp не принял")
                    .to_string();
                if v["permanent"].as_bool() == Some(true) {
                    Outcome::Rejected(why)
                } else {
                    Outcome::Retry(Duration::from_secs(60), why)
                }
            }
            Err(why) => Outcome::Retry(Duration::from_secs(60), why),
        }
    }

    fn pause(&self) -> Duration {
        let (a, b) = self.pause_ms;
        Duration::from_millis(if b > a { rand::random_range(a..=b) } else { a })
    }
}

/// Филиал, чьи настройки действуют для номера (номер один на все филиалы, ответ 2б): первый.
async fn main_branch(conn: &mut PgConnection) -> AppResult<Uuid> {
    Ok(
        sqlx::query_scalar!("select id from branches order by created_at limit 1")
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Отправлено в WhatsApp по QR-коду сегодня (по Бишкеку), без пробных.
async fn sent_today(conn: &mut PgConnection) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from client_messages
           where channel = 'whatsapp_qr' and status = 'sent' and not test
             and (sent_at at time zone 'Asia/Bishkek')::date = (now() at time zone 'Asia/Bishkek')::date"#
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Отправка ждущих в WhatsApp: не больше `PER_PASS` за проход, с паузой, в пределах дня;
/// сообщения о покупке — первыми.
pub async fn send_pass(conn: &mut PgConnection, ch: &Channels) -> AppResult<usize> {
    let Some(gate) = &ch.wa_gate else {
        return Ok(0);
    };
    let branch = main_branch(conn).await?;
    let limit = i64::from(load_settings(conn, branch).await?.wa_daily_limit);
    let room = (limit - sent_today(conn).await?).clamp(0, PER_PASS);
    if room == 0 {
        return Ok(0);
    }
    let rows = sqlx::query!(
        r#"select id, recipient, text, attempts from client_messages
           where status = 'pending' and channel = 'whatsapp_qr' and next_try_at <= now()
           order by (kind <> 'purchase'), created_at
           limit $1"#,
        room
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut sent = 0;
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(gate.pause()).await;
        }
        let out = gate.send(&r.recipient, &r.text).await;
        let retry = matches!(out, Outcome::Retry(..));
        if matches!(out, Outcome::Sent(_)) {
            sent += 1;
        }
        client_messages::record(conn, r.id, r.attempts, &out).await?;
        if retry {
            break;
        }
    }
    Ok(sent)
}

// ---------- Входящие: меню ----------

/// Что спросил клиент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Menu,
    Debt,
    Cars,
    Bonus,
    Human,
    /// Голосовое, фото, стикер — без текста.
    Media,
}

pub fn ask(text: &str, media: bool) -> Ask {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        return if media { Ask::Media } else { Ask::Menu };
    }
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    match t.as_str() {
        "1" => return Ask::Debt,
        "2" => return Ask::Cars,
        "3" => return Ask::Bonus,
        "4" => return Ask::Human,
        _ => {}
    }
    if has(&[
        "администратор",
        "человек",
        "оператор",
        "позвоните",
        "позовите",
        "менеджер",
    ]) {
        Ask::Human
    } else if has(&["долг", "должен", "задолж"]) {
        Ask::Debt
    } else if has(&["балл", "бонус"]) {
        Ask::Bonus
    } else if has(&["машин", "замен", "масл"]) {
        Ask::Cars
    } else {
        Ask::Menu
    }
}

pub const MENU: &str = "Напишите цифру:\n1 — мой долг\n2 — мои машины и когда замена\n3 — баллы\n4 — позвать администратора";
pub const HANDOFF: &str = "Передал администратору — вам ответят в этом чате или перезвонят.";
pub const MEDIA: &str = "Пока понимаю только текст.";
pub const UNKNOWN: &str = "Чтобы видеть долг, машины и баллы, назовите этот номер администратору — его добавят в вашу карточку.";

/// Карточка клиента по телефону: активная, клиент; несколько — самая старая.
async fn party_by_phone(conn: &mut PgConnection, phone: &str) -> AppResult<Option<(Uuid, String)>> {
    let Some(key) = crate::domain::phone::key(phone) else {
        return Ok(None);
    };
    Ok(sqlx::query!(
        r#"select id, case when kind = 'company' or first_name = '' then name else first_name end as "name!"
           from parties where active and role = 'customer' and phone_norm(phone) = $1
           order by created_at limit 1"#,
        key
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| (r.id, r.name)))
}

/// Машины клиента: последняя замена и следующая по книжке.
pub async fn cars_text(conn: &mut PgConnection, party_id: Uuid) -> AppResult<String> {
    let rows = sqlx::query!(
        r#"select v.brand, v.model, v.plate,
                  c.change_date as "last_date?", c.mileage_value as "last_value?", c.mileage_unit as "last_unit?",
                  n.next_value as "next_value?", n.next_unit as "next_unit?", n.next_date as "next_date?"
           from party_vehicles v
           left join lateral (select c.id, c.change_date, c.mileage_value, c.mileage_unit from oil_changes c
                              where c.vehicle_id = v.id
                              order by c.change_date desc, c.created_at desc limit 1) c on true
           left join lateral (select i.next_value, i.next_unit, i.next_date from oil_change_items i
                              where i.oil_change_id = c.id and i.slot = 'oil'
                                and (i.next_value is not null or i.next_date is not null)
                              limit 1) n on true
           where v.party_id = $1 and v.active and v.status = 'active'
           order by v.created_at
           limit 10"#,
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok("Машин в вашей книжке пока нет.".into());
    }
    let lines: Vec<String> = rows
        .into_iter()
        .map(|r| {
            let mut s = format!(
                "🚗 {}",
                client_messages::car_text(&r.brand, &r.model, &r.plate)
            );
            match (r.last_date, r.last_value) {
                (Some(d), Some(v)) => s.push_str(&format!(
                    "\nпоследняя замена — {} на {}",
                    d.format("%d.%m.%Y"),
                    mileage::text(v, r.last_unit.as_deref().unwrap_or("km"))
                )),
                (Some(d), None) => {
                    s.push_str(&format!("\nпоследняя замена — {}", d.format("%d.%m.%Y")))
                }
                _ => s.push_str("\nзамен в книжке пока нет"),
            }
            match (r.next_value, r.next_date) {
                (Some(v), _) => s.push_str(&format!(
                    "\nследующая — на {}",
                    mileage::text(v, r.next_unit.as_deref().unwrap_or("km"))
                )),
                (None, Some(d)) => {
                    s.push_str(&format!("\nследующая — до {}", d.format("%d.%m.%Y")))
                }
                _ => {}
            }
            s
        })
        .collect();
    Ok(lines.join("\n\n"))
}

/// Ответ на входящее и нужна ли передача администратору.
async fn reply_for(
    conn: &mut PgConnection,
    party: Option<&(Uuid, String)>,
    a: Ask,
    shop_info: &str,
) -> AppResult<(String, bool)> {
    let hello = |name: &str| {
        if name.trim().is_empty() {
            "Здравствуйте! Это Avtodom.".to_string()
        } else {
            format!("Здравствуйте, {}! Это Avtodom.", name.trim())
        }
    };
    if a == Ask::Human {
        return Ok((HANDOFF.into(), true));
    }
    let Some((id, name)) = party else {
        let lead = if a == Ask::Media {
            format!("{MEDIA} ")
        } else {
            String::new()
        };
        return Ok((
            format!(
                "{lead}{}\n\n{shop_info}\n\n{UNKNOWN}\n4 — позвать администратора",
                hello("")
            ),
            false,
        ));
    };
    let text = match a {
        Ask::Debt => crate::api::bot_debt::customer_debt(conn, *id).await?,
        Ask::Cars => cars_text(conn, *id).await?,
        Ask::Bonus => crate::api::telegram::bonus_history(conn, *id).await?,
        Ask::Media => format!("{MEDIA}\n\n{MENU}"),
        Ask::Menu | Ask::Human => format!("{}\n\n{MENU}", hello(name)),
    };
    let text = client_messages::wa_text(&text)
        .replace("кнопка «🚗 Мои машины»", "цифра 2")
        .replace("«⭐ Баллы»", "цифра 3");
    Ok((text, false))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GateEvent {
    State {
        state: String,
        #[serde(default)]
        phone: String,
    },
    Message {
        from: String,
        #[serde(default)]
        text: String,
        #[serde(default)]
        media: bool,
        #[serde(default)]
        from_me: bool,
    },
}

/// Событие шлюза: состояние номера или сообщение. Только с ключом шлюза.
async fn gate_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(ev): Json<GateEvent>,
) -> AppResult<Json<Value>> {
    let Some(gate) = &state.wa_gate else {
        return Err(AppError::NotFound);
    };
    let got = headers
        .get("x-gate-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !gate.key_matches(got) {
        return Err(AppError::Unauthorized);
    }
    let mut conn = state.pool.acquire().await?;
    match ev {
        GateEvent::State { state: st, phone } => on_state(&mut conn, &st, &phone).await?,
        GateEvent::Message {
            from,
            text,
            media,
            from_me,
        } => {
            on_message(&mut conn, gate, &from, &text, media, from_me).await?;
        }
    }
    Ok(Json(json!({ "ok": true })))
}

async fn audit_system(conn: &mut PgConnection, action: &str, data: Value) -> AppResult<()> {
    let branch = main_branch(conn).await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data)
           values ($1, $2, null, null, $3, 'whatsapp', null, $4)"#,
        ops::new_id(),
        branch,
        action,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn on_state(conn: &mut PgConnection, st: &str, phone: &str) -> AppResult<()> {
    let before: Option<String> = sqlx::query_scalar!(
        r#"select value->>'state' as "s" from bot_state where key = 'whatsapp_qr'"#
    )
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    sqlx::query!(
        r#"insert into bot_state (key, value) values ('whatsapp_qr', $1)
           on conflict (key) do update set value = excluded.value"#,
        json!({ "state": st, "phone": phone, "at": Utc::now() })
    )
    .execute(&mut *conn)
    .await?;
    let was = before.as_deref() == Some("connected");
    if st == "connected" && !was {
        audit_system(conn, "whatsapp.connected", json!({ "phone": phone })).await?;
    } else if st != "connected" && was {
        audit_system(
            conn,
            "whatsapp.disconnected",
            json!({ "phone": phone, "state": st }),
        )
        .await?;
    }
    Ok(())
}

async fn log_inbox(
    conn: &mut PgConnection,
    phone: &str,
    party: Option<Uuid>,
    direction: &str,
    text: &str,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into whatsapp_inbox (id, phone, party_id, direction, text) values ($1, $2, $3, $4, $5)"#,
        ops::new_id(),
        phone,
        party,
        direction,
        text.chars().take(4000).collect::<String>()
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn on_message(
    conn: &mut PgConnection,
    gate: &WaGate,
    from: &str,
    text: &str,
    media: bool,
    from_me: bool,
) -> AppResult<()> {
    let party = party_by_phone(conn, from).await?;
    let party_id = party.as_ref().map(|p| p.0);
    if from_me {
        // Ответил человек с телефона магазина — бот замолкает с этим клиентом.
        return log_inbox(conn, from, party_id, "human", text).await;
    }
    log_inbox(
        conn,
        from,
        party_id,
        "in",
        if media && text.is_empty() {
            "[не текст]"
        } else {
            text
        },
    )
    .await?;
    let branch = main_branch(conn).await?;
    let settings = load_settings(conn, branch).await?;
    if !settings.wa_menu_enabled {
        return Ok(());
    }
    let human_recent = sqlx::query_scalar!(
        r#"select exists(select 1 from whatsapp_inbox where phone = $1 and direction = 'human'
                         and created_at > now() - make_interval(hours => $2)) as "e!""#,
        from,
        settings.wa_human_pause_hours
    )
    .fetch_one(&mut *conn)
    .await?;
    if human_recent {
        return Ok(());
    }
    let a = ask(text, media);
    let (reply, handoff) = reply_for(conn, party.as_ref(), a, &settings.shop_info).await?;
    if handoff {
        // Что клиент писал до «4» — администратору с запросом.
        let before: Vec<String> = sqlx::query_scalar!(
            r#"select text from whatsapp_inbox where phone = $1 and direction = 'in'
               order by created_at desc limit 4"#,
            from
        )
        .fetch_all(&mut *conn)
        .await?;
        let said: Vec<String> = before
            .into_iter()
            .rev()
            .filter(|t| ask(t, false) != Ask::Human)
            .collect();
        audit_system(
            conn,
            "whatsapp.handoff",
            json!({
                "phone": from,
                "name": party.as_ref().map(|p| p.1.clone()).unwrap_or_default(),
                "party_id": party_id,
                "text": said.join(" / "),
            }),
        )
        .await?;
    }
    let out = gate.send(from, &reply).await;
    let note = match &out {
        Outcome::Sent(_) => reply,
        Outcome::Rejected(why) | Outcome::Retry(_, why) => format!("[не ушло: {why}] {reply}"),
    };
    log_inbox(conn, from, party_id, "out", &note).await
}

// ---------- Экран владельца ----------

#[derive(Serialize)]
struct StatusOut {
    /// На сервере заданы `WA_GATE_URL` и `WA_GATE_KEY`.
    configured: bool,
    /// Ответ шлюза: state (off | qr | code | connecting | connected | logged_out | unreachable),
    /// phone, since, qr (строка для QR-кода), code (8 знаков), error.
    gate: Value,
    sent_today: i64,
    daily_limit: i32,
    pending: i64,
}

async fn status(State(state): State<AppState>, Owner(user): Owner) -> AppResult<Json<StatusOut>> {
    let mut conn = state.pool.acquire().await?;
    let gate = match &state.wa_gate {
        None => json!({ "state": "off" }),
        Some(g) => match g.call(reqwest::Method::GET, "/status", None).await {
            Ok((200, v)) => v,
            Ok((_, v)) => json!({ "state": "unreachable", "error": v["error"] }),
            Err(why) => json!({ "state": "unreachable", "error": why }),
        },
    };
    let pending = sqlx::query_scalar!(
        r#"select count(*) as "n!" from client_messages where channel = 'whatsapp_qr' and status = 'pending'"#
    )
    .fetch_one(&mut *conn)
    .await?;
    let _ = user;
    let branch = main_branch(&mut conn).await?;
    Ok(Json(StatusOut {
        configured: state.wa_gate.is_some(),
        gate,
        sent_today: sent_today(&mut conn).await?,
        daily_limit: load_settings(&mut conn, branch).await?.wa_daily_limit,
        pending,
    }))
}

#[derive(Deserialize)]
struct ConnectReq {
    /// Номер для кода из 8 знаков вместо QR; пусто — QR-код.
    #[serde(default)]
    phone: String,
}

async fn connect(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ConnectReq>,
) -> AppResult<Json<Value>> {
    ctx.user.require_owner()?;
    let Some(g) = &state.wa_gate else {
        return Err(invalid(
            "шлюз WhatsApp не настроен на сервере (WA_GATE_URL, WA_GATE_KEY)",
        ));
    };
    let phone = req.phone.trim();
    let digits = match phone {
        "" => String::new(),
        p => client_messages::wa_phone(p)
            .ok_or_else(|| invalid("номер телефона — например 0555 12 34 56"))?,
    };
    let body = if digits.is_empty() {
        json!({})
    } else {
        json!({ "phone": digits })
    };
    let (code, v) = g
        .call(reqwest::Method::POST, "/connect", Some(body))
        .await
        .map_err(invalid)?;
    if code != 200 {
        return Err(invalid(
            v["error"]
                .as_str()
                .unwrap_or("шлюз WhatsApp не ответил")
                .to_string(),
        ));
    }
    let mut conn = state.pool.acquire().await?;
    ops::audit(
        &mut conn,
        &ctx,
        "whatsapp.connect",
        "whatsapp",
        None,
        json!({ "mode": if digits.is_empty() { "qr" } else { "code" } }),
    )
    .await?;
    Ok(Json(v))
}

async fn logout(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<Value>> {
    ctx.user.require_owner()?;
    let Some(g) = &state.wa_gate else {
        return Err(invalid("шлюз WhatsApp не настроен на сервере"));
    };
    let (_, v) = g
        .call(reqwest::Method::POST, "/logout", None)
        .await
        .map_err(invalid)?;
    let mut conn = state.pool.acquire().await?;
    on_state(&mut conn, "off", "").await?;
    ops::audit(
        &mut conn,
        &ctx,
        "whatsapp.disconnect",
        "whatsapp",
        None,
        json!({}),
    )
    .await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct InboxQuery {
    limit: Option<i64>,
}

#[derive(Serialize)]
struct InboxRow {
    created_at: DateTime<Utc>,
    phone: String,
    party_id: Option<Uuid>,
    party_name: Option<String>,
    direction: String,
    text: String,
}

async fn inbox(
    State(state): State<AppState>,
    Owner(_user): Owner,
    Query(q): Query<InboxQuery>,
) -> AppResult<Json<Vec<InboxRow>>> {
    let mut conn = state.pool.acquire().await?;
    let rows = sqlx::query_as!(
        InboxRow,
        r#"select w.created_at, w.phone, w.party_id, p.name as "party_name?", w.direction, w.text
           from whatsapp_inbox w left join parties p on p.id = w.party_id
           order by w.created_at desc limit $1"#,
        q.limit.unwrap_or(100).clamp(1, 500)
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_digits_and_words() {
        assert_eq!(ask("1", false), Ask::Debt);
        assert_eq!(ask(" 2 ", false), Ask::Cars);
        assert_eq!(ask("3", false), Ask::Bonus);
        assert_eq!(ask("4", false), Ask::Human);
        assert_eq!(ask("Сколько я должен?", false), Ask::Debt);
        assert_eq!(ask("позовите администратора", false), Ask::Human);
        assert_eq!(ask("когда менять масло", false), Ask::Cars);
        assert_eq!(ask("баллы есть?", false), Ask::Bonus);
        assert_eq!(ask("здравствуйте", false), Ask::Menu);
        assert_eq!(ask("", true), Ask::Media);
        assert_eq!(ask("", false), Ask::Menu);
    }

    #[test]
    fn gate_key_compare() {
        let g = WaGate::new("http://x".into(), SecretKey("abc-key-1234567890".into()));
        assert!(g.key_matches("abc-key-1234567890"));
        assert!(!g.key_matches("abc-key-123456789"));
        assert!(!g.key_matches(""));
    }

    #[test]
    fn whatsapp_text_without_telegram_buttons() {
        assert_eq!(
            client_messages::wa_text("Долг — 1 000 сом. Подробнее — кнопка «💳 Мой долг»."),
            "Долг — 1 000 сом. Подробнее — напишите «1»."
        );
    }
}
