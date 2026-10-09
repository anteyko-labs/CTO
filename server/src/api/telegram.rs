//! Телеграм-бот владельца: уведомления из журнала и сводка по командам (SPEC-18, ADR-050).
//! Логика (привязка, команды, отбор уведомлений) отделена от сети и проверяется тестами;
//! сетевая часть — два фоновых цикла с исходящими запросами к Телеграму.

use axum::extract::State;
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::api::notifications::{WATCHED, describe};
use crate::auth::{Ctx, Owner};
use crate::domain::money::format_som;
use crate::error::{AppError, AppResult};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/telegram", routing::get(status).delete(unlink))
        .route("/settings/telegram/code", routing::post(new_code))
}

// ---------- Владелец: привязка ----------

#[derive(Serialize)]
struct StatusOut {
    /// Ключ бота задан на сервере (TELEGRAM_BOT_TOKEN).
    enabled: bool,
    linked: bool,
    linked_at: Option<DateTime<Utc>>,
}

async fn status(State(state): State<AppState>, Owner(user): Owner) -> AppResult<Json<StatusOut>> {
    let linked_at = sqlx::query_scalar!(
        "select linked_at from telegram_links where user_id = $1",
        user.id
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(StatusOut {
        enabled: state.telegram_bot,
        linked: linked_at.is_some(),
        linked_at,
    }))
}

#[derive(Serialize)]
struct CodeOut {
    code: String,
    expires_at: DateTime<Utc>,
}

/// Одноразовый код на 15 минут: владелец отправляет его боту, и чат привязывается к нему.
async fn new_code(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<CodeOut>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let code = format!("{:06}", rand::random_range(0..1_000_000u32));
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "delete from telegram_codes where user_id = $1 or expires_at < now()",
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    let expires_at = sqlx::query_scalar!(
        r#"insert into telegram_codes (code, user_id, expires_at) values ($1, $2, now() + interval '15 minutes')
           on conflict (code) do update set user_id = excluded.user_id, expires_at = excluded.expires_at
           returning expires_at"#,
        code,
        ctx.user.id
    )
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(CodeOut { code, expires_at }))
}

async fn unlink(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    sqlx::query!("delete from telegram_links where user_id = $1", ctx.user.id)
        .execute(&mut *tx)
        .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "telegram.unlink",
        "user",
        Some(ctx.user.id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- Логика бота ----------

const HELP: &str = "Команды:\n/сегодня — выручка и прибыль за день, деньги в кассах\n/смена — открытая смена и сколько должно быть в кассе\n/долги — кто сколько должен";

/// Владелец, к которому привязан чат (только активный владелец).
async fn chat_owner(conn: &mut PgConnection, chat_id: i64) -> AppResult<Option<(Uuid, Uuid)>> {
    Ok(sqlx::query!(
        r#"select u.id, u.branch_id from telegram_links t join users u on u.id = t.user_id
           where t.chat_id = $1 and u.active and u.role = 'owner'"#,
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| (r.id, r.branch_id)))
}

/// Ответ на сообщение. Пока чат не привязан, бот отвечает только на код привязки.
pub async fn handle_text(conn: &mut PgConnection, chat_id: i64, text: &str) -> AppResult<String> {
    let text = text.trim();
    let word = text.split_whitespace().next().unwrap_or("").to_lowercase();
    let arg = text.split_whitespace().nth(1).unwrap_or("");
    // «/start 123456» из ссылки или просто код цифрами.
    let code = if word == "/start" {
        arg
    } else if text.len() == 6 && text.chars().all(|c| c.is_ascii_digit()) {
        text
    } else {
        ""
    };
    if !code.is_empty() {
        return link(conn, chat_id, code).await;
    }
    let Some((_, branch_id)) = chat_owner(conn, chat_id).await? else {
        return Ok("Этот бот — для владельца точки. Откройте в системе «Настройки → Телеграм», получите код и отправьте его сюда.".into());
    };
    let cmd = word.trim_start_matches('/');
    match cmd {
        "сегодня" | "today" => today(conn, branch_id).await,
        "смена" | "shift" => shift(conn, branch_id).await,
        "долги" | "debts" => debts(conn, branch_id).await,
        _ => Ok(HELP.into()),
    }
}

async fn link(conn: &mut PgConnection, chat_id: i64, code: &str) -> AppResult<String> {
    let owner = sqlx::query!(
        r#"select u.id, u.branch_id, u.full_name from telegram_codes c join users u on u.id = c.user_id
           where c.code = $1 and c.expires_at > now() and u.active and u.role = 'owner'"#,
        code
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(o) = owner else {
        return Ok("Код не подошёл или устарел. Получите новый в «Настройки → Телеграм».".into());
    };
    sqlx::query!("delete from telegram_codes where code = $1", code)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "delete from telegram_links where chat_id = $1 or user_id = $2",
        chat_id,
        o.id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "insert into telegram_links (chat_id, user_id, branch_id) values ($1, $2, $3)",
        chat_id,
        o.id,
        o.branch_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, action, entity, entity_id, data)
           values ($1, $2, $3, 'telegram.link', 'user', $3, '{}')"#,
        ops::new_id(),
        o.branch_id,
        o.id
    )
    .execute(&mut *conn)
    .await?;
    Ok(format!(
        "Готово, {}: важные события точки будут приходить сюда.\n\n{HELP}",
        o.full_name
    ))
}

async fn today(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let day = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let t = crate::api::reports::totals_for(conn, branch_id, day, day).await?;
    crate::api::cash::ensure_accounts(conn, branch_id).await?;
    let accounts = sqlx::query!(
        r#"select name, balance_tyiyn from cash_accounts where branch_id = $1 and active
           order by is_default desc, kind, name"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let money = accounts
        .iter()
        .map(|a| format!("  {}: {}", a.name, format_som(a.balance_tyiyn)))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "Сегодня, {}\nЧеков: {}\nВыручка: {}\nВаловая прибыль: {}\nОплата труда: {}\nКомиссия банка: {}\nРасходы: {}\nЧистая прибыль: {}\n\nДеньги:\n{money}",
        day.format("%d.%m.%Y"),
        t.sales_count,
        format_som(t.goods_tyiyn.saturating_add(t.services_tyiyn)),
        format_som(t.gross_tyiyn),
        format_som(t.payroll_tyiyn),
        format_som(t.bank_fee_tyiyn),
        format_som(t.expenses_tyiyn),
        format_som(t.net_tyiyn),
    ))
}

async fn shift(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let till = crate::api::cash::default_account(conn, branch_id).await?;
    let Some(id) = crate::api::cash::open_shift_id(conn, branch_id, till).await? else {
        return Ok("Смена не открыта.".into());
    };
    let s = crate::api::cash::load_shift(conn, branch_id, id).await?;
    Ok(format!(
        "Смена № {} с {}, кассир {}\nДолжно быть в кассе: {}\nНаличными: {}\nКартой: {}\nQR: {}\nВ долг: {}",
        s.number,
        // Бишкек — всегда UTC+6, без перехода на летнее время.
        (s.opened_at + chrono::Duration::hours(6)).format("%d.%m %H:%M"),
        s.cashier_name,
        format_som(s.expected_tyiyn),
        format_som(s.cash_sales_tyiyn),
        format_som(s.card_tyiyn),
        format_som(s.transfer_tyiyn),
        format_som(s.debt_tyiyn),
    ))
}

async fn debts(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let rows = sqlx::query!(
        r#"select name, balance_tyiyn from parties
           where branch_id = $1 and role = 'customer' and balance_tyiyn > 0
           order by balance_tyiyn desc limit 15"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let total = sqlx::query_scalar!(
        r#"select coalesce(sum(balance_tyiyn) filter (where role = 'customer' and balance_tyiyn > 0), 0)::bigint as "us!"
           from parties where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok("Нам никто не должен.".into());
    }
    let list = rows
        .iter()
        .map(|r| format!("  {} — {}", r.name, format_som(r.balance_tyiyn)))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("Должны нам всего: {}\n{list}", format_som(total)))
}

/// Сообщение в чат и время события: курсор двигается только до отправленного.
pub struct Outgoing {
    pub at: DateTime<Utc>,
    pub chat_id: i64,
    pub text: String,
}

/// Новые события журнала для привязанных чатов и курсор, до которого дошли.
/// Первый запуск начинает с «сейчас»: прошлое не присылаем пачкой.
pub async fn pending(conn: &mut PgConnection) -> AppResult<(Vec<Outgoing>, Option<DateTime<Utc>>)> {
    let cursor = sqlx::query_scalar!("select value from bot_state where key = 'cursor'")
        .fetch_optional(&mut *conn)
        .await?
        .and_then(|v| v.as_str().map(str::to_string))
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|d| d.with_timezone(&Utc));
    let Some(cursor) = cursor else {
        return Ok((Vec::new(), Some(Utc::now())));
    };
    let watched: Vec<String> = WATCHED.iter().map(|s| (*s).to_string()).collect();
    let rows = sqlx::query!(
        r#"select a.action, a.data, a.created_at, u.full_name as "who?", t.chat_id
           from audit_log a
           join telegram_links t on t.branch_id = a.branch_id
           left join users u on u.id = a.user_id
           where a.created_at > $1 and a.action = any($2)
           order by a.created_at
           limit 100"#,
        cursor,
        &watched
    )
    .fetch_all(&mut *conn)
    .await?;
    let next = rows.last().map(|r| r.created_at);
    let out = rows
        .into_iter()
        .map(|r| {
            let (title, details) = describe(&r.action, &r.data);
            let who = r.who.map(|w| format!("\n— {w}")).unwrap_or_default();
            let text = if details.is_empty() {
                format!("{title}{who}")
            } else {
                format!("{title}\n{details}{who}")
            };
            Outgoing {
                at: r.created_at,
                chat_id: r.chat_id,
                text,
            }
        })
        .collect();
    Ok((out, next))
}

pub async fn save_cursor(conn: &mut PgConnection, at: DateTime<Utc>) -> AppResult<()> {
    sqlx::query!(
        r#"insert into bot_state (key, value) values ('cursor', $1)
           on conflict (key) do update set value = excluded.value"#,
        json!(at.to_rfc3339())
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------- Сеть ----------

#[derive(Deserialize)]
struct TgResponse<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    chat: Chat,
    text: Option<String>,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
}

impl Api {
    /// Отправка сообщения; `false` — Телеграм не принял (нет связи, ошибка ответа).
    async fn send(&self, chat_id: i64, text: &str) -> bool {
        let r = self
            .http
            .post(format!("{}/sendMessage", self.base))
            .json(&json!({ "chat_id": chat_id, "text": text }))
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await;
        match r {
            Ok(resp) if resp.status().is_success() => true,
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "телеграм: сообщение не принято");
                // Чат удалён или бот заблокирован — повторять бессмысленно, идём дальше.
                resp.status().is_client_error()
            }
            Err(e) => {
                tracing::warn!(error = %e, "телеграм: сообщение не отправлено");
                false
            }
        }
    }

    async fn updates(&self, offset: i64) -> Option<Vec<Update>> {
        let r = self
            .http
            .get(format!("{}/getUpdates", self.base))
            .query(&[("offset", offset.to_string()), ("timeout", "25".into())])
            .timeout(std::time::Duration::from_secs(35))
            .send()
            .await
            .ok()?
            .json::<TgResponse<Vec<Update>>>()
            .await
            .ok()?;
        r.ok.then_some(r.result.unwrap_or_default())
    }
}

/// Запуск бота: если ключ задан — два фоновых цикла. Без интернета они ждут и пробуют снова.
pub fn spawn(pool: PgPool, token: String) {
    let base =
        std::env::var("TELEGRAM_API_BASE").unwrap_or_else(|_| "https://api.telegram.org".into());
    let api = Api {
        http: reqwest::Client::new(),
        base: format!("{base}/bot{token}"),
    };
    tokio::spawn(commands_loop(pool.clone(), api.clone()));
    tokio::spawn(notify_loop(pool, api));
    tracing::info!("телеграм-бот запущен");
}

async fn commands_loop(pool: PgPool, api: Api) {
    let mut offset = 0i64;
    loop {
        let Some(updates) = api.updates(offset).await else {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            continue;
        };
        for u in updates {
            offset = offset.max(u.update_id + 1);
            let Some(m) = u.message else { continue };
            let Some(text) = m.text else { continue };
            let reply = match pool.acquire().await {
                Ok(mut conn) => handle_text(&mut conn, m.chat.id, &text).await,
                Err(e) => Err(AppError::from(e)),
            };
            match reply {
                Ok(r) => {
                    api.send(m.chat.id, &r).await;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "телеграм: команда не обработана");
                    api.send(m.chat.id, "Не получилось, попробуйте позже.")
                        .await;
                }
            }
        }
    }
}

async fn notify_loop(pool: PgPool, api: Api) {
    loop {
        if let Ok(mut conn) = pool.acquire().await {
            match pending(&mut conn).await {
                Ok((messages, next)) => {
                    // Курсор — до последнего отправленного: без связи события подождут.
                    let mut sent_up_to = if messages.is_empty() { next } else { None };
                    for m in messages {
                        if !api.send(m.chat_id, &m.text).await {
                            break;
                        }
                        sent_up_to = Some(m.at);
                    }
                    if let Some(at) = sent_up_to
                        && let Err(e) = save_cursor(&mut conn, at).await
                    {
                        tracing::warn!(error = %e, "телеграм: курсор не сохранён");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "телеграм: уведомления не собраны"),
            }
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
}
