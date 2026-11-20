//! Рабочий день по лицу (задача 039 A, ADR-200). Работник нажимает в боте «▶️ Начать рабочий
//! день» (или «⏹ Закончить») — заявка ждёт окно (по умолчанию 3 мин); терминал Dahua филиала
//! узнаёт лицо и шлёт событие на `POST /faceid/dahua/{ключ}` — день начат или закончен, работнику
//! в бот приходит время. Лицо без заявки день не начинает. Ответ терминалу — всегда 200, чтобы он
//! не повторял: неверный ключ, неизвестный номер, повтор — только запись в журнал терминала
//! (`faceid_events`, только добавление). Часы — «Сотрудники → Табель»; незакрытый к концу суток
//! (по Бишкеку) день — «не закрыт», без часов, владелец закрывает его временем с причиной.
//! Табель и настройки терминала — владелец и право «Табель».

use std::collections::{BTreeMap, HashSet};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::{Json, Router, routing};
use chrono::{DateTime, Datelike, Months, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::bot_staff::Employee;
use crate::api::telegram::{Api, Delivery, Keys, PassOutcome, Reply};
use crate::auth::{Ctx, CurrentUser, require_permission};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

/// Право на табель и настройки терминала (владельцу не нужно).
pub const PERMISSION: &str = "timesheet";
/// Окно подтверждения по умолчанию, мин.
pub const DEFAULT_WINDOW_MIN: i32 = 3;
/// Событие терминала — с фото бывает до нескольких МБ; больше не читаем.
const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
/// В журнал терминала — не больше этого куска события.
const MAX_PAYLOAD_CHARS: usize = 4000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/faceid/dahua/{key}",
            routing::post(dahua_event).get(dahua_ping),
        )
        .route(
            "/faceid/dahua/{key}/{*rest}",
            routing::post(dahua_event_nested).get(dahua_ping),
        )
        .layer(DefaultBodyLimit::max(MAX_EVENT_BYTES))
        .route(
            "/settings/faceid",
            routing::get(get_settings).put(put_settings),
        )
        .route("/employees/{id}/faceid", routing::put(set_employee_id))
        .route("/staff/timesheet", routing::get(timesheet))
        .route(
            "/staff/work-sessions/{id}/close",
            routing::post(close_session),
        )
}

fn new_key() -> String {
    format!("{:032x}", rand::random::<u128>())
}

// ---------- Разбор события терминала ----------

/// Что взято из одного события терминала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DahuaEvent {
    /// Номер лица в терминале (UserID / PersonID / CardName).
    pub user_id: Option<String>,
    /// Терминал сказал «не узнал» (Status = 0).
    pub failed: bool,
    /// Отпечаток события для повтора: ключ, номер, время события (или само событие).
    pub fingerprint: String,
    pub raw: Value,
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Первое значение по одному из ключей (без учёта регистра), в глубину.
fn find(v: &Value, keys: &[&str]) -> Option<Value> {
    match v {
        Value::Object(m) => {
            for k in keys {
                if let Some((_, x)) = m.iter().find(|(name, _)| name.eq_ignore_ascii_case(k))
                    && scalar(x).is_some()
                {
                    return Some(x.clone());
                }
            }
            m.values().find_map(|x| find(x, keys))
        }
        Value::Array(a) => a.iter().find_map(|x| find(x, keys)),
        _ => None,
    }
}

/// Все JSON-объекты в теле: само тело, части multipart или строки «ключ=значение».
fn json_parts(body: &[u8]) -> Vec<Value> {
    if let Ok(v) = serde_json::from_slice::<Value>(body)
        && v.is_object()
    {
        return vec![v];
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] == b'{' {
            let mut it = serde_json::Deserializer::from_slice(&body[i..]).into_iter::<Value>();
            if let Some(Ok(v)) = it.next() {
                let used = it.byte_offset();
                if v.is_object() {
                    out.push(v);
                }
                i += used.max(1);
                continue;
            }
        }
        i += 1;
    }
    if out.is_empty() {
        // Форма или текст «UserID=12&Status=1» / «UserID: 12».
        let text = String::from_utf8_lossy(body);
        let mut m = serde_json::Map::new();
        for pair in text.split(['&', '\n', '\r', ';']) {
            if let Some((k, v)) = pair.split_once('=').or_else(|| pair.split_once(':')) {
                let (k, v) = (k.trim(), v.trim().trim_matches('"'));
                if !k.is_empty() && !v.is_empty() && k.len() <= 40 {
                    m.insert(k.to_string(), Value::String(v.to_string()));
                }
            }
        }
        if !m.is_empty() {
            out.push(Value::Object(m));
        }
    }
    out
}

/// События из тела загрузки Dahua: `{"Events":[…]}`, одно событие или multipart с JSON-частью.
pub fn parse_dahua(body: &[u8]) -> Vec<DahuaEvent> {
    let mut events = Vec::new();
    for part in json_parts(body) {
        let list: Vec<Value> = match find_array(&part, "Events") {
            Some(a) if !a.is_empty() => a,
            _ => vec![part],
        };
        for ev in list {
            let user_id = find(&ev, &["UserID", "PersonID", "CardName"]).and_then(|v| scalar(&v));
            let failed = find(&ev, &["Status"])
                .and_then(|v| scalar(&v))
                .is_some_and(|s| s == "0");
            let when = find(&ev, &["UTC", "CreateTime", "RecNo", "Time", "EventTime"])
                .and_then(|v| scalar(&v));
            let fingerprint = match &when {
                Some(t) => format!("{}|{t}", user_id.as_deref().unwrap_or("")),
                None => ev.to_string(),
            };
            events.push(DahuaEvent {
                user_id,
                failed,
                fingerprint,
                raw: ev,
            });
        }
    }
    events
}

fn find_array(v: &Value, key: &str) -> Option<Vec<Value>> {
    match v {
        Value::Object(m) => {
            if let Some((_, Value::Array(a))) = m.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))
            {
                return Some(a.clone());
            }
            m.values().find_map(|x| find_array(x, key))
        }
        _ => None,
    }
}

fn digest(s: &str) -> String {
    let h = Sha256::digest(s.as_bytes());
    h.iter().map(|b| format!("{b:02x}")).collect()
}

fn short_payload(v: &Value) -> Value {
    let s = v.to_string();
    if s.chars().count() <= MAX_PAYLOAD_CHARS {
        v.clone()
    } else {
        json!({ "truncated": s.chars().take(MAX_PAYLOAD_CHARS).collect::<String>() })
    }
}

// ---------- Приём события ----------

async fn dahua_ping() -> StatusCode {
    StatusCode::OK
}

async fn dahua_event_nested(
    State(state): State<AppState>,
    Path((key, _rest)): Path<(String, String)>,
    body: Bytes,
) -> StatusCode {
    receive_logged(&state, &key, &body).await
}

async fn dahua_event(
    State(state): State<AppState>,
    Path(key): Path<String>,
    body: Bytes,
) -> StatusCode {
    receive_logged(&state, &key, &body).await
}

/// Терминалу — всегда 200: ошибка базы пишется в лог сервера, событие терминал не повторяет.
async fn receive_logged(state: &AppState, key: &str, body: &[u8]) -> StatusCode {
    match state.pool.acquire().await {
        Ok(mut conn) => {
            if let Err(e) = receive(&mut conn, key, body).await {
                tracing::warn!(error = %e, "face id: событие не обработано");
            }
        }
        Err(e) => tracing::warn!(error = %e, "face id: нет соединения с базой"),
    }
    StatusCode::OK
}

/// Итог одного события — для журнала терминала и тестов.
pub async fn receive(conn: &mut PgConnection, key: &str, body: &[u8]) -> AppResult<Vec<String>> {
    let branch = sqlx::query_scalar!("select branch_id from faceid_devices where key = $1", key)
        .fetch_optional(&mut *conn)
        .await?;
    let events = parse_dahua(body);
    let Some(branch_id) = branch else {
        // Чужой ключ: одна запись на запрос, без действия.
        sqlx::query!(
            r#"insert into faceid_events (id, branch_id, dedup, user_id, outcome, payload)
               values ($1, null, $2, $3, 'bad_key', $4)"#,
            ops::new_id(),
            format!("bad|{}", Uuid::now_v7()),
            events.first().and_then(|e| e.user_id.clone()),
            json!({ "key_tail": key.chars().rev().take(4).collect::<String>() })
        )
        .execute(&mut *conn)
        .await?;
        return Ok(vec!["bad_key".into()]);
    };
    let mut out = Vec::new();
    for ev in events {
        out.push(one_event(conn, branch_id, ev).await?);
    }
    Ok(out)
}

async fn one_event(conn: &mut PgConnection, branch_id: Uuid, ev: DahuaEvent) -> AppResult<String> {
    use sqlx::Connection;
    let dedup = digest(&format!("{branch_id}|{}", ev.fingerprint));
    let mut tx = conn.begin().await?;
    let seen = sqlx::query_scalar!(
        r#"select exists (select 1 from faceid_events where dedup = $1) as "e!""#,
        dedup
    )
    .fetch_one(&mut *tx)
    .await?;
    if seen {
        return Ok("duplicate".into());
    }
    let employee = match &ev.user_id {
        Some(uid) => sqlx::query!(
            "select id, full_name from employees where branch_id = $1 and faceid_user_id = $2 and active",
            branch_id,
            uid
        )
        .fetch_optional(&mut *tx)
        .await?
        .map(|r| (r.id, r.full_name)),
        None => None,
    };
    let outcome = match (&ev.user_id, &employee) {
        (None, _) => "no_user",
        (Some(_), None) => "unknown_user",
        _ if ev.failed => "failed_face",
        (Some(_), Some((employee_id, name))) => {
            apply(&mut tx, branch_id, *employee_id, name).await?
        }
    };
    let inserted = sqlx::query!(
        r#"insert into faceid_events (id, branch_id, dedup, user_id, employee_id, outcome, payload)
           values ($1, $2, $3, $4, $5, $6, $7) on conflict (dedup) do nothing"#,
        ops::new_id(),
        branch_id,
        dedup,
        ev.user_id,
        employee.as_ref().map(|e| e.0),
        outcome,
        short_payload(&ev.raw)
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        // То же событие пришло одновременно — действие уже сделал первый запрос.
        return Ok("duplicate".into());
    }
    tx.commit().await?;
    Ok(outcome.into())
}

/// Лицо узнано: заявка в окне — день начат или закончен; без заявки — ничего.
async fn apply(
    tx: &mut PgConnection,
    branch_id: Uuid,
    employee_id: Uuid,
    name: &str,
) -> AppResult<&'static str> {
    let Some(req) = sqlx::query!(
        r#"select id, kind from workday_requests
           where employee_id = $1 and branch_id = $2 and status = 'pending' and expires_at > now()
           order by created_at desc limit 1 for update"#,
        employee_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Ok("no_request");
    };
    let open = sqlx::query_scalar!(
        "select id from work_sessions where employee_id = $1 and ended_at is null for update",
        employee_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let start = req.kind == "start";
    let session = match (start, open) {
        (true, Some(id)) => id,
        (true, None) => {
            let id = ops::new_id();
            sqlx::query!(
                "insert into work_sessions (id, employee_id, branch_id, started_at) values ($1, $2, $3, now())",
                id,
                employee_id,
                branch_id
            )
            .execute(&mut *tx)
            .await?;
            id
        }
        (false, Some(id)) => {
            sqlx::query!(
                "update work_sessions set ended_at = now() where id = $1",
                id
            )
            .execute(&mut *tx)
            .await?;
            id
        }
        (false, None) => Uuid::nil(),
    };
    sqlx::query!(
        "update workday_requests set status = 'done', done_at = now() where id = $1",
        req.id
    )
    .execute(&mut *tx)
    .await?;
    // Событие владельцу (по умолчанию в ленту бота не идёт): без пользователя — это терминал.
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data)
           values ($1, $2, null, null, 'staff.workday', 'employee', $3, $4)"#,
        ops::new_id(),
        branch_id,
        employee_id,
        json!({ "name": name, "kind": req.kind, "session_id": session })
    )
    .execute(&mut *tx)
    .await?;
    Ok("applied")
}

// ---------- Бот ----------

/// Чат работника с начатым днём: внизу — «⏹ Закончить рабочий день».
pub async fn chat_working(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from telegram_employees t
                          join work_sessions w on w.employee_id = t.employee_id and w.ended_at is null
                          where t.chat_id = $1) as "e!""#,
        chat_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// У сотрудника открыт рабочий день (кнопка в меню администратора со входом сотрудника).
pub async fn employee_working(conn: &mut PgConnection, employee_id: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from work_sessions
                          where employee_id = $1 and ended_at is null) as "e!""#,
        employee_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Кнопка «Начать / Закончить рабочий день»: заявка ждёт лица в окне.
pub async fn press(
    conn: &mut PgConnection,
    e: &Employee,
    chat_id: i64,
    start: bool,
) -> AppResult<Reply> {
    let window = sqlx::query_scalar!(
        "select window_min from faceid_devices where branch_id = $1",
        e.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(window) = window else {
        return Ok(Reply::new(
            "Терминал Face ID в филиале не настроен — обратитесь к владельцу.",
            Keys::Employee,
        ));
    };
    let open = sqlx::query!(
        r#"select to_char(started_at at time zone 'Asia/Bishkek', 'HH24:MI') as "at!"
           from work_sessions where employee_id = $1 and ended_at is null"#,
        e.id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match (start, &open) {
        (true, Some(o)) => {
            return Ok(Reply::new(
                format!("Рабочий день уже начат в {}.", o.at),
                Keys::Employee,
            ));
        }
        (false, None) => {
            return Ok(Reply::new(
                "Рабочий день не начат — нажмите «▶️ Начать рабочий день».",
                Keys::Employee,
            ));
        }
        _ => {}
    }
    sqlx::query!(
        "update workday_requests set status = 'cancelled', notified_at = now() where employee_id = $1 and status = 'pending'",
        e.id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"insert into workday_requests (id, employee_id, branch_id, kind, chat_id, expires_at)
           values ($1, $2, $3, $4, $5, now() + make_interval(mins => $6))"#,
        ops::new_id(),
        e.id,
        e.branch_id,
        if start { "start" } else { "stop" },
        chat_id,
        window
    )
    .execute(&mut *conn)
    .await?;
    Ok(Reply::new(
        format!("Подойдите к терминалу и посмотрите в камеру ({window} мин)."),
        Keys::Employee,
    ))
}

/// Сообщение работнику по заявке.
#[derive(Debug, Clone)]
pub struct Notice {
    pub id: Uuid,
    pub chat_id: i64,
    pub text: String,
    /// Кнопки внизу по состоянию дня; `None` — чат не только работника, клавиатуру не трогаем.
    pub keys: Option<Keys>,
}

fn duration_text(minutes: i64) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    if h > 0 {
        format!("{h} ч {m:02} мин")
    } else {
        format!("{m} мин")
    }
}

/// Просроченные заявки — «истекла»; что сказать работникам по готовым и истёкшим заявкам.
pub async fn notices(conn: &mut PgConnection) -> AppResult<Vec<Notice>> {
    sqlx::query!(
        "update workday_requests set status = 'expired' where status = 'pending' and expires_at <= now()"
    )
    .execute(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"select r.id, r.kind, r.status, r.chat_id as "chat_id!", r.employee_id,
                  to_char(r.done_at at time zone 'Asia/Bishkek', 'HH24:MI') as at,
                  (select (extract(epoch from w.ended_at - w.started_at) / 60)::bigint
                   from work_sessions w where w.employee_id = r.employee_id and w.ended_at is not null
                   order by w.ended_at desc limit 1) as minutes,
                  exists (select 1 from work_sessions w where w.employee_id = r.employee_id
                          and w.ended_at is null) as "working!",
                  (exists (select 1 from telegram_employees t where t.chat_id = r.chat_id
                           and t.employee_id = r.employee_id)
                   and not exists (select 1 from telegram_customers c where c.chat_id = r.chat_id)
                   and not exists (select 1 from users u where u.employee_id = r.employee_id))
                   as "plain!"
           from workday_requests r
           where r.notified_at is null and r.status in ('done', 'expired') and r.chat_id is not null
             and r.created_at > now() - interval '1 day'
           order by r.created_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let at = r.at.unwrap_or_default();
            let text = match (r.status.as_str(), r.kind.as_str()) {
                ("done", "start") => format!("✅ Рабочий день начат в {at}."),
                ("done", _) => match r.minutes {
                    Some(m) => format!(
                        "✅ Рабочий день закончен в {at}, отработано {}.",
                        duration_text(m)
                    ),
                    None => format!("✅ Рабочий день закончен в {at}."),
                },
                _ => "⏱ Не подтверждено лицом — нажмите кнопку снова и посмотрите в камеру терминала."
                    .to_string(),
            };
            Notice {
                id: r.id,
                chat_id: r.chat_id,
                text,
                keys: r.plain.then_some(if r.working {
                    Keys::EmployeeWorking
                } else {
                    Keys::Employee
                }),
            }
        })
        .collect())
}

pub async fn mark_notice(conn: &mut PgConnection, id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "update workday_requests set notified_at = now() where id = $1",
        id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Проход рассылки: время начала и конца дня, «не подтверждено».
pub async fn notify_pass(conn: &mut PgConnection, api: &Api) -> AppResult<PassOutcome> {
    let mut out = PassOutcome::default();
    let mut blocked: HashSet<i64> = HashSet::new();
    for n in notices(conn).await? {
        if blocked.contains(&n.chat_id) {
            continue;
        }
        let d = match n.keys {
            Some(k) => api.push_with(n.chat_id, &n.text, k.markup()).await,
            None => api.push(n.chat_id, &n.text).await,
        };
        match d {
            Delivery::Sent | Delivery::Skip => mark_notice(conn, n.id).await?,
            Delivery::Retry { after, all } => {
                out.retry(after, all);
                if all {
                    break;
                }
                blocked.insert(n.chat_id);
            }
        }
    }
    Ok(out)
}

// ---------- Настройки терминала ----------

#[derive(Serialize)]
struct EmployeeId {
    id: Uuid,
    full_name: String,
    faceid_user_id: Option<String>,
}

#[derive(Serialize)]
struct EventOut {
    created_at: DateTime<Utc>,
    user_id: Option<String>,
    employee: Option<String>,
    outcome: String,
}

#[derive(Serialize)]
struct SettingsOut {
    key: String,
    /// Путь приёма событий; адрес целиком — сервер + путь.
    path: String,
    window_min: i32,
    employees: Vec<EmployeeId>,
    events: Vec<EventOut>,
}

async fn device(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<(String, i32)> {
    sqlx::query!(
        "insert into faceid_devices (branch_id, key, window_min) values ($1, $2, $3) on conflict do nothing",
        branch_id,
        new_key(),
        DEFAULT_WINDOW_MIN
    )
    .execute(&mut *conn)
    .await?;
    let r = sqlx::query!(
        "select key, window_min from faceid_devices where branch_id = $1",
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok((r.key, r.window_min))
}

async fn settings_out(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<SettingsOut> {
    let (key, window_min) = device(conn, branch_id).await?;
    let employees = sqlx::query_as!(
        EmployeeId,
        "select id, full_name, faceid_user_id from employees where branch_id = $1 and active order by full_name",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let events = sqlx::query_as!(
        EventOut,
        r#"select f.created_at, f.user_id, e.full_name as "employee?", f.outcome
           from faceid_events f left join employees e on e.id = f.employee_id
           where f.branch_id = $1 order by f.created_at desc limit 20"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(SettingsOut {
        path: format!("/api/v1/faceid/dahua/{key}"),
        key,
        window_min,
        employees,
        events,
    })
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<SettingsOut>> {
    let mut conn = state.pool.acquire().await?;
    require_permission(&mut conn, &user, PERMISSION).await?;
    Ok(Json(settings_out(&mut conn, user.branch_id).await?))
}

#[derive(Deserialize)]
struct SettingsIn {
    window_min: Option<i32>,
    #[serde(default)]
    new_key: bool,
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<SettingsOut>> {
    if let Some(w) = req.window_min
        && !(1..=30).contains(&w)
    {
        return Err(invalid("окно подтверждения — от 1 до 30 минут"));
    }
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, PERMISSION).await?;
    let branch_id = ctx.user.branch_id;
    let (_, before) = device(&mut tx, branch_id).await?;
    sqlx::query!(
        r#"update faceid_devices set window_min = coalesce($2, window_min),
             key = case when $3 then $4 else key end, updated_at = now()
           where branch_id = $1"#,
        branch_id,
        req.window_min,
        req.new_key,
        new_key()
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.faceid",
        "branch",
        Some(branch_id),
        json!({ "window_min": req.window_min.unwrap_or(before), "window_before": before, "new_key": req.new_key }),
    )
    .await?;
    let out = settings_out(&mut tx, branch_id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct EmployeeIdIn {
    faceid_user_id: Option<String>,
}

async fn set_employee_id(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<EmployeeIdIn>,
) -> AppResult<Json<Value>> {
    let value = req
        .faceid_user_id
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if value.as_ref().is_some_and(|v| v.chars().count() > 40) {
        return Err(invalid("номер в терминале не длиннее 40 знаков"));
    }
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, PERMISSION).await?;
    let prev = sqlx::query!(
        "select full_name, faceid_user_id from employees where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if let Some(v) = &value {
        let taken = sqlx::query_scalar!(
            "select full_name from employees where branch_id = $1 and faceid_user_id = $2 and id <> $3",
            ctx.user.branch_id,
            v,
            id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(other) = taken {
            return Err(AppError::Conflict(format!(
                "номер {v} в терминале уже у сотрудника {other}"
            )));
        }
    }
    sqlx::query!(
        "update employees set faceid_user_id = $2 where id = $1",
        id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "employee.faceid",
        "employee",
        Some(id),
        json!({ "full_name": prev.full_name, "before": prev.faceid_user_id, "after": value }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "faceid_user_id": value })))
}

// ---------- Табель ----------

#[derive(Deserialize)]
struct MonthQuery {
    /// ГГГГ-ММ; пусто — текущий месяц по Бишкеку.
    month: Option<String>,
}

#[derive(Serialize)]
pub struct SessionOut {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// `closed` — закрыт; `open` — идёт сегодня; `unclosed` — не закрыт к концу суток.
    pub status: String,
    pub minutes: Option<i64>,
    pub closed_by: Option<String>,
    pub close_reason: Option<String>,
}

#[derive(Serialize)]
pub struct DayOut {
    pub date: NaiveDate,
    pub minutes: i64,
    /// В дне есть незакрытая сессия прошлого дня.
    pub unclosed: bool,
    pub sessions: Vec<SessionOut>,
}

#[derive(Serialize)]
pub struct RowOut {
    pub employee_id: Uuid,
    pub full_name: String,
    pub total_minutes: i64,
    pub days: Vec<DayOut>,
    /// Выходные по графику за месяц (задача 037 п.5).
    pub days_off: Vec<NaiveDate>,
}

#[derive(Serialize)]
pub struct TimesheetOut {
    pub month: String,
    pub days_in_month: u32,
    pub rows: Vec<RowOut>,
}

fn parse_month(s: &str) -> AppResult<NaiveDate> {
    NaiveDate::parse_from_str(&format!("{}-01", s.trim()), "%Y-%m-%d")
        .map_err(|_| invalid("месяц — в виде ГГГГ-ММ"))
}

async fn timesheet(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<MonthQuery>,
) -> AppResult<Json<TimesheetOut>> {
    let mut conn = state.pool.acquire().await?;
    require_permission(&mut conn, &user, PERMISSION).await?;
    let from = match q.month.as_deref().filter(|m| !m.trim().is_empty()) {
        Some(m) => parse_month(m)?,
        None => {
            let today =
                sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                    .fetch_one(&mut *conn)
                    .await?;
            today.with_day0(0).unwrap_or(today)
        }
    };
    let to = from
        .checked_add_months(Months::new(1))
        .ok_or_else(|| invalid("месяц вне пределов"))?;
    Ok(Json(
        timesheet_of(&mut conn, user.branch_id, from, to).await?,
    ))
}

pub async fn timesheet_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<TimesheetOut> {
    let rows = sqlx::query!(
        r#"select w.id, w.employee_id, e.full_name, w.started_at, w.ended_at, w.close_reason,
                  u.full_name as "closed_by?",
                  (w.started_at at time zone 'Asia/Bishkek')::date as "day!",
                  case when w.ended_at is null then null
                       else (extract(epoch from w.ended_at - w.started_at) / 60)::bigint end as minutes,
                  (w.started_at at time zone 'Asia/Bishkek')::date
                      < (now() at time zone 'Asia/Bishkek')::date as "past!"
           from work_sessions w join employees e on e.id = w.employee_id
           left join users u on u.id = w.closed_by
           where w.branch_id = $1
             and (w.started_at at time zone 'Asia/Bishkek')::date >= $2
             and (w.started_at at time zone 'Asia/Bishkek')::date < $3
           order by e.full_name, w.started_at"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut people: BTreeMap<(String, Uuid), BTreeMap<NaiveDate, DayOut>> = BTreeMap::new();
    // Работники с номером в терминале — строкой даже без часов.
    for e in sqlx::query!(
        "select id, full_name from employees where branch_id = $1 and active and faceid_user_id is not null",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    {
        people.entry((e.full_name, e.id)).or_default();
    }
    let offs = crate::api::days_off::days_off_of(&mut *conn, branch_id, from, to).await?;
    if !offs.is_empty() {
        for e in sqlx::query!(
            "select id, full_name from employees where id = any($1)",
            &offs.iter().map(|o| o.0).collect::<Vec<Uuid>>()
        )
        .fetch_all(&mut *conn)
        .await?
        {
            people.entry((e.full_name, e.id)).or_default();
        }
    }
    for r in rows {
        let status = match (r.ended_at, r.past) {
            (Some(_), _) => "closed",
            (None, true) => "unclosed",
            (None, false) => "open",
        };
        let day = people
            .entry((r.full_name, r.employee_id))
            .or_default()
            .entry(r.day)
            .or_insert_with(|| DayOut {
                date: r.day,
                minutes: 0,
                unclosed: false,
                sessions: Vec::new(),
            });
        day.minutes += r.minutes.unwrap_or(0);
        day.unclosed |= status == "unclosed";
        day.sessions.push(SessionOut {
            id: r.id,
            started_at: r.started_at,
            ended_at: r.ended_at,
            status: status.into(),
            minutes: r.minutes,
            closed_by: r.closed_by,
            close_reason: r.close_reason,
        });
    }
    let rows = people
        .into_iter()
        .map(|((full_name, employee_id), days)| {
            let days: Vec<DayOut> = days.into_values().collect();
            RowOut {
                employee_id,
                full_name,
                total_minutes: days.iter().map(|d| d.minutes).sum(),
                days,
                days_off: offs
                    .iter()
                    .filter(|o| o.0 == employee_id)
                    .map(|o| o.1)
                    .collect(),
            }
        })
        .collect();
    Ok(TimesheetOut {
        month: from.format("%Y-%m").to_string(),
        days_in_month: u32::try_from((to - from).num_days()).unwrap_or(31),
        rows,
    })
}

#[derive(Deserialize)]
struct CloseIn {
    ended_at: DateTime<Utc>,
    reason: String,
}

/// Владелец (или право «Табель») закрывает незакрытый день временем с причиной; в журнал.
async fn close_session(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CloseIn>,
) -> AppResult<Json<Value>> {
    let reason = req.reason.trim().to_string();
    if reason.is_empty() {
        return Err(invalid("укажите причину"));
    }
    if reason.chars().count() > 300 {
        return Err(invalid("причина не длиннее 300 знаков"));
    }
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, PERMISSION).await?;
    let s = sqlx::query!(
        r#"select w.started_at, w.ended_at, w.employee_id, e.full_name
           from work_sessions w join employees e on e.id = w.employee_id
           where w.id = $1 and w.branch_id = $2 for update of w"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if s.ended_at.is_some() {
        return Err(AppError::Conflict("день уже закрыт".into()));
    }
    if req.ended_at < s.started_at {
        return Err(invalid("конец дня раньше начала"));
    }
    if req.ended_at > Utc::now() {
        return Err(invalid("конец дня — не позже текущего времени"));
    }
    sqlx::query!(
        "update work_sessions set ended_at = $2, closed_by = $3, close_reason = $4 where id = $1",
        id,
        req.ended_at,
        ctx.user.id,
        reason
    )
    .execute(&mut *tx)
    .await?;
    // Заявка «закончить», которая ещё ждёт лица, больше не нужна.
    sqlx::query!(
        "update workday_requests set status = 'cancelled', notified_at = now() where employee_id = $1 and status = 'pending' and kind = 'stop'",
        s.employee_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "staff.workday_close",
        "work_session",
        Some(id),
        json!({
            "name": s.full_name,
            "employee_id": s.employee_id,
            "started_at": s.started_at,
            "ended_at": req.ended_at,
            "reason": reason,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": id, "ended_at": req.ended_at })))
}

#[cfg(test)]
mod tests {
    use super::parse_dahua;

    #[test]
    fn events_json() {
        let b = br#"{"Events":[{"Code":"AccessControl","Data":{"UserID":"12","Status":1,"UTC":1700000000}}]}"#;
        let e = parse_dahua(b);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].user_id.as_deref(), Some("12"));
        assert!(!e[0].failed);
    }

    #[test]
    fn multipart_and_failed() {
        let b = b"--myboundary\r\nContent-Type: application/json\r\n\r\n{\"Code\":\"AccessControl\",\"Data\":{\"PersonID\":7,\"Status\":0}}\r\n--myboundary\r\nContent-Type: image/jpeg\r\n\r\n\xff\xd8{garbage\r\n--myboundary--";
        let e = parse_dahua(b);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].user_id.as_deref(), Some("7"));
        assert!(e[0].failed);
    }

    #[test]
    fn form_text() {
        let e = parse_dahua(b"UserID=33&Status=1");
        assert_eq!(e[0].user_id.as_deref(), Some("33"));
    }
}
