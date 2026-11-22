//! Подключение клиента в боте по двум признакам из трёх (задача 046, ADR-212): ИНН, ФИО или
//! название карточки, госномер машины. Подключаем, только если ровно одна активная карточка
//! клиента совпала минимум по двум разным признакам; что именно совпало — не говорим.

use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::telegram::{Keys, Reply, link_party};
use crate::domain::plate;
use crate::error::AppResult;

pub const BTN_SIGNS: &str = "🔎 Найти себя по ИНН, ФИО или госномеру";
/// Неудачных попыток за сутки на чат.
pub const MAX_TRIES: i64 = 5;

const PROMPT: &str = "Пришлите одним сообщением два признака из трёх, через запятую или с новой строки:\n• ИНН (12–14 цифр);\n• ФИО или название фирмы — как записано у нас на кассе;\n• госномер машины.\n\nНапример: Иванов Иван Иванович, 01KG123ABC";
const NOT_FOUND: &str =
    "Не нашли вас по этим данным. Нажмите «📱 Поделиться номером» или обратитесь к администратору.";
const TOO_MANY: &str = "Слишком много попыток за сутки. Нажмите «📱 Поделиться номером» или обратитесь к администратору.";
const TEXT_ONLY: &str = "Пришлите признаки текстом: два из трёх — ИНН, ФИО или название, госномер.";

/// Признаки из сообщения клиента.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Signs {
    pub inn: Option<String>,
    pub name: Option<String>,
    pub plate: Option<String>,
}

impl Signs {
    fn count(&self) -> usize {
        [
            self.inn.is_some(),
            self.name.is_some(),
            self.plate.is_some(),
        ]
        .iter()
        .filter(|x| **x)
        .count()
    }
}

/// Имя для сравнения: без регистра и лишних пробелов.
pub fn norm_name(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn as_inn(s: &str) -> Option<String> {
    let d: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    ((12..=14).contains(&d.len()) && d.bytes().all(|b| b.is_ascii_digit())).then_some(d)
}

fn as_plate(s: &str) -> Option<String> {
    let p = plate::parse(s)?;
    (p.bytes().any(|b| b.is_ascii_digit()) && p.bytes().any(|b| b.is_ascii_alphabetic()))
        .then_some(p)
}

/// Разобрать сообщение терпимо: части через запятую, точку с запятой или перевод строки;
/// 12–14 цифр — ИНН, похоже на госномер — госномер, остальное — имя.
pub fn parse(text: &str) -> Signs {
    let mut s = Signs::default();
    for part in text.split([',', ';', '\n']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(i) = as_inn(part) {
            s.inn.get_or_insert(i);
            continue;
        }
        if let Some(p) = as_plate(part) {
            s.plate.get_or_insert(p);
            continue;
        }
        // Часть без разделителей: ИНН и госномер — отдельными словами, остальное — имя.
        let mut rest = Vec::new();
        for w in part.split_whitespace() {
            if let Some(i) = as_inn(w) {
                s.inn.get_or_insert(i);
            } else if let Some(p) = as_plate(w) {
                s.plate.get_or_insert(p);
            } else {
                rest.push(w);
            }
        }
        if !rest.is_empty() && s.name.is_none() {
            s.name = Some(norm_name(&rest.join(" ")));
        }
    }
    s
}

async fn waiting(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from bot_sign_wait where chat_id = $1 and expires_at > now()) as "e!""#,
        chat_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

async fn clear(conn: &mut PgConnection, chat_id: i64) -> AppResult<()> {
    sqlx::query!("delete from bot_sign_wait where chat_id = $1", chat_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn exhausted(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    let n = sqlx::query_scalar!(
        r#"select count(*) as "n!" from bot_sign_tries
           where chat_id = $1 and created_at > now() - interval '1 day'"#,
        chat_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(n >= MAX_TRIES)
}

/// Сообщение неподключённого чата (не владелец, не работник, не администратор). `None` — не наше.
pub async fn on_message(
    conn: &mut PgConnection,
    chat_id: i64,
    text: Option<&str>,
) -> AppResult<Option<Reply>> {
    let t = text.map(str::trim).unwrap_or("");
    if t == BTN_SIGNS {
        if exhausted(conn, chat_id).await? {
            return Ok(Some(Reply::new(TOO_MANY, Keys::Contact)));
        }
        sqlx::query!(
            r#"insert into bot_sign_wait (chat_id, expires_at) values ($1, now() + interval '15 minutes')
               on conflict (chat_id) do update set expires_at = excluded.expires_at"#,
            chat_id
        )
        .execute(&mut *conn)
        .await?;
        return Ok(Some(Reply::new(PROMPT, Keys::Contact)));
    }
    if !waiting(conn, chat_id).await? {
        return Ok(None);
    }
    let Some(text) = text else {
        return Ok(Some(Reply::new(TEXT_ONLY, Keys::Contact)));
    };
    clear(conn, chat_id).await?;
    if t.starts_with('/') {
        return Ok(None);
    }
    if exhausted(conn, chat_id).await? {
        return Ok(Some(Reply::new(TOO_MANY, Keys::Contact)));
    }
    let signs = parse(text);
    if let Some((id, name, phone, branch)) = find(conn, &signs).await? {
        return link_party(conn, chat_id, id, &name, &phone, branch, "signs", false)
            .await
            .map(Some);
    }
    sqlx::query!("insert into bot_sign_tries (chat_id) values ($1)", chat_id)
        .execute(&mut *conn)
        .await?;
    Ok(Some(Reply::new(NOT_FOUND, Keys::Contact)))
}

/// Ровно одна активная карточка клиента с совпадением минимум по двум признакам.
async fn find(
    conn: &mut PgConnection,
    s: &Signs,
) -> AppResult<Option<(Uuid, String, String, Uuid)>> {
    if s.count() < 2 {
        return Ok(None);
    }
    let rows = sqlx::query!(
        r#"select id as "id!", name as "name!", phone as "phone!", branch_id as "branch_id!" from (
             select p.id, p.name, p.phone, p.branch_id,
                    (($1::text is not null and p.inn <> ''
                      and regexp_replace(p.inn, '\s', '', 'g') = $1)::int
                     + ($2::text is not null
                        and lower(regexp_replace(btrim(p.name), '\s+', ' ', 'g')) = $2)::int
                     + ($3::text is not null and exists (
                          select 1 from party_vehicles v
                          where v.party_id = p.id and v.active and v.plate_norm = $3))::int) as hits
             from parties p
             where p.role = 'customer' and p.active
           ) m where hits >= 2
           limit 2"#,
        s.inn.as_deref(),
        s.name.as_deref(),
        s.plate.as_deref()
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(match rows.as_slice() {
        [one] => Some((one.id, one.name.clone(), one.phone.clone(), one.branch_id)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parts() {
        let s = parse("Иванов  Иван, 01 kg-123 АВС\n123456789012");
        assert_eq!(s.name.as_deref(), Some("иванов иван"));
        assert_eq!(s.plate.as_deref(), Some("01KG123ABC"));
        assert_eq!(s.inn.as_deref(), Some("123456789012"));
        let s = parse("ОсОО Ромашка 01KG123ABC");
        assert_eq!(s.name.as_deref(), Some("осоо ромашка"));
        assert_eq!(s.plate.as_deref(), Some("01KG123ABC"));
        assert_eq!(parse("Иванов").count(), 1);
    }
}
