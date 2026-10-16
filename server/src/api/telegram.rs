//! Телеграм-бот владельца: уведомления из журнала и сводка по командам (SPEC-18, ADR-050).
//! Логика (привязка, команды, отбор уведомлений) отделена от сети и проверяется тестами;
//! сетевая часть — два фоновых цикла с исходящими запросами к Телеграму.
//! Вопросы владельца обычным текстом разбирает `api::assistant` (SPEC-21, ADR-060).
//! Доставка событий по каждому чату отдельно, пределы привязки и окно напоминаний — ADR-063.
//! Кабинет работника (свой заработок по номеру из карточки сотрудника) — `api::bot_staff`
//! (SPEC-23, ADR-066). Черновики операций владельца и кнопки «✅ Провести» / «✖ Отмена» —
//! `api::bot_drafts` (SPEC-24, ADR-068).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use crate::api::assistant::{self, Assistant};
use crate::api::bot_staff::{self, EmployeeLink};
use crate::api::notifications::{WATCHED, describe};
use crate::api::parties::{ListQuery, PartyOut, search_parties};
use crate::auth::{Ctx, Owner};
use crate::domain::money::{format_som, format_som_signed};
use crate::error::{AppError, AppResult};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings/telegram", routing::get(status).delete(unlink))
        .route("/settings/telegram/code", routing::post(new_code))
        .route(
            "/settings/telegram/questions",
            routing::get(questions_usage),
        )
        .route("/telegram/bot", routing::get(bot_info))
}

#[derive(Serialize)]
struct BotInfo {
    /// Имя бота для ссылки и QR-кода: t.me/<username>. Пусто — бот ещё ни разу не запускался.
    username: Option<String>,
}

/// Ссылка на бота — для QR-кода на чеке и в масляной книжке, видна любому вошедшему.
async fn bot_info(
    State(state): State<AppState>,
    _user: crate::auth::CurrentUser,
) -> AppResult<Json<BotInfo>> {
    let username = sqlx::query_scalar!("select value from bot_state where key = 'username'")
        .fetch_optional(&state.pool)
        .await?
        .and_then(|v| v.as_str().map(str::to_string));
    Ok(Json(BotInfo { username }))
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
struct UsageOut {
    questions: i64,
    input_tokens: i64,
    output_tokens: i64,
}

#[derive(Serialize)]
struct QuestionsOut {
    /// Вопросы текстом включены: на сервере заданы ключи бота и сервиса разбора вопросов.
    enabled: bool,
    daily_limit: Option<i64>,
    today: UsageOut,
    month: UsageOut,
}

/// Расход на вопросы боту текстом за сегодня и за месяц по Бишкеку (SPEC-21).
async fn questions_usage(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<QuestionsOut>> {
    let r = sqlx::query!(
        r#"with b as (
             select ((now() at time zone 'Asia/Bishkek')::date)::timestamp at time zone 'Asia/Bishkek' as day,
                    date_trunc('month', now() at time zone 'Asia/Bishkek') at time zone 'Asia/Bishkek' as month)
           select count(*) filter (where q.asked_at >= b.day) as "day_n!",
                  coalesce(sum(q.input_tokens) filter (where q.asked_at >= b.day), 0)::bigint as "day_in!",
                  coalesce(sum(q.output_tokens) filter (where q.asked_at >= b.day), 0)::bigint as "day_out!",
                  count(q.id) as "month_n!",
                  coalesce(sum(q.input_tokens), 0)::bigint as "month_in!",
                  coalesce(sum(q.output_tokens), 0)::bigint as "month_out!"
           from b left join bot_questions q
             on q.branch_id = $1 and q.asked_at >= b.month and q.error is distinct from $2"#,
        user.branch_id,
        assistant::LIMIT_ERROR
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(QuestionsOut {
        enabled: state.telegram_bot && state.assistant_daily_limit.is_some(),
        daily_limit: state.assistant_daily_limit,
        today: UsageOut {
            questions: r.day_n,
            input_tokens: r.day_in,
            output_tokens: r.day_out,
        },
        month: UsageOut {
            questions: r.month_n,
            input_tokens: r.month_in,
            output_tokens: r.month_out,
        },
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

/// Пользователь отключён или больше не владелец: его чат перестаёт получать события и команды
/// (ADR-063). Вызывается в транзакции правки пользователя.
pub(crate) async fn drop_owner_link(
    conn: &mut PgConnection,
    ctx: &Ctx,
    user_id: Uuid,
) -> AppResult<()> {
    sqlx::query!("delete from telegram_codes where user_id = $1", user_id)
        .execute(&mut *conn)
        .await?;
    // Режим вопросов отключённого владельца закрывается вместе с привязкой (ADR-067).
    sqlx::query!("delete from bot_question_mode where user_id = $1", user_id)
        .execute(&mut *conn)
        .await?;
    let gone = sqlx::query!("delete from telegram_links where user_id = $1", user_id)
        .execute(&mut *conn)
        .await?
        .rows_affected();
    if gone > 0 {
        ops::audit(
            conn,
            ctx,
            "telegram.unlink",
            "user",
            Some(user_id),
            json!({ "reason": "пользователь отключён или больше не владелец" }),
        )
        .await?;
    }
    Ok(())
}

// ---------- Логика бота ----------

const HELP: &str = "Нажмите кнопку внизу:\n📊 Сегодня — выручка и прибыль за день, деньги в кассах\n💵 Смена — открытая смена и сколько должно быть в кассе\n📒 Долги — кто сколько должен";

// Кнопки внизу чата: нажатие отправляет их текст, как команду.
const BTN_TODAY: &str = "📊 Сегодня";
const BTN_SHIFT: &str = "💵 Смена";
const BTN_DEBTS: &str = "📒 Долги";
const BTN_CARS: &str = "🚗 Мои машины";
const BTN_BONUS: &str = "⭐ Баллы";
const BTN_LEAVE: &str = "🚪 Отключиться";
const BTN_LEAVE_YES: &str = "Да, отключиться";
const BTN_BACK: &str = "Назад";
const BTN_CONTACT: &str = "📱 Поделиться номером";
/// Режим вопросов владельца (SPEC-21, ADR-067): только в нём текст уходит в сервис разбора.
const BTN_ASK: &str = "💬 Спросить";
const BTN_END: &str = "✖ Закончить";

/// Строка подсказки владельцу, когда сервис разбора вопросов задан.
const ASK_HELP: &str = "\n💬 Спросить — вопрос о делах точки своими словами";
const ASK_PROMPT: &str = "Задайте вопрос о делах точки. Закончить — «✖ Закончить»";
const ASK_ENDED: &str = "Режим вопросов закрыт. Спросить снова — «💬 Спросить».";
const ASK_EXPIRED: &str = "Режим вопросов закрыт — нажмите «💬 Спросить»";

/// Кнопки клиента: из чата владельца они не вопрос и не команда.
const CUSTOMER_BUTTONS: [&str; 6] = [
    BTN_CARS,
    BTN_BONUS,
    BTN_LEAVE,
    BTN_LEAVE_YES,
    BTN_BACK,
    BTN_CONTACT,
];

/// Чат клиента отвязан: карточка отключена или номер в ней изменился (ADR-063).
pub const RESHARE: &str = "Номер в вашей карточке у нас изменился или карточка отключена, поэтому бот вас отключил. Накопленные баллы сохранились.\n\nЧтобы подключиться снова, нажмите «📱 Поделиться номером».";

/// Какие кнопки показать под ответом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keys {
    /// Не подключён: одна кнопка «Поделиться номером».
    Contact,
    /// Владелец без сервиса разбора вопросов: «Сегодня», «Смена», «Долги».
    Owner,
    /// Владелец, сервис разбора задан, режим вопросов выключен: плюс «💬 Спросить».
    OwnerAsk,
    /// Владелец в режиме вопросов: «✖ Закончить» и кнопки владельца.
    OwnerAsking,
    Customer,
    /// Работник (SPEC-23): «Сегодня», «Месяц», «Удержания и премии», «Мои работы».
    Employee,
    /// Номер и у работника, и у клиента: «Мой заработок» и кнопки клиента.
    Both,
    ConfirmLeave,
}

impl Keys {
    fn markup(self) -> Value {
        let rows: Vec<Vec<Value>> = match self {
            Keys::Contact => vec![vec![
                json!({ "text": BTN_CONTACT, "request_contact": true }),
            ]],
            Keys::Owner => vec![vec![json!(BTN_TODAY), json!(BTN_SHIFT), json!(BTN_DEBTS)]],
            Keys::OwnerAsk => vec![
                vec![json!(BTN_TODAY), json!(BTN_SHIFT), json!(BTN_DEBTS)],
                vec![json!(BTN_ASK)],
            ],
            Keys::OwnerAsking => vec![
                vec![json!(BTN_END)],
                vec![json!(BTN_TODAY), json!(BTN_SHIFT), json!(BTN_DEBTS)],
            ],
            Keys::Customer => vec![
                vec![json!(BTN_CARS), json!(BTN_BONUS)],
                vec![json!(BTN_LEAVE)],
            ],
            Keys::Employee => vec![
                vec![json!(bot_staff::BTN_DAY), json!(bot_staff::BTN_MONTH)],
                vec![json!(bot_staff::BTN_ADJUST), json!(bot_staff::BTN_WORKS)],
                vec![json!(BTN_LEAVE)],
            ],
            Keys::Both => vec![
                vec![json!(bot_staff::BTN_EARNINGS)],
                vec![json!(BTN_CARS), json!(BTN_BONUS)],
                vec![json!(BTN_LEAVE)],
            ],
            Keys::ConfirmLeave => vec![vec![json!(BTN_LEAVE_YES), json!(BTN_BACK)]],
        };
        json!({ "keyboard": rows, "resize_keyboard": true, "is_persistent": true })
    }
}

/// Команда из текста: первое слово без значков и знаков — «📊 Сегодня» → «сегодня», «/долги» → «долги».
pub(crate) fn command(text: &str) -> String {
    text.split_whitespace()
        .find(|w| w.chars().any(char::is_alphanumeric))
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase()
}

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

/// Режим вопросов в чате владельца (SPEC-21, ADR-067).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AskMode {
    /// Не включали или закончили кнопкой.
    Off,
    /// Включён, последнее сообщение — меньше 10 минут назад.
    On,
    /// Включали, но 10 минут без сообщений прошли.
    Expired,
}

/// Режим вопросов этого чата для этого владельца и филиала. Строка другого владельца (чат
/// перепривязали) — режим выключен.
async fn ask_mode(
    conn: &mut PgConnection,
    chat_id: i64,
    user_id: Uuid,
    branch_id: Uuid,
) -> AppResult<AskMode> {
    let on = sqlx::query_scalar!(
        r#"select last_at > now() - interval '10 minutes' as "on!" from bot_question_mode
           where chat_id = $1 and user_id = $2 and branch_id = $3"#,
        chat_id,
        user_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(match on {
        None => AskMode::Off,
        Some(true) => AskMode::On,
        Some(false) => AskMode::Expired,
    })
}

/// Включает режим вопросов — одной транзакцией с журналом.
async fn ask_start(
    conn: &mut PgConnection,
    chat_id: i64,
    user_id: Uuid,
    branch_id: Uuid,
) -> AppResult<()> {
    let mut tx = conn.begin().await?;
    sqlx::query!(
        r#"insert into bot_question_mode (chat_id, user_id, branch_id, started_at, last_at)
           values ($1, $2, $3, now(), now())
           on conflict (chat_id) do update
              set user_id = excluded.user_id, branch_id = excluded.branch_id,
                  started_at = excluded.started_at, last_at = excluded.last_at"#,
        chat_id,
        user_id,
        branch_id
    )
    .execute(&mut *tx)
    .await?;
    ask_journal(
        &mut tx,
        chat_id,
        user_id,
        branch_id,
        "telegram.ask_on",
        None,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Сообщение владельца в режиме: ещё 10 минут.
async fn ask_touch(conn: &mut PgConnection, chat_id: i64) -> AppResult<()> {
    sqlx::query!(
        "update bot_question_mode set last_at = now() where chat_id = $1",
        chat_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Причина закрытия режима по времени — в журнале `telegram.ask_off`.
const ASK_TIMEOUT_REASON: &str = "время";

/// Закрывает режим вопросов (кнопкой или по времени): строка удаляется, память разговора
/// чистится — следующий режим начинается с чистого листа; открытый черновик чата снимается
/// (SPEC-24). Одной транзакцией с журналом.
async fn ask_stop(
    conn: &mut PgConnection,
    chat_id: i64,
    user_id: Uuid,
    branch_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    let mut tx = conn.begin().await?;
    let gone = sqlx::query!("delete from bot_question_mode where chat_id = $1", chat_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    sqlx::query!("delete from bot_dialog where chat_id = $1", chat_id)
        .execute(&mut *tx)
        .await?;
    crate::api::bot_drafts::close_open(&mut tx, chat_id, "режим вопросов закрыт").await?;
    if gone > 0 {
        ask_journal(
            &mut tx,
            chat_id,
            user_id,
            branch_id,
            "telegram.ask_off",
            Some(reason),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn ask_journal(
    conn: &mut PgConnection,
    chat_id: i64,
    user_id: Uuid,
    branch_id: Uuid,
    action: &str,
    reason: Option<&str>,
) -> AppResult<()> {
    let mut data = json!({ "chat": chat_id });
    if let Some(r) = reason {
        data["reason"] = json!(r);
    }
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, action, entity, entity_id, data)
           values ($1, $2, $3, $4, 'user', $3, $5)"#,
        ops::new_id(),
        branch_id,
        user_id,
        action,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Кнопки владельца: без сервиса разбора — три команды; с ним — ещё «💬 Спросить» или, в режиме
/// вопросов, «✖ Закончить».
async fn owner_keys(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
) -> AppResult<Keys> {
    if assistant.is_none() {
        return Ok(Keys::Owner);
    }
    let Some((user_id, branch_id)) = chat_owner(conn, chat_id).await? else {
        return Ok(Keys::Owner);
    };
    Ok(match ask_mode(conn, chat_id, user_id, branch_id).await? {
        AskMode::On => Keys::OwnerAsking,
        AskMode::Off | AskMode::Expired => Keys::OwnerAsk,
    })
}

/// Чат владельца в режиме вопросов (не истёкшем) — только тогда свободный текст идёт в очередь
/// вопросов (SPEC-21, ADR-067).
pub async fn owner_asking(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    let Some((user_id, branch_id)) = chat_owner(conn, chat_id).await? else {
        return Ok(false);
    };
    Ok(ask_mode(conn, chat_id, user_id, branch_id).await? == AskMode::On)
}

/// Кнопка под сообщением: текст и данные, которые Телеграм вернёт при нажатии.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub text: String,
    pub data: String,
}

/// Ответ бота и кнопки под ним.
pub struct Reply {
    pub text: String,
    pub keys: Keys,
    /// Кнопки под самим сообщением (SPEC-22); пусто — только кнопки внизу чата.
    pub inline: Vec<Vec<Button>>,
    /// Следующее сообщение — черновик операции владельца с кнопками «✅ Провести» и «✖ Отмена»
    /// (SPEC-24).
    pub follow: Option<Follow>,
}

/// Отдельное сообщение после ответа: текст и кнопки под ним.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    pub text: String,
    pub inline: Vec<Vec<Button>>,
}

impl Follow {
    /// Черновик операции с кнопками подтверждения.
    fn draft(d: &crate::api::bot_drafts::Shown) -> Self {
        Self {
            text: d.summary.clone(),
            inline: vec![
                crate::api::bot_drafts::buttons(d.id)
                    .into_iter()
                    .map(|(text, data)| Button { text, data })
                    .collect(),
            ],
        }
    }

    fn markup(&self) -> Value {
        inline_markup(&self.inline)
    }
}

fn inline_markup(rows: &[Vec<Button>]) -> Value {
    let rows: Vec<Vec<Value>> = rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|b| json!({ "text": b.text, "callback_data": b.data }))
                .collect()
        })
        .collect();
    json!({ "inline_keyboard": rows })
}

impl Reply {
    pub(crate) fn new(text: impl Into<String>, keys: Keys) -> Self {
        Self {
            text: text.into(),
            keys,
            inline: Vec::new(),
            follow: None,
        }
    }

    /// Ответ клиенту с кнопками под сообщением.
    pub(crate) fn with_buttons(text: impl Into<String>, rows: Vec<Vec<Button>>) -> Self {
        Self {
            text: text.into(),
            keys: Keys::Customer,
            inline: rows,
            follow: None,
        }
    }

    /// Разметка кнопок для Телеграма: под сообщением, если они есть, иначе внизу чата.
    fn markup(&self) -> Value {
        if self.inline.is_empty() {
            return self.keys.markup();
        }
        inline_markup(&self.inline)
    }
}

/// Кнопка меню клиента или команда: отменяет ожидание ввода, а не становится номером или пометкой.
fn is_menu(text: &str) -> bool {
    let t = text.trim();
    t.starts_with('/')
        || [BTN_CARS, BTN_BONUS, BTN_LEAVE, BTN_LEAVE_YES, BTN_BACK].contains(&t)
        || bot_staff::BUTTONS.contains(&t)
}

/// Код привязки цифрами: ровно шесть цифр.
fn is_code(text: &str) -> bool {
    text.len() == 6 && text.chars().all(|c| c.is_ascii_digit())
}

/// Свободный текст владельца: не кнопка, не `/команда`, не одно слово команды и не код.
/// В сервис разбора вопросов он уходит только в режиме «💬 Спросить» (SPEC-21, ADR-067); вне
/// режима на него — подсказка с кнопками. Только такие сообщения ждут очереди вопросов —
/// кнопки отвечают сразу (ADR-063).
pub fn is_question(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || text.starts_with('/') || is_code(text) {
        return false;
    }
    if CUSTOMER_BUTTONS.contains(&text)
        || bot_staff::BUTTONS.contains(&text)
        || [BTN_ASK, BTN_END].contains(&text)
    {
        return false;
    }
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count();
    if words == 0 {
        return false;
    }
    let known = matches!(
        command(text).as_str(),
        "сегодня" | "today" | "смена" | "shift" | "долги" | "debts"
    );
    !known || words > 1
}

/// Номер для сравнения: последние 9 цифр — так совпадут «+996 555 12 34 56» и «0555123456».
pub fn phone_key(phone: &str) -> Option<String> {
    let digits: String = phone.chars().filter(char::is_ascii_digit).collect();
    (digits.len() >= 9).then(|| digits[digits.len() - 9..].to_string())
}

/// Контакт из Телеграма: номер и чей он (кнопка «Поделиться номером» шлёт свой).
pub struct SharedContact {
    pub phone: String,
    pub user_id: Option<i64>,
}

/// Запись в журнал действий от имени бота: пользователя и устройства нет, чат — в данных.
pub(crate) async fn bot_journal(
    conn: &mut PgConnection,
    branch_id: Uuid,
    action: &str,
    entity: &str,
    entity_id: Option<Uuid>,
    data: Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, $3, $4, $5, $6)"#,
        ops::new_id(),
        branch_id,
        action,
        entity,
        entity_id,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Чат клиента: подключён, не подключён или только что отвязан проверкой.
enum CustomerLink {
    None,
    Linked(Uuid),
    Dropped,
}

/// Подключение действует, пока карточка клиента активна и номер в ней тот же, каким клиент
/// подключился (ADR-063). Иначе чат отвязывается с записью в журнал.
fn link_valid(active: bool, role: &str, card_phone: &str, link_phone: &str) -> bool {
    active && role == "customer" && phone_key(card_phone) == phone_key(link_phone)
}

async fn drop_customer(
    conn: &mut PgConnection,
    chat_id: i64,
    party_id: Uuid,
    branch_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    let mut tx = conn.begin().await?;
    let gone = sqlx::query!(
        "delete from telegram_customers where chat_id = $1 and party_id = $2",
        chat_id,
        party_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if gone > 0 {
        bot_journal(
            &mut tx,
            branch_id,
            "telegram.customer_unlink",
            "party",
            Some(party_id),
            json!({ "chat": chat_id, "reason": reason }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

const STALE_REASON: &str = "карточка отключена или номер в ней изменён";

async fn customer_link(conn: &mut PgConnection, chat_id: i64) -> AppResult<CustomerLink> {
    let r = sqlx::query!(
        r#"select t.party_id, t.phone as link_phone, p.phone, p.active, p.role, p.branch_id
           from telegram_customers t join parties p on p.id = t.party_id
           where t.chat_id = $1"#,
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(r) = r else {
        return Ok(CustomerLink::None);
    };
    if link_valid(r.active, &r.role, &r.phone, &r.link_phone) {
        return Ok(CustomerLink::Linked(r.party_id));
    }
    drop_customer(conn, chat_id, r.party_id, r.branch_id, STALE_REASON).await?;
    Ok(CustomerLink::Dropped)
}

/// Отвязать чаты клиентов, чья карточка отключена или сменила номер, — перед напоминаниями и
/// баллами. Возвращает чаты, которым надо сказать, что номер нужен снова.
pub async fn drop_stale_customers(conn: &mut PgConnection) -> AppResult<Vec<i64>> {
    let rows = sqlx::query!(
        r#"select t.chat_id, t.party_id, t.phone as link_phone, p.phone, p.active, p.role, p.branch_id
           from telegram_customers t join parties p on p.id = t.party_id"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        if !link_valid(r.active, &r.role, &r.phone, &r.link_phone) {
            drop_customer(conn, r.chat_id, r.party_id, r.branch_id, STALE_REASON).await?;
            out.push(r.chat_id);
        }
    }
    Ok(out)
}

/// Сообщение в бот без вопросов текстом — ровно SPEC-18.
pub async fn handle_message(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    text: Option<&str>,
    contact: Option<SharedContact>,
) -> AppResult<Reply> {
    handle_message_with(conn, None, chat_id, from_id, text, contact).await
}

/// Кнопки внизу чата по тому, кем подключён чат.
fn section_keys(customer: bool, employee: bool) -> Keys {
    match (customer, employee) {
        (true, true) => Keys::Both,
        (true, false) => Keys::Customer,
        (false, true) => Keys::Employee,
        (false, false) => Keys::Contact,
    }
}

/// Чат подключён и клиентом, и работником (SPEC-23): внизу — кнопки обоих разделов, а не одного.
async fn fit_keys(conn: &mut PgConnection, chat_id: i64, mut r: Reply) -> AppResult<Reply> {
    if matches!(r.keys, Keys::Customer | Keys::Employee) {
        let both = sqlx::query_scalar!(
            r#"select exists (select 1 from telegram_customers where chat_id = $1)
                      and exists (select 1 from telegram_employees where chat_id = $1) as "b!""#,
            chat_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if both {
            r.keys = Keys::Both;
        }
    }
    Ok(r)
}

/// Сообщение в бот: владелец — команды, привязка кодом и вопросы текстом (если сервис разбора
/// задан, SPEC-21); клиент — своя масляная книжка; работник — свой заработок (SPEC-23).
/// `text = None` — не текст (наклейка, фото).
pub async fn handle_message_with(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    from_id: i64,
    text: Option<&str>,
    contact: Option<SharedContact>,
) -> AppResult<Reply> {
    let r = message(conn, assistant, chat_id, from_id, text, contact).await?;
    fit_keys(conn, chat_id, r).await
}

async fn message(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    from_id: i64,
    text: Option<&str>,
    contact: Option<SharedContact>,
) -> AppResult<Reply> {
    let owner = chat_owner(conn, chat_id).await?.is_some();
    if let Some(c) = contact {
        // Владельцу контакт ни к чему: клавиатуры клиента и работника он не получает.
        if owner {
            let keys = owner_keys(conn, assistant, chat_id).await?;
            return Ok(Reply::new(HELP, keys));
        }
        return link_contact(conn, chat_id, from_id, c).await;
    }
    let raw = text;
    let text = text.unwrap_or("");
    let (mut customer, mut employee) = (None, None);
    let mut dropped: Vec<&str> = Vec::new();
    if !owner {
        // Подключение работника и клиента проверяется при каждом сообщении (ADR-063, SPEC-23).
        match bot_staff::employee_link(conn, chat_id).await? {
            EmployeeLink::Linked(e) => employee = Some(e),
            EmployeeLink::None => {}
            EmployeeLink::Dropped => dropped.push(bot_staff::RESHARE),
        }
        match customer_link(conn, chat_id).await? {
            CustomerLink::Linked(p) => customer = Some(p),
            CustomerLink::None => {}
            CustomerLink::Dropped => dropped.push(RESHARE),
        }
    }
    // Код привязки владельца из такого чата всё равно проверяем.
    if !dropped.is_empty() && !text.trim().starts_with("/start") {
        return Ok(Reply::new(
            dropped.join("\n\n"),
            section_keys(customer.is_some(), employee.is_some()),
        ));
    }
    let both = customer.is_some() && employee.is_some();
    // Чат работника — не больше 30 действий в минуту (SPEC-23).
    if employee.is_some() && bot_staff::too_often(conn, chat_id).await? {
        return Ok(bot_staff::too_often_reply());
    }
    // Клиент пишет номер, марку или пометку: это не команда и не код привязки (SPEC-22).
    if let Some(party_id) = customer {
        if is_menu(text) {
            crate::api::bot_cars::clear_input(conn, chat_id).await?;
        } else if let Some(r) = crate::api::bot_cars::on_input(conn, party_id, chat_id, raw).await?
        {
            return Ok(r);
        }
    }
    if let Some(e) = &employee
        && let Some(s) = bot_staff::section(text)
    {
        return bot_staff::show(conn, e, s, both).await;
    }
    let linked = customer.is_some() || employee.is_some();
    let answer = handle_text(conn, assistant, chat_id, from_id, text, linked).await?;
    if let Some((r, draft)) = answer {
        // Код привязки мог прислать и не владелец: кнопки — по тому, кто он сейчас.
        let keys = if chat_owner(conn, chat_id).await?.is_some() {
            owner_keys(conn, assistant, chat_id).await?
        } else {
            section_keys(customer.is_some(), employee.is_some())
        };
        let mut reply = Reply::new(r, keys);
        reply.follow = draft.as_ref().map(Follow::draft);
        return Ok(reply);
    }
    // Отключение — только кнопкой подтверждения или ровно «/стоп»: фраза, начатая со «стоп»
    // или «да», — обычный текст (ADR-063). Отключается всё, к чему подключён чат.
    let exact = text.trim();
    if linked && (exact == BTN_LEAVE_YES || exact.eq_ignore_ascii_case("/stop") || exact == "/стоп")
    {
        let mut done = "Готово, вы отключены.".to_string();
        if let Some(party_id) = customer {
            let branch_id =
                sqlx::query_scalar!("select branch_id from parties where id = $1", party_id)
                    .fetch_one(&mut *conn)
                    .await?;
            drop_customer(conn, chat_id, party_id, branch_id, "клиент отключился сам").await?;
            done.push_str(" Накопленные баллы сохранились.");
        }
        if let Some(e) = &employee {
            bot_staff::drop_chat(conn, chat_id, e.id, e.branch_id, "работник отключился сам")
                .await?;
        }
        return Ok(Reply::new(
            format!("{done}\n\nЧтобы вернуться, нажмите «📱 Поделиться номером»."),
            Keys::Contact,
        ));
    }
    if linked && command(text) == "отключиться" {
        let mut ask = "Отключиться от бота?".to_string();
        if customer.is_some() {
            ask.push_str("\n\nНапоминаний о замене больше не будет и новые баллы не начислятся. Накопленные баллы сохранятся.");
        }
        if employee.is_some() {
            ask.push_str("\n\nЗаработок в боте не будет виден, пока снова не поделитесь номером.");
        }
        return Ok(Reply::new(ask, Keys::ConfirmLeave));
    }
    // Не клиент: работнику на свободный текст — его кнопки (вопросы текстом ему недоступны,
    // SPEC-23), остальным — кнопка номера.
    let Some(party_id) = customer else {
        if let Some(e) = &employee {
            if exact == BTN_BACK {
                return bot_staff::show(conn, e, bot_staff::Section::Day, false).await;
            }
            return Ok(bot_staff::help(e, false));
        }
        return Ok(Reply::new(
            "Здравствуйте! Это бот Avtodom.\n\nЗдесь вы увидите, когда меняли масло, какое залили, когда следующая замена, и свои бонусные баллы.\n\nНажмите кнопку «📱 Поделиться номером» внизу — нужен тот номер, что записан у нас на кассе.",
            Keys::Contact,
        ));
    };
    match command(text).as_str() {
        "баллы" | "bonus" => Ok(Reply::new(
            bonus_history(conn, party_id).await?,
            Keys::Customer,
        )),
        "мои" | "машины" | "cars" => {
            crate::api::bot_cars::cars_for(conn, party_id, chat_id).await
        }
        _ => Ok(Reply::new(
            customer_book(conn, party_id).await?,
            Keys::Customer,
        )),
    }
}

const NOT_LINKED: &str = "Сначала подключитесь: нажмите «📱 Поделиться номером» внизу.";

/// Нажатие кнопки под сообщением. Только подключённому клиенту или работнику: владелец и чужой
/// чат их не получают. Нажал тот, чей это личный чат (`from_id = chat_id`).
pub async fn handle_callback(
    conn: &mut PgConnection,
    chat_id: i64,
    data: &str,
) -> AppResult<Reply> {
    handle_callback_from(conn, None, chat_id, chat_id, data).await
}

/// Нажатие кнопки под сообщением; `assistant` — только чтобы показать владельцу его кнопки
/// («💬 Спросить» или «✖ Закончить»), вопросов здесь нет.
pub async fn handle_callback_with(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    data: &str,
) -> AppResult<Reply> {
    handle_callback_from(conn, assistant, chat_id, chat_id, data).await
}

/// Нажатие кнопки: `from_id` — кто нажал в Телеграме. Кнопки черновика владельца принимаются
/// только от того, кто привязал чат (ADR-070).
pub async fn handle_callback_from(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    from_id: i64,
    data: &str,
) -> AppResult<Reply> {
    let r = callback(conn, assistant, chat_id, from_id, data).await?;
    fit_keys(conn, chat_id, r).await
}

async fn callback(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    from_id: i64,
    data: &str,
) -> AppResult<Reply> {
    if let Some((user_id, branch_id)) = chat_owner(conn, chat_id).await? {
        // Нажатие в открытом режиме вопросов — тоже сообщение владельца: режим продлевается.
        if assistant.is_some() && ask_mode(conn, chat_id, user_id, branch_id).await? == AskMode::On
        {
            ask_touch(conn, chat_id).await?;
        }
        let keys = owner_keys(conn, assistant, chat_id).await?;
        // «✅ Провести» и «✖ Отмена» под черновиком операции (SPEC-24): чат, владелец, кто нажал,
        // филиал и срок черновика сверяет `bot_drafts::press`.
        if data.starts_with("d:") {
            let text = crate::api::bot_drafts::press(conn, chat_id, from_id, data).await?;
            return Ok(Reply::new(text, keys));
        }
        return Ok(Reply::new(HELP, keys));
    }
    // Кнопки кабинета работника: сотрудник из данных кнопки сверяется с привязкой чата.
    if data.starts_with("e:") {
        let employee = bot_staff::employee_link(conn, chat_id).await?;
        let customer = matches!(customer_link(conn, chat_id).await?, CustomerLink::Linked(_));
        return match employee {
            EmployeeLink::Linked(e) => {
                if bot_staff::too_often(conn, chat_id).await? {
                    return Ok(bot_staff::too_often_reply());
                }
                bot_staff::callback(conn, &e, data, customer).await
            }
            EmployeeLink::Dropped => Ok(Reply::new(
                bot_staff::RESHARE,
                section_keys(customer, false),
            )),
            EmployeeLink::None if customer => Ok(Reply::new(
                "Кнопка устарела — нажмите кнопку внизу.",
                Keys::Customer,
            )),
            EmployeeLink::None => Ok(Reply::new(NOT_LINKED, Keys::Contact)),
        };
    }
    match customer_link(conn, chat_id).await? {
        CustomerLink::Linked(party_id) => {
            crate::api::bot_cars::callback(conn, party_id, chat_id, data).await
        }
        CustomerLink::Dropped => Ok(Reply::new(RESHARE, Keys::Contact)),
        CustomerLink::None => {
            if matches!(
                bot_staff::employee_link(conn, chat_id).await?,
                EmployeeLink::Linked(_)
            ) {
                return Ok(Reply::new(
                    "Кнопка устарела — нажмите кнопку внизу.",
                    Keys::Employee,
                ));
            }
            Ok(Reply::new(NOT_LINKED, Keys::Contact))
        }
    }
}

const AMBIGUOUS: &str = "По этому номеру у нас несколько карточек клиентов, и мы не знаем, какая ваша. Обратитесь на точку — оставят номер в одной карточке, и подключитесь снова.";
const NOT_CUSTOMER: &str = "Этого номера нет среди наших клиентов. Назовите его кассиру при следующей покупке или замене — и потом снова нажмите «📱 Поделиться номером».";

/// Контакт из кнопки «Поделиться номером»: номер сверяется сначала с активными сотрудниками
/// (SPEC-23), потом — независимо — с клиентами (SPEC-18). Совпал и тот, и другой — чат получает
/// оба раздела.
async fn link_contact(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    c: SharedContact,
) -> AppResult<Reply> {
    // Только свой номер: чужую карточку контакта переслать можно, но она ничего не откроет.
    if c.user_id != Some(from_id) {
        return Ok(Reply::new(
            "Нужен ваш собственный номер: нажмите кнопку «📱 Поделиться номером» внизу.",
            Keys::Contact,
        ));
    }
    let Some(key) = phone_key(&c.phone) else {
        return Ok(Reply::new("Номер не распознан.", Keys::Contact));
    };
    // Ровно один активный сотрудник — чат работника; несколько — не угадываем (SPEC-23).
    let mut staff = bot_staff::by_phone(conn, &key).await?;
    let staff_ambiguous = staff.len() > 1;
    let employee = if staff.len() == 1 { staff.pop() } else { None };
    if let Some(e) = &employee {
        bot_staff::link(conn, chat_id, e, &c.phone).await?;
    }
    let customer = link_customer(conn, chat_id, &key, &c.phone).await?;
    match (employee, customer) {
        (Some(e), CustomerMatch::Linked { party_id, .. }) => {
            let book = customer_book(conn, party_id).await?;
            Ok(Reply::new(
                format!(
                    "{}, вы подключены ✅\n\n💼 Как работник: кнопка «💼 Мой заработок» — сколько заработали за день и месяц, удержания и премии, ваши работы.\n\n🚗 Как клиент: напомним о замене масла, с покупок — бонусные баллы (1 балл = 1 сом). Кнопки «🚗 Мои машины», «⭐ Баллы».\n\n{book}",
                    e.name
                ),
                Keys::Both,
            ))
        }
        (Some(e), _) => {
            let day = bot_staff::show(conn, &e, bot_staff::Section::Day, false)
                .await?
                .text;
            Ok(Reply::new(
                format!(
                    "{}, вы подключены ✅\n\n{}\n\n{day}",
                    e.name,
                    bot_staff::HELP
                ),
                Keys::Employee,
            ))
        }
        (None, CustomerMatch::Linked { party_id, name }) => {
            let book = customer_book(conn, party_id).await?;
            let staff_note = if staff_ambiguous {
                format!("\n\n{}", bot_staff::AMBIGUOUS)
            } else {
                String::new()
            };
            Ok(Reply::new(
                format!(
                    "{name}, вы подключены ✅\n\n• Напомним о замене масла за неделю и за день до срока.\n• С каждой покупки — бонусные баллы: 1 балл = 1 сом. Чтобы списать, назовите номер на кассе.\n\nКнопки внизу: «🚗 Мои машины», «⭐ Баллы».\n\n{book}{staff_note}"
                ),
                Keys::Customer,
            ))
        }
        (None, CustomerMatch::Ambiguous) if staff_ambiguous => Ok(Reply::new(
            format!("{}\n\n{AMBIGUOUS}", bot_staff::AMBIGUOUS),
            Keys::Contact,
        )),
        (None, CustomerMatch::Ambiguous) => Ok(Reply::new(AMBIGUOUS, Keys::Contact)),
        (None, CustomerMatch::NotFound) if staff_ambiguous => {
            Ok(Reply::new(bot_staff::AMBIGUOUS, Keys::Contact))
        }
        (None, CustomerMatch::NotFound) => Ok(Reply::new(NOT_CUSTOMER, Keys::Contact)),
    }
}

/// Что дала сверка номера с карточками клиентов.
enum CustomerMatch {
    NotFound,
    Ambiguous,
    Linked { party_id: Uuid, name: String },
}

async fn link_customer(
    conn: &mut PgConnection,
    chat_id: i64,
    key: &str,
    phone: &str,
) -> AppResult<CustomerMatch> {
    // Только телефон в самой карточке клиента (человек или фирма, SPEC-18); телефоны контактов
    // фирмы не подключают. Несколько карточек — не угадываем (ADR-063).
    let found = sqlx::query!(
        r#"select id, name, branch_id from parties
           where role = 'customer' and active and length(regexp_replace(phone, '[^0-9]', '', 'g')) >= 9
             and right(regexp_replace(phone, '[^0-9]', '', 'g'), 9) = $1
           order by created_at desc"#,
        key
    )
    .fetch_all(&mut *conn)
    .await?;
    let p = match found.as_slice() {
        [] => return Ok(CustomerMatch::NotFound),
        [one] => one,
        many => {
            let mut tx = conn.begin().await?;
            let mut branches: Vec<Uuid> = many.iter().map(|p| p.branch_id).collect();
            branches.sort();
            branches.dedup();
            for b in branches {
                // Владельцу — не чаще раза в сутки на чат.
                let told = sqlx::query_scalar!(
                    r#"select exists (select 1 from audit_log
                                      where branch_id = $1 and action = 'telegram.customer_ambiguous'
                                        and data->>'chat' = $2 and created_at > now() - interval '1 day') as "e!""#,
                    b,
                    chat_id.to_string()
                )
                .fetch_one(&mut *tx)
                .await?;
                if told {
                    continue;
                }
                let names: Vec<&str> = many
                    .iter()
                    .filter(|p| p.branch_id == b)
                    .map(|p| p.name.as_str())
                    .collect();
                bot_journal(
                    &mut tx,
                    b,
                    "telegram.customer_ambiguous",
                    "party",
                    None,
                    json!({ "chat": chat_id.to_string(), "phone": format!("…{}", &key[key.len() - 4..]), "names": names }),
                )
                .await?;
            }
            tx.commit().await?;
            return Ok(CustomerMatch::Ambiguous);
        }
    };
    let mut tx = conn.begin().await?;
    sqlx::query!(
        r#"insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, $3)
           on conflict (chat_id) do update set party_id = excluded.party_id, phone = excluded.phone, linked_at = now()"#,
        chat_id,
        p.id,
        phone
    )
    .execute(&mut *tx)
    .await?;
    bot_journal(
        &mut tx,
        p.branch_id,
        "telegram.customer_link",
        "party",
        Some(p.id),
        json!({ "chat": chat_id }),
    )
    .await?;
    tx.commit().await?;
    Ok(CustomerMatch::Linked {
        party_id: p.id,
        name: p.name.clone(),
    })
}

pub(crate) fn km(v: i32) -> String {
    let s = v.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    format!("{out} км")
}

/// Масляная книжка клиента текстом: по машинам — последняя замена и следующая.
fn points(v: i64) -> String {
    // «12,50 с» → «12,50»: баллы показываем без валюты.
    format_som(v).trim_end_matches(" с").to_string()
}

/// Баллы клиента и последние операции (SPEC-19).
pub async fn bonus_history(conn: &mut PgConnection, party_id: Uuid) -> AppResult<String> {
    let h = crate::api::loyalty::history(conn, party_id, 10).await?;
    let mut lines = vec![format!(
        "Баллов: {} (1 балл = 1 сом, списать — назовите номер на кассе)",
        points(h.balance_tyiyn)
    )];
    for r in &h.rows {
        let what = match r.kind.as_str() {
            "accrual" => "начислено",
            "redeem" => "списано",
            "refund" => "возвращено при возврате товара",
            _ => "снято при возврате товара",
        };
        let sign = if r.amount_tyiyn > 0 { "+" } else { "−" };
        lines.push(format!(
            "{} {sign}{} — {what}{}",
            // Бишкек — UTC+6 круглый год.
            (r.created_at + chrono::Duration::hours(6)).format("%d.%m"),
            points(r.amount_tyiyn.abs()),
            r.sale_number
                .map(|n| format!(", чек № {n}"))
                .unwrap_or_default()
        ));
    }
    Ok(lines.join("\n"))
}

/// Сообщения клиентам об операциях с баллами: по одному на чек.
pub struct BonusNotice {
    pub chat_id: i64,
    pub text: String,
    pub ledger_ids: Vec<Uuid>,
}

pub async fn bonus_notices(conn: &mut PgConnection) -> AppResult<Vec<BonusNotice>> {
    let rows = sqlx::query!(
        r#"select l.id, l.kind, l.amount_tyiyn, l.party_id, l.doc_id, s.number as "sale_number?", t.chat_id
           from loyalty_ledger l
           join telegram_customers t on t.party_id = l.party_id and l.created_at >= t.linked_at
           left join sales s on s.id = l.doc_id
           where l.created_at > now() - interval '3 days'
             and not exists (select 1 from loyalty_notices n where n.ledger_id = l.id)
           order by l.created_at, l.doc_id"#
    )
    .fetch_all(&mut *conn)
    .await?;
    struct Group {
        chat_id: i64,
        doc_id: Uuid,
        party_id: Uuid,
        number: Option<i64>,
        ops: Vec<(String, i64)>,
        ids: Vec<Uuid>,
    }
    let mut out: Vec<Group> = Vec::new();
    for r in rows {
        match out
            .iter_mut()
            .find(|x| x.chat_id == r.chat_id && x.doc_id == r.doc_id)
        {
            Some(x) => {
                x.ops.push((r.kind, r.amount_tyiyn));
                x.ids.push(r.id);
            }
            None => out.push(Group {
                chat_id: r.chat_id,
                doc_id: r.doc_id,
                party_id: r.party_id,
                number: r.sale_number,
                ops: vec![(r.kind, r.amount_tyiyn)],
                ids: vec![r.id],
            }),
        }
    }
    let mut notices = Vec::new();
    for g in out {
        let parts: Vec<String> = g
            .ops
            .iter()
            .map(|(kind, v)| match kind.as_str() {
                "accrual" => format!("начислено {}", points(*v)),
                "redeem" => format!("списано {}", points(-v)),
                "refund" => format!("возвращено {}", points(*v)),
                _ => format!("снято {}", points(-v)),
            })
            .collect();
        let balance = crate::api::loyalty::balance(conn, g.party_id).await?;
        let (chat_id, number, ids) = (g.chat_id, g.number, g.ids);
        notices.push(BonusNotice {
            chat_id,
            text: format!(
                "Баллы{}: {}. Теперь у вас {}.",
                number
                    .map(|n| format!(" по чеку № {n}"))
                    .unwrap_or_default(),
                parts.join(", "),
                points(balance)
            ),
            ledger_ids: ids,
        });
    }
    Ok(notices)
}

pub async fn mark_bonus_notice(conn: &mut PgConnection, n: &BonusNotice) -> AppResult<()> {
    sqlx::query!(
        "insert into loyalty_notices (ledger_id) select unnest($1::uuid[]) on conflict do nothing",
        &n.ledger_ids[..]
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Машин в книжке одним сообщением; остальные — по номеру или кнопкой «🚗 Мои машины».
pub const BOOK_CARS: usize = 20;

pub async fn customer_book(conn: &mut PgConnection, party_id: Uuid) -> AppResult<String> {
    let branch_id = sqlx::query_scalar!("select branch_id from parties where id = $1", party_id)
        .fetch_one(&mut *conn)
        .await?;
    let bonus = format!(
        "⭐ Баллов: {} — подробнее кнопкой «⭐ Баллы»",
        points(crate::api::loyalty::balance(conn, party_id).await?)
    );
    let books = crate::api::oil_book::books(conn, branch_id, Some(party_id), None).await?;
    if books.is_empty() {
        return Ok(format!(
            "{bonus}\n\nМашин у вас пока не записано — назовите госномер кассиру при замене."
        ));
    }
    let mut parts: Vec<String> = books
        .iter()
        .take(BOOK_CARS)
        .map(|b| {
            let car = [b.brand.as_str(), b.model.as_str()]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let head = if car.is_empty() {
                b.plate.clone()
            } else {
                format!("{} ({car})", b.plate)
            };
            let Some(last) = b.records.first() else {
                return format!("{head}\nЗамен пока не было.");
            };
            let mut lines = vec![
                head,
                format!(
                    "Последняя замена: {}{}",
                    last.change_date.format("%d.%m.%Y"),
                    last.mileage_km
                        .map(|m| format!(", {}", km(m)))
                        .unwrap_or_default()
                ),
            ];
            lines.extend(crate::api::oil_book::record_lines(last, ""));
            if !last.declined.is_empty() {
                lines.push(format!("Мастер рекомендовал: {}", last.declined));
            }
            // Следующая замена по каждой строке книжки (SPEC-27).
            let next = crate::api::oil_book::next_lines(b, "  ");
            if !next.is_empty() {
                lines.push("Следующая замена:".into());
                lines.extend(next);
            }
            lines.join("\n")
        })
        .collect();
    if books.len() > BOOK_CARS {
        parts.push(format!(
            "… и ещё {} — откройте «🚗 Мои машины» и напишите номер машины.",
            books.len() - BOOK_CARS
        ));
    }
    Ok(format!("{bonus}\n\n{}", parts.join("\n\n")))
}

// ---------- Напоминания о замене ----------

/// Напоминания клиентам: за 7 дней и за 1 день до срока замены, каждое один раз (ADR-051).
/// Пропущенное (сервер был выключен) догоняется: «за неделю» — пока до срока больше дня,
/// «за день» — в последний день и в сам день срока; оба сразу не уходят (ADR-063).
/// Срок считается по каждой строке книжки отдельно (SPEC-27).
pub struct Reminder {
    pub chat_id: i64,
    pub vehicle_id: Uuid,
    pub due: NaiveDate,
    pub days_before: i32,
    /// Строки книжки, срок которых подошёл в этот день (SPEC-27): одним сообщением.
    pub slots: Vec<String>,
    pub text: String,
}

/// «масла», «масляного фильтра» — что менять, в напоминании.
fn slot_genitive(slot: &str) -> &'static str {
    match slot {
        "oil" => "масла",
        "oil_filter" => "масляного фильтра",
        "air_filter" => "воздушного фильтра",
        "cabin_filter" => "салонного фильтра",
        "fuel_filter" => "топливного фильтра",
        _ => "прочих работ",
    }
}

/// Напоминания и сообщения о баллах уходят с 09:00 до 20:00 по Бишкеку (ADR-063).
pub fn in_send_window(hour: u32) -> bool {
    (9..20).contains(&hour)
}

/// «2 дня», «5 дней».
fn days_word(n: i64) -> String {
    let word = match (n % 10, n % 100) {
        (1, x) if x != 11 => "день",
        (2..=4, x) if !(12..=14).contains(&x) => "дня",
        _ => "дней",
    };
    format!("{n} {word}")
}

pub async fn due_reminders(conn: &mut PgConnection) -> AppResult<Vec<Reminder>> {
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let links = sqlx::query!(
        r#"select t.chat_id, t.party_id, p.branch_id from telegram_customers t
           join parties p on p.id = t.party_id where p.active and p.role = 'customer'"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for l in links {
        for b in crate::api::oil_book::books(conn, l.branch_id, Some(l.party_id), None).await? {
            // Каждая строка книжки — своё напоминание; подошедшие в один день — одним сообщением.
            let mut groups: Vec<(NaiveDate, i32, Vec<&crate::api::oil_book::SlotNext>)> =
                Vec::new();
            for n in &b.slots {
                let Some(due) = n.next_date else { continue };
                let left = (due - today).num_days();
                let days_before = match left {
                    0..=1 => 1,
                    2..=7 => 7,
                    _ => continue,
                };
                let sent = sqlx::query_scalar!(
                    r#"select exists (select 1 from oil_reminders
                                      where vehicle_id = $1 and slot = $2 and due_date = $3
                                        and days_before = $4) as "e!""#,
                    b.vehicle_id,
                    n.slot,
                    due,
                    days_before
                )
                .fetch_one(&mut *conn)
                .await?;
                if sent {
                    continue;
                }
                match groups.iter_mut().find(|g| g.0 == due && g.1 == days_before) {
                    Some(g) => g.2.push(n),
                    None => groups.push((due, days_before, vec![n])),
                }
            }
            for (due, days_before, slots) in groups {
                let left = (due - today).num_days();
                let when = match left {
                    0 => "сегодня".to_string(),
                    1 => "завтра".to_string(),
                    n => format!("через {}", days_word(n)),
                };
                let what = slots
                    .iter()
                    .map(|n| slot_genitive(&n.slot))
                    .collect::<Vec<_>>()
                    .join(", ");
                let km_part = slots
                    .iter()
                    .find_map(|n| n.next_km)
                    .map(|k| format!(" или на {}", km(k)))
                    .unwrap_or_default();
                out.push(Reminder {
                    chat_id: l.chat_id,
                    vehicle_id: b.vehicle_id,
                    due,
                    days_before,
                    slots: slots.iter().map(|n| n.slot.clone()).collect(),
                    text: format!(
                        "Напоминаем: замена {what} для {} — {when}, до {}{km_part}. Ждём вас в Avtodom!",
                        b.plate,
                        due.format("%d.%m.%Y")
                    ),
                });
            }
        }
    }
    Ok(out)
}

/// Отметить напоминание; ушло «за день» — «за неделю» больше не нужно, отмечается тоже.
pub async fn mark_reminded(conn: &mut PgConnection, r: &Reminder) -> AppResult<()> {
    let days: Vec<i32> = if r.days_before == 1 {
        vec![1, 7]
    } else {
        vec![r.days_before]
    };
    for slot in &r.slots {
        sqlx::query!(
            r#"insert into oil_reminders (vehicle_id, slot, due_date, days_before)
               select $1, $2, $3, unnest($4::int[]) on conflict do nothing"#,
            r.vehicle_id,
            slot,
            r.due,
            &days[..]
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

// ---------- Команды владельца и привязка кодом ----------

/// Неверных кодов привязки за 15 минут: с одного чата и всего (ADR-063).
pub const CODE_TRIES_PER_CHAT: i64 = 5;
pub const CODE_TRIES_TOTAL: i64 = 20;

/// Ответ на текст и черновик операции владельца, если разбор вопроса его подготовил (SPEC-24).
type Said = (String, Option<crate::api::bot_drafts::Shown>);

/// Команды владельца, привязка кодом и вопросы текстом в режиме «💬 Спросить» (ADR-067).
/// `None` — чат не владельца и это не код: дальше решает клиентская часть. `customer` — чат
/// подключённого клиента: шесть цифр от него — не код, если не начинаются с `/start`.
/// `from_id` — кто пишет в Телеграме: при привязке он запоминается как владелец чата (ADR-070).
async fn handle_text(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    chat_id: i64,
    from_id: i64,
    text: &str,
    customer: bool,
) -> AppResult<Option<Said>> {
    let text = text.trim();
    let word = text.split_whitespace().next().unwrap_or("").to_lowercase();
    let arg = text.split_whitespace().nth(1).unwrap_or("");
    let owner = chat_owner(conn, chat_id).await?;
    let mode = match (owner, assistant) {
        (Some((user_id, branch_id)), Some(_)) => {
            ask_mode(conn, chat_id, user_id, branch_id).await?
        }
        _ => AskMode::Off,
    };
    // «/start 123456» из ссылки или просто код цифрами. В открытом режиме вопросов чат уже
    // привязан: шесть цифр — вопрос, а не код (ADR-070).
    let code = if word == "/start" {
        arg
    } else if is_code(text) && !customer && mode != AskMode::On {
        text
    } else {
        ""
    };
    if !code.is_empty() {
        return link(conn, chat_id, from_id, code)
            .await
            .map(|t| Some((t, None)));
    }
    let Some((user_id, branch_id)) = owner else {
        return Ok(None);
    };
    let help = if assistant.is_some() {
        format!("{HELP}{ASK_HELP}")
    } else {
        HELP.to_string()
    };
    // Кнопки клиента из чата владельца — не вопрос и не команда.
    if CUSTOMER_BUTTONS.contains(&text) {
        return Ok(Some((help, None)));
    }
    // Вопросы — только в режиме «💬 Спросить», который владелец включает сам (ADR-067).
    // Без ключа сервиса режим не включается: кнопки нет, на её текст — подсказка.
    if text == BTN_ASK || text == "/спросить" {
        if assistant.is_none() {
            return Ok(Some((help, None)));
        }
        match mode {
            // Уже включён: только продлеваем, второй записи о включении нет.
            AskMode::On => ask_touch(conn, chat_id).await?,
            // Прежний режим истёк: сначала закрываем его как положено, потом открываем новый.
            AskMode::Expired => {
                ask_stop(conn, chat_id, user_id, branch_id, ASK_TIMEOUT_REASON).await?;
                ask_start(conn, chat_id, user_id, branch_id).await?;
            }
            AskMode::Off => ask_start(conn, chat_id, user_id, branch_id).await?,
        }
        return Ok(Some((ASK_PROMPT.into(), None)));
    }
    if text == BTN_END || text == "/закончить" {
        ask_stop(conn, chat_id, user_id, branch_id, "кнопкой").await?;
        return Ok(Some((
            if assistant.is_some() {
                ASK_ENDED.into()
            } else {
                help
            },
            None,
        )));
    }
    let question = is_question(text) || (mode == AskMode::On && is_code(text));
    // В режиме вопрос — свободный текст; кнопка, «/команда» и одно слово команды — команда.
    if let Some(a) = assistant
        && mode == AskMode::On
        && question
    {
        ask_touch(conn, chat_id).await?;
        return assistant::answer(conn, a, chat_id, user_id, branch_id, text)
            .await
            .map(|a| Some((a.text, a.draft)));
    }
    // Команды работают и в режиме, и без него; режим они не закрывают, а продлевают.
    let reply = match command(text).as_str() {
        "сегодня" | "today" => Some(today(conn, branch_id).await?),
        "смена" | "shift" => Some(shift(conn, branch_id).await?),
        "долги" | "debts" => Some(debts(conn, branch_id).await?),
        _ => None,
    };
    if let Some(r) = reply {
        if mode == AskMode::On {
            ask_touch(conn, chat_id).await?;
        }
        return Ok(Some((r, None)));
    }
    // Режим закрылся по времени: первый же текст после — сообщение об этом, без обращения
    // к сервису разбора.
    if assistant.is_some() && mode == AskMode::Expired && question {
        ask_stop(conn, chat_id, user_id, branch_id, ASK_TIMEOUT_REASON).await?;
        return Ok(Some((ASK_EXPIRED.into(), None)));
    }
    // Вне режима свободный текст — подсказка с кнопками, как без ключа (SPEC-18).
    Ok(Some((help, None)))
}

/// Привязка чата к владельцу по коду — одной транзакцией с журналом. Неверные коды
/// ограничены: 5 на чат и 20 всего за 15 минут; на 20-м владельцу уходит уведомление (ADR-063).
async fn link(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    code: &str,
) -> AppResult<String> {
    let mut tx = conn.begin().await?;
    let tries = sqlx::query!(
        r#"select count(*) filter (where chat_id = $1) as "chat!", count(*) as "all!"
           from telegram_code_failures where at > now() - interval '15 minutes'"#,
        chat_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if tries.chat >= CODE_TRIES_PER_CHAT {
        return Ok("Слишком много неверных кодов. Попробуйте через 15 минут.".into());
    }
    if tries.all >= CODE_TRIES_TOTAL {
        return Ok(
            "Привязка временно закрыта: слишком много неверных кодов. Попробуйте через 15 минут."
                .into(),
        );
    }
    let owner = sqlx::query!(
        r#"select u.id, u.branch_id, u.full_name from telegram_codes c join users u on u.id = c.user_id
           where c.code = $1 and c.expires_at > now() and u.active and u.role = 'owner'"#,
        code
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(o) = owner else {
        sqlx::query!("delete from telegram_code_failures where at < now() - interval '1 day'")
            .execute(&mut *tx)
            .await?;
        sqlx::query!(
            "insert into telegram_code_failures (chat_id) values ($1)",
            chat_id
        )
        .execute(&mut *tx)
        .await?;
        if tries.all + 1 >= CODE_TRIES_TOTAL {
            code_flood(&mut tx).await?;
        }
        tx.commit().await?;
        return Ok("Код не подошёл или устарел. Получите новый в «Настройки → Телеграм».".into());
    };
    sqlx::query!("delete from telegram_codes where code = $1", code)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "delete from telegram_links where chat_id = $1 or user_id = $2",
        chat_id,
        o.id
    )
    .execute(&mut *tx)
    .await?;
    // Кто привязал чат: кнопки черновиков принимаются только от него (ADR-070).
    sqlx::query!(
        "insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $4)",
        chat_id,
        o.id,
        o.branch_id,
        from_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, action, entity, entity_id, data)
           values ($1, $2, $3, 'telegram.link', 'user', $3, $4)"#,
        ops::new_id(),
        o.branch_id,
        o.id,
        json!({ "chat": chat_id })
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(format!(
        "Готово, {}: важные события точки будут приходить сюда.\n\n{HELP}",
        o.full_name
    ))
}

/// Подбирают код привязки: владельцам всех филиалов — одно уведомление за 15 минут.
async fn code_flood(conn: &mut PgConnection) -> AppResult<()> {
    let branches = sqlx::query_scalar!(
        r#"select distinct u.branch_id from users u
           where u.active and u.role = 'owner'
             and not exists (select 1 from audit_log a
                             where a.branch_id = u.branch_id and a.action = 'telegram.code_flood'
                               and a.created_at > now() - interval '15 minutes')"#
    )
    .fetch_all(&mut *conn)
    .await?;
    for b in branches {
        bot_journal(
            conn,
            b,
            "telegram.code_flood",
            "telegram",
            None,
            json!({ "failures": CODE_TRIES_TOTAL, "minutes": 15 }),
        )
        .await?;
    }
    Ok(())
}

/// «Айгуль — 308,00 с (2 % с чеков 308,00 с)» по строкам начислений.
pub(crate) fn staff_lines(staff: &[crate::api::payroll::StaffPay]) -> String {
    if staff.is_empty() {
        return "  пока никому не начислено".into();
    }
    staff
        .iter()
        .map(|p| {
            let parts = p
                .items
                .iter()
                .map(|i| {
                    format!(
                        "{} — {}",
                        pay_kind(&i.kind, i.count),
                        format_som(i.amount_tyiyn)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let owed = match p.owed_tyiyn {
                0 => "всё выдано".to_string(),
                o if o > 0 => format!("к выдаче {}", format_som(o)),
                o => format!("аванс {}", format_som(-o)),
            };
            format!(
                "  {} — {} ({parts}); {owed}",
                p.name,
                format_som(p.total_tyiyn)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Вид начисления словами.
fn pay_kind(kind: &str, count: i64) -> String {
    match kind {
        "service_fee" => format!("замены ×{count}"),
        "revenue_percent" => "% с чеков".into(),
        "shift_fee" => "за смену".into(),
        "monthly_salary" => "оклад".into(),
        "bonus" => "премия".into(),
        "penalty" => "удержание".into(),
        "shortage" => "недостача".into(),
        other => other.to_string(),
    }
}

async fn today(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let day = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    day_report(conn, branch_id, day, true).await
}

/// Итог учётного дня: чеки, прибыль, кто сколько заработал, деньги в кассах сейчас.
async fn day_report(
    conn: &mut PgConnection,
    branch_id: Uuid,
    day: NaiveDate,
    is_today: bool,
) -> AppResult<String> {
    let t = crate::api::reports::totals_for(conn, branch_id, day, day).await?;
    let staff = crate::api::payroll::staff_pay(conn, branch_id, day, day).await?;
    let money = money_now(conn, branch_id).await?;
    let revenue = t.goods_tyiyn.saturating_add(t.services_tyiyn);
    let mut minus = vec![format!("  − зарплата: {}", format_som(t.payroll_tyiyn))];
    if t.bank_fee_tyiyn != 0 {
        minus.push(format!(
            "  − комиссия банка: {}",
            format_som(t.bank_fee_tyiyn)
        ));
    }
    if t.expenses_tyiyn != 0 {
        minus.push(format!("  − расходы: {}", format_som(t.expenses_tyiyn)));
    }
    if t.bonus_tyiyn != 0 {
        minus.push(format!("  − скидки баллами: {}", format_som(t.bonus_tyiyn)));
    }
    // Как в отчёте о прибыли: недостача — минус, излишек — плюс (ADR-065).
    if t.cash_diff_tyiyn != 0 {
        minus.push(format!(
            "  ± недостача / излишек кассы: {}",
            format_som_signed(t.cash_diff_tyiyn)
        ));
    }
    // Брак (SPEC-25): списание — минус, разница возмещения — со своим знаком.
    if t.defect_writeoff_tyiyn != 0 {
        minus.push(format!(
            "  − списание брака: {}",
            format_som(t.defect_writeoff_tyiyn)
        ));
    }
    if t.defect_diff_tyiyn != 0 {
        minus.push(format!(
            "  ± брак: разница возмещения: {}",
            format_som_signed(t.defect_diff_tyiyn)
        ));
    }
    let (head, earned) = if is_today {
        ("📊 Сегодня", "👥 Заработали сегодня")
    } else {
        ("📊 Итог дня", "👥 Заработали за день")
    };
    Ok(format!(
        "{head}, {}\n\n🧾 Продано: {} на {}\n  товары {} · работы {}\n\n💰 Прибыль\n  Выручка: {}\n  − закупка проданного: {}\n  = валовая: {}\n{}\n  = ЧИСТАЯ ПРИБЫЛЬ: {}\n\n{earned}\n{}\n\n💵 Деньги сейчас\n{money}",
        day.format("%d.%m.%Y"),
        checks_word(t.sales_count),
        format_som(revenue),
        format_som(t.goods_tyiyn),
        format_som(t.services_tyiyn),
        format_som(revenue),
        format_som(t.cost_tyiyn),
        format_som(t.gross_tyiyn),
        minus.join("\n"),
        format_som(t.net_tyiyn),
        staff_lines(&staff),
    ))
}

/// Деньги сейчас по кассам: «Касса», «Счёт», «Сейф владельца» — строка на кассу.
pub(crate) async fn money_now(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    crate::api::cash::ensure_accounts(conn, branch_id).await?;
    let accounts = sqlx::query!(
        r#"select name, balance_tyiyn from cash_accounts where branch_id = $1 and active
           order by is_default desc, kind, name"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(accounts
        .iter()
        .map(|a| format!("  {}: {}", a.name, format_som(a.balance_tyiyn)))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// «1 чек», «3 чека», «12 чеков».
fn checks_word(n: i64) -> String {
    let word = match (n % 10, n % 100) {
        (1, x) if x != 11 => "чек",
        (2..=4, x) if !(12..=14).contains(&x) => "чека",
        _ => "чеков",
    };
    format!("{n} {word}")
}

pub(crate) async fn shift(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let till = crate::api::cash::default_account(conn, branch_id).await?;
    let Some(id) = crate::api::cash::open_shift_id(conn, branch_id, till).await? else {
        return Ok("💵 Смена не открыта.".into());
    };
    let s = crate::api::cash::load_shift(conn, branch_id, id).await?;
    // Наличные в кассе: с чего начали и что двигало деньги.
    let mut cash = vec![format!(
        "  на начало (размен): {}",
        format_som(s.opening_expected_tyiyn)
    )];
    if let Some(rows) = s.breakdown.as_array() {
        for r in rows {
            let kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
            let sum = r.get("sum_tyiyn").and_then(Value::as_i64).unwrap_or(0);
            if sum == 0 {
                continue;
            }
            let what = match kind {
                "sale" => "продажи наличными",
                "sale_return" => "возвраты",
                "cash_in" => "внесли",
                "cash_out" => "изъяли",
                "transfer_in" => "переведено в кассу",
                "transfer_out" => "переведено из кассы",
                "expense" => "расходы из кассы",
                "payout" => "выплаты сотрудникам",
                "supplier_payment" => "оплата поставщикам",
                "supplier_refund" => "возмещения поставщиков за брак",
                "debt_repayment" => "погашения долгов",
                "battery_intake" => "приём аккумуляторов",
                "count_diff" => "пересчёт",
                "reversal" => "сторно",
                other => other,
            };
            let sign = if sum > 0 { "+" } else { "−" };
            cash.push(format!("  {sign} {what}: {}", format_som(sum.abs())));
        }
    }
    let mut pays = vec![
        format!("  наличными: {}", format_som(s.cash_sales_tyiyn)),
        format!("  картой: {}", format_som(s.card_tyiyn)),
        format!("  QR: {}", format_som(s.transfer_tyiyn)),
    ];
    if s.debt_tyiyn != 0 {
        pays.push(format!("  в долг: {}", format_som(s.debt_tyiyn)));
    }
    if s.bonus_tyiyn != 0 {
        pays.push(format!("  баллами: {}", format_som(s.bonus_tyiyn)));
    }
    if s.battery_tyiyn != 0 {
        pays.push(format!("  аккумуляторами: {}", format_som(s.battery_tyiyn)));
    }
    if s.bank_fee_tyiyn != 0 {
        pays.push(format!(
            "  (комиссия банка {})",
            format_som(s.bank_fee_tyiyn)
        ));
    }
    let returns = if s.returns_tyiyn != 0 {
        format!(", возвраты {}", format_som(s.returns_tyiyn))
    } else {
        String::new()
    };
    Ok(format!(
        "💵 Смена № {} с {}, кассир {}\n\n💵 В кассе должно быть: {}\n{}\n\n🧾 Продано: {} на {}{returns}\n{}\n\n👥 Заработали\n{}",
        s.number,
        // Бишкек — всегда UTC+6, без перехода на летнее время.
        (s.opened_at + chrono::Duration::hours(6)).format("%d.%m %H:%M"),
        s.cashier_name,
        format_som(s.expected_tyiyn),
        cash.join("\n"),
        checks_word(s.sales_count),
        format_som(s.sales_total_tyiyn),
        pays.join("\n"),
        staff_lines(&s.pay),
    ))
}

/// Долги, как на экране «Долги» и в «Нам должны» сводки: все контрагенты филиала с ненулевым
/// балансом. Первый список — кто должен нам (по убыванию), второй — кому должны мы.
pub(crate) async fn debt_lists(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<(Vec<PartyOut>, Vec<PartyOut>)> {
    let rows = search_parties(
        conn,
        branch_id,
        ListQuery {
            only_debtors: true,
            ..Default::default()
        },
    )
    .await?;
    let (mut to_us, mut from_us): (Vec<PartyOut>, Vec<PartyOut>) = rows
        .into_iter()
        .filter(|p| p.balance_tyiyn != 0)
        .partition(|p| p.balance_tyiyn > 0);
    to_us.sort_by_key(|p| std::cmp::Reverse(p.balance_tyiyn));
    from_us.sort_by_key(|p| p.balance_tyiyn);
    Ok((to_us, from_us))
}

/// Строк в каждом списке «📒 Долги».
const DEBT_ROWS: usize = 15;

async fn debts(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let (to_us, from_us) = debt_lists(conn, branch_id).await?;
    let total = |list: &[PartyOut]| {
        list.iter().fold(0i64, |acc, p| {
            acc.saturating_add(p.balance_tyiyn.saturating_abs())
        })
    };
    let section = |list: &[PartyOut]| {
        let mut lines: Vec<String> = list
            .iter()
            .take(DEBT_ROWS)
            .map(|p| {
                format!(
                    "  {} — {}",
                    p.name,
                    format_som(p.balance_tyiyn.saturating_abs())
                )
            })
            .collect();
        if list.len() > DEBT_ROWS {
            lines.push(format!("  … и ещё {}", list.len() - DEBT_ROWS));
        }
        lines.join("\n")
    };
    let mut out = if to_us.is_empty() {
        "Нам никто не должен.".to_string()
    } else {
        format!(
            "Должны нам всего: {}\n{}",
            format_som(total(&to_us)),
            section(&to_us)
        )
    };
    if from_us.is_empty() {
        out.push_str("\n\nМы никому не должны.");
    } else {
        out.push_str(&format!(
            "\n\nМы должны всего: {}\n{}",
            format_som(total(&from_us)),
            section(&from_us)
        ));
    }
    Ok(out)
}

// ---------- События журнала в чат владельца ----------

/// Сообщение о событии журнала в чат владельца.
pub struct Outgoing {
    pub audit_id: Uuid,
    pub at: DateTime<Utc>,
    pub chat_id: i64,
    pub text: String,
}

/// Событий за один проход.
const FEED_BATCH: i64 = 100;

async fn state_time(conn: &mut PgConnection, key: &str) -> AppResult<Option<DateTime<Utc>>> {
    Ok(
        sqlx::query_scalar!("select value from bot_state where key = $1", key)
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|v| v.as_str().map(str::to_string))
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
    )
}

async fn set_state(conn: &mut PgConnection, key: &str, value: Value) -> AppResult<()> {
    sqlx::query!(
        r#"insert into bot_state (key, value) values ($1, $2)
           on conflict (key) do update set value = excluded.value"#,
        key,
        value
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn watched_actions() -> Vec<String> {
    // Владельцу в бот — всё: важные события экрана «Уведомления» и вся текущая работа точки.
    WATCHED
        .iter()
        .chain(FEED.iter())
        .map(|s| (*s).to_string())
        .collect()
}

/// Нижняя граница поиска и начало рассылки. Первый запуск начинает с «сейчас»: прошлое не
/// присылаем пачкой. `None` — это и был первый запуск.
async fn feed_bounds(conn: &mut PgConnection) -> AppResult<Option<(DateTime<Utc>, DateTime<Utc>)>> {
    let now = sqlx::query_scalar!(r#"select now() as "n!""#)
        .fetch_one(&mut *conn)
        .await?;
    let Some(cursor) = state_time(conn, "cursor").await? else {
        set_state(conn, "cursor", json!(now.to_rfc3339())).await?;
        set_state(conn, "feed_since", json!(now.to_rfc3339())).await?;
        return Ok(None);
    };
    // Курсор от прежней версии бота: всё до него уже ушло.
    let since = match state_time(conn, "feed_since").await? {
        Some(s) => s,
        None => {
            set_state(conn, "feed_since", json!(cursor.to_rfc3339())).await?;
            cursor
        }
    };
    Ok(Some((cursor, since)))
}

/// Ещё не отправленные события журнала для чатов владельцев (ADR-063). Ищутся после курсора с
/// запасом 15 минут назад — запись журнала получает время начала своей транзакции и может стать
/// видна позже более новых, — но не раньше суток; что уже ушло в чат, отмечено в
/// `telegram_sent`. Только чаты активных владельцев.
pub async fn pending(conn: &mut PgConnection) -> AppResult<Vec<Outgoing>> {
    let Some((cursor, since)) = feed_bounds(conn).await? else {
        return Ok(Vec::new());
    };
    let watched = watched_actions();
    let rows = sqlx::query!(
        r#"select a.id, a.action, a.data, a.created_at, a.entity_id, a.branch_id,
                  w.full_name as "who?", t.chat_id,
                  e.full_name as "employee?"
           from audit_log a
           join telegram_links t on t.branch_id = a.branch_id
           join users o on o.id = t.user_id and o.active and o.role = 'owner'
           left join users w on w.id = a.user_id
           left join employees e on e.id = a.entity_id
           where a.created_at > greatest($1::timestamptz - interval '15 minutes', $2::timestamptz,
                                         now() - interval '1 day')
             and a.created_at >= t.linked_at
             and a.action = any($3)
             and not exists (select 1 from telegram_sent s where s.chat_id = t.chat_id and s.audit_id = a.id)
           order by a.created_at, a.id, t.chat_id
           limit $4"#,
        cursor,
        since,
        &watched,
        FEED_BATCH
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut texts: HashMap<Uuid, String> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let text = match texts.get(&r.id) {
            Some(t) => t.clone(),
            None => {
                let ev = Event {
                    action: &r.action,
                    data: &r.data,
                    entity_id: r.entity_id,
                    employee: r.employee.as_deref(),
                };
                let (title, details) = event_text(conn, &ev).await?;
                let who = r.who.map(|w| format!("\n— {w}")).unwrap_or_default();
                let mut text = if details.is_empty() {
                    format!("{title}{who}")
                } else {
                    format!("{title}\n{details}{who}")
                };
                // Смену закрыли — следом итог её учётного дня, чтобы не нажимать «Сегодня».
                if r.action == "shift.close"
                    && let Some(id) = r.entity_id
                    && let Some(day) =
                        sqlx::query_scalar!("select business_date from shifts where id = $1", id)
                            .fetch_optional(&mut *conn)
                            .await?
                {
                    text = format!(
                        "{text}\n\n{}",
                        day_report(conn, r.branch_id, day, false).await?
                    );
                }
                texts.insert(r.id, text.clone());
                text
            }
        };
        out.push(Outgoing {
            audit_id: r.id,
            at: r.created_at,
            chat_id: r.chat_id,
            text,
        });
    }
    Ok(out)
}

/// Событие ушло в чат (или Телеграм его никогда не примет — бот заблокирован).
pub async fn mark_sent(conn: &mut PgConnection, m: &Outgoing) -> AppResult<()> {
    sqlx::query!(
        "insert into telegram_sent (chat_id, audit_id) values ($1, $2) on conflict do nothing",
        m.chat_id,
        m.audit_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Курсор — до самого старого ещё не доставленного события (во все чаты); всё доставлено —
/// до «сейчас». Отметки старше двух дней больше не нужны: окно поиска не длиннее суток.
pub async fn advance_cursor(conn: &mut PgConnection) -> AppResult<()> {
    let Some((cursor, since)) = feed_bounds(conn).await? else {
        return Ok(());
    };
    let watched = watched_actions();
    let low = sqlx::query_scalar!(
        r#"select coalesce(min(a.created_at), now()) as "low!"
           from audit_log a
           join telegram_links t on t.branch_id = a.branch_id
           join users o on o.id = t.user_id and o.active and o.role = 'owner'
           where a.created_at > greatest($1::timestamptz - interval '15 minutes', $2::timestamptz,
                                         now() - interval '1 day')
             and a.created_at >= t.linked_at
             and a.action = any($3)
             and not exists (select 1 from telegram_sent s where s.chat_id = t.chat_id and s.audit_id = a.id)"#,
        cursor,
        since,
        &watched
    )
    .fetch_one(&mut *conn)
    .await?;
    set_state(conn, "cursor", json!(low.to_rfc3339())).await?;
    sqlx::query!("delete from telegram_sent where sent_at < now() - interval '2 days'")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Текущая работа точки, которая идёт владельцу в бот сверх экрана «Уведомления».
const FEED: [&str; 17] = [
    "sale.post",
    "shift.open",
    "shift.close",
    "receipt.post",
    "expense.post",
    "expense.reverse",
    "debt.repayment",
    "payout.post",
    "payroll.salary",
    "payroll.accrual",
    "cash.movement",
    "cash.transfer",
    "battery.intake",
    "oil.transfer",
    "product.merge",
    "product.create",
    "revision.cancel",
];

fn money_of(data: &Value, key: &str) -> String {
    format_som(data.get(key).and_then(Value::as_i64).unwrap_or(0))
}

/// Строка журнала для текста сообщения.
struct Event<'a> {
    action: &'a str,
    data: &'a Value,
    entity_id: Option<Uuid>,
    employee: Option<&'a str>,
}

async fn account_name(conn: &mut PgConnection, id: Option<Uuid>) -> AppResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_scalar!("select name from cash_accounts where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

async fn party_of(
    conn: &mut PgConnection,
    id: Option<Uuid>,
) -> AppResult<Option<(String, String)>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query!("select name, role from parties where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?
            .map(|p| (p.name, p.role)),
    )
}

async fn shift_number(conn: &mut PgConnection, id: Option<Uuid>) -> AppResult<i64> {
    let Some(id) = id else { return Ok(0) };
    Ok(
        sqlx::query_scalar!("select number from shifts where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or(0),
    )
}

/// Текст события для владельца: заголовок и подробности. Названия касс, клиентов и
/// сотрудников берутся из учёта по номеру записи.
async fn event_text(conn: &mut PgConnection, ev: &Event<'_>) -> AppResult<(String, String)> {
    let data = ev.data;
    let num = |k: &str| data.get(k).and_then(Value::as_i64).unwrap_or(0);
    let txt = |k: &str| {
        data.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let emp = ev.employee.unwrap_or("сотрудник");
    Ok(match ev.action {
        "sale.post" => match ev.entity_id {
            Some(id) => sale_text(conn, id).await?,
            None => describe(ev.action, data),
        },
        "shift.close" => {
            let diff = num("diff");
            let result = match diff {
                0 => "сошлось".to_string(),
                d if d < 0 => format!("недостача {}", format_som(-d)),
                d => format!("излишек {}", format_som(d)),
            };
            (
                format!(
                    "🔴 Смена № {} закрыта",
                    shift_number(conn, ev.entity_id).await?
                ),
                format!(
                    "Должно быть {}, посчитали {} — {result}",
                    money_of(data, "expected"),
                    money_of(data, "counted")
                ),
            )
        }
        // О закрытии уже было сообщение «🔴 Смена № … закрыта» — здесь только сдача кассы.
        "shift.handover" => (
            format!("📥 Касса смены № {} сдана", num("number")),
            format!(
                "в сейф {}, в кассе осталось {}",
                money_of(data, "to_safe"),
                money_of(data, "left")
            ),
        ),
        "shift.reopen" => (
            format!(
                "🔓 Смена № {} переоткрыта",
                shift_number(conn, ev.entity_id).await?
            ),
            format!("причина: {}", txt("reason")),
        ),
        "expense.post" => {
            let source = match ev.entity_id {
                Some(id) => sqlx::query!(
                    r#"select e.source, a.name as "account?" from expenses e
                       left join cash_accounts a on a.id = e.account_id where e.id = $1"#,
                    id
                )
                .fetch_optional(&mut *conn)
                .await?
                .map(|r| match (r.source.as_str(), r.account) {
                    ("outside", _) => "из своих, мимо касс".to_string(),
                    (_, Some(name)) => format!("из «{name}»"),
                    _ => "из кассы".to_string(),
                }),
                None => None,
            }
            .unwrap_or_else(|| {
                // «from» — с SPEC-24 («source» в журнале — источник: программа или бот).
                if txt("from") == "outside" || txt("source") == "outside" {
                    "из своих, мимо касс".into()
                } else {
                    "из кассы".into()
                }
            });
            (
                format!("💸 Расход: {}", txt("article")),
                format!("{} · {source}", money_of(data, "amount_tyiyn")),
            )
        }
        "debt.repayment" => repayment_text(conn, ev).await?,
        "debt.reverse" => {
            let name = party_of(conn, ev.entity_id)
                .await?
                .map_or_else(|| "контрагент".to_string(), |p| p.0);
            (
                format!("↩️ Сторно записи долга: {name}"),
                format!(
                    "{}{}",
                    money_of(data, "amount_tyiyn"),
                    comment_tail(&txt("comment"))
                ),
            )
        }
        "cash.movement" => {
            let acc = account_name(conn, ev.entity_id)
                .await?
                .unwrap_or_else(|| "кассу".into());
            (
                if txt("kind") == "cash_in" {
                    format!("➕ Внесли в «{acc}»")
                } else {
                    format!("➖ Изъяли из «{acc}»")
                },
                format!(
                    "{}{}",
                    format_som(num("amount_tyiyn").saturating_abs()),
                    comment_tail(&txt("comment"))
                ),
            )
        }
        "cash.transfer" => {
            let route = match ev.entity_id {
                Some(id) => sqlx::query!(
                    r#"select f.name as from_name, t.name as to_name from cash_transfers x
                       join cash_accounts f on f.id = x.from_account_id
                       join cash_accounts t on t.id = x.to_account_id
                       where x.id = $1"#,
                    id
                )
                .fetch_optional(&mut *conn)
                .await?
                .map(|r| format!(": «{}» → «{}»", r.from_name, r.to_name)),
                None => None,
            }
            .unwrap_or_default();
            (
                format!("🔁 Перевод денег № {}{route}", num("number")),
                money_of(data, "amount_tyiyn"),
            )
        }
        "sale.credit_limit_exceeded" => {
            let party = data
                .get("party_id")
                .and_then(Value::as_str)
                .and_then(|s| Uuid::parse_str(s).ok());
            let (title, details) = describe(ev.action, data);
            match party_of(conn, party).await? {
                Some((name, _)) => (format!("{title}: {name}"), details),
                None => (title, details),
            }
        }
        "revision.cancel" => {
            let n = match ev.entity_id {
                Some(id) => sqlx::query_scalar!("select number from revisions where id = $1", id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .unwrap_or(0),
                None => 0,
            };
            (format!("🗑 Отменена ревизия № {n}"), String::new())
        }
        "payout.reverse" => (
            format!("↩️ Сторно выплаты: {emp}"),
            format!(
                "{}{}",
                money_of(data, "amount_tyiyn"),
                comment_tail(&txt("comment"))
            ),
        ),
        "payroll.accrual_reverse" => (
            format!("↩️ Сторно начисления: {emp}"),
            format!(
                "{} — {}{}",
                pay_kind(&txt("kind"), 0),
                format_som(num("amount_tyiyn").saturating_abs()),
                comment_tail(&txt("comment"))
            ),
        ),
        "payroll.rule" | "payroll.rule_close" => {
            let (title, details) = describe(ev.action, data);
            (format!("{title}: {emp}"), details)
        }
        a if FEED.contains(&a) => feed_text(a, data, ev.employee),
        _ => describe(ev.action, data),
    })
}

fn comment_tail(c: &str) -> String {
    if c.trim().is_empty() {
        String::new()
    } else {
        format!(" · {}", c.trim())
    }
}

/// Погашение долга: кто платил, кому, чем и куда легли деньги.
async fn repayment_text(conn: &mut PgConnection, ev: &Event<'_>) -> AppResult<(String, String)> {
    let data = ev.data;
    let method = data.get("method").and_then(Value::as_str).unwrap_or("cash");
    let (name, role) = party_of(conn, ev.entity_id)
        .await?
        .unwrap_or_else(|| ("контрагент".into(), "customer".into()));
    let supplier = role == "supplier";
    let account =
        match data
            .get("repayment_id")
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            Some(rid) => sqlx::query_scalar!(
                r#"select a.name from cash_movements m join cash_accounts a on a.id = m.account_id
               where m.doc_type = 'repayment' and m.doc_id = $1 and m.kind <> 'bank_fee'
               order by m.created_at limit 1"#,
                rid
            )
            .fetch_optional(&mut *conn)
            .await?,
            None => None,
        };
    let how = match method {
        "card" => "картой",
        "transfer" => "переводом",
        "outside" => "мимо касс",
        _ => "наличными",
    };
    let place = match (method, account) {
        ("outside", _) if supplier => " (из своих)".to_string(),
        ("outside", _) => String::new(),
        (_, Some(a)) if supplier => format!(" из «{a}»"),
        (_, Some(a)) => format!(" в «{a}»"),
        _ => String::new(),
    };
    let amount = money_of(data, "amount_tyiyn");
    let balance = data
        .get("balance_tyiyn")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if supplier {
        // У поставщика долг наш — баланс отрицательный.
        let rest = match balance {
            b if b < 0 => format!("ещё должны {}", format_som(-b)),
            0 => "долга нет".to_string(),
            b => format!("переплата {}", format_som(b)),
        };
        Ok((
            format!("💸 Оплатили поставщику {name}: {amount}"),
            format!("{how}{place} · {rest}"),
        ))
    } else {
        let rest = match balance {
            b if b > 0 => format!("осталось {}", format_som(b)),
            0 => "долг закрыт".to_string(),
            b => format!("аванс клиента {}", format_som(-b)),
        };
        Ok((
            format!("💰 Клиент {name} погасил {amount}"),
            format!("{how}{place} · {rest}"),
        ))
    }
}

fn feed_text(action: &str, data: &Value, employee: Option<&str>) -> (String, String) {
    let num = |k: &str| data.get(k).and_then(Value::as_i64).unwrap_or(0);
    let txt = |k: &str| {
        data.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let emp = employee.unwrap_or("сотрудник");
    match action {
        "shift.open" => (
            format!("🟢 Смена № {} открыта", num("number")),
            format!(
                "Размен в кассе: {} (по учёту {})",
                money_of(data, "counted"),
                money_of(data, "expected")
            ),
        ),
        "receipt.post" => (
            format!("📦 Приход № {}", num("number")),
            format!("Товара на {}", money_of(data, "total")),
        ),
        "expense.reverse" => ("↩️ Расход отменён".into(), money_of(data, "amount_tyiyn")),
        "payout.post" => (
            format!("👛 Выплата: {emp}"),
            format!(
                "{}{}",
                money_of(data, "amount_tyiyn"),
                if data.get("advance").and_then(Value::as_bool) == Some(true) {
                    " · аванс сверх заработанного"
                } else {
                    ""
                }
            ),
        ),
        "payroll.salary" => (
            format!("🗓 Оклад: {emp}"),
            format!("{} за {}", money_of(data, "amount_tyiyn"), txt("month")),
        ),
        "payroll.accrual" => (
            format!(
                "{}: {emp}",
                if txt("kind") == "bonus" {
                    "🎁 Премия"
                } else {
                    "➖ Удержание"
                }
            ),
            format!("{} · {}", money_of(data, "amount_tyiyn"), txt("comment")),
        ),
        "battery.intake" => (
            format!("🔋 Приём аккумуляторов № {}", num("number")),
            format!(
                "{},{:03} кг · выдали {}",
                num("grams") / 1000,
                num("grams") % 1000,
                money_of(data, "amount")
            ),
        ),
        "oil.transfer" => (
            format!("🛢 Перелив № {}", num("number")),
            format!(
                "{} → {}, {} мл",
                txt("from_name"),
                txt("to_name"),
                num("qty_ml")
            ),
        ),
        "product.merge" => (
            "🗂 Объединены карточки товара".into(),
            format!("«{}» влита в «{}»", txt("from_name"), txt("into_name")),
        ),
        "product.create" => (format!("🆕 Новый товар: {}", txt("name")), String::new()),
        _ => describe(action, data),
    }
}

/// Чек для владельца: номер, сумма, как платили, кто продал, клиент и машина.
pub async fn sale_text(conn: &mut PgConnection, id: Uuid) -> AppResult<(String, String)> {
    let s = sqlx::query!(
        r#"select s.number, s.total_tyiyn, s.sale_type, c.full_name as cashier,
                  m.full_name as "master?", p.name as "party?", v.plate as "plate?",
                  (select count(*) from sale_lines l where l.sale_id = s.id) as "lines!"
           from sales s
           join employees c on c.id = s.cashier_id
           left join employees m on m.id = s.master_id
           left join parties p on p.id = s.party_id
           left join party_vehicles v on v.id = s.vehicle_id
           where s.id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let pays = sqlx::query!(
        "select method, amount_tyiyn, grams from sale_payments where sale_id = $1 order by amount_tyiyn desc",
        id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|p| {
        let how = match p.method.as_str() {
            "cash" => "наличными",
            "card" => "картой",
            "transfer" => "QR",
            "debt" => "в долг",
            "bonus" => "баллами",
            // Аккумулятор в оплату: «аккумулятор 12 кг — 600 с» (SPEC-27).
            "battery" => {
                return format!(
                    "аккумулятор {} — {}",
                    crate::domain::money::format_kg(p.grams.unwrap_or(0)),
                    format_som(p.amount_tyiyn)
                );
            }
            other => other,
        };
        format!("{how} {}", format_som(p.amount_tyiyn))
    })
    .collect::<Vec<_>>()
    .join(", ");
    let mut details = vec![pays];
    let mut who = format!("кассир {}", s.cashier);
    if s.sale_type == "service" {
        who.push_str(&format!(
            ", в сервис, мастер {}",
            s.master.unwrap_or_default()
        ));
    }
    details.push(who);
    if let Some(p) = s.party {
        details.push(match s.plate {
            Some(plate) => format!("клиент {p}, {plate}"),
            None => format!("клиент {p}"),
        });
    }
    Ok((
        format!(
            "🧾 Чек № {} — {} ({} поз.)",
            s.number,
            format_som(s.total_tyiyn),
            s.lines
        ),
        details.join("\n"),
    ))
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
    callback_query: Option<CallbackQuery>,
}

/// Нажатие кнопки под сообщением.
#[derive(Deserialize)]
struct CallbackQuery {
    id: String,
    from: From,
    message: Option<CallbackMessage>,
    data: Option<String>,
}

#[derive(Deserialize)]
struct CallbackMessage {
    chat: Chat,
}

#[derive(Deserialize)]
struct Message {
    chat: Chat,
    from: Option<From>,
    text: Option<String>,
    contact: Option<Contact>,
}

#[derive(Deserialize)]
struct From {
    id: i64,
}

#[derive(Deserialize)]
struct Contact {
    phone_number: String,
    user_id: Option<i64>,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
    /// `private`, `group`, `supergroup`, `channel`.
    #[serde(rename = "type", default)]
    kind: String,
}

/// Что пришло от Телеграма и на что бот отвечает: только из личного чата (ADR-070). Группы
/// и каналы не привязываются, не получают команд владельца, вопросов, черновиков, разделов
/// клиента и работника — бот в них молчит.
pub enum Incoming {
    Message {
        chat_id: i64,
        from_id: i64,
        text: Option<String>,
        contact: Option<SharedContact>,
    },
    Callback {
        /// Номер нажатия — ответить Телеграму, что кнопка принята.
        id: String,
        chat_id: i64,
        from_id: i64,
        data: String,
    },
}

/// Разбор обновления Телеграма. `None` — не личный чат или нечего обрабатывать.
fn incoming(u: Update) -> Option<Incoming> {
    if let Some(q) = u.callback_query {
        let m = q.message?;
        if m.chat.kind != "private" {
            return None;
        }
        return Some(Incoming::Callback {
            id: q.id,
            chat_id: m.chat.id,
            from_id: q.from.id,
            data: q.data.unwrap_or_default(),
        });
    }
    let m = u.message?;
    if m.chat.kind != "private" {
        return None;
    }
    Some(Incoming::Message {
        chat_id: m.chat.id,
        from_id: m.from.as_ref().map_or(m.chat.id, |f| f.id),
        text: m.text,
        contact: m.contact.map(|c| SharedContact {
            phone: c.phone_number,
            user_id: c.user_id,
        }),
    })
}

/// Обновление Телеграма как оно пришло (JSON): `None` — ответа нет (не личный чат, нечего
/// обрабатывать). Тот же путь, что у цикла бота, только вопросы владельца — без очереди.
pub async fn handle_update(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    update: Value,
) -> AppResult<Option<Reply>> {
    let Ok(u) = serde_json::from_value::<Update>(update) else {
        return Ok(None);
    };
    let Some(inc) = incoming(u) else {
        return Ok(None);
    };
    handle_incoming(conn, assistant, inc).await.map(Some)
}

async fn handle_incoming(
    conn: &mut PgConnection,
    assistant: Option<&Assistant>,
    inc: Incoming,
) -> AppResult<Reply> {
    match inc {
        Incoming::Message {
            chat_id,
            from_id,
            text,
            contact,
        } => handle_message_with(conn, assistant, chat_id, from_id, text.as_deref(), contact).await,
        Incoming::Callback {
            chat_id,
            from_id,
            data,
            ..
        } => handle_callback_from(conn, assistant, chat_id, from_id, &data).await,
    }
}

/// Предел одного сообщения в единицах UTF-16, как считает Телеграм (у него 4096, с запасом).
pub const MAX_MESSAGE: usize = 4000;

/// Делит текст на части не длиннее `MAX_MESSAGE` по границам строк; строка длиннее предела
/// режется по знакам.
pub fn split_message(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for line in text.split('\n') {
        let len: usize = line.chars().map(char::len_utf16).sum();
        let extra = if cur.is_empty() { len } else { len + 1 };
        if cur_len + extra <= MAX_MESSAGE {
            if !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(line);
            cur_len += extra;
            continue;
        }
        if !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur_len = 0;
        for ch in line.chars() {
            let l = ch.len_utf16();
            if cur_len + l > MAX_MESSAGE {
                parts.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            cur.push(ch);
            cur_len += l;
        }
    }
    if !cur.is_empty() || parts.is_empty() {
        parts.push(cur);
    }
    parts
}

/// Чем кончилась отправка сообщения.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Телеграм не примет никогда (бот заблокирован, чат не найден, неверный запрос) —
    /// сообщение пропускается, повторять бессмысленно.
    Skip,
    /// Повторить позже: перегрузка (429), сбой Телеграма (5xx), нет связи, неверный ключ.
    /// `all` — остановить всю пачку (нет связи, 429, ключ), иначе — только этот чат.
    Retry {
        after: Duration,
        all: bool,
    },
}

/// Ответ Телеграма на отправку → что делать дальше.
pub fn classify(status: u16, body: &Value) -> Delivery {
    match status {
        200..=299 => Delivery::Sent,
        429 => {
            let secs = body
                .get("parameters")
                .and_then(|p| p.get("retry_after"))
                .and_then(Value::as_u64)
                .unwrap_or(5)
                .clamp(1, 3600);
            Delivery::Retry {
                after: Duration::from_secs(secs),
                all: true,
            }
        }
        400 | 403 => Delivery::Skip,
        s if s >= 500 => Delivery::Retry {
            after: Duration::from_secs(5),
            all: false,
        },
        // 401, 404 — неверный ключ бота: ждём исправления настройки, события не теряем.
        _ => Delivery::Retry {
            after: Duration::from_secs(60),
            all: true,
        },
    }
}

/// Очередь отправки: между любыми сообщениями не меньше `gap`, в один чат при рассылке — не чаще
/// раза в `chat_gap` (ограничения Телеграма).
struct Pace {
    last: Option<Instant>,
    chats: HashMap<i64, Instant>,
}

/// Подключение к Телеграму. Адрес содержит ключ бота: он не печатается и не пишется в журнал
/// (ошибки сети — без адреса).
#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    base: String,
    pace: std::sync::Arc<tokio::sync::Mutex<Pace>>,
    gap: Duration,
    chat_gap: Duration,
}

impl Api {
    /// `url` — адрес API Телеграма (`TELEGRAM_API_BASE`, по умолчанию https://api.telegram.org).
    pub fn new(url: &str, token: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: format!("{}/bot{token}", url.trim_end_matches('/')),
            pace: std::sync::Arc::new(tokio::sync::Mutex::new(Pace {
                last: None,
                chats: HashMap::new(),
            })),
            gap: Duration::from_millis(50),
            chat_gap: Duration::from_secs(1),
        }
    }

    /// Другие паузы между сообщениями (для тестов).
    pub fn with_pace(mut self, gap: Duration, chat_gap: Duration) -> Self {
        self.gap = gap;
        self.chat_gap = chat_gap;
        self
    }

    async fn wait_turn(&self, chat_id: i64, backlog: bool) {
        let wait = {
            let mut p = self.pace.lock().await;
            let now = Instant::now();
            let mut at = p.last.map_or(now, |l| (l + self.gap).max(now));
            if backlog && let Some(c) = p.chats.get(&chat_id) {
                at = at.max(*c + self.chat_gap);
            }
            p.last = Some(at);
            p.chats.insert(chat_id, at);
            if p.chats.len() > 1000 {
                p.chats
                    .retain(|_, t| now.saturating_duration_since(*t) < Duration::from_secs(60));
            }
            at.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Ответ в чат.
    pub async fn send(&self, chat_id: i64, text: &str) -> Delivery {
        self.deliver(chat_id, text, None, false).await
    }

    pub async fn send_with(&self, chat_id: i64, text: &str, markup: Option<Value>) -> Delivery {
        self.deliver(chat_id, text, markup, false).await
    }

    /// Рассылка (события, напоминания, баллы): в один чат не чаще раза в секунду.
    pub async fn push(&self, chat_id: i64, text: &str) -> Delivery {
        self.deliver(chat_id, text, None, true).await
    }

    async fn push_with(&self, chat_id: i64, text: &str, markup: Value) -> Delivery {
        self.deliver(chat_id, text, Some(markup), true).await
    }

    /// Длинный текст уходит частями по строкам; кнопки — под последней частью.
    async fn deliver(
        &self,
        chat_id: i64,
        text: &str,
        markup: Option<Value>,
        backlog: bool,
    ) -> Delivery {
        let parts = split_message(text);
        let last = parts.len().saturating_sub(1);
        for (i, part) in parts.iter().enumerate() {
            self.wait_turn(chat_id, backlog).await;
            let mut body = json!({ "chat_id": chat_id, "text": part });
            if i == last
                && let Some(m) = &markup
            {
                body["reply_markup"] = m.clone();
            }
            let r = self
                .http
                .post(format!("{}/sendMessage", self.base))
                .json(&body)
                .timeout(Duration::from_secs(20))
                .send()
                .await;
            let d = match r {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let v = resp.json::<Value>().await.unwrap_or(Value::Null);
                    let d = classify(status, &v);
                    if d != Delivery::Sent {
                        let why: String = v
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .chars()
                            .take(200)
                            .collect();
                        tracing::warn!(status, chat_id, описание = %why, "телеграм: сообщение не принято");
                    }
                    d
                }
                Err(e) => {
                    tracing::warn!(error = %e.without_url(), "телеграм: сообщение не отправлено");
                    Delivery::Retry {
                        after: Duration::from_secs(10),
                        all: true,
                    }
                }
            };
            if d != Delivery::Sent {
                return d;
            }
        }
        Delivery::Sent
    }

    /// Снять «часики» с нажатой кнопки; ответ идёт отдельным сообщением.
    async fn answer_callback(&self, id: &str) {
        let r = self
            .http
            .post(format!("{}/answerCallbackQuery", self.base))
            .json(&json!({ "callback_query_id": id }))
            .timeout(Duration::from_secs(20))
            .send()
            .await;
        if let Err(e) = r {
            tracing::warn!(error = %e.without_url(), "телеграм: нажатие не подтверждено");
        }
    }

    /// Новые сообщения боту. `Err` — сколько подождать перед следующей попыткой.
    async fn updates(&self, offset: i64) -> Result<Vec<Update>, Duration> {
        let sent = self
            .http
            .get(format!("{}/getUpdates", self.base))
            .query(&[("offset", offset.to_string()), ("timeout", "25".into())])
            .timeout(Duration::from_secs(35))
            .send()
            .await;
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e.without_url(), "телеграм: обновления не получены");
                return Err(Duration::from_secs(10));
            }
        };
        let status = resp.status().as_u16();
        if status == 409 {
            tracing::warn!(
                status,
                "телеграм: этим ключом бота уже пользуется другой экземпляр бота (второй сервер или вебхук) — остановите его"
            );
            return Err(Duration::from_secs(30));
        }
        if !(200..300).contains(&status) {
            tracing::warn!(status, "телеграм: обновления не получены");
            return Err(Duration::from_secs(if matches!(status, 401 | 404) {
                60
            } else {
                10
            }));
        }
        match resp.json::<TgResponse<Vec<Update>>>().await {
            Ok(r) if r.ok => Ok(r.result.unwrap_or_default()),
            Ok(_) => {
                tracing::warn!("телеграм: обновления не получены (ok = false)");
                Err(Duration::from_secs(10))
            }
            Err(e) => {
                tracing::warn!(error = %e.without_url(), "телеграм: непонятный ответ на getUpdates");
                Err(Duration::from_secs(10))
            }
        }
    }

    async fn username(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct Me {
            username: Option<String>,
        }
        let r = self
            .http
            .get(format!("{}/getMe", self.base))
            .timeout(Duration::from_secs(20))
            .send()
            .await
            .ok()?
            .json::<TgResponse<Me>>()
            .await
            .ok()?;
        r.result.and_then(|m| m.username)
    }
}

/// Имя бота запоминаем в базе: по нему касса рисует QR-код на бота.
async fn remember_username(pool: PgPool, api: Api) {
    loop {
        if let Some(name) = api.username().await {
            let saved = sqlx::query!(
                r#"insert into bot_state (key, value) values ('username', $1)
                   on conflict (key) do update set value = excluded.value"#,
                json!(name)
            )
            .execute(&pool)
            .await;
            if saved.is_ok() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

/// Запуск бота: если ключ задан — два фоновых цикла. Без интернета они ждут и пробуют снова.
/// `assistant` — вопросы владельца текстом (SPEC-21); `None` — бот ровно по SPEC-18.
pub fn spawn(pool: PgPool, token: String, assistant: Option<Assistant>) {
    let base =
        std::env::var("TELEGRAM_API_BASE").unwrap_or_else(|_| "https://api.telegram.org".into());
    let api = Api::new(&base, &token);
    tokio::spawn(remember_username(pool.clone(), api.clone()));
    tokio::spawn(commands_loop(pool.clone(), api.clone(), assistant));
    tokio::spawn(notify_loop(pool, api));
    tracing::info!("телеграм-бот запущен");
}

/// Смещение обновлений Телеграма: после перезапуска бот не получает уже обработанное.
pub async fn load_offset(conn: &mut PgConnection) -> AppResult<i64> {
    Ok(
        sqlx::query_scalar!("select value from bot_state where key = 'offset'")
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    )
}

pub async fn save_offset(conn: &mut PgConnection, offset: i64) -> AppResult<()> {
    set_state(conn, "offset", json!(offset)).await
}

async fn store_offset(pool: &PgPool, offset: i64) {
    let r = match pool.acquire().await {
        Ok(mut conn) => save_offset(&mut conn, offset).await,
        Err(e) => Err(AppError::from(e)),
    };
    if let Err(e) = r {
        tracing::warn!(error = %e, "телеграм: смещение обновлений не сохранено");
    }
}

/// Чат владельца в режиме вопросов. Ошибка базы — «нет»: сообщение обработается обычным путём.
async fn is_asking_chat(pool: &PgPool, chat_id: i64) -> bool {
    match pool.acquire().await {
        Ok(mut conn) => matches!(owner_asking(&mut conn, chat_id).await, Ok(true)),
        Err(_) => false,
    }
}

/// Ответ в чат и следом — черновик операции с кнопками, если он есть (SPEC-24). Черновик
/// уходит, только если ответ принят: иначе кнопки пришли бы без объяснения.
async fn send_reply(api: &Api, chat_id: i64, r: &Reply) {
    let sent = api.send_with(chat_id, &r.text, Some(r.markup())).await;
    if let Some(f) = &r.follow
        && sent == Delivery::Sent
    {
        api.send_with(chat_id, &f.text, Some(f.markup())).await;
    }
}

async fn commands_loop(pool: PgPool, api: Api, assistant: Option<Assistant>) {
    let owner_turn = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let mut offset = match pool.acquire().await {
        Ok(mut conn) => load_offset(&mut conn).await.unwrap_or(0),
        Err(_) => 0,
    };
    loop {
        let updates = match api.updates(offset).await {
            Ok(u) => u,
            Err(wait) => {
                tokio::time::sleep(wait).await;
                continue;
            }
        };
        let before = offset;
        for u in updates {
            offset = offset.max(u.update_id + 1);
            // Группы и каналы бот не слушает (ADR-070).
            let Some(inc) = incoming(u) else { continue };
            let chat_id = match &inc {
                Incoming::Message { chat_id, .. } | Incoming::Callback { chat_id, .. } => *chat_id,
            };
            if let Incoming::Callback { id, .. } = &inc {
                api.answer_callback(id).await;
            }
            // Вопрос владельца ждёт сервис разбора до 30 с — отдельной задачей, чтобы клиенты
            // и кнопки в это время не стояли. Вопрос — только свободный текст (или шесть цифр)
            // в режиме «💬 Спросить» (ADR-067, ADR-070). Вопросы идут по очереди (owner_turn);
            // кнопки и команды владельца очереди не ждут.
            let asking = match (&inc, assistant.as_ref()) {
                (Incoming::Message { text: Some(t), .. }, Some(_))
                    if is_question(t) || is_code(t.trim()) =>
                {
                    is_asking_chat(&pool, chat_id).await
                }
                _ => false,
            };
            if asking && let Some(a) = assistant.as_ref() {
                // Смещение — до запуска: после перезапуска вопрос не задаётся второй раз
                // (вопрос, который разбирался в момент остановки, теряется).
                store_offset(&pool, offset).await;
                tracing::info!(chat_id, "телеграм: вопрос владельца принят");
                let (pool, api, a, turn) =
                    (pool.clone(), api.clone(), a.clone(), owner_turn.clone());
                tokio::spawn(async move {
                    let _turn = turn.lock().await;
                    let reply = match pool.acquire().await {
                        Ok(mut conn) => handle_incoming(&mut conn, Some(&a), inc).await,
                        Err(e) => Err(AppError::from(e)),
                    };
                    match reply {
                        Ok(r) => send_reply(&api, chat_id, &r).await,
                        Err(e) => {
                            tracing::warn!(error = %e, "телеграм: вопрос владельца не обработан");
                            api.send(chat_id, "Не получилось, попробуйте позже.").await;
                        }
                    }
                });
                continue;
            }
            let reply = match pool.acquire().await {
                Ok(mut conn) => handle_incoming(&mut conn, assistant.as_ref(), inc).await,
                Err(e) => Err(AppError::from(e)),
            };
            match reply {
                Ok(r) => send_reply(&api, chat_id, &r).await,
                Err(e) => {
                    tracing::warn!(error = %e, "телеграм: сообщение не обработано");
                    api.send(chat_id, "Не получилось, попробуйте позже.").await;
                }
            }
        }
        if offset != before {
            store_offset(&pool, offset).await;
        }
    }
}

/// Итог прохода рассылки: сколько подождать, если Телеграм попросил (`None` — обычная пауза).
#[derive(Debug, Default)]
pub struct PassOutcome {
    pub pause: Option<Duration>,
    /// Остановлен весь проход (нет связи, 429).
    pub stopped: bool,
}

impl PassOutcome {
    fn retry(&mut self, after: Duration, all: bool) {
        self.pause = Some(self.pause.map_or(after, |p| p.max(after)));
        self.stopped |= all;
    }
}

/// События журнала владельцам: что ушло или никогда не уйдёт — отмечается по чату; чат с
/// временной ошибкой пропускается до следующего прохода, остальные получают своё.
pub async fn feed_pass(conn: &mut PgConnection, api: &Api) -> AppResult<PassOutcome> {
    let mut out = PassOutcome::default();
    let mut blocked: HashSet<i64> = HashSet::new();
    for m in pending(conn).await? {
        if blocked.contains(&m.chat_id) {
            continue;
        }
        match api.push(m.chat_id, &m.text).await {
            Delivery::Sent | Delivery::Skip => mark_sent(conn, &m).await?,
            Delivery::Retry { after, all } => {
                out.retry(after, all);
                if all {
                    break;
                }
                blocked.insert(m.chat_id);
            }
        }
    }
    advance_cursor(conn).await?;
    Ok(out)
}

/// Напоминания о замене (раз в час, только в окно 09:00–20:00 по Бишкеку).
pub async fn reminders_pass(conn: &mut PgConnection, api: &Api) -> AppResult<PassOutcome> {
    let mut out = PassOutcome::default();
    let mut blocked: HashSet<i64> = HashSet::new();
    for r in due_reminders(conn).await? {
        if blocked.contains(&r.chat_id) {
            continue;
        }
        match api.push(r.chat_id, &r.text).await {
            Delivery::Sent | Delivery::Skip => mark_reminded(conn, &r).await?,
            Delivery::Retry { after, all } => {
                out.retry(after, all);
                if all {
                    break;
                }
                blocked.insert(r.chat_id);
            }
        }
    }
    Ok(out)
}

/// Сообщения клиентам о баллах (окно 09:00–20:00 по Бишкеку).
pub async fn bonus_pass(conn: &mut PgConnection, api: &Api) -> AppResult<PassOutcome> {
    let mut out = PassOutcome::default();
    let mut blocked: HashSet<i64> = HashSet::new();
    for n in bonus_notices(conn).await? {
        if blocked.contains(&n.chat_id) {
            continue;
        }
        match api.push(n.chat_id, &n.text).await {
            Delivery::Sent | Delivery::Skip => mark_bonus_notice(conn, &n).await?,
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

/// Один проход рассылки: просроченные черновики владельца помечаются «истёк» (SPEC-24),
/// отвязка устаревших клиентов, напоминания (если `reminders` и окно), баллы (в окно), события
/// владельцам. `hour` — час по Бишкеку.
pub async fn notify_pass(
    conn: &mut PgConnection,
    api: &Api,
    hour: u32,
    reminders: bool,
) -> AppResult<PassOutcome> {
    let mut out = PassOutcome::default();
    crate::api::bot_drafts::expire_drafts(conn).await?;
    for chat in drop_stale_customers(conn).await? {
        if let Delivery::Retry { after, all: true } =
            api.push_with(chat, RESHARE, Keys::Contact.markup()).await
        {
            out.retry(after, true);
            return Ok(out);
        }
    }
    if in_send_window(hour) {
        if reminders {
            let r = reminders_pass(conn, api).await?;
            if r.stopped {
                return Ok(r);
            }
            out.pause = out.pause.max(r.pause);
        }
        let b = bonus_pass(conn, api).await?;
        if b.stopped {
            return Ok(b);
        }
        out.pause = out.pause.max(b.pause);
    }
    let f = feed_pass(conn, api).await?;
    out.pause = out.pause.max(f.pause);
    out.stopped = f.stopped;
    Ok(out)
}

async fn bishkek_hour(conn: &mut PgConnection) -> AppResult<u32> {
    let h = sqlx::query_scalar!(
        r#"select extract(hour from now() at time zone 'Asia/Bishkek')::int as "h!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(u32::try_from(h).unwrap_or(0))
}

async fn notify_loop(pool: PgPool, api: Api) {
    let mut last_reminders: Option<Instant> = None;
    loop {
        let mut wait = Duration::from_secs(10);
        if let Ok(mut conn) = pool.acquire().await {
            let due = last_reminders.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600));
            let pass = match bishkek_hour(&mut conn).await {
                Ok(hour) => notify_pass(&mut conn, &api, hour, due)
                    .await
                    .map(|o| (o, in_send_window(hour))),
                Err(e) => Err(e),
            };
            match pass {
                Ok((o, window)) => {
                    // Напоминания отмечаются пройденными только в окно и без остановки.
                    if due && window && !o.stopped {
                        last_reminders = Some(Instant::now());
                    }
                    if let Some(p) = o.pause {
                        wait = wait.max(p);
                    }
                }
                Err(e) => tracing::warn!(error = %e, "телеграм: рассылка не прошла"),
            }
        }
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_lines_and_limit() {
        let line = "строка ".repeat(50);
        let text = vec![line.as_str(); 40].join("\n");
        let parts = split_message(&text);
        assert!(parts.len() > 1);
        for p in &parts {
            assert!(p.chars().map(char::len_utf16).sum::<usize>() <= MAX_MESSAGE);
            assert!(!p.starts_with('\n'));
        }
        assert_eq!(parts.join("\n"), text);
        let long = "я".repeat(9000);
        let parts = split_message(&long);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts.concat(), long);
        assert_eq!(split_message("коротко"), vec!["коротко".to_string()]);
    }

    #[test]
    fn telegram_answers_are_classified() {
        assert_eq!(classify(200, &Value::Null), Delivery::Sent);
        assert_eq!(classify(403, &Value::Null), Delivery::Skip);
        assert_eq!(
            classify(
                400,
                &json!({ "description": "Bad Request: chat not found" })
            ),
            Delivery::Skip
        );
        assert_eq!(
            classify(429, &json!({ "parameters": { "retry_after": 7 } })),
            Delivery::Retry {
                after: Duration::from_secs(7),
                all: true
            }
        );
        assert!(matches!(
            classify(502, &Value::Null),
            Delivery::Retry { all: false, .. }
        ));
        assert!(matches!(
            classify(401, &Value::Null),
            Delivery::Retry { all: true, .. }
        ));
    }

    #[test]
    fn questions_are_only_free_text() {
        assert!(is_question("сколько заработали вчера?"));
        assert!(is_question("сегодня сколько масла продали"));
        assert!(!is_question("📊 Сегодня"));
        assert!(!is_question("💬 Спросить"));
        assert!(!is_question("✖ Закончить"));
        assert!(!is_question("долги"));
        assert!(!is_question("/смена"));
        assert!(!is_question("123456"));
        assert!(!is_question("🚗 Мои машины"));
        assert!(!is_question("Да, отключиться"));
        assert!(!is_question("👍"));
    }

    #[test]
    fn send_window_and_days() {
        assert!(!in_send_window(8));
        assert!(in_send_window(9));
        assert!(in_send_window(19));
        assert!(!in_send_window(20));
        assert_eq!(days_word(2), "2 дня");
        assert_eq!(days_word(5), "5 дней");
    }
}
