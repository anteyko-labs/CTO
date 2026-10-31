//! Администратор в боте (SPEC-40, ADR-{h16.1}). Меню чата администратора строится по его правам
//! (те же права, что в программе): накладные, должники и старый долг, смена сейчас, остаток
//! товара, расход из кассы смены, масляная книжка по номеру. Пункт без права не показывается,
//! команда без права — «Нет права: …» (I12). Права читаются при каждом сообщении — меню меняется
//! со следующего.
//!
//! Денег владельца администратор в боте не видит (I13): данные берутся в виде администратора
//! (отчёт смены `owner = false`, товары по роли), тексты строятся здесь только из разрешённых
//! полей, и каждый ответ его чату проходит [`guard`] — строка со словами о прибыли,
//! себестоимости, марже и валовой вырезается. Лента событий администратору по умолчанию не идёт;
//! владелец включает ему отдельные события из [`EVENTS`] — только без денег владельца, тексты —
//! [`event_text`], свой филиал (I16).

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::bot_debtors::{BTN_ADD_DEBT, BTN_DEBTORS};
use crate::api::bot_drafts::{DraftOp, Prepared};
use crate::api::expenses::ExpenseReq;
use crate::api::shift_report::{ShiftReport, method_label};
use crate::api::telegram::{BTN_INVOICE, BTN_MY_INVOICES, Button, Keys, Outgoing, Reply};
use crate::auth::{Access, Ctx, CurrentUser, Owner, Role};
use crate::domain::money::{format_som, parse_spoken_som};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/users/{id}/bot", routing::get(get_bot))
        .route("/users/{id}/bot-feed", routing::put(put_feed))
}

// ---------- Меню ----------

pub const BTN_SHIFT_NOW: &str = "🧾 Смена сейчас";
pub const BTN_STOCK: &str = "📦 Остаток";
pub const BTN_EXPENSE: &str = "💸 Расход";
pub const BTN_BOOK: &str = "📒 Книжка";

/// Кнопки администратора, которые не ввод: отменяют ожидание поиска и шагов расхода.
pub const BUTTONS: [&str; 4] = [BTN_SHIFT_NOW, BTN_STOCK, BTN_EXPENSE, BTN_BOOK];

/// Пункт меню администратора: кнопка, команда, права (все сразу; пусто — всегда) и что он даёт.
pub struct Item {
    pub key: &'static str,
    pub button: &'static str,
    pub command: &'static str,
    pub rights: &'static [&'static str],
    pub what: &'static str,
}

/// Пункты меню по порядку кнопок внизу чата (по два в ряд).
pub const ITEMS: [Item; 8] = [
    Item {
        key: "invoice",
        button: BTN_INVOICE,
        command: "накладная",
        rights: &["receipts"],
        what: "как снять накладную; фото накладной в чат — черновик прихода",
    },
    Item {
        key: "my_invoices",
        button: BTN_MY_INVOICES,
        command: "мои",
        rights: &["receipts"],
        what: "последние 5 своих накладных, без сумм",
    },
    Item {
        key: "shift",
        button: BTN_SHIFT_NOW,
        command: "смена",
        rights: &[],
        what: "выручка смены по способам оплаты, чеки, касса «должно быть», расходы смены",
    },
    Item {
        key: "stock",
        button: BTN_STOCK,
        command: "остаток",
        rights: &["view_stock"],
        what: "остаток и цена продажи товара по названию или коду",
    },
    Item {
        key: "debtors",
        button: BTN_DEBTORS,
        command: "должники",
        rights: &["view_debts"],
        what: "должники филиала с просрочкой",
    },
    Item {
        key: "add_debt",
        button: BTN_ADD_DEBT,
        command: "занести",
        rights: &["view_debts", "old_debts"],
        what: "старый долг клиента из тетради по шагам",
    },
    Item {
        key: "expense",
        button: BTN_EXPENSE,
        command: "расход",
        rights: &["expenses"],
        what: "расход из кассы смены: черновик и «✅ Провести»",
    },
    Item {
        key: "book",
        button: BTN_BOOK,
        command: "книжка",
        rights: &["clients"],
        what: "масляная книжка машины по госномеру",
    },
];

/// Какие пункты [`ITEMS`] есть у администратора — по биту на пункт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Menu(u16);

impl Menu {
    /// Все пункты — для проверок.
    pub fn all() -> Self {
        Self((1u16 << ITEMS.len()) - 1)
    }

    pub fn has(self, key: &str) -> bool {
        ITEMS
            .iter()
            .position(|i| i.key == key)
            .is_some_and(|n| self.0 & (1 << n) != 0)
    }

    pub fn items(self) -> impl Iterator<Item = &'static Item> {
        ITEMS
            .iter()
            .enumerate()
            .filter(move |(n, _)| self.0 & (1 << n) != 0)
            .map(|(_, i)| i)
    }

    /// Кнопки внизу чата: по две в ряд в порядке [`ITEMS`].
    pub fn rows(self) -> Vec<Vec<&'static str>> {
        let buttons: Vec<&'static str> = self.items().map(|i| i.button).collect();
        buttons.chunks(2).map(<[&str]>::to_vec).collect()
    }
}

/// Отказ в праве словами — как ответ сервера 403 (ADR-097, ADR-152).
fn denied_text(a: &Access, key: &str) -> String {
    let label = crate::auth::permission_def(key).map_or(key, |p| p.label);
    match a {
        Access::Cashier(name) => format!("Нет права у кассира {name}: {label}"),
        _ => format!("Нет права: {label}"),
    }
}

/// Чего не хватает для пункта: `None` — пункт доступен.
async fn missing(
    conn: &mut PgConnection,
    user: &CurrentUser,
    item: &Item,
) -> AppResult<Option<String>> {
    for key in item.rights {
        let a = crate::auth::access(conn, user, key).await?;
        if !a.allowed() {
            return Ok(Some(denied_text(&a, key)));
        }
    }
    Ok(None)
}

/// Меню администратора по его правам сейчас (I12: решает сервер).
pub async fn menu(conn: &mut PgConnection, user: &CurrentUser) -> AppResult<Menu> {
    let mut bits = 0u16;
    for (n, item) in ITEMS.iter().enumerate() {
        if missing(conn, user, item).await?.is_none() {
            bits |= 1 << n;
        }
    }
    Ok(Menu(bits))
}

/// Администратор, к которому привязан чат (только активный), и кто в Телеграме привязывал чат.
pub async fn chat_user(
    conn: &mut PgConnection,
    chat_id: i64,
) -> AppResult<Option<(CurrentUser, Option<i64>)>> {
    Ok(crate::api::receipt_drafts::chat_sender(conn, chat_id)
        .await?
        .filter(|(u, _)| u.role == Role::Admin))
}

/// Кнопки чата администратора по его правам.
pub async fn keys(conn: &mut PgConnection, chat_id: i64) -> AppResult<Keys> {
    Ok(match chat_user(conn, chat_id).await? {
        Some((user, _)) => Keys::Admin(menu(conn, &user).await?),
        None => Keys::Admin(Menu::default()),
    })
}

/// Подсказка администратору: что ему доступно.
pub fn help(menu: Menu) -> String {
    let mut lines = vec!["Что можно в боте:".to_string()];
    lines.extend(menu.items().map(|i| format!("{} — {}", i.button, i.what)));
    if menu.has("invoice") {
        lines.push("Фото накладной присылайте прямо в чат.".into());
    }
    lines.push("Остальное — в программе.".into());
    lines.join("\n")
}

// ---------- Защита ответов (I13) ----------

/// Начала слов, которых в ответе администратору быть не может.
const FORBIDDEN: [&str; 6] = ["прибыл", "себестоим", "марж", "валов", "чистая", "наценк"];

/// В строке есть слово о прибыли, себестоимости, марже или валовой.
pub fn leaks(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| FORBIDDEN.iter().any(|f| w.starts_with(f)))
}

/// Текст без строк о деньгах владельца. Таких строк быть не должно: вырезанная — ошибка в
/// построении текста, в журнал сервера.
pub fn scrub(text: &str) -> String {
    if !leaks(text) {
        return text.to_string();
    }
    tracing::warn!("бот администратора: вырезана строка о деньгах владельца");
    text.lines()
        .filter(|l| !leaks(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ответ чату администратора — через [`scrub`]: текст, кнопки под ним и следующее сообщение.
pub fn guard(mut r: Reply) -> Reply {
    r.text = scrub(&r.text);
    for b in r.inline.iter_mut().flatten() {
        b.text = scrub(&b.text);
    }
    if let Some(f) = r.follow.as_mut() {
        f.text = scrub(&f.text);
    }
    r
}

/// Чат привязан к активному администратору.
pub async fn is_admin_chat(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from telegram_links t join users u on u.id = t.user_id
                          where t.chat_id = $1 and u.active and u.role = 'admin') as "e!""#,
        chat_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

// ---------- Ожидание ввода ----------

/// Ожидание ввода, минут.
const WAIT_MINUTES: i32 = 30;
const KINDS: [&str; 2] = ["admin_find", "admin_expense"];

async fn waiting(conn: &mut PgConnection, chat_id: i64) -> AppResult<Option<(String, Value)>> {
    let kinds: Vec<String> = KINDS.iter().map(|s| (*s).to_string()).collect();
    Ok(sqlx::query!(
        "select kind, data from bot_input where chat_id = $1 and kind = any($2) and expires_at > now()",
        chat_id,
        &kinds
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| (r.kind, r.data)))
}

async fn save(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    kind: &str,
    data: Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into bot_input (chat_id, branch_id, kind, data, expires_at)
           values ($1, $2, $3, $4, now() + make_interval(mins => $5))
           on conflict (chat_id) do update set branch_id = excluded.branch_id, kind = excluded.kind,
               vehicle_id = null, data = excluded.data, expires_at = excluded.expires_at"#,
        chat_id,
        user.branch_id,
        kind,
        data,
        WAIT_MINUTES
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Снять ожидание поиска и шагов расхода (ввод старого долга — у `bot_debtors`).
pub async fn clear(conn: &mut PgConnection, chat_id: i64) -> AppResult<()> {
    let kinds: Vec<String> = KINDS.iter().map(|s| (*s).to_string()).collect();
    sqlx::query!(
        "delete from bot_input where chat_id = $1 and kind = any($2)",
        chat_id,
        &kinds
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------- Сообщения ----------

const ASK_STOCK: &str = "📦 Остаток: напишите название, артикул или штрихкод товара.";
const ASK_BOOK: &str = "📒 Книжка: напишите госномер машины (например: 01KG123ABC).";
const AGAIN: &str = "\n\nЕщё поиск — напишите снова; меню — кнопки внизу.";

/// Пункт по тексту: кнопка, «/команда» или одно слово команды.
fn item_of(t: &str) -> Option<&'static Item> {
    if let Some(i) = ITEMS.iter().find(|i| i.button == t) {
        return Some(i);
    }
    let cmd = crate::api::telegram::command(t);
    let one_word = t.split_whitespace().count() == 1;
    if !(t.starts_with('/') || one_word) {
        return None;
    }
    let cmd = match cmd.as_str() {
        "остатки" => "остаток",
        c => c,
    };
    ITEMS.iter().find(|i| i.command == cmd)
}

/// Текст в чате администратора. `None` — это не чат администратора, код привязки или
/// `/start`: дальше решает остальной бот. Должники и ввод старого долга — `bot_debtors`, он
/// отвечает раньше.
pub async fn on_message(
    conn: &mut PgConnection,
    chat_id: i64,
    text: Option<&str>,
) -> AppResult<Option<Reply>> {
    let Some((user, _)) = chat_user(conn, chat_id).await? else {
        return Ok(None);
    };
    let t = text.map(str::trim).unwrap_or("");
    if t.starts_with("/start") {
        return Ok(None);
    }
    let menu = menu(conn, &user).await?;
    let answer = |text: String| Some(Reply::new(text, Keys::Admin(menu)));
    if let Some(item) = item_of(t) {
        clear(conn, chat_id).await?;
        if let Some(why) = missing(conn, &user, item).await? {
            return Ok(answer(format!("{why} — его выдаёт владелец.")));
        }
        // «/остаток масло», «/книжка 01KG123» — сразу поиск.
        let arg = t
            .starts_with('/')
            .then(|| t.split_once(char::is_whitespace).map(|x| x.1.trim()))
            .flatten()
            .unwrap_or("");
        let text = match item.key {
            "invoice" => {
                crate::api::receipt_drafts::invoice_hint_text(conn, user.branch_id).await?
            }
            "my_invoices" => crate::api::receipt_drafts::my_drafts(conn, &user).await?,
            "shift" => shift_now(conn, user.branch_id).await?,
            "stock" | "book" => {
                save(
                    conn,
                    &user,
                    chat_id,
                    "admin_find",
                    json!({ "what": item.key }),
                )
                .await?;
                match (item.key, arg.is_empty()) {
                    ("stock", true) => ASK_STOCK.into(),
                    ("book", true) => ASK_BOOK.into(),
                    ("stock", false) => format!("{}{AGAIN}", stock_text(conn, &user, arg).await?),
                    _ => format!("{}{AGAIN}", book_text(conn, arg).await?),
                }
            }
            "expense" => return expense_start(conn, &user, chat_id, menu).await.map(Some),
            // Должники и старый долг отвечает `bot_debtors` раньше.
            _ => help(menu),
        };
        return Ok(answer(text));
    }
    if let Some((kind, data)) = waiting(conn, chat_id).await? {
        if t.starts_with('/') {
            clear(conn, chat_id).await?;
        } else if !t.is_empty() {
            match kind.as_str() {
                "admin_find" => {
                    let what = data.get("what").and_then(Value::as_str).unwrap_or("");
                    let found = match what {
                        "stock" if menu.has("stock") => stock_text(conn, &user, t).await?,
                        "book" if menu.has("book") => book_text(conn, t).await?,
                        _ => {
                            clear(conn, chat_id).await?;
                            return Ok(answer(help(menu)));
                        }
                    };
                    save(conn, &user, chat_id, "admin_find", data).await?;
                    return Ok(answer(format!("{found}{AGAIN}")));
                }
                _ => {
                    let d: Spend = serde_json::from_value(data).unwrap_or_default();
                    return expense_text(conn, &user, chat_id, menu, d, t)
                        .await
                        .map(Some);
                }
            }
        }
    }
    // Код привязки шестью цифрами решает остальной бот.
    if crate::api::telegram::is_code(t) {
        return Ok(None);
    }
    Ok(answer(help(menu)))
}

// ---------- 🧾 Смена сейчас ----------

/// Открытая смена кассы филиала в виде администратора; не открыта — последняя смена.
pub async fn shift_now(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<String> {
    let till = crate::api::cash::default_account(conn, branch_id).await?;
    if let Some(id) = crate::api::cash::open_shift_id(conn, branch_id, till).await? {
        let r = crate::api::shift_report::load(conn, branch_id, false, id).await?;
        return Ok(shift_text(&r));
    }
    let last = sqlx::query_scalar!(
        "select id from shifts where branch_id = $1 and account_id = $2 order by opened_at desc limit 1",
        branch_id,
        till
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = last else {
        return Ok("🧾 Смена не открыта.".into());
    };
    let r = crate::api::shift_report::load(conn, branch_id, false, id).await?;
    Ok(format!(
        "🧾 Смена не открыта. Последняя смена:\n\n{}",
        shift_text(&r)
    ))
}

/// Время по Бишкеку: «20.10 09:07» (UTC+6 круглый год).
fn local(t: DateTime<Utc>) -> String {
    (t + chrono::Duration::hours(6))
        .format("%d.%m %H:%M")
        .to_string()
}

/// Отчёт смены администратору: выручка по способам оплаты, чеки, деньги в кассе, расходы смены.
/// Читает только поля, которые сервер отдаёт администратору: отчёт в виде владельца сюда не
/// попадает (I13).
pub fn shift_text(r: &ShiftReport) -> String {
    if r.owner_view {
        return "🧾 Отчёт смены недоступен.".into();
    }
    let s = &r.shift;
    let c = &r.cash;
    let v = &r.revenue;
    let when = match s.closed_at {
        Some(at) => format!("{} — {}", local(s.opened_at), local(at)),
        None => format!("с {}, открыта", local(s.opened_at)),
    };
    let mut out = vec![format!(
        "🧾 Смена № {} · кассир {} · касса «{}»\n{when}",
        s.number, s.cashier_name, s.account_name
    )];
    let mut rev = vec![format!(
        "Выручка: {} · чеков {}{}",
        format_som(v.net_tyiyn),
        v.sales_count,
        if v.returns_count > 0 {
            format!(
                " · возвраты {} ({})",
                v.returns_count,
                format_som(v.returns_tyiyn)
            )
        } else {
            String::new()
        }
    )];
    for m in &v.methods {
        rev.push(format!(
            "  {}: {}",
            method_label(&m.method),
            format_som(m.amount_tyiyn)
        ));
    }
    out.push(rev.join("\n"));
    let mut cash = vec![
        "💰 Касса".to_string(),
        format!("  на начало: {}", format_som(c.opening_tyiyn)),
        format!("  + приход: {}", format_som(c.income_tyiyn)),
        format!("  − расход: {}", format_som(c.outgo_tyiyn)),
        format!("  должно быть: {}", format_som(c.expected_tyiyn)),
    ];
    if let (Some(counted), Some(diff)) = (c.counted_tyiyn, c.diff_tyiyn) {
        let result = match diff {
            0 => "сошлось".to_string(),
            d if d < 0 => format!("недостача {}", format_som(-d)),
            d => format!("излишек {}", format_som(d)),
        };
        cash.push(format!("  пересчитали: {} — {result}", format_som(counted)));
    }
    out.push(cash.join("\n"));
    let spent: Vec<_> = r
        .money
        .iter()
        .filter(|m| m.kind == "expenses" && m.account_name == s.account_name)
        .collect();
    if !spent.is_empty() {
        let total = spent.iter().fold(0i64, |a, m| {
            a.saturating_add(m.amount_tyiyn.saturating_abs())
        });
        let mut lines = vec![format!(
            "💸 Расходы смены: {} ({})",
            format_som(total),
            spent.len()
        )];
        for m in spent.iter().take(10) {
            let what = m.counterparty.clone().unwrap_or_else(|| m.label.clone());
            lines.push(format!(
                "  {}{} — {}",
                if m.reversal { "сторно: " } else { "" },
                what,
                format_som(m.amount_tyiyn.saturating_abs())
            ));
        }
        out.push(lines.join("\n"));
    }
    out.join("\n\n")
}

// ---------- 📦 Остаток ----------

/// Товары по названию, артикулу или коду: остаток в филиале и цена продажи. Товары — по роли
/// администратора (`query_products` себестоимости ему не отдаёт), в текст — только остаток и цена.
pub async fn stock_text(conn: &mut PgConnection, user: &CurrentUser, q: &str) -> AppResult<String> {
    let q = q.trim();
    if q.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
        return Ok("Напишите хотя бы 2 буквы или цифры названия, артикула или кода.".into());
    }
    let found = crate::api::catalog::query_products(
        conn,
        user,
        crate::api::catalog::ProductFilter {
            q: Some(q.to_string()),
            limit: 9,
            ..Default::default()
        },
    )
    .await?;
    if found.is_empty() {
        return Ok(format!(
            "📦 По «{q}» ничего не нашли. Напишите иначе — часть названия, артикул или штрихкод."
        ));
    }
    let more = found.len() > 8;
    let mut lines = vec![format!("📦 Остаток по «{q}»:")];
    for p in found.iter().take(8) {
        let article = if p.article.is_empty() {
            String::new()
        } else {
            format!(" (арт. {})", p.article)
        };
        let pour = p
            .pour_price_per_l_tyiyn
            .map(|v| format!(", розлив {} за 1 л", format_som(v)))
            .unwrap_or_default();
        lines.push(format!(
            "• {}{article} — {} · цена {}{pour}",
            p.name,
            crate::domain::qty::stock_text(p.stock_qty, &p.unit, p.container_ml),
            format_som(p.sale_price_tyiyn)
        ));
    }
    if more {
        lines.push("Нашлось больше 8 — уточните название.".into());
    }
    Ok(lines.join("\n"))
}

// ---------- 📒 Книжка ----------

/// Масляная книжка машины по госномеру (книжка общая для филиалов, SPEC-33): что залито, когда
/// и следующая замена; себестоимости в книжке нет (ADR-061).
pub async fn book_text(conn: &mut PgConnection, q: &str) -> AppResult<String> {
    let norm = crate::domain::plate::normalize(q.trim());
    if norm.chars().count() < 3 {
        return Ok("Напишите госномер — хотя бы 3 знака.".into());
    }
    let like = crate::api::catalog::escape_like(&norm);
    let cars = sqlx::query!(
        r#"select v.id, v.plate, p.name as party from party_vehicles v
           join parties p on p.id = v.party_id
           where v.active and p.active and v.plate_norm like '%' || $1 || '%'
           order by (v.plate_norm = $2) desc, v.plate
           limit 6"#,
        like,
        norm
    )
    .fetch_all(&mut *conn)
    .await?;
    if cars.is_empty() {
        return Ok(format!("🚗 Машину с номером «{}» не нашли.", q.trim()));
    }
    if cars.len() > 3 {
        let mut lines = vec![format!(
            "🚗 По «{}» несколько машин — напишите номер точнее:",
            q.trim()
        )];
        lines.extend(cars.iter().map(|c| format!("• {} — {}", c.plate, c.party)));
        return Ok(lines.join("\n"));
    }
    let mut parts = Vec::with_capacity(cars.len());
    for c in &cars {
        let books = crate::api::oil_book::books(conn, None, Some(c.id)).await?;
        let Some(b) = books.first() else { continue };
        let car = [b.brand.as_str(), b.model.as_str()]
            .into_iter()
            .filter(|x| !x.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let head = if car.is_empty() {
            format!("🚗 {} · {}", b.plate, c.party)
        } else {
            format!("🚗 {} ({car}) · {}", b.plate, c.party)
        };
        parts.push(crate::api::bot_book::car_book(b, head));
    }
    Ok(parts.join("\n\n"))
}

// ---------- 💸 Расход ----------

/// Шаги расхода: статья, сумма, комментарий, «Кому» (если обязательно) → черновик.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Spend {
    step: String,
    /// Операция (I8): черновик и проведение — с этим `op_id`.
    op_id: Uuid,
    #[serde(default)]
    article_id: Option<Uuid>,
    #[serde(default)]
    article: String,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    payee: String,
}

fn button(text: impl Into<String>, data: impl Into<String>) -> Button {
    Button {
        text: text.into(),
        data: data.into(),
    }
}

fn with_buttons(text: impl Into<String>, menu: Menu, rows: Vec<Vec<Button>>) -> Reply {
    let mut r = Reply::new(text, Keys::Admin(menu));
    r.inline = rows;
    r
}

fn cancel_row() -> Vec<Button> {
    vec![button("✖ Отмена", "ax:no")]
}

/// Статьи, по которым администратор платит: действующие, не «только владельцу».
async fn articles(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Vec<(Uuid, String)>> {
    crate::api::expenses::ensure_articles(conn, branch_id).await?;
    Ok(sqlx::query!(
        "select id, name from expense_articles where branch_id = $1 and active and not owner_only order by name",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|a| (a.id, a.name))
    .collect())
}

async fn expense_start(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    menu: Menu,
) -> AppResult<Reply> {
    let d = Spend {
        step: "article".into(),
        op_id: ops::new_id(),
        ..Spend::default()
    };
    save(conn, user, chat_id, "admin_expense", json!(d)).await?;
    ask_article(
        conn,
        user,
        menu,
        "💸 Расход из кассы смены. Статья расхода?",
    )
    .await
}

async fn ask_article(
    conn: &mut PgConnection,
    user: &CurrentUser,
    menu: Menu,
    text: &str,
) -> AppResult<Reply> {
    let list = articles(conn, user.branch_id).await?;
    let mut rows: Vec<Vec<Button>> = list
        .chunks(2)
        .map(|c| {
            c.iter()
                .map(|(id, name)| button(name.clone(), format!("ax:a:{id}")))
                .collect()
        })
        .collect();
    rows.push(cancel_row());
    Ok(with_buttons(text, menu, rows))
}

/// Вопрос шага расхода.
fn ask_step(d: &Spend, menu: Menu) -> Reply {
    let skip = || vec![button("Пропустить", "ax:skip"), button("✖ Отмена", "ax:no")];
    match d.step.as_str() {
        "amount" => with_buttons(
            format!(
                "Статья: {}. Сумма, сом (например: 1 500 или 250,50).",
                d.article
            ),
            menu,
            vec![cancel_row()],
        ),
        "comment" if d.article == "Прочее" => with_buttons(
            "За что? По статье «Прочее» комментарий обязателен.",
            menu,
            vec![cancel_row()],
        ),
        "comment" => with_buttons(
            "Комментарий — за что (до 200 знаков) — или «Пропустить».",
            menu,
            vec![skip()],
        ),
        "payee" => with_buttons(
            "Кому отдали деньги? (обязательно по настройке владельца)",
            menu,
            vec![cancel_row()],
        ),
        _ => with_buttons("Статья расхода?", menu, vec![cancel_row()]),
    }
}

async fn payee_required(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<bool> {
    Ok(crate::api::pnl::profit_settings(conn, branch_id)
        .await?
        .expense_payee
        == "required")
}

/// Следующий шаг после комментария: «Кому» (если обязательно) или черновик.
async fn after_comment(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    menu: Menu,
    mut d: Spend,
) -> AppResult<Reply> {
    if d.payee.is_empty() && payee_required(conn, user.branch_id).await? {
        d.step = "payee".into();
        save(conn, user, chat_id, "admin_expense", json!(d)).await?;
        return Ok(ask_step(&d, menu));
    }
    draft(conn, user, chat_id, menu, d).await
}

/// Текст на шаге расхода.
async fn expense_text(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    menu: Menu,
    mut d: Spend,
    t: &str,
) -> AppResult<Reply> {
    if !menu.has("expense") {
        clear(conn, chat_id).await?;
        return Ok(Reply::new(
            "Нет права: Расходы — его выдаёт владелец.",
            Keys::Admin(menu),
        ));
    }
    match d.step.as_str() {
        "article" => {
            let wanted = crate::api::bot_drafts::norm(t);
            let list = articles(conn, user.branch_id).await?;
            let found: Vec<_> = list
                .iter()
                .filter(|(_, n)| crate::api::bot_drafts::norm(n) == wanted)
                .chain(
                    list.iter()
                        .filter(|(_, n)| crate::api::bot_drafts::sounds_like(n, t)),
                )
                .collect();
            let Some((id, name)) = found.first().copied() else {
                return ask_article(conn, user, menu, "Такой статьи нет — выберите кнопкой.").await;
            };
            d.article_id = Some(*id);
            d.article = name.clone();
            d.step = "amount".into();
        }
        "amount" => match parse_spoken_som(t).filter(|v| *v > 0) {
            Some(v) => {
                d.amount = v;
                d.step = "comment".into();
            }
            None => {
                return Ok(with_buttons(
                    "Не понял сумму. Напишите числом, например: 1 500 или 250,50.",
                    menu,
                    vec![cancel_row()],
                ));
            }
        },
        "comment" => {
            d.comment = t.chars().take(200).collect();
            return after_comment(conn, user, chat_id, menu, d).await;
        }
        "payee" => {
            d.payee = t.chars().take(100).collect();
            return draft(conn, user, chat_id, menu, d).await;
        }
        _ => return Ok(ask_step(&d, menu)),
    }
    save(conn, user, chat_id, "admin_expense", json!(d)).await?;
    Ok(ask_step(&d, menu))
}

/// Черновик расхода из кассы смены — тем же путём, что черновик владельца (SPEC-24): проверка
/// той же функцией в откатываемой транзакции, `bot_drafts`, «✅ Провести» / «✖ Отмена».
async fn draft(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    menu: Menu,
    d: Spend,
) -> AppResult<Reply> {
    clear(conn, chat_id).await?;
    let Some(article_id) = d.article_id else {
        return Ok(Reply::new(
            "Статья не выбрана — начните заново: «💸 Расход».",
            Keys::Admin(menu),
        ));
    };
    let till = crate::api::cash::default_account(conn, user.branch_id).await?;
    let till_name = sqlx::query_scalar!("select name from cash_accounts where id = $1", till)
        .fetch_one(&mut *conn)
        .await?;
    let today = crate::api::fees::today(conn).await?;
    let to_whom = if d.payee.is_empty() {
        String::new()
    } else {
        format!("\nКому: «{}»", d.payee)
    };
    let summary = format!(
        "Расход\nСтатья: {}\nСумма: {}{to_whom}\nОткуда: касса смены «{till_name}»\nДата: {}\nКомментарий: {}",
        d.article,
        format_som(d.amount),
        today.format("%d.%m.%Y"),
        if d.comment.is_empty() {
            "—".to_string()
        } else {
            format!("«{}»", d.comment)
        }
    );
    let op = DraftOp::Expense(ExpenseReq {
        op_id: d.op_id,
        article_id,
        amount_tyiyn: d.amount,
        source: Some("account".into()),
        account_id: Some(till),
        expense_date: None,
        comment: d.comment.clone(),
        payee: d.payee.clone(),
    });
    match crate::api::bot_drafts::store_draft(conn, user, chat_id, d.op_id, op, "expense", summary)
        .await?
    {
        Prepared::Draft(shown) => Ok(with_buttons(
            shown.summary,
            menu,
            vec![
                crate::api::bot_drafts::buttons(shown.id)
                    .into_iter()
                    .map(|(text, data)| button(text, data))
                    .collect(),
            ],
        )),
        Prepared::Refused(why) => Ok(Reply::new(
            format!("{why}\nНачать заново — «💸 Расход»."),
            Keys::Admin(menu),
        )),
    }
}

/// Нажатие под шагами расхода (`ax:`): статья, «Пропустить», «✖ Отмена».
pub async fn callback(conn: &mut PgConnection, chat_id: i64, data: &str) -> AppResult<Reply> {
    let Some((user, _)) = chat_user(conn, chat_id).await? else {
        return Ok(Reply::new("Кнопка устарела.", Keys::Admin(Menu::default())));
    };
    let menu = menu(conn, &user).await?;
    if data == "ax:no" {
        clear(conn, chat_id).await?;
        return Ok(Reply::new(
            "Отменено — расход не проведён.",
            Keys::Admin(menu),
        ));
    }
    if !menu.has("expense") {
        clear(conn, chat_id).await?;
        return Ok(Reply::new(
            "Нет права: Расходы — его выдаёт владелец.",
            Keys::Admin(menu),
        ));
    }
    let d = match waiting(conn, chat_id).await? {
        Some((kind, v)) if kind == "admin_expense" => {
            serde_json::from_value::<Spend>(v).unwrap_or_default()
        }
        _ => {
            return Ok(Reply::new(
                "Кнопка устарела. Начать заново — «💸 Расход».",
                Keys::Admin(menu),
            ));
        }
    };
    let mut d = d;
    if let Some(id) = data.strip_prefix("ax:a:").map(Uuid::parse_str) {
        let Ok(id) = id else {
            return Ok(ask_step(&d, menu));
        };
        let Some((_, name)) = articles(conn, user.branch_id)
            .await?
            .into_iter()
            .find(|(a, _)| *a == id)
        else {
            return ask_article(conn, &user, menu, "Статья не найдена — выберите кнопкой.").await;
        };
        if d.step != "article" {
            return Ok(ask_step(&d, menu));
        }
        d.article_id = Some(id);
        d.article = name;
        d.step = "amount".into();
        save(conn, &user, chat_id, "admin_expense", json!(d)).await?;
        return Ok(ask_step(&d, menu));
    }
    if data == "ax:skip" && d.step == "comment" && d.article != "Прочее" {
        d.comment.clear();
        return after_comment(conn, &user, chat_id, menu, d).await;
    }
    Ok(ask_step(&d, menu))
}

// ---------- Лента администратора ----------

/// Событие ленты администратора: действие журнала, группа, подпись и право, без которого оно не
/// приходит. Только события без денег владельца: без прибыли, себестоимости, закупочных сумм,
/// безнала и сводных доходов (I13); тексты — [`event_text`].
pub struct AdminEvent {
    pub action: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub right: Option<&'static str>,
}

const fn ev(
    action: &'static str,
    group: &'static str,
    label: &'static str,
    right: Option<&'static str>,
) -> AdminEvent {
    AdminEvent {
        action,
        group,
        label,
        right,
    }
}

/// Группы экрана: ключ и подпись.
pub const GROUPS: [(&str, &str); 5] = [
    ("shift", "Смена"),
    ("sales", "Касса"),
    ("stock", "Склад"),
    ("money", "Деньги"),
    ("debts", "Долги"),
];

pub const EVENTS: [AdminEvent; 15] = [
    ev("shift.open", "shift", "Смена открыта", None),
    ev(
        "shift.close",
        "shift",
        "Смена закрыта: выручка и касса",
        None,
    ),
    ev("shift.handover", "shift", "Сдача кассы", None),
    ev("shift.reopen", "shift", "Смена переоткрыта", None),
    ev(
        "shift.pin_locked",
        "shift",
        "PIN кассира заблокирован",
        None,
    ),
    ev(
        "sale.offline_rejected",
        "sales",
        "Чек без сети не принят сервером",
        None,
    ),
    ev(
        "bank_qr.unclaimed",
        "sales",
        "QR оплачен, а чек не проведён",
        None,
    ),
    ev(
        "receipt.post",
        "stock",
        "Приход товара: поставщик и строки, без сумм",
        Some("receipts"),
    ),
    ev(
        "receipt.accept",
        "stock",
        "Приёмка товара на планшете, без сумм",
        Some("receipts"),
    ),
    ev(
        "acceptance.refuse",
        "stock",
        "Отказ от приёмки",
        Some("receipts"),
    ),
    ev(
        "defect.mark",
        "stock",
        "Товар в брак, без стоимости",
        Some("defects"),
    ),
    ev(
        "stock_in.post",
        "stock",
        "Оприходование, без сумм",
        Some("stock_in"),
    ),
    ev(
        "expense.post",
        "money",
        "Расход из кассы смены",
        Some("expenses"),
    ),
    ev(
        "debt.repayment",
        "debts",
        "Клиент погасил долг",
        Some("view_debts"),
    ),
    ev(
        "debt.old",
        "debts",
        "Старый долг из тетради",
        Some("view_debts"),
    ),
];

fn event_def(action: &str) -> Option<&'static AdminEvent> {
    EVENTS.iter().find(|e| e.action == action)
}

/// Настройка ленты администратора: включённые события и с какого времени каждое включено
/// (прошлое пачкой не приходит); по умолчанию — ничего (ADR-135).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AdminFeed {
    #[serde(default)]
    pub on: Vec<String>,
    #[serde(default)]
    pub from: std::collections::BTreeMap<String, DateTime<Utc>>,
}

fn feed_key(user_id: Uuid) -> String {
    format!("bot_feed_admin:{user_id}")
}

async fn load_feed(conn: &mut PgConnection, user_id: Uuid) -> AppResult<AdminFeed> {
    let v = sqlx::query_scalar!(
        r#"select s.value from settings s join users u on u.id = $1
           where s.branch_id = u.branch_id and s.key = $2"#,
        user_id,
        feed_key(user_id)
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value::<AdminFeed>(v).ok())
        .unwrap_or_default())
}

/// Известные события без повторов, в порядке [`EVENTS`]; чужое — 422.
fn normalize(on: &[String]) -> AppResult<Vec<String>> {
    if let Some(bad) = on.iter().find(|a| event_def(a).is_none()) {
        return Err(invalid(format!(
            "событие «{bad}» администратору не присылается"
        )));
    }
    Ok(EVENTS
        .iter()
        .map(|e| e.action)
        .filter(|a| on.iter().any(|o| o == a))
        .map(str::to_string)
        .collect())
}

/// Новая настройка: время включения у прежних событий сохраняется, новые включены с `now`.
fn turned_on(before: &AdminFeed, on: Vec<String>, now: DateTime<Utc>) -> AdminFeed {
    let from = on
        .iter()
        .map(|a| {
            let at = before
                .from
                .get(a)
                .copied()
                .filter(|_| before.on.contains(a))
                .unwrap_or(now);
            (a.clone(), at)
        })
        .collect();
    AdminFeed { on, from }
}

fn admin_actions() -> Vec<String> {
    EVENTS.iter().map(|e| e.action.to_string()).collect()
}

/// События журнала для чатов администраторов: только включённые владельцем, только филиал
/// администратора (I16), только при праве события на момент отправки. Окно и курсор — те же, что
/// у ленты владельца (`telegram::pending`); событие без права или без текста отмечается
/// отправленным и не держит курсор.
pub async fn pending(conn: &mut PgConnection) -> AppResult<Vec<Outgoing>> {
    let Some((cursor, since)) = crate::api::telegram::feed_bounds(conn).await? else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query!(
        r#"select a.id, a.action, a.data, a.created_at, a.entity_id, a.branch_id,
                  w.full_name as "who?", t.chat_id, u.id as user_id, u.login, u.full_name
           from audit_log a
           join users u on u.branch_id = a.branch_id and u.active and u.role = 'admin'
           join telegram_links t on t.user_id = u.id
           join settings f on f.branch_id = u.branch_id and f.key = 'bot_feed_admin:' || u.id::text
           left join users w on w.id = a.user_id
           where a.created_at > greatest($1::timestamptz - interval '15 minutes', $2::timestamptz,
                                         now() - interval '1 day')
             and a.created_at >= t.linked_at
             and a.action = any($3)
             and coalesce(f.value -> 'on' ? a.action, false)
             -- Включённое приходит с момента включения (SPEC-40 §2).
             and a.created_at >= coalesce((f.value -> 'from' ->> a.action)::timestamptz, '-infinity')
             and not exists (select 1 from telegram_sent s where s.chat_id = t.chat_id and s.audit_id = a.id)
           order by a.created_at, a.id, t.chat_id
           limit 100"#,
        cursor,
        since,
        &admin_actions()
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut rights: HashMap<(Uuid, &'static str), bool> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let skip = Outgoing {
            audit_id: r.id,
            at: r.created_at,
            chat_id: r.chat_id,
            text: String::new(),
            signature_of: None,
        };
        let Some(def) = event_def(&r.action) else {
            continue;
        };
        if let Some(key) = def.right {
            let allowed = match rights.get(&(r.user_id, key)) {
                Some(a) => *a,
                None => {
                    let user = CurrentUser {
                        id: r.user_id,
                        branch_id: r.branch_id,
                        login: r.login.clone(),
                        full_name: r.full_name.clone(),
                        role: Role::Admin,
                    };
                    let a = crate::auth::has_permission(conn, &user, key).await?;
                    rights.insert((r.user_id, key), a);
                    a
                }
            };
            if !allowed {
                crate::api::telegram::mark_sent(conn, &skip).await?;
                continue;
            }
        }
        let data = crate::api::notifications::safe_data(&r.data);
        let Some((title, details)) =
            event_text(conn, &r.action, &data, r.entity_id, r.branch_id).await?
        else {
            crate::api::telegram::mark_sent(conn, &skip).await?;
            continue;
        };
        let who = r.who.map(|w| format!("\n— {w}")).unwrap_or_default();
        let text = if details.is_empty() {
            format!("{title}{who}")
        } else {
            format!("{title}\n{details}{who}")
        };
        out.push(Outgoing {
            text: scrub(&text),
            ..skip
        });
    }
    Ok(out)
}

/// Самое старое ещё не отправленное событие ленты администраторов (для курсора ленты).
pub async fn feed_low(
    conn: &mut PgConnection,
    cursor: DateTime<Utc>,
    since: DateTime<Utc>,
) -> AppResult<DateTime<Utc>> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(min(a.created_at), now()) as "low!"
           from audit_log a
           join users u on u.branch_id = a.branch_id and u.active and u.role = 'admin'
           join telegram_links t on t.user_id = u.id
           join settings f on f.branch_id = u.branch_id and f.key = 'bot_feed_admin:' || u.id::text
           where a.created_at > greatest($1::timestamptz - interval '15 minutes', $2::timestamptz,
                                         now() - interval '1 day')
             and a.created_at >= t.linked_at
             and a.action = any($3)
             and coalesce(f.value -> 'on' ? a.action, false)
             -- Включённое приходит с момента включения (SPEC-40 §2).
             and a.created_at >= coalesce((f.value -> 'from' ->> a.action)::timestamptz, '-infinity')
             and not exists (select 1 from telegram_sent s where s.chat_id = t.chat_id and s.audit_id = a.id)"#,
        cursor,
        since,
        &admin_actions()
    )
    .fetch_one(&mut *conn)
    .await?)
}

fn text_of(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn num_of(data: &Value, key: &str) -> i64 {
    data.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn som_of(data: &Value, key: &str) -> String {
    format_som(num_of(data, key))
}

fn lines_word(n: i64) -> String {
    format!(
        "{n} {}",
        crate::domain::qty::plural(n, "строка", "строки", "строк")
    )
}

fn doc_tail(doc: &str) -> String {
    if doc.is_empty() {
        String::new()
    } else {
        format!(", накладная № {doc}")
    }
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

/// Текст события для администратора: заголовок и подробности. Строится заново, только из полей
/// без денег владельца (тексты владельца — `telegram::event_text` — сюда не идут). `None` —
/// событие администратору не показывается (расход не из кассы смены, оплата поставщику).
pub async fn event_text(
    conn: &mut PgConnection,
    action: &str,
    data: &Value,
    entity_id: Option<Uuid>,
    branch_id: Uuid,
) -> AppResult<Option<(String, String)>> {
    Ok(Some(match action {
        "shift.open" => {
            let cashier = match entity_id {
                Some(id) => sqlx::query_scalar!(
                    r#"select e.full_name as "n?" from shifts s
                       left join employees e on e.id = s.cashier_employee_id where s.id = $1"#,
                    id
                )
                .fetch_optional(&mut *conn)
                .await?
                .flatten(),
                None => None,
            };
            (
                format!("🟢 Смена № {} открыта", num_of(data, "number")),
                format!(
                    "{}размен в кассе {}",
                    cashier
                        .map(|c| format!("кассир {c} · "))
                        .unwrap_or_default(),
                    som_of(data, "counted")
                ),
            )
        }
        "shift.close" => {
            let Some(id) = entity_id else { return Ok(None) };
            let n = shift_number(conn, entity_id).await?;
            let r = crate::api::shift_report::load(conn, branch_id, false, id).await?;
            (format!("🔴 Смена № {n} закрыта"), shift_text(&r))
        }
        "shift.handover" => {
            let to = text_of(data, "to_name");
            let nothing = data.get("to_safe").and_then(Value::as_i64) == Some(0);
            if nothing {
                (
                    format!(
                        "📥 Смена № {}: деньги остались в кассе",
                        num_of(data, "number")
                    ),
                    format!("в кассе осталось {}", som_of(data, "left")),
                )
            } else {
                (
                    format!("📥 Касса смены № {} сдана", num_of(data, "number")),
                    format!(
                        "сдали {}{}, в кассе осталось {}",
                        som_of(data, "to_safe"),
                        if to.is_empty() {
                            String::new()
                        } else {
                            format!(" → {to}")
                        },
                        som_of(data, "left")
                    ),
                )
            }
        }
        "shift.reopen" => (
            format!(
                "🔓 Смена № {} переоткрыта",
                shift_number(conn, entity_id).await?
            ),
            format!("причина: {}", text_of(data, "reason")),
        ),
        "shift.pin_locked" => (
            format!(
                "🔒 PIN кассира заблокирован: {}",
                text_of(data, "full_name")
            ),
            "неверный PIN несколько раз подряд — если это не кассир, скажите владельцу".into(),
        ),
        "sale.offline_rejected" => (
            "⚠️ Чек без сети не принят сервером".into(),
            format!("{} — разберите его на кассе", text_of(data, "error")),
        ),
        "bank_qr.unclaimed" => (
            format!("⚠️ QR оплачен, чек не проведён: {}", som_of(data, "amount")),
            "покупатель оплатил по QR банка, а чека с этой оплатой нет — проверьте кассу".into(),
        ),
        "receipt.post" => {
            let Some(id) = entity_id else { return Ok(None) };
            // Приход по приёмке на планшете — сообщением о приёмке.
            let r = sqlx::query!(
                r#"select r.number, r.supplier_doc, sp.name as "supplier?",
                          (select count(*) from receipt_lines l where l.receipt_id = r.id) as "lines!",
                          exists (select 1 from receipt_acceptances x where x.receipt_id = r.id) as "accepted!"
                   from receipts r left join suppliers sp on sp.id = r.supplier_id
                   where r.id = $1 and r.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            let Some(r) = r.filter(|r| !r.accepted) else {
                return Ok(None);
            };
            (
                format!("📦 Приход № {}", r.number),
                format!(
                    "{}{} — {}",
                    r.supplier.unwrap_or_else(|| "без поставщика".into()),
                    doc_tail(&r.supplier_doc),
                    lines_word(r.lines)
                ),
            )
        }
        "receipt.accept" => {
            let cashier = Some(text_of(data, "cashier"))
                .filter(|c| !c.is_empty())
                .unwrap_or_else(|| "кассир".into());
            (
                format!("📦 Приход № {} принял {cashier}", num_of(data, "number")),
                format!(
                    "{}{} — {}",
                    text_of(data, "supplier"),
                    doc_tail(&text_of(data, "doc")),
                    lines_word(num_of(data, "lines"))
                ),
            )
        }
        "acceptance.refuse" => (
            format!("✋ Отказ от приёмки: {}", text_of(data, "supplier")),
            format!(
                "накладная{} — {}: «{}»",
                Some(text_of(data, "doc"))
                    .filter(|d| !d.is_empty())
                    .map(|d| format!(" № {d}"))
                    .unwrap_or_default(),
                Some(text_of(data, "cashier"))
                    .filter(|c| !c.is_empty())
                    .unwrap_or_else(|| "кассир".into()),
                text_of(data, "reason")
            ),
        ),
        "defect.mark" => (
            format!(
                "🧯 В брак: {} × {}",
                text_of(data, "name"),
                text_of(data, "qty_text")
            ),
            text_of(data, "reason"),
        ),
        "stock_in.post" => {
            let goods = match data.get("lines").and_then(Value::as_array) {
                Some(lines) => lines
                    .iter()
                    .map(|l| format!("{} × {}", text_of(l, "name"), text_of(l, "qty_text")))
                    .collect::<Vec<_>>()
                    .join(", "),
                None => format!("{} × {}", text_of(data, "name"), text_of(data, "qty_text")),
            };
            (
                format!("📥 Оприходование № {}", num_of(data, "number")),
                goods,
            )
        }
        "expense.post" => {
            let Some(id) = entity_id else { return Ok(None) };
            let e = sqlx::query!(
                r#"select e.number, e.amount_tyiyn, e.comment, e.payee, x.name as article,
                          x.owner_only as article_owner, c.name as "account?",
                          coalesce(c.kind = 'register' and c.is_default and not c.owner_only, false) as "till!"
                   from expenses e
                   join expense_articles x on x.id = e.article_id
                   left join cash_accounts c on c.id = e.account_id
                   where e.id = $1 and e.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            // Только расходы из кассы смены — то, что проводит и видит администратор.
            let Some(e) = e.filter(|e| e.till && !e.article_owner) else {
                return Ok(None);
            };
            let mut details = format!(
                "{} из «{}»",
                format_som(e.amount_tyiyn),
                e.account.unwrap_or_default()
            );
            if !e.payee.trim().is_empty() {
                details.push_str(&format!(" · кому: {}", e.payee.trim()));
            }
            if !e.comment.trim().is_empty() {
                details.push_str(&format!(" — {}", e.comment.trim()));
            }
            (format!("💸 Расход № {}: {}", e.number, e.article), details)
        }
        "debt.repayment" => {
            let Some(id) = entity_id else { return Ok(None) };
            let p = sqlx::query!("select name, role from parties where id = $1", id)
                .fetch_optional(&mut *conn)
                .await?;
            // Оплаты поставщикам — не администратору.
            let Some(p) = p.filter(|p| p.role == "customer") else {
                return Ok(None);
            };
            let rest = match num_of(data, "balance_tyiyn") {
                b if b > 0 => format!("осталось {}", format_som(b)),
                0 => "долг закрыт".to_string(),
                b => format!("аванс клиента {}", format_som(-b)),
            };
            (
                format!(
                    "💰 Клиент {} погасил {}",
                    p.name,
                    som_of(data, "amount_tyiyn")
                ),
                rest,
            )
        }
        "debt.old" => {
            let date = text_of(data, "date");
            let date = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .map(|d| d.format("%d.%m.%Y").to_string())
                .unwrap_or(date);
            (
                format!(
                    "📒 Старый долг: {} — {} от {date}",
                    text_of(data, "name"),
                    som_of(data, "amount_tyiyn")
                ),
                format!("долг сейчас {}", som_of(data, "balance_tyiyn")),
            )
        }
        _ => return Ok(None),
    }))
}

// ---------- Экран «Пользователи → Телеграм» ----------

#[derive(Serialize)]
struct ItemOut {
    key: &'static str,
    button: &'static str,
    what: &'static str,
    on: bool,
    /// Чего не хватает: «Нет права: …».
    #[serde(skip_serializing_if = "Option::is_none")]
    need: Option<String>,
}

#[derive(Serialize)]
struct EventOut {
    action: &'static str,
    label: &'static str,
    on: bool,
    /// Право события есть: без него включённое не приходит.
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    need: Option<String>,
}

#[derive(Serialize)]
struct GroupOut {
    key: &'static str,
    title: &'static str,
    events: Vec<EventOut>,
}

#[derive(Serialize)]
struct BotOut {
    linked: bool,
    linked_at: Option<DateTime<Utc>>,
    /// Что видит администратор в боте: пункты меню по его правам.
    items: Vec<ItemOut>,
    on: Vec<String>,
    groups: Vec<GroupOut>,
}

#[derive(Deserialize)]
struct FeedIn {
    op_id: Uuid,
    #[serde(default)]
    on: Vec<String>,
}

/// Администратор филиала владельца (I16): чужой филиал — как несуществующий.
async fn branch_admin(
    conn: &mut PgConnection,
    owner: &CurrentUser,
    id: Uuid,
) -> AppResult<CurrentUser> {
    let u = sqlx::query!(
        "select id, branch_id, login, full_name, role from users where id = $1 and branch_id = $2",
        id,
        owner.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if u.role != "admin" {
        return Err(invalid(
            "бот владельца настраивается в «Настройки → Уведомления в бот»",
        ));
    }
    Ok(CurrentUser {
        id: u.id,
        branch_id: u.branch_id,
        login: u.login,
        full_name: u.full_name,
        role: Role::Admin,
    })
}

async fn bot_out(conn: &mut PgConnection, admin: &CurrentUser) -> AppResult<BotOut> {
    let linked_at = sqlx::query_scalar!(
        "select linked_at from telegram_links where user_id = $1 order by linked_at desc limit 1",
        admin.id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let mut items = Vec::with_capacity(ITEMS.len());
    for i in &ITEMS {
        let need = missing(conn, admin, i).await?;
        items.push(ItemOut {
            key: i.key,
            button: i.button,
            what: i.what,
            on: need.is_none(),
            need,
        });
    }
    let feed = load_feed(conn, admin.id).await?;
    let mut groups = Vec::with_capacity(GROUPS.len());
    for (key, title) in GROUPS {
        let mut events = Vec::new();
        for e in EVENTS.iter().filter(|e| e.group == key) {
            let need = match e.right {
                Some(r) => {
                    let a = crate::auth::access(conn, admin, r).await?;
                    // Право, снятое у кассира смены, ленту не закрывает: лента — входу.
                    (a == Access::NoRight).then(|| denied_text(&a, r))
                }
                None => None,
            };
            events.push(EventOut {
                action: e.action,
                label: e.label,
                on: feed.on.iter().any(|o| o == e.action),
                allowed: need.is_none(),
                need,
            });
        }
        groups.push(GroupOut { key, title, events });
    }
    Ok(BotOut {
        linked: linked_at.is_some(),
        linked_at,
        items,
        on: feed.on,
        groups,
    })
}

/// «Пользователи → Телеграм»: привязка, что видит администратор в боте и его лента. Только
/// владелец.
async fn get_bot(
    State(state): State<AppState>,
    Owner(user): Owner,
    Path(id): Path<Uuid>,
) -> AppResult<Json<BotOut>> {
    let mut conn = state.pool.acquire().await?;
    let admin = branch_admin(&mut conn, &user, id).await?;
    Ok(Json(bot_out(&mut conn, &admin).await?))
}

/// Лента администратора: что из [`EVENTS`] ему приходит. Только владелец; `op_id` (I8), журнал
/// `user.bot_feed` — что включено и выключено (I5).
async fn put_feed(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<FeedIn>,
) -> AppResult<Json<AdminFeed>> {
    ctx.user.require_owner()?;
    const KIND: &str = "user.bot_feed";
    let mut tx = state.pool.begin().await?;
    let admin = branch_admin(&mut tx, &ctx.user, id).await?;
    let on = normalize(&req.on)?;
    if let Some(done) = ops::begin_op::<AdminFeed>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load_feed(&mut tx, admin.id).await?;
    let now = sqlx::query_scalar!(r#"select now() as "n!""#)
        .fetch_one(&mut *tx)
        .await?;
    let feed = turned_on(&before, on, now);
    let value = serde_json::to_value(&feed).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, $2, $3)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        admin.branch_id,
        feed_key(admin.id),
        value
    )
    .execute(&mut *tx)
    .await?;
    if before.on != feed.on {
        let added: Vec<&String> = feed.on.iter().filter(|a| !before.on.contains(a)).collect();
        let removed: Vec<&String> = before.on.iter().filter(|a| !feed.on.contains(a)).collect();
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "user",
            Some(admin.id),
            json!({ "full_name": admin.full_name, "on": added, "off": removed }),
        )
        .await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &feed).await?;
    tx.commit().await?;
    Ok(Json(feed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::notifications::WATCHED;
    use crate::api::telegram::FEED;

    #[test]
    fn admin_events_are_feed_events_without_owner_money() {
        let mut seen = Vec::new();
        for e in &EVENTS {
            assert!(
                WATCHED.contains(&e.action) || FEED.contains(&e.action),
                "{} не событие ленты",
                e.action
            );
            assert!(GROUPS.iter().any(|(g, _)| *g == e.group), "{}", e.action);
            assert!(!leaks(e.label), "{}", e.label);
            if let Some(r) = e.right {
                assert!(crate::auth::permission_def(r).is_some(), "{r}");
            }
            assert!(!seen.contains(&e.action), "повтор {}", e.action);
            seen.push(e.action);
        }
        // Чеки с прибылью, ревизия с оценкой, перелив с себестоимостью, сводка — не администратору.
        for owner_only in [
            "sale.post",
            "sale.return",
            "sale.below_cost",
            "sale.cost_unknown",
            "revision.post",
            "oil.transfer_admin",
            "receipt.cost_above_price",
            "product.prices",
            "payout.post",
        ] {
            assert!(event_def(owner_only).is_none(), "{owner_only}");
        }
    }

    #[test]
    fn menu_rights_are_known_and_buttons_unique() {
        for i in &ITEMS {
            for r in i.rights {
                assert!(crate::auth::permission_def(r).is_some(), "{r}");
            }
            assert!(!leaks(i.what), "{}", i.what);
        }
        let all = Menu::all();
        assert_eq!(all.items().count(), ITEMS.len());
        assert_eq!(
            all.rows(),
            vec![
                vec![BTN_INVOICE, BTN_MY_INVOICES],
                vec![BTN_SHIFT_NOW, BTN_STOCK],
                vec![BTN_DEBTORS, BTN_ADD_DEBT],
                vec![BTN_EXPENSE, BTN_BOOK],
            ]
        );
        assert!(Menu::default().items().next().is_none());
        assert_eq!(item_of("/остаток масло").map(|i| i.key), Some("stock"));
        assert_eq!(item_of("книжка").map(|i| i.key), Some("book"));
        assert_eq!(item_of(BTN_SHIFT_NOW).map(|i| i.key), Some("shift"));
        assert!(item_of("масло моторное").is_none());
    }

    #[test]
    fn guard_cuts_owner_money_lines() {
        assert!(leaks("валовая прибыль: 100 с"));
        assert!(leaks("Себестоимость 50 с"));
        assert!(leaks("маржа 20 %"));
        assert!(leaks("чистая: 5 с"));
        assert!(!leaks("интервалов замены"));
        assert!(!leaks("Чистка инжектора"));
        assert_eq!(
            scrub("Выручка 10 с\nприбыль 5 с\nчеков 2"),
            "Выручка 10 с\nчеков 2"
        );
        let on = normalize(&[
            "receipt.post".into(),
            "shift.open".into(),
            "shift.open".into(),
        ])
        .unwrap();
        assert_eq!(on, vec!["shift.open", "receipt.post"]);
        assert!(normalize(&["sale.post".into()]).is_err());
        // Время включения: прежнее у оставшихся событий, новое — у включённых сейчас.
        let t0 = DateTime::<Utc>::from_timestamp(1_000, 0).unwrap();
        let t1 = DateTime::<Utc>::from_timestamp(2_000, 0).unwrap();
        let a = turned_on(&AdminFeed::default(), on, t0);
        assert_eq!(a.from.get("shift.open"), Some(&t0));
        let b = turned_on(&a, vec!["shift.open".into(), "debt.old".into()], t1);
        assert_eq!(
            (b.from.get("shift.open"), b.from.get("debt.old")),
            (Some(&t0), Some(&t1))
        );
        assert!(!b.from.contains_key("receipt.post"));
        // Выключили и включили снова — с нового времени.
        let c = turned_on(&b, vec!["debt.old".into()], t1);
        let d = turned_on(&c, vec!["debt.old".into(), "shift.open".into()], t1);
        assert_eq!(d.from.get("shift.open"), Some(&t1));
    }
}
