//! Должники в боте владельца (задача 002, П8). «📋 Должники» — кто сколько должен филиалу чата,
//! с просрочкой, постранично. «➕ Занести долг» — старый долг из тетради по шагам: клиент (поиск
//! по имени или телефону, или новый), сумма, дата долга, примечание → «✅ Провести». Проводит та
//! же функция, что окно «Занести старый долг» (SPEC-38, `parties::post_old_debt_tx`): право
//! `old_debts` у администратора, `op_id` черновика (повтор нажатия — та же запись), журнал
//! `debt.old`. Ожидание ввода — `bot_input` вида `old_debt` на 30 минут; любая кнопка внизу или
//! `/команда` его отменяет.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::api::parties::{ListQuery, NewPartyReq, OldDebtReq, search_parties};
use crate::api::telegram::{Button, Keys, Reply};
use crate::auth::{Ctx, CurrentUser, Role};
use crate::domain::money::{format_som, parse_spoken_som};
use crate::error::{AppError, AppResult};

/// Кнопки внизу чата владельца (и администратора с правом «Старые долги из тетради»).
pub const BTN_DEBTORS: &str = "📋 Должники";
pub const BTN_ADD_DEBT: &str = "➕ Занести долг";

/// Должников на странице списка.
pub const PAGE: usize = 10;
/// Клиентов в ответе на поиск.
const FOUND: usize = 6;
/// Ожидание ответа на шаг, минут.
const WAIT_MINUTES: i32 = 30;

const ASK_WHO: &str = "➕ Занести долг (старый долг из тетради).\n\nКто должен? Напишите имя, фамилию или телефон клиента.";
const ASK_PERSON: &str =
    "Новый клиент — физлицо. Напишите фамилию и имя через пробел (например: Иванов Пётр).";
const ASK_COMPANY: &str = "Новый клиент — фирма. Напишите название (например: ОсОО Бишкек такси).";
const ASK_PHONE: &str = "Телефон клиента (например: 0555 12 34 56) — или «Пропустить».";
const ASK_AMOUNT: &str = "Сумма долга, сом (например: 5 000 или 1 250,50).";
const ASK_DATE: &str = "Дата долга — когда взял, ДД.ММ.ГГГГ (например: 15.03.2026). От неё считаются срок оплаты и просрочка.";
const ASK_COMMENT: &str =
    "Примечание: что брал, страница тетради (до 500 знаков) — или «Пропустить».";
const STALE: &str = "Кнопка устарела. Начать заново — «➕ Занести долг».";
const NO_RIGHT: &str = "Нет права «Старые долги из тетради» — его выдаёт владелец.";

/// Шаги ввода: что бот ждёт следующим.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
struct Dialog {
    step: String,
    /// Операция (I8): повтор «✅ Провести» не заносит долг второй раз.
    op_id: Uuid,
    #[serde(default)]
    party_id: Option<Uuid>,
    #[serde(default)]
    party_name: String,
    #[serde(default)]
    new_party: Option<NewPartyReq>,
    #[serde(default)]
    amount: i64,
    #[serde(default)]
    date: Option<NaiveDate>,
    #[serde(default)]
    comment: String,
}

fn button(text: impl Into<String>, data: impl Into<String>) -> Button {
    Button {
        text: text.into(),
        data: data.into(),
    }
}

fn reply(text: impl Into<String>, rows: Vec<Vec<Button>>) -> Reply {
    let mut r = Reply::new(text, Keys::Owner);
    r.inline = rows;
    r
}

fn day(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

/// Кто ведёт долги в этом чате: активный владелец (филиал, выбранный в чате) или администратор
/// своего филиала с правом «Старые долги из тетради»; второе — кто в Телеграме привязывал чат.
pub async fn chat_user(
    conn: &mut PgConnection,
    chat_id: i64,
) -> AppResult<Option<(CurrentUser, Option<i64>)>> {
    let Some((user, tg)) = crate::api::receipt_drafts::chat_sender(&mut *conn, chat_id).await?
    else {
        return Ok(None);
    };
    if user.role == Role::Owner
        || crate::auth::has_permission(&mut *conn, &user, "old_debts").await?
    {
        return Ok(Some((user, tg)));
    }
    Ok(None)
}

async fn pending(conn: &mut PgConnection, chat_id: i64) -> AppResult<Option<Dialog>> {
    let v = sqlx::query_scalar!(
        "select data from bot_input where chat_id = $1 and kind = 'old_debt' and expires_at > now()",
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

async fn save(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    d: &Dialog,
) -> AppResult<()> {
    let data = serde_json::to_value(d).unwrap_or(Value::Null);
    sqlx::query!(
        r#"insert into bot_input (chat_id, branch_id, kind, data, expires_at)
           values ($1, $2, 'old_debt', $3, now() + make_interval(mins => $4))
           on conflict (chat_id) do update set branch_id = excluded.branch_id, kind = excluded.kind,
               vehicle_id = null, data = excluded.data, expires_at = excluded.expires_at"#,
        chat_id,
        user.branch_id,
        data,
        WAIT_MINUTES
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Отменить ввод долга в чате (кнопка внизу, команда, «✖ Отмена»).
pub async fn clear(conn: &mut PgConnection, chat_id: i64) -> AppResult<()> {
    sqlx::query!(
        "delete from bot_input where chat_id = $1 and kind = 'old_debt'",
        chat_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------- «📋 Должники» ----------

/// Страница списка должников филиала: имя, долг в филиале, просрочка; кнопки «◀ ▶» и
/// «➕ Занести долг».
pub async fn list(conn: &mut PgConnection, user: &CurrentUser, page: usize) -> AppResult<Reply> {
    let rows = crate::api::parties::debts_list_in(&mut *conn, user.branch_id, false).await?;
    let mut debtors: Vec<_> = rows
        .into_iter()
        .filter(|r| r.party.role == "customer" && r.party.branch_balance_tyiyn > 0)
        .collect();
    debtors.sort_by(|a, b| {
        b.party
            .branch_balance_tyiyn
            .cmp(&a.party.branch_balance_tyiyn)
            .then_with(|| a.party.name.cmp(&b.party.name))
    });
    let add = vec![button(BTN_ADD_DEBT, "od:start")];
    if debtors.is_empty() {
        return Ok(reply("📋 Должников нет — нам никто не должен.", vec![add]));
    }
    let total = debtors.iter().fold(0i64, |acc, r| {
        acc.saturating_add(r.party.branch_balance_tyiyn)
    });
    let pages = debtors.len().div_ceil(PAGE);
    let page = page.min(pages.saturating_sub(1));
    let mut lines = vec![format!(
        "📋 Должники: {}, всего {}{}",
        debtors.len(),
        format_som(total),
        if pages > 1 {
            format!(" · стр. {} из {pages}", page + 1)
        } else {
            String::new()
        }
    )];
    for (i, r) in debtors.iter().enumerate().skip(page * PAGE).take(PAGE) {
        let mut line = format!(
            "{}. {} — {}",
            i + 1,
            r.party.name,
            format_som(r.party.branch_balance_tyiyn)
        );
        if r.overdue_days > 0 && r.party.overdue_tyiyn > 0 {
            line.push_str(&format!(
                " · просрочка {} дн. ({})",
                r.overdue_days,
                format_som(r.party.overdue_tyiyn)
            ));
        }
        lines.push(line);
    }
    let mut nav = Vec::new();
    if page > 0 {
        nav.push(button("◀ Назад", format!("dl:{}", page - 1)));
    }
    if page + 1 < pages {
        nav.push(button("Дальше ▶", format!("dl:{}", page + 1)));
    }
    let mut rows = Vec::new();
    if !nav.is_empty() {
        rows.push(nav);
    }
    rows.push(add);
    Ok(reply(lines.join("\n"), rows))
}

// ---------- «➕ Занести долг» ----------

async fn start(conn: &mut PgConnection, user: &CurrentUser, chat_id: i64) -> AppResult<Reply> {
    let d = Dialog {
        step: "who".into(),
        op_id: crate::ops::new_id(),
        ..Dialog::default()
    };
    save(conn, user, chat_id, &d).await?;
    Ok(reply(ASK_WHO, vec![vec![button("✖ Отмена", "od:no")]]))
}

/// Вопрос текущего шага (и его кнопки).
fn ask(d: &Dialog) -> Reply {
    let cancel = button("✖ Отмена", "od:no");
    match d.step.as_str() {
        "new_person" => reply(ASK_PERSON, vec![vec![cancel]]),
        "new_company" => reply(ASK_COMPANY, vec![vec![cancel]]),
        "phone" => reply(
            ASK_PHONE,
            vec![vec![button("Пропустить", "od:skip"), cancel]],
        ),
        "amount" => reply(
            format!("Клиент: {}.\n\n{ASK_AMOUNT}", d.party_name),
            vec![vec![cancel]],
        ),
        "date" => reply(ASK_DATE, vec![vec![button("Сегодня", "od:today"), cancel]]),
        "comment" => reply(
            ASK_COMMENT,
            vec![vec![button("Пропустить", "od:skip"), cancel]],
        ),
        _ => reply(ASK_WHO, vec![vec![cancel]]),
    }
}

/// Дата долга: «15.03.2026», «15.03.26», «15.03» (этот год), «2026-03-15».
pub fn parse_date(s: &str, today: NaiveDate) -> Option<NaiveDate> {
    let s = s.trim().trim_end_matches('.');
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    let parts: Vec<&str> = s
        .split(['.', '/', '-', ' '])
        .filter(|p| !p.is_empty())
        .collect();
    let num = |p: &str| p.parse::<u32>().ok();
    match parts.as_slice() {
        [dd, mm] => NaiveDate::from_ymd_opt(chrono::Datelike::year(&today), num(mm)?, num(dd)?),
        [dd, mm, yy] => {
            let y = yy.parse::<i32>().ok()?;
            let y = if yy.len() == 2 { 2000 + y } else { y };
            NaiveDate::from_ymd_opt(y, num(mm)?, num(dd)?)
        }
        _ => None,
    }
}

/// Итог перед проведением: кто, сколько, дата, примечание, как изменится долг.
async fn summary(conn: &mut PgConnection, d: &Dialog) -> AppResult<Reply> {
    let mut lines = vec!["Занести старый долг?".to_string()];
    let before = match d.party_id {
        Some(id) => {
            sqlx::query_scalar!("select balance_tyiyn from parties where id = $1", id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let who = if d.new_party.is_some() {
        format!("{} (новый клиент)", d.party_name)
    } else {
        d.party_name.clone()
    };
    lines.push(format!("Клиент: {who}"));
    if let Some(np) = d
        .new_party
        .as_ref()
        .filter(|np| !np.phone.trim().is_empty())
    {
        lines.push(format!("Телефон: {}", np.phone.trim()));
    }
    lines.push(format!("Сумма: {}", format_som(d.amount)));
    if let Some(date) = d.date {
        lines.push(format!("Дата долга: {}", day(date)));
        let today = crate::api::fees::today(&mut *conn).await?;
        if (today - date).num_days() > 365 {
            lines.push("⚠ Долгу больше года — проверьте год.".into());
        }
    }
    if !d.comment.is_empty() {
        lines.push(format!("Примечание: {}", d.comment));
    }
    let before = before.unwrap_or(0);
    lines.push(format!(
        "Долг клиента сейчас {} → станет {}",
        format_som(before),
        format_som(before.saturating_add(d.amount))
    ));
    Ok(reply(
        lines.join("\n"),
        vec![vec![
            button("✅ Провести", "od:go"),
            button("✖ Отмена", "od:no"),
        ]],
    ))
}

/// Следующий шаг после выбора клиента или ввода поля.
async fn next(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    mut d: Dialog,
    step: &str,
) -> AppResult<Reply> {
    d.step = step.to_string();
    save(conn, user, chat_id, &d).await?;
    if step == "confirm" {
        return summary(conn, &d).await;
    }
    Ok(ask(&d))
}

/// Ответ на поиск клиента: найденные кнопками и «новый клиент».
async fn found(conn: &mut PgConnection, user: &CurrentUser, q: &str) -> AppResult<Reply> {
    let rows = search_parties(
        &mut *conn,
        user.branch_id,
        ListQuery {
            role: Some("customer".into()),
            q: Some(q.to_string()),
            ..ListQuery::default()
        },
    )
    .await?;
    let hits: Vec<_> = rows.into_iter().filter(|p| p.active).take(FOUND).collect();
    let mut buttons: Vec<Vec<Button>> = hits
        .iter()
        .map(|p| {
            let mut t = p.name.clone();
            if !p.phone.is_empty() {
                t.push_str(&format!(", {}", p.phone));
            }
            if p.balance_tyiyn > 0 {
                t.push_str(&format!(" · долг {}", format_som(p.balance_tyiyn)));
            }
            vec![button(t, format!("od:p:{}", p.id.simple()))]
        })
        .collect();
    buttons.push(vec![
        button("👤 Новый: физлицо", "od:np"),
        button("🏢 Новый: фирма", "od:nc"),
    ]);
    buttons.push(vec![button("✖ Отмена", "od:no")]);
    let text = if hits.is_empty() {
        format!("Не нашли «{q}». Напишите иначе или заведите нового клиента.")
    } else {
        "Выберите клиента — или напишите иначе, или заведите нового:".into()
    };
    Ok(reply(text, buttons))
}

/// Текст на шаге ввода долга. `None` — ввода нет, или это не владелец/администратор с правом:
/// дальше решает остальной бот. `menu` — кнопка внизу или `/команда`: ввод отменяется.
pub async fn on_message(
    conn: &mut PgConnection,
    chat_id: i64,
    text: Option<&str>,
    menu: bool,
) -> AppResult<Option<Reply>> {
    let t = text.map(str::trim).unwrap_or("");
    if t == BTN_DEBTORS || t == BTN_ADD_DEBT || t == "/должники" {
        let Some((user, _)) = chat_user(conn, chat_id).await? else {
            return Ok(Some(Reply::new(NO_RIGHT, Keys::Owner)));
        };
        clear(conn, chat_id).await?;
        return Ok(Some(if t == BTN_ADD_DEBT {
            start(conn, &user, chat_id).await?
        } else {
            list(conn, &user, 0).await?
        }));
    }
    let Some(mut d) = pending(conn, chat_id).await? else {
        return Ok(None);
    };
    if menu {
        clear(conn, chat_id).await?;
        return Ok(None);
    }
    let Some((user, _)) = chat_user(conn, chat_id).await? else {
        clear(conn, chat_id).await?;
        return Ok(None);
    };
    if t.is_empty() {
        return Ok(Some(ask(&d)));
    }
    let r = match d.step.as_str() {
        "who" => found(conn, &user, t).await?,
        "new_person" => {
            let mut words = t.split_whitespace();
            let last = words.next().unwrap_or("").to_string();
            let first = words.collect::<Vec<_>>().join(" ");
            if first.is_empty() {
                return Ok(Some(reply(
                    "Нужны фамилия и имя через пробел, например: Иванов Пётр.",
                    vec![vec![button("✖ Отмена", "od:no")]],
                )));
            }
            d.party_name = format!("{last} {first}");
            d.new_party = Some(NewPartyReq {
                kind: "person".into(),
                first_name: first,
                last_name: last,
                ..NewPartyReq::default()
            });
            next(conn, &user, chat_id, d, "phone").await?
        }
        "new_company" => {
            d.party_name = t.to_string();
            d.new_party = Some(NewPartyReq {
                kind: "company".into(),
                name: t.to_string(),
                ..NewPartyReq::default()
            });
            next(conn, &user, chat_id, d, "phone").await?
        }
        "phone" => {
            if let Some(np) = d.new_party.as_mut() {
                np.phone = t.to_string();
            }
            next(conn, &user, chat_id, d, "amount").await?
        }
        "amount" => match parse_spoken_som(t).filter(|v| *v > 0) {
            Some(v) if crate::ops::check_amount(v).is_ok() => {
                d.amount = v;
                next(conn, &user, chat_id, d, "date").await?
            }
            _ => reply(
                format!(
                    "Сумму «{t}» не разобрать: нужны цифры больше нуля, например 5 000 или 1 250,50."
                ),
                vec![vec![button("✖ Отмена", "od:no")]],
            ),
        },
        "date" => {
            let today = crate::api::fees::today(&mut *conn).await?;
            match parse_date(t, today) {
                Some(date) => match crate::api::parties::check_old_debt_date(date, today) {
                    Ok(()) => {
                        d.date = Some(date);
                        next(conn, &user, chat_id, d, "comment").await?
                    }
                    Err(e) => reply(
                        format!("Не подходит: {e}. {ASK_DATE}"),
                        vec![vec![
                            button("Сегодня", "od:today"),
                            button("✖ Отмена", "od:no"),
                        ]],
                    ),
                },
                None => reply(
                    format!("Дату «{t}» не разобрать. {ASK_DATE}"),
                    vec![vec![
                        button("Сегодня", "od:today"),
                        button("✖ Отмена", "od:no"),
                    ]],
                ),
            }
        }
        "comment" => {
            if t.chars().count() > 500 {
                reply(
                    "Примечание — не длиннее 500 знаков.",
                    vec![vec![
                        button("Пропустить", "od:skip"),
                        button("✖ Отмена", "od:no"),
                    ]],
                )
            } else {
                d.comment = crate::api::bot_drafts::one_line(t);
                next(conn, &user, chat_id, d, "confirm").await?
            }
        }
        // Итог показан — ждём кнопку; текст повторяет итог.
        _ => summary(conn, &d).await?,
    };
    Ok(Some(r))
}

/// Кнопки под сообщениями: `dl:<страница>` — список должников, `od:…` — шаги ввода долга.
pub async fn callback(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    data: &str,
) -> AppResult<Reply> {
    let Some((user, tg)) = chat_user(conn, chat_id).await? else {
        return Ok(Reply::new(NO_RIGHT, Keys::Owner));
    };
    if let Some(p) = data.strip_prefix("dl:") {
        return list(conn, &user, p.parse().unwrap_or(0)).await;
    }
    if data == "od:start" {
        clear(conn, chat_id).await?;
        return start(conn, &user, chat_id).await;
    }
    let Some(mut d) = pending(conn, chat_id).await? else {
        return Ok(Reply::new(STALE, Keys::Owner));
    };
    match data {
        "od:no" => {
            clear(conn, chat_id).await?;
            Ok(Reply::new("Отменено — долг не занесён.", Keys::Owner))
        }
        "od:np" => next(conn, &user, chat_id, d, "new_person").await,
        "od:nc" => next(conn, &user, chat_id, d, "new_company").await,
        "od:skip" if d.step == "phone" => next(conn, &user, chat_id, d, "amount").await,
        "od:skip" if d.step == "comment" => {
            d.comment.clear();
            next(conn, &user, chat_id, d, "confirm").await
        }
        "od:today" if d.step == "date" => {
            d.date = Some(crate::api::fees::today(&mut *conn).await?);
            next(conn, &user, chat_id, d, "comment").await
        }
        // Провести — только тот, кто привязал чат (ADR-070).
        "od:go" | "od:dup" if d.step == "confirm" => {
            if tg.is_some_and(|t| t != from_id) {
                return Ok(Reply::new(
                    "Провести может только тот, кто привязал этот чат.",
                    Keys::Owner,
                ));
            }
            post(conn, &user, chat_id, &d, data == "od:dup").await
        }
        _ => match data.strip_prefix("od:p:").map(Uuid::parse_str) {
            Some(Ok(id)) if d.step == "who" => {
                let p = sqlx::query!(
                    "select name from parties where id = $1 and role = 'customer' and active",
                    id
                )
                .fetch_optional(&mut *conn)
                .await?;
                let Some(p) = p else {
                    return Ok(reply(
                        "Клиент не найден или отключён. Напишите имя ещё раз.",
                        vec![vec![button("✖ Отмена", "od:no")]],
                    ));
                };
                d.party_id = Some(id);
                d.party_name = p.name;
                d.new_party = None;
                next(conn, &user, chat_id, d, "amount").await
            }
            _ => Ok(ask(&d)),
        },
    }
}

/// «✅ Провести»: старый долг одной транзакцией (новый клиент — в той же). Повтор долга (та же
/// сумма и дата у клиента) — вопрос «Занести ещё раз?».
async fn post(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    d: &Dialog,
    confirm_duplicate: bool,
) -> AppResult<Reply> {
    let Some(date) = d.date else {
        return Ok(ask(d));
    };
    let req = OldDebtReq {
        op_id: d.op_id,
        party_id: d.party_id,
        new_party: d.new_party.clone(),
        amount_tyiyn: d.amount,
        date,
        comment: d.comment.clone(),
        confirm_duplicate,
    };
    let ctx = Ctx::bot(user.clone(), chat_id);
    let mut tx = conn.begin().await?;
    let out = match crate::api::parties::post_old_debt_tx(&mut tx, &ctx, req).await {
        Ok(out) => out,
        Err(AppError::Conflict(why)) => {
            tx.rollback().await?;
            return Ok(reply(
                why,
                vec![vec![
                    button("Да, занести ещё раз", "od:dup"),
                    button("✖ Отмена", "od:no"),
                ]],
            ));
        }
        Err(
            e @ (AppError::Validation(_)
            | AppError::Forbidden
            | AppError::ForbiddenWith(_)
            | AppError::NotFound),
        ) => {
            tx.rollback().await?;
            return Ok(reply(
                format!("Не получилось: {e}."),
                vec![vec![button("✖ Отмена", "od:no")]],
            ));
        }
        Err(e) => {
            tx.rollback().await?;
            return Err(e);
        }
    };
    tx.commit().await?;
    clear(conn, chat_id).await?;
    let mut text = format!(
        "✅ Занесено: {} — {} от {} (из тетради). Долг клиента — {}.",
        d.party_name,
        format_som(d.amount),
        day(date),
        format_som(out.balance_tyiyn)
    );
    if let Some(due) = out.due_date {
        text.push_str(&format!("\nСрок оплаты — {}", day(due)));
        if out.overdue_days > 0 {
            text.push_str(&format!(", просрочен на {} дн.", out.overdue_days));
        }
    }
    Ok(reply(text, vec![vec![button("➕ Ещё долг", "od:start")]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debt_dates_as_people_write_them() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 31).unwrap_or_default();
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd);
        assert_eq!(parse_date("15.03.2026", today), d(2026, 3, 15));
        assert_eq!(parse_date("15.03.26", today), d(2026, 3, 15));
        assert_eq!(parse_date("15.03", today), d(2026, 3, 15));
        assert_eq!(parse_date("2026-03-15", today), d(2026, 3, 15));
        assert_eq!(parse_date("5/3/2026", today), d(2026, 3, 5));
        assert_eq!(parse_date("31.02.2026", today), None);
        assert_eq!(parse_date("вчера", today), None);
    }
}
