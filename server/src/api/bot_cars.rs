//! Машины клиента в Телеграм-боте: список, карточка с историей замен, добавление по госномеру,
//! удаление и пометки (SPEC-22, ADR-061). Карточка показывает книжку машины целиком, длинная
//! уходит несколькими сообщениями (SPEC-30 §10, `api::bot_book`). Клиент видит и меняет только
//! машины своей карточки: каждая выборка ограничена его `party_id`, номер машины из кнопки сам
//! по себе ничего не открывает.

use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::api::oil_book::{OilRecord, books};
use crate::api::telegram::{Button, Keys, Reply};
use crate::domain::plate;
use crate::error::AppResult;
use crate::ops::new_id;

/// Сколько активных машин может быть у клиента.
pub const MAX_CARS: i64 = 10;
/// Сколько неудалённых пометок у одной машины.
pub const MAX_NOTES: i64 = 30;
/// Длина пометки, знаков.
pub const NOTE_LEN: usize = 500;
/// Длина «марка и модель», знаков.
const BRAND_LEN: usize = 60;
/// Действий в минуту на чат (добавление, пометка, удаление).
pub const RATE_PER_MIN: i64 = 30;
/// Машин кнопками в списке; остальные открываются по номеру (ADR-063).
pub const LIST_CARS: usize = 20;
/// Машин у клиента всего, вместе с убранными: дальше добавляет только точка (ADR-063).
pub const MAX_CARS_EVER: i64 = 50;
/// Уведомлений владельцу «номер у другого клиента» от одного клиента за сутки (ADR-063).
pub const CONFLICTS_PER_DAY: i64 = 3;

/// Действия клиента в журнале — по ним же считается частота.
const CLIENT_ACTIONS: [&str; 4] = [
    "vehicle.client_added",
    "vehicle.client_removed",
    "vehicle.client_note",
    "vehicle.client_note_deleted",
];

const PLATE_HINT: &str = "Напишите госномер машины, например 01KG123ABC.";
const PLATE_BAD: &str =
    "Не похоже на госномер, пример: 01KG123ABC. Напишите ещё раз или нажмите кнопку меню.";
const TOO_MANY_CARS: &str = "У вас уже 10 машин — уберите лишнюю, потом добавьте новую.";
const TOO_OFTEN: &str = "Слишком часто — подождите минуту.";
const NOT_FOUND: &str = "Машина не найдена. Откройте «🚗 Мои машины».";
const COMPANY: &str =
    "Машины фирмы ведёт точка: добавить, убрать машину или записать пометку можно у кассира.";
const TOO_MANY_EVER: &str =
    "Новую машину в боте добавить нельзя: у вас уже много машин в истории. Обратитесь на точку.";

/// Клиент, к которому привязан чат.
struct Customer {
    party_id: Uuid,
    branch_id: Uuid,
    name: String,
    /// Фирма (SPEC-12): машины в боте только смотрят.
    company: bool,
}

async fn customer(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Customer> {
    let p = sqlx::query!(
        "select id, branch_id, name, kind from parties where id = $1",
        party_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Customer {
        party_id: p.id,
        branch_id: p.branch_id,
        name: p.name,
        company: p.kind == "company",
    })
}

/// Машина клиента: только активная и только его.
struct Car {
    id: Uuid,
    plate: String,
}

async fn own_car(conn: &mut PgConnection, c: &Customer, id: Uuid) -> AppResult<Option<Car>> {
    Ok(sqlx::query!(
        r#"select id, plate from party_vehicles
           where id = $1 and party_id = $2 and branch_id = $3 and active
           for update"#,
        id,
        c.party_id,
        c.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| Car {
        id: r.id,
        plate: r.plate,
    }))
}

async fn active_cars(conn: &mut PgConnection, c: &Customer) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from party_vehicles where party_id = $1 and branch_id = $2 and active"#,
        c.party_id,
        c.branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Добавления клиента по очереди: проверка «не больше 10 машин» не обгоняет сама себя.
async fn lock_customer(conn: &mut PgConnection, c: &Customer) -> AppResult<()> {
    sqlx::query!(
        "select id from parties where id = $1 for update",
        c.party_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(())
}

/// Запись в журнал действий от имени бота: пользователя и устройства нет, чат — в данных.
async fn journal(
    conn: &mut PgConnection,
    branch_id: Uuid,
    action: &str,
    entity_id: Uuid,
    data: Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, $3, 'vehicle', $4, $5)"#,
        new_id(),
        branch_id,
        action,
        entity_id,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Не больше 30 действий в минуту на чат — по журналу, отдельного счётчика не нужно.
async fn too_often(conn: &mut PgConnection, c: &Customer, chat_id: i64) -> AppResult<bool> {
    let actions: Vec<String> = CLIENT_ACTIONS.iter().map(|s| (*s).to_string()).collect();
    let n = sqlx::query_scalar!(
        r#"select count(*) as "n!" from audit_log
           where branch_id = $1 and created_at > now() - interval '1 minute'
             and action = any($2) and data->>'chat' = $3"#,
        c.branch_id,
        &actions,
        chat_id.to_string()
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(n >= RATE_PER_MIN)
}

// ---------- Ожидание ввода ----------

struct Pending {
    kind: String,
    vehicle_id: Option<Uuid>,
    data: Value,
}

async fn pending(conn: &mut PgConnection, chat_id: i64) -> AppResult<Option<Pending>> {
    Ok(sqlx::query!(
        "select kind, vehicle_id, data from bot_input where chat_id = $1 and expires_at > now()",
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| Pending {
        kind: r.kind,
        vehicle_id: r.vehicle_id,
        data: r.data,
    }))
}

async fn wait_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    chat_id: i64,
    kind: &str,
    vehicle_id: Option<Uuid>,
    data: Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into bot_input (chat_id, branch_id, kind, vehicle_id, data, expires_at)
           values ($1, $2, $3, $4, $5, now() + interval '10 minutes')
           on conflict (chat_id) do update set branch_id = excluded.branch_id, kind = excluded.kind,
               vehicle_id = excluded.vehicle_id, data = excluded.data, expires_at = excluded.expires_at"#,
        chat_id,
        branch_id,
        kind,
        vehicle_id,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Любая кнопка меню отменяет ожидание ввода.
pub async fn clear_input(conn: &mut PgConnection, chat_id: i64) -> AppResult<()> {
    sqlx::query!("delete from bot_input where chat_id = $1", chat_id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Сообщение от клиента, когда бот ждёт номер, марку или пометку. `None` в ответе — ничего не
/// ждём. `text = None` — не текст (наклейка, фото): ожидание остаётся, бот просит написать текстом.
pub async fn on_input(
    conn: &mut PgConnection,
    party_id: Uuid,
    chat_id: i64,
    text: Option<&str>,
) -> AppResult<Option<Reply>> {
    let Some(p) = pending(conn, chat_id).await? else {
        return Ok(None);
    };
    let Some(text) = text else {
        return Ok(Some(match p.kind.as_str() {
            "brand" => Reply::with_buttons(
                "Напишите марку и модель текстом (например, Toyota Camry) или нажмите «Пропустить».",
                vec![vec![button("Пропустить", "skip")]],
            ),
            "note" => Reply::new("Напишите текст пометки.", Keys::Customer),
            _ => Reply::new(PLATE_HINT, Keys::Customer),
        }));
    };
    let c = customer(conn, party_id).await?;
    let reply = match p.kind.as_str() {
        "plate" => plate_entered(conn, &c, chat_id, text).await?,
        "find" => find_entered(conn, &c, chat_id, text).await?,
        "brand" => {
            let plate = p
                .data
                .get("plate")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            brand_entered(conn, &c, chat_id, &plate, text).await?
        }
        "note" => match p.vehicle_id {
            Some(id) => note_entered(conn, &c, chat_id, id, text).await?,
            None => {
                clear_input(conn, chat_id).await?;
                cars(conn, &c, chat_id, None).await?
            }
        },
        _ => {
            clear_input(conn, chat_id).await?;
            return Ok(None);
        }
    };
    Ok(Some(reply))
}

// ---------- Список и карточка ----------

fn car_name(plate: &str, brand: &str, model: &str) -> String {
    let car = [brand, model]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if car.is_empty() {
        plate.to_string()
    } else {
        format!("{plate} · {car}")
    }
}

fn button(text: impl Into<String>, data: impl Into<String>) -> Button {
    Button {
        text: text.into(),
        data: data.into(),
    }
}

fn with_header(header: Option<&str>, text: String) -> String {
    match header {
        Some(h) => format!("{h}\n\n{text}"),
        None => text,
    }
}

/// «🚗 Мои машины»: машины кнопками и «➕ Добавить машину». Машин нет — сразу просим номер.
pub async fn cars_for(conn: &mut PgConnection, party_id: Uuid, chat_id: i64) -> AppResult<Reply> {
    let c = customer(conn, party_id).await?;
    cars(conn, &c, chat_id, None).await
}

async fn cars(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    header: Option<&str>,
) -> AppResult<Reply> {
    let list = sqlx::query!(
        r#"select id, plate, brand, model from party_vehicles
           where party_id = $1 and branch_id = $2 and active order by plate"#,
        c.party_id,
        c.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if list.is_empty() {
        if c.company {
            return Ok(Reply::new(
                with_header(header, "Машин фирмы пока нет — их записывает точка.".into()),
                Keys::Customer,
            ));
        }
        wait_for(conn, c.branch_id, chat_id, "plate", None, json!({})).await?;
        return Ok(Reply::new(
            with_header(
                header,
                format!("Машин у вас пока нет — добавим?\n\n{PLATE_HINT}"),
            ),
            Keys::Customer,
        ));
    }
    let more = list.len().saturating_sub(LIST_CARS);
    let mut rows: Vec<Vec<Button>> = list
        .iter()
        .take(LIST_CARS)
        .map(|v| {
            vec![button(
                car_name(&v.plate, &v.brand, &v.model),
                format!("car:{}", v.id),
            )]
        })
        .collect();
    let mut text = if c.company {
        format!("Машины фирмы — нажмите, чтобы открыть историю.\n{COMPANY}")
    } else {
        rows.push(vec![button("➕ Добавить машину", "add")]);
        "Ваши машины — нажмите, чтобы открыть историю замен и пометки.".to_string()
    };
    if more > 0 {
        // Длинный список не помещается в сообщение: остальные — по номеру.
        wait_for(conn, c.branch_id, chat_id, "find", None, json!({})).await?;
        text.push_str(&format!(
            "\n\n… и ещё {more}. Чтобы открыть другую машину, напишите её номер."
        ));
    }
    Ok(Reply::with_buttons(with_header(header, text), rows))
}

/// Обрезка по знакам, а не байтам.
fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// Карточка машины: следующая замена, вся история замен от новой к старой (SPEC-30 §10),
/// пометки клиента. Длинная карточка уходит несколькими сообщениями, кнопки — под последним.
async fn card(
    conn: &mut PgConnection,
    c: &Customer,
    id: Uuid,
    header: Option<&str>,
) -> AppResult<Reply> {
    if own_car(conn, c, id).await?.is_none() {
        return Ok(Reply::new(NOT_FOUND, Keys::Customer));
    }
    // Выборка по карточке клиента: чужую машину книжка не вернёт.
    let Some(b) = books(conn, c.branch_id, Some(c.party_id), Some(id))
        .await?
        .pop()
    else {
        return Ok(Reply::new(NOT_FOUND, Keys::Customer));
    };
    let name = car_name(&b.plate, &b.brand, &b.model);
    let records: Vec<&OilRecord> = b.records.iter().collect();
    let x = crate::api::bot_book::extras(conn, &records).await?;
    let mut text = crate::api::bot_book::full_book(&b, &x, format!("🚗 {name}"));
    let notes: Vec<_> = b.client_notes.iter().filter(|n| !n.deleted).collect();
    if !notes.is_empty() {
        text.push_str("\n\n📝 Пометки:");
        for n in &notes {
            // Бишкек — UTC+6 круглый год.
            text.push_str(&format!(
                "\n{} — {}",
                (n.created_at + chrono::Duration::hours(6)).format("%d.%m"),
                n.text
            ));
        }
    }
    if c.company {
        text.push_str(&format!("\n\n{COMPANY}"));
    }
    let text = with_header(header, text);

    let mut rows = Vec::new();
    if !c.company {
        rows.push(vec![
            button("📝 Пометка", format!("note:{id}")),
            button("🗑 Убрать машину", format!("rm:{id}")),
        ]);
        for n in &notes {
            rows.push(vec![button(
                format!(
                    "✖ {} {}",
                    (n.created_at + chrono::Duration::hours(6)).format("%d.%m"),
                    cut(&n.text, 24)
                ),
                format!("dn:{}", n.id),
            )]);
        }
    }
    rows.push(vec![button("← Назад", "cars")]);
    Ok(Reply::with_buttons(text, rows))
}

// ---------- Кнопки под сообщениями ----------

/// Нажатие кнопки под сообщением. Каждый номер из кнопки проверяется по карточке клиента.
pub async fn callback(
    conn: &mut PgConnection,
    party_id: Uuid,
    chat_id: i64,
    data: &str,
) -> AppResult<Reply> {
    let c = customer(conn, party_id).await?;
    let (verb, arg) = data.split_once(':').unwrap_or((data, ""));
    // Кнопка — тоже меню: ожидание ввода отменяется, кроме «Пропустить» марку.
    if verb != "skip" {
        clear_input(conn, chat_id).await?;
    }
    if c.company && matches!(verb, "add" | "skip" | "note" | "rm" | "rmy" | "dn") {
        return Ok(Reply::new(COMPANY, Keys::Customer));
    }
    let id = || arg.split(':').next().and_then(|s| Uuid::parse_str(s).ok());
    match (verb, id()) {
        // «Ещё» из прежних сообщений (замены по 10) — теперь карточка целиком (SPEC-30 §10).
        ("car" | "more", Some(id)) => card(conn, &c, id, None).await,
        ("add", _) => start_add(conn, &c, chat_id).await,
        ("skip", _) => match pending(conn, chat_id).await? {
            Some(p) if p.kind == "brand" => {
                let plate = p
                    .data
                    .get("plate")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                brand_entered(conn, &c, chat_id, &plate, "").await
            }
            _ => cars(conn, &c, chat_id, None).await,
        },
        ("note", Some(id)) => start_note(conn, &c, chat_id, id).await,
        ("rm", Some(id)) => match own_car(conn, &c, id).await? {
            Some(car) => Ok(Reply::with_buttons(
                format!(
                    "Убрать {} из ваших машин? История останется на точке.",
                    car.plate
                ),
                vec![vec![
                    button("Да, убрать", format!("rmy:{id}")),
                    button("Назад", format!("car:{id}")),
                ]],
            )),
            None => Ok(Reply::new(NOT_FOUND, Keys::Customer)),
        },
        ("rmy", Some(id)) => remove(conn, &c, chat_id, id).await,
        ("dn", Some(id)) => delete_note(conn, &c, chat_id, id).await,
        _ => cars(conn, &c, chat_id, None).await,
    }
}

// ---------- Добавить машину ----------

async fn start_add(conn: &mut PgConnection, c: &Customer, chat_id: i64) -> AppResult<Reply> {
    if active_cars(conn, c).await? >= MAX_CARS {
        return Ok(Reply::new(TOO_MANY_CARS, Keys::Customer));
    }
    wait_for(conn, c.branch_id, chat_id, "plate", None, json!({})).await?;
    Ok(Reply::new(PLATE_HINT, Keys::Customer))
}

/// Своя машина с таким номером (в том числе убранная): самая свежая, активные — первыми.
async fn own_by_plate(
    conn: &mut PgConnection,
    c: &Customer,
    norm: &str,
) -> AppResult<Option<(Uuid, String, bool)>> {
    Ok(sqlx::query!(
        r#"select id, plate, active from party_vehicles
           where party_id = $1 and branch_id = $2 and plate_norm = $3
           order by active desc, created_at desc limit 1
           for update"#,
        c.party_id,
        c.branch_id,
        norm
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| (r.id, r.plate, r.active)))
}

/// Вернуть свою машину: снова активна, история на месте. Вызывается в транзакции.
async fn reactivate(
    tx: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    id: Uuid,
    plate: &str,
    active: bool,
) -> AppResult<Option<&'static str>> {
    if active {
        return Ok(None);
    }
    if too_often(tx, c, chat_id).await? {
        return Ok(Some(TOO_OFTEN));
    }
    if active_cars(tx, c).await? >= MAX_CARS {
        return Ok(Some(TOO_MANY_CARS));
    }
    sqlx::query!(
        "update party_vehicles set active = true where id = $1 and party_id = $2",
        id,
        c.party_id
    )
    .execute(&mut *tx)
    .await?;
    journal(
        tx,
        c.branch_id,
        "vehicle.client_added",
        id,
        json!({ "chat": chat_id, "plate": plate, "reactivated": true }),
    )
    .await?;
    Ok(None)
}

const FOUND: &str = "Нашли вашу машину, история замен ниже.";

/// Машин клиента всего, вместе с убранными.
async fn cars_ever(conn: &mut PgConnection, c: &Customer) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from party_vehicles where party_id = $1 and branch_id = $2"#,
        c.party_id,
        c.branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Номер машины из длинного списка: открывается только своя активная машина.
async fn find_entered(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    text: &str,
) -> AppResult<Reply> {
    let Some(norm) = plate::parse(text) else {
        return Ok(Reply::new(PLATE_BAD, Keys::Customer));
    };
    let found = sqlx::query_scalar!(
        r#"select id from party_vehicles
           where party_id = $1 and branch_id = $2 and plate_norm = $3 and active
           order by created_at desc limit 1"#,
        c.party_id,
        c.branch_id,
        norm
    )
    .fetch_optional(&mut *conn)
    .await?;
    match found {
        Some(id) => {
            clear_input(conn, chat_id).await?;
            card(conn, c, id, None).await
        }
        None => Ok(Reply::new(
            "Среди ваших машин такого номера нет. Напишите другой номер или нажмите кнопку меню.",
            Keys::Customer,
        )),
    }
}

async fn plate_entered(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    text: &str,
) -> AppResult<Reply> {
    let Some(norm) = plate::parse(text) else {
        return Ok(Reply::new(PLATE_BAD, Keys::Customer));
    };
    let mut tx = conn.begin().await?;
    lock_customer(&mut tx, c).await?;
    // Номер сравнивается только с машинами этого клиента — по чужому номеру ничего не узнать.
    if let Some((id, plate, active)) = own_by_plate(&mut tx, c, &norm).await? {
        if let Some(refused) = reactivate(&mut tx, c, chat_id, id, &plate, active).await? {
            clear_input(&mut tx, chat_id).await?;
            tx.commit().await?;
            return Ok(Reply::new(refused, Keys::Customer));
        }
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return card(conn, c, id, Some(FOUND)).await;
    }
    if active_cars(&mut tx, c).await? >= MAX_CARS {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(TOO_MANY_CARS, Keys::Customer));
    }
    if cars_ever(&mut tx, c).await? >= MAX_CARS_EVER {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(TOO_MANY_EVER, Keys::Customer));
    }
    wait_for(
        &mut tx,
        c.branch_id,
        chat_id,
        "brand",
        None,
        json!({ "plate": norm }),
    )
    .await?;
    tx.commit().await?;
    Ok(Reply::with_buttons(
        "Марка и модель? (например, Toyota Camry)",
        vec![vec![button("Пропустить", "skip")]],
    ))
}

/// Ответ на новый номер одинаков, есть ли он у другого клиента или нет (ADR-061).
pub fn added_text(plate: &str) -> String {
    format!("Машина {plate} добавлена.")
}

async fn brand_entered(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    plate: &str,
    text: &str,
) -> AppResult<Reply> {
    let Some(norm) = plate::parse(plate) else {
        clear_input(conn, chat_id).await?;
        return cars(conn, c, chat_id, None).await;
    };
    let text = text.trim();
    if text.chars().count() > BRAND_LEN {
        return Ok(Reply::with_buttons(
            "Слишком длинно — до 60 знаков. Напишите марку и модель или нажмите «Пропустить».",
            vec![vec![button("Пропустить", "skip")]],
        ));
    }
    let (brand, model) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let mut tx = conn.begin().await?;
    lock_customer(&mut tx, c).await?;
    if too_often(&mut tx, c, chat_id).await? {
        return Ok(Reply::new(TOO_OFTEN, Keys::Customer));
    }
    // Пока клиент писал марку, номер мог появиться у него же — не заводим дубль.
    if let Some((id, plate, active)) = own_by_plate(&mut tx, c, &norm).await? {
        let refused = reactivate(&mut tx, c, chat_id, id, &plate, active).await?;
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return match refused {
            Some(r) => Ok(Reply::new(r, Keys::Customer)),
            None => card(conn, c, id, Some(FOUND)).await,
        };
    }
    if active_cars(&mut tx, c).await? >= MAX_CARS {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(TOO_MANY_CARS, Keys::Customer));
    }
    if cars_ever(&mut tx, c).await? >= MAX_CARS_EVER {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(TOO_MANY_EVER, Keys::Customer));
    }
    let id = new_id();
    sqlx::query!(
        r#"insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, brand, model, added_by)
           values ($1, $2, $3, $4, $4, $5, $6, 'client')"#,
        id,
        c.branch_id,
        c.party_id,
        norm,
        brand,
        model.trim()
    )
    .execute(&mut *tx)
    .await?;
    // Номер есть у другого клиента точки: новая машина без истории, разбирается владелец.
    let others = sqlx::query_scalar!(
        r#"select distinct p.name from party_vehicles v join parties p on p.id = v.party_id
           where v.branch_id = $1 and v.plate_norm = $2 and v.party_id <> $3
           order by p.name"#,
        c.branch_id,
        norm,
        c.party_id
    )
    .fetch_all(&mut *tx)
    .await?;
    if !others.is_empty() {
        // Владельцу — не больше трёх таких уведомлений от клиента за сутки; остальные только
        // в журнал, без уведомления.
        let told = sqlx::query_scalar!(
            r#"select count(*) as "n!" from audit_log
               where branch_id = $1 and action = 'vehicle.plate_conflict'
                 and data->>'party_id' = $2 and created_at > now() - interval '1 day'"#,
            c.branch_id,
            c.party_id.to_string()
        )
        .fetch_one(&mut *tx)
        .await?;
        let action = if told < CONFLICTS_PER_DAY {
            "vehicle.plate_conflict"
        } else {
            "vehicle.plate_conflict_quiet"
        };
        journal(
            &mut tx,
            c.branch_id,
            action,
            id,
            json!({ "plate": norm, "client": c.name, "other": others.join(", "), "party_id": c.party_id }),
        )
        .await?;
    }
    journal(
        &mut tx,
        c.branch_id,
        "vehicle.client_added",
        id,
        json!({ "chat": chat_id, "plate": norm }),
    )
    .await?;
    clear_input(&mut tx, chat_id).await?;
    tx.commit().await?;
    card(conn, c, id, Some(&added_text(&norm))).await
}

// ---------- Убрать машину ----------

async fn remove(conn: &mut PgConnection, c: &Customer, chat_id: i64, id: Uuid) -> AppResult<Reply> {
    let mut tx = conn.begin().await?;
    let Some(car) = own_car(&mut tx, c, id).await? else {
        return Ok(Reply::new(NOT_FOUND, Keys::Customer));
    };
    if too_often(&mut tx, c, chat_id).await? {
        return Ok(Reply::new(TOO_OFTEN, Keys::Customer));
    }
    // Только отметка: замены, чеки и пометки остаются на точке.
    sqlx::query!(
        "update party_vehicles set active = false where id = $1 and party_id = $2",
        car.id,
        c.party_id
    )
    .execute(&mut *tx)
    .await?;
    journal(
        &mut tx,
        c.branch_id,
        "vehicle.client_removed",
        car.id,
        json!({ "chat": chat_id, "plate": car.plate }),
    )
    .await?;
    tx.commit().await?;
    let header = format!(
        "Машина {} убрана. История осталась на точке — чтобы вернуть, добавьте тот же номер.",
        car.plate
    );
    cars(conn, c, chat_id, Some(&header)).await
}

// ---------- Пометки ----------

async fn notes_count(conn: &mut PgConnection, c: &Customer, id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from vehicle_notes
           where vehicle_id = $1 and party_id = $2 and deleted_at is null"#,
        id,
        c.party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

const TOO_MANY_NOTES: &str = "У этой машины уже 30 пометок — удалите лишнюю кнопкой «✖».";

async fn start_note(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    id: Uuid,
) -> AppResult<Reply> {
    let Some(car) = own_car(conn, c, id).await? else {
        return Ok(Reply::new(NOT_FOUND, Keys::Customer));
    };
    if notes_count(conn, c, id).await? >= MAX_NOTES {
        return Ok(Reply::new(TOO_MANY_NOTES, Keys::Customer));
    }
    wait_for(conn, c.branch_id, chat_id, "note", Some(car.id), json!({})).await?;
    Ok(Reply::new(
        format!("Напишите пометку к {} — до 500 знаков.", car.plate),
        Keys::Customer,
    ))
}

async fn note_entered(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    id: Uuid,
    text: &str,
) -> AppResult<Reply> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Reply::new("Напишите текст пометки.", Keys::Customer));
    }
    let len = text.chars().count();
    if len > NOTE_LEN {
        return Ok(Reply::new(
            format!("Слишком длинно: {len} знаков, можно до 500. Сократите и отправьте ещё раз."),
            Keys::Customer,
        ));
    }
    let mut tx = conn.begin().await?;
    let Some(car) = own_car(&mut tx, c, id).await? else {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(NOT_FOUND, Keys::Customer));
    };
    if too_often(&mut tx, c, chat_id).await? {
        return Ok(Reply::new(TOO_OFTEN, Keys::Customer));
    }
    if notes_count(&mut tx, c, car.id).await? >= MAX_NOTES {
        clear_input(&mut tx, chat_id).await?;
        tx.commit().await?;
        return Ok(Reply::new(TOO_MANY_NOTES, Keys::Customer));
    }
    let note_id = new_id();
    sqlx::query!(
        r#"insert into vehicle_notes (id, branch_id, vehicle_id, party_id, text)
           values ($1, $2, $3, $4, $5)"#,
        note_id,
        c.branch_id,
        car.id,
        c.party_id,
        text
    )
    .execute(&mut *tx)
    .await?;
    journal(
        &mut tx,
        c.branch_id,
        "vehicle.client_note",
        car.id,
        json!({ "chat": chat_id, "plate": car.plate, "note_id": note_id }),
    )
    .await?;
    clear_input(&mut tx, chat_id).await?;
    tx.commit().await?;
    card(conn, c, car.id, Some("Сохранено.")).await
}

async fn delete_note(
    conn: &mut PgConnection,
    c: &Customer,
    chat_id: i64,
    note_id: Uuid,
) -> AppResult<Reply> {
    let mut tx = conn.begin().await?;
    // Своя пометка на своей машине — иначе кнопка ничего не делает.
    let note = sqlx::query!(
        r#"select n.vehicle_id, v.plate from vehicle_notes n
           join party_vehicles v on v.id = n.vehicle_id
           where n.id = $1 and n.party_id = $2 and v.party_id = $2 and n.branch_id = $3
             and v.active and n.deleted_at is null
           for update of n"#,
        note_id,
        c.party_id,
        c.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(note) = note else {
        return Ok(Reply::new(
            "Пометка не найдена. Откройте «🚗 Мои машины».",
            Keys::Customer,
        ));
    };
    if too_often(&mut tx, c, chat_id).await? {
        return Ok(Reply::new(TOO_OFTEN, Keys::Customer));
    }
    sqlx::query!(
        "update vehicle_notes set deleted_at = now() where id = $1 and party_id = $2",
        note_id,
        c.party_id
    )
    .execute(&mut *tx)
    .await?;
    journal(
        &mut tx,
        c.branch_id,
        "vehicle.client_note_deleted",
        note.vehicle_id,
        json!({ "chat": chat_id, "plate": note.plate, "note_id": note_id }),
    )
    .await?;
    tx.commit().await?;
    card(conn, c, note.vehicle_id, Some("Пометка удалена.")).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cut_counts_chars() {
        assert_eq!(cut("стук справа", 4), "стук…");
        assert_eq!(cut("стук", 4), "стук");
    }

    #[test]
    fn car_name_skips_empty_parts() {
        assert_eq!(car_name("01KG123ABC", "", ""), "01KG123ABC");
        assert_eq!(
            car_name("01KG123ABC", "Toyota", "Camry"),
            "01KG123ABC · Toyota Camry"
        );
    }
}
