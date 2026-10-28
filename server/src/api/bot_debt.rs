//! Долг клиента в боте (SPEC-27 §7): кнопка «💳 Мой долг» и напоминания о сроке.
//!
//! Сумма, срок и записи — из тех же данных, что карточка клиента (SPEC-10): баланс и просрочка
//! карточки (`load_party`), срок — `debt_dues`, записи — журнал долга `party_ledger`. Напоминания
//! уходят в окно рассылок 09:00–20:00 (ADR-063): за 3 дня до срока и в день срока, каждое
//! один раз (`debt_reminders`). Фирмы к боту не подключаются (ADR-063), поэтому их здесь нет.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::domain::money::format_som;
use crate::error::AppResult;

/// Кнопка клиента внизу чата.
pub const BTN_DEBT: &str = "💳 Мой долг";

/// Записей долга в ответе — последние.
pub const LAST_ENTRIES: i64 = 10;

/// Бишкек — UTC+6 круглый год.
fn local_day(t: DateTime<Utc>) -> String {
    (t + chrono::Duration::hours(6))
        .format("%d.%m.%Y")
        .to_string()
}

fn day(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

/// Запись долга для клиента: «взяли в долг», «оплатили» — суммой без знака, как в карточке.
fn entry_title(kind: &str, amount: i64, reversal: bool) -> &'static str {
    match (kind, reversal) {
        (_, true) => "исправлено",
        ("debt", _) if amount > 0 => "взяли в долг",
        ("debt", _) => "долг уменьшен возвратом",
        ("repayment", _) => "оплатили",
        _ if amount > 0 => "добавлено к долгу",
        _ => "списано с долга",
    }
}

/// Ответ на «💳 Мой долг»: сумма, срок и последние покупки в долг и оплаты.
pub async fn customer_debt(conn: &mut PgConnection, party_id: Uuid) -> AppResult<String> {
    let branch_id = sqlx::query_scalar!("select branch_id from parties where id = $1", party_id)
        .fetch_one(&mut *conn)
        .await?;
    let party = crate::api::parties::load_party(&mut *conn, branch_id, party_id).await?;
    // Клиент общий: долг и записи — по всем филиалам, у записи — филиал (SPEC-33 §2).
    let several = crate::api::branches::several(&mut *conn).await?;
    let entries = sqlx::query!(
        r#"select l.kind, l.amount_tyiyn, l.created_at, l.reversal_of is not null as "reversal!",
                  s.number as "sale_number?", b.name as branch_name, l.doc_type, l.business_date
           from party_ledger l
           join branches b on b.id = l.branch_id
           left join sales s on l.doc_type = 'sale' and s.id = l.doc_id
           where l.party_id = $1 and l.amount_tyiyn <> 0
           order by l.created_at desc limit $2"#,
        party_id,
        LAST_ENTRIES
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut lines = Vec::new();
    if party.balance_tyiyn > 0 {
        lines.push(format!("💳 Ваш долг: {}", format_som(party.balance_tyiyn)));
        // Долг в нескольких филиалах — по филиалам; гасить можно в любом.
        if let Some(list) = party.branch_balances.as_array().filter(|l| l.len() > 1) {
            for b in list {
                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let v = b
                    .get("balance_tyiyn")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                lines.push(format!("  • {name}: {}", format_som(v)));
            }
        }
        let dues = crate::api::parties::debt_dues(
            &mut *conn,
            &[(party.id, party.balance_tyiyn, party.due_days)],
        )
        .await?;
        if let Some(due) = dues.get(&party.id).and_then(|d| d.due_date) {
            let today =
                sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                    .fetch_one(&mut *conn)
                    .await?;
            let left = (due - today).num_days();
            lines.push(match left {
                n if n > 0 => format!(
                    "Оплатить до {} включительно (осталось {})",
                    day(due),
                    crate::api::telegram::days_word(n)
                ),
                0 => format!("Оплатить сегодня, {}", day(due)),
                n => format!(
                    "Срок оплаты прошёл {} — просрочка {}{}",
                    day(due),
                    crate::api::telegram::days_word(-n),
                    if party.overdue_tyiyn > 0 {
                        format!(", просрочено {}", format_som(party.overdue_tyiyn))
                    } else {
                        String::new()
                    }
                ),
            });
        }
    } else if party.balance_tyiyn < 0 {
        lines.push(format!(
            "Долга нет ✅ У вас аванс {}.",
            format_som(-party.balance_tyiyn)
        ));
    } else {
        lines.push("Долга нет ✅".to_string());
    }
    if !entries.is_empty() {
        lines.push(String::new());
        lines.push("Последние покупки в долг и оплаты:".to_string());
        for e in &entries {
            let check = e
                .sale_number
                .map(|n| format!(" (чек № {n})"))
                .unwrap_or_default();
            let check = if several {
                format!("{check}, {}", e.branch_name)
            } else {
                check
            };
            // Старый долг из тетради — датой долга, а не днём, когда его занесли (SPEC-38).
            let old = e.kind == "debt" && e.doc_type == crate::api::parties::OLD_DEBT_DOC;
            lines.push(format!(
                "{} — {} {}{check}",
                if old {
                    day(e.business_date)
                } else {
                    local_day(e.created_at)
                },
                if old {
                    "старый долг (из тетради)"
                } else {
                    entry_title(&e.kind, e.amount_tyiyn, e.reversal)
                },
                format_som(e.amount_tyiyn.abs())
            ));
        }
    }
    Ok(lines.join("\n"))
}

/// Напоминание о сроке долга одному чату.
pub struct DebtReminder {
    pub chat_id: i64,
    pub party_id: Uuid,
    pub due: NaiveDate,
    /// 3 — за три дня (догоняется, пока до срока 1–3 дня), 0 — в день срока.
    pub days_before: i32,
    pub text: String,
}

/// Какие напоминания о долге пора отправить. Просроченный долг не напоминается каждый день:
/// только «за 3 дня» и «в день срока», каждое по одному разу на срок.
pub async fn due_debt_reminders(conn: &mut PgConnection) -> AppResult<Vec<DebtReminder>> {
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let links = sqlx::query!(
        r#"select t.chat_id, p.id, p.balance_tyiyn, p.due_days as "due_days?"
           from telegram_customers t join parties p on p.id = t.party_id
           where p.active and p.role = 'customer' and p.balance_tyiyn > 0
             and p.due_days is not null"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let keys: Vec<(Uuid, i64, Option<i32>)> = links
        .iter()
        .map(|l| (l.id, l.balance_tyiyn, l.due_days))
        .collect();
    let dues = crate::api::parties::debt_dues(&mut *conn, &keys).await?;
    let mut out = Vec::new();
    for l in links {
        let Some(due) = dues.get(&l.id).and_then(|d| d.due_date) else {
            continue;
        };
        let days_before = match (due - today).num_days() {
            0 => 0,
            1..=3 => 3,
            _ => continue,
        };
        let sent = sqlx::query_scalar!(
            r#"select exists (select 1 from debt_reminders
                              where party_id = $1 and due_date = $2 and days_before = $3) as "e!""#,
            l.id,
            due,
            days_before
        )
        .fetch_one(&mut *conn)
        .await?;
        if sent {
            continue;
        }
        let when = match (due - today).num_days() {
            0 => "сегодня".to_string(),
            1 => "завтра".to_string(),
            n => format!("через {}", crate::api::telegram::days_word(n)),
        };
        out.push(DebtReminder {
            chat_id: l.chat_id,
            party_id: l.id,
            due,
            days_before,
            text: format!(
                "Напоминаем: срок оплаты долга {} — {when}, {}. Подробнее — кнопка «{BTN_DEBT}».",
                format_som(l.balance_tyiyn),
                day(due)
            ),
        });
    }
    Ok(out)
}

/// Отметить напоминание; ушло «в день срока» — «за 3 дня» больше не нужно, отмечается тоже.
pub async fn mark_debt_reminded(conn: &mut PgConnection, r: &DebtReminder) -> AppResult<()> {
    let days: Vec<i32> = if r.days_before == 0 {
        vec![0, 3]
    } else {
        vec![r.days_before]
    };
    sqlx::query!(
        r#"insert into debt_reminders (party_id, due_date, days_before)
           select $1, $2, unnest($3::int[]) on conflict do nothing"#,
        r.party_id,
        r.due,
        &days[..]
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}
