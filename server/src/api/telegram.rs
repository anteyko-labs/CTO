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

/// Какие кнопки показать под ответом.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keys {
    /// Не подключён: одна кнопка «Поделиться номером».
    Contact,
    Owner,
    Customer,
    ConfirmLeave,
}

impl Keys {
    fn markup(self) -> Value {
        let rows: Vec<Vec<Value>> = match self {
            Keys::Contact => vec![vec![
                json!({ "text": BTN_CONTACT, "request_contact": true }),
            ]],
            Keys::Owner => vec![vec![json!(BTN_TODAY), json!(BTN_SHIFT), json!(BTN_DEBTS)]],
            Keys::Customer => vec![
                vec![json!(BTN_CARS), json!(BTN_BONUS)],
                vec![json!(BTN_LEAVE)],
            ],
            Keys::ConfirmLeave => vec![vec![json!(BTN_LEAVE_YES), json!(BTN_BACK)]],
        };
        json!({ "keyboard": rows, "resize_keyboard": true, "is_persistent": true })
    }
}

/// Команда из текста: первое слово без значков и знаков — «📊 Сегодня» → «сегодня», «/долги» → «долги».
fn command(text: &str) -> String {
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

/// Ответ бота и кнопки под ним.
pub struct Reply {
    pub text: String,
    pub keys: Keys,
}

impl Reply {
    fn new(text: impl Into<String>, keys: Keys) -> Self {
        Self {
            text: text.into(),
            keys,
        }
    }
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

/// Сообщение в бот: владелец — команды и привязка кодом; клиент — своя масляная книжка.
pub async fn handle_message(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    text: Option<&str>,
    contact: Option<SharedContact>,
) -> AppResult<Reply> {
    if let Some(c) = contact {
        return link_customer(conn, chat_id, from_id, c).await;
    }
    let text = text.unwrap_or("");
    let owner = handle_text(conn, chat_id, text).await?;
    let customer = sqlx::query_scalar!(
        "select party_id from telegram_customers where chat_id = $1",
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(r) = owner {
        // Код привязки мог прислать и не владелец: кнопки — по тому, кто он сейчас.
        let keys = if chat_owner(conn, chat_id).await?.is_some() {
            Keys::Owner
        } else if customer.is_some() {
            Keys::Customer
        } else {
            Keys::Contact
        };
        return Ok(Reply::new(r, keys));
    }
    // Не владелец: подключившийся клиент видит свои машины, остальным — кнопка номера.
    let Some(party_id) = customer else {
        return Ok(Reply::new(
            "Здравствуйте! Это бот Avtodom.\n\nЗдесь вы увидите, когда меняли масло, какое залили, когда следующая замена, и свои бонусные баллы.\n\nНажмите кнопку «📱 Поделиться номером» внизу — нужен тот номер, что записан у нас на кассе.",
            Keys::Contact,
        ));
    };
    match command(text).as_str() {
        "отключиться" => Ok(Reply::new(
            "Отключиться от бота?\n\nНапоминаний о замене больше не будет и новые баллы не начислятся. Накопленные баллы сохранятся.",
            Keys::ConfirmLeave,
        )),
        "да" | "стоп" | "stop" => {
            sqlx::query!("delete from telegram_customers where chat_id = $1", chat_id)
                .execute(&mut *conn)
                .await?;
            Ok(Reply::new(
                "Готово, вы отключены. Накопленные баллы сохранились.\n\nЧтобы вернуться, нажмите «📱 Поделиться номером».",
                Keys::Contact,
            ))
        }
        "баллы" | "bonus" => Ok(Reply::new(
            bonus_history(conn, party_id).await?,
            Keys::Customer,
        )),
        _ => Ok(Reply::new(
            customer_book(conn, party_id).await?,
            Keys::Customer,
        )),
    }
}

async fn link_customer(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    c: SharedContact,
) -> AppResult<Reply> {
    // Только свой номер: чужую карточку контакта переслать можно, но книжку она не откроет.
    if c.user_id != Some(from_id) {
        return Ok(Reply::new(
            "Нужен ваш собственный номер: нажмите кнопку «📱 Поделиться номером» внизу.",
            Keys::Contact,
        ));
    }
    let Some(key) = phone_key(&c.phone) else {
        return Ok(Reply::new("Номер не распознан.", Keys::Contact));
    };
    let party = sqlx::query!(
        r#"select id, name from parties
           where role = 'customer' and active and length(regexp_replace(phone, '[^0-9]', '', 'g')) >= 9
             and right(regexp_replace(phone, '[^0-9]', '', 'g'), 9) = $1
           order by created_at desc limit 1"#,
        key
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(p) = party else {
        return Ok(Reply::new(
            "Этого номера нет среди наших клиентов. Назовите его кассиру при следующей покупке или замене — и потом снова нажмите «📱 Поделиться номером».",
            Keys::Contact,
        ));
    };
    sqlx::query!(
        r#"insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, $3)
           on conflict (chat_id) do update set party_id = excluded.party_id, phone = excluded.phone, linked_at = now()"#,
        chat_id,
        p.id,
        c.phone
    )
    .execute(&mut *conn)
    .await?;
    let book = customer_book(conn, p.id).await?;
    Ok(Reply::new(
        format!(
            "{}, вы подключены ✅\n\n• Напомним о замене масла за неделю и за день до срока.\n• С каждой покупки — бонусные баллы: 1 балл = 1 сом. Чтобы списать, назовите номер на кассе.\n\nКнопки внизу: «🚗 Мои машины», «⭐ Баллы».\n\n{book}",
            p.name
        ),
        Keys::Customer,
    ))
}

fn km(v: i32) -> String {
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
    let parts: Vec<String> = books
        .iter()
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
            if !last.oil_text.is_empty() {
                lines.push(format!("Масло: {}", last.oil_text));
            }
            if !last.filter_text.is_empty() {
                lines.push(format!("Фильтр: {}", last.filter_text));
            }
            let next = [
                b.next_km.map(|k| format!("на {}", km(k))),
                b.next_date.map(|d| format!("до {}", d.format("%d.%m.%Y"))),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" или ");
            if !next.is_empty() {
                lines.push(format!("Следующая замена: {next}"));
            }
            lines.join("\n")
        })
        .collect();
    Ok(format!("{bonus}\n\n{}", parts.join("\n\n")))
}

/// Напоминания клиентам: за 7 дней и за 1 день до срока замены, каждое один раз (ADR-051).
pub struct Reminder {
    pub chat_id: i64,
    pub vehicle_id: Uuid,
    pub due: chrono::NaiveDate,
    pub days_before: i32,
    pub text: String,
}

pub async fn due_reminders(conn: &mut PgConnection) -> AppResult<Vec<Reminder>> {
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let links = sqlx::query!("select chat_id, party_id from telegram_customers")
        .fetch_all(&mut *conn)
        .await?;
    let mut out = Vec::new();
    for l in links {
        let branch_id =
            sqlx::query_scalar!("select branch_id from parties where id = $1", l.party_id)
                .fetch_one(&mut *conn)
                .await?;
        for b in crate::api::oil_book::books(conn, branch_id, Some(l.party_id), None).await? {
            let Some(due) = b.next_date else { continue };
            let left = (due - today).num_days();
            let days_before = match left {
                7 => 7,
                1 => 1,
                _ => continue,
            };
            let sent = sqlx::query_scalar!(
                r#"select exists (select 1 from oil_reminders
                                  where vehicle_id = $1 and due_date = $2 and days_before = $3) as "e!""#,
                b.vehicle_id,
                due,
                days_before
            )
            .fetch_one(&mut *conn)
            .await?;
            if sent {
                continue;
            }
            let when = if days_before == 1 {
                "завтра".to_string()
            } else {
                format!("через {days_before} дней")
            };
            let km_part = b
                .next_km
                .map(|k| format!(" или на {}", km(k)))
                .unwrap_or_default();
            out.push(Reminder {
                chat_id: l.chat_id,
                vehicle_id: b.vehicle_id,
                due,
                days_before,
                text: format!(
                    "Напоминаем: замена масла для {} — {when}, до {}{km_part}. Ждём вас в Avtodom!",
                    b.plate,
                    due.format("%d.%m.%Y")
                ),
            });
        }
    }
    Ok(out)
}

pub async fn mark_reminded(conn: &mut PgConnection, r: &Reminder) -> AppResult<()> {
    sqlx::query!(
        r#"insert into oil_reminders (vehicle_id, due_date, days_before) values ($1, $2, $3)
           on conflict do nothing"#,
        r.vehicle_id,
        r.due,
        r.days_before
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Команды владельца и привязка кодом. `None` — чат не владельца и это не код: дальше решает клиентская часть.
pub async fn handle_text(
    conn: &mut PgConnection,
    chat_id: i64,
    text: &str,
) -> AppResult<Option<String>> {
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
        return link(conn, chat_id, code).await.map(Some);
    }
    let Some((_, branch_id)) = chat_owner(conn, chat_id).await? else {
        return Ok(None);
    };
    match command(text).as_str() {
        "сегодня" | "today" => today(conn, branch_id).await.map(Some),
        "смена" | "shift" => shift(conn, branch_id).await.map(Some),
        "долги" | "debts" => debts(conn, branch_id).await.map(Some),
        _ => Ok(Some(HELP.into())),
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

/// «Айгуль — 308,00 с (2 % с чеков 308,00 с)» по строкам начислений.
fn staff_lines(staff: &[crate::api::payroll::StaffPay]) -> String {
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
                    let what = match i.kind.as_str() {
                        "service_fee" => format!("замены ×{}", i.count),
                        "revenue_percent" => "% с чеков".into(),
                        "shift_fee" => "за смену".into(),
                        "monthly_salary" => "оклад".into(),
                        "bonus" => "премия".into(),
                        "penalty" => "удержание".into(),
                        "shortage" => "недостача".into(),
                        other => other.to_string(),
                    };
                    format!("{what} — {}", format_som(i.amount_tyiyn))
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

async fn today(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let day = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let t = crate::api::reports::totals_for(conn, branch_id, day, day).await?;
    let staff = crate::api::payroll::staff_pay(conn, branch_id, day, day).await?;
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
    Ok(format!(
        "📊 Сегодня, {}\n\n🧾 Продано: {} на {}\n  товары {} · работы {}\n\n💰 Прибыль\n  Выручка: {}\n  − закупка проданного: {}\n  = валовая: {}\n{}\n  = ЧИСТАЯ ПРИБЫЛЬ: {}\n\n👥 Заработали сегодня\n{}\n\n💵 Деньги сейчас\n{money}",
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

/// «1 чек», «3 чека», «12 чеков».
fn checks_word(n: i64) -> String {
    let word = match (n % 10, n % 100) {
        (1, x) if x != 11 => "чек",
        (2..=4, x) if !(12..=14).contains(&x) => "чека",
        _ => "чеков",
    };
    format!("{n} {word}")
}

async fn shift(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
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
    // Владельцу в бот — всё: важные события экрана «Уведомления» и вся текущая работа точки.
    let watched: Vec<String> = WATCHED
        .iter()
        .chain(FEED.iter())
        .map(|s| (*s).to_string())
        .collect();
    let rows = sqlx::query!(
        r#"select a.action, a.data, a.created_at, a.entity_id, a.branch_id,
                  u.full_name as "who?", t.chat_id,
                  e.full_name as "employee?"
           from audit_log a
           join telegram_links t on t.branch_id = a.branch_id
           left join users u on u.id = a.user_id
           left join employees e on e.id = a.entity_id
           where a.created_at > $1 and a.action = any($2)
           order by a.created_at
           limit 100"#,
        cursor,
        &watched
    )
    .fetch_all(&mut *conn)
    .await?;
    let next = rows.last().map(|r| r.created_at);
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let (title, details) = match r.action.as_str() {
            "sale.post" => match r.entity_id {
                Some(id) => sale_text(conn, id).await?,
                None => describe(&r.action, &r.data),
            },
            a if FEED.contains(&a) => feed_text(a, &r.data, r.employee.as_deref()),
            _ => describe(&r.action, &r.data),
        };
        let who = r.who.map(|w| format!("\n— {w}")).unwrap_or_default();
        let mut text = if details.is_empty() {
            format!("{title}{who}")
        } else {
            format!("{title}\n{details}{who}")
        };
        // Смену закрыли — следом итог дня, чтобы не нажимать «Сегодня».
        if r.action == "shift.close" {
            text = format!("{text}\n\n{}", today(conn, r.branch_id).await?);
        }
        out.push(Outgoing {
            at: r.created_at,
            chat_id: r.chat_id,
            text,
        });
    }
    Ok((out, next))
}

/// Текущая работа точки, которая идёт владельцу в бот сверх экрана «Уведомления».
const FEED: [&str; 15] = [
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
];

fn money_of(data: &Value, key: &str) -> String {
    format_som(data.get(key).and_then(Value::as_i64).unwrap_or(0))
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
        "shift.close" => {
            let diff = num("diff");
            let result = match diff {
                0 => "сошлось".to_string(),
                d if d < 0 => format!("недостача {}", format_som(-d)),
                d => format!("излишек {}", format_som(d)),
            };
            (
                "🔴 Смена закрыта".into(),
                format!(
                    "Должно быть {}, посчитали {} — {result}",
                    money_of(data, "expected"),
                    money_of(data, "counted")
                ),
            )
        }
        "receipt.post" => (
            format!("📦 Приход № {}", num("number")),
            format!("Товара на {}", money_of(data, "total")),
        ),
        "expense.post" => (
            format!("💸 Расход: {}", txt("article")),
            format!(
                "{}{}",
                money_of(data, "amount_tyiyn"),
                match txt("source").as_str() {
                    "outside" => " · не из денег точки",
                    "bank" => " · со счёта",
                    _ => " · из кассы",
                }
            ),
        ),
        "expense.reverse" => ("↩️ Расход отменён".into(), money_of(data, "amount_tyiyn")),
        "debt.repayment" => (
            "💰 Погашение долга".into(),
            format!(
                "Заплатили {} · осталось {}",
                money_of(data, "amount_tyiyn"),
                money_of(data, "balance_tyiyn")
            ),
        ),
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
        "cash.movement" => (
            if txt("kind") == "cash_in" {
                "➕ Внесли в кассу".into()
            } else {
                "➖ Изъяли из кассы".into()
            },
            format!("{} · {}", money_of(data, "amount_tyiyn"), txt("comment")),
        ),
        "cash.transfer" => (
            format!("🔁 Перевод денег № {}", num("number")),
            money_of(data, "amount_tyiyn"),
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
        "product.merge" => ("🗂 Объединены карточки товара".into(), String::new()),
        _ => describe(action, data),
    }
}

/// Чек для владельца: номер, сумма, как платили, кто продал, клиент и машина.
async fn sale_text(conn: &mut PgConnection, id: Uuid) -> AppResult<(String, String)> {
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
        "select method, amount_tyiyn from sale_payments where sale_id = $1 order by amount_tyiyn desc",
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
}

#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
}

impl Api {
    /// Отправка сообщения; `false` — Телеграм не принял (нет связи, ошибка ответа).
    async fn send(&self, chat_id: i64, text: &str) -> bool {
        self.send_with(chat_id, text, None).await
    }

    async fn send_with(&self, chat_id: i64, text: &str, markup: Option<Value>) -> bool {
        let mut body = json!({ "chat_id": chat_id, "text": text });
        if let Some(m) = markup {
            body["reply_markup"] = m;
        }
        let r = self
            .http
            .post(format!("{}/sendMessage", self.base))
            .json(&body)
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

    async fn username(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct Me {
            username: Option<String>,
        }
        let r = self
            .http
            .get(format!("{}/getMe", self.base))
            .timeout(std::time::Duration::from_secs(20))
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
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
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
    tokio::spawn(remember_username(pool.clone(), api.clone()));
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
            let from_id = m.from.as_ref().map_or(m.chat.id, |f| f.id);
            let contact = m.contact.map(|c| SharedContact {
                phone: c.phone_number,
                user_id: c.user_id,
            });
            let reply = match pool.acquire().await {
                Ok(mut conn) => {
                    handle_message(&mut conn, m.chat.id, from_id, m.text.as_deref(), contact).await
                }
                Err(e) => Err(AppError::from(e)),
            };
            match reply {
                Ok(r) => {
                    api.send_with(m.chat.id, &r.text, Some(r.keys.markup()))
                        .await;
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
    let mut last_reminders: Option<std::time::Instant> = None;
    loop {
        // Напоминания клиентам о замене — раз в час.
        if last_reminders.is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(3600))
            && let Ok(mut conn) = pool.acquire().await
        {
            match due_reminders(&mut conn).await {
                Ok(list) => {
                    for r in list {
                        if api.send(r.chat_id, &r.text).await
                            && let Err(e) = mark_reminded(&mut conn, &r).await
                        {
                            tracing::warn!(error = %e, "телеграм: напоминание не отмечено");
                        }
                    }
                    last_reminders = Some(std::time::Instant::now());
                }
                Err(e) => tracing::warn!(error = %e, "телеграм: напоминания не собраны"),
            }
        }
        if let Ok(mut conn) = pool.acquire().await {
            match bonus_notices(&mut conn).await {
                Ok(list) => {
                    for n in list {
                        if !api.send(n.chat_id, &n.text).await {
                            break;
                        }
                        if let Err(e) = mark_bonus_notice(&mut conn, &n).await {
                            tracing::warn!(error = %e, "телеграм: баллы не отмечены");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "телеграм: баллы не собраны"),
            }
        }
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
