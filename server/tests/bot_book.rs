//! Книжка в боте (SPEC-36 §1 на сервере, §6; ADR-115, ADR-119): «Сейчас в машине» по строкам
//! книжки, следующая замена только пробегом, «📜 Прошлые ТО» по одной дате с кнопкой
//! «⬅ Показать до …», проверка машины по карточке клиента.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

use avtodom_server::api::oil_book::{BookLine, SlotNext, line_next_text, next_text};
use avtodom_server::api::telegram::{Reply, customer_book, handle_callback, handle_message};
use avtodom_server::bootstrap;

struct World {
    branch: Uuid,
    owner: Uuid,
    /// Услуга «Замена масла (масло клиента)» со строкой книжки «масло».
    own_oil: Uuid,
}

async fn world(pool: &PgPool) -> World {
    bootstrap::ensure_owner(pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(pool)
        .await
        .unwrap();
    let owner: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
        .fetch_one(pool)
        .await
        .unwrap();
    let own_oil = Uuid::now_v7();
    sqlx::query("insert into services (id, branch_id, name, price_tyiyn, book_slot) values ($1, $2, 'Замена масла (масло клиента)', 40000, 'oil')")
        .bind(own_oil)
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    World {
        branch,
        owner,
        own_oil,
    }
}

async fn customer(pool: &PgPool, w: &World, name: &str, chat: i64) -> Uuid {
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', $3, '')")
        .bind(party)
        .bind(w.branch)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, '')")
        .bind(chat)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    party
}

async fn car(pool: &PgPool, w: &World, party: Uuid, plate: &str, unit: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, mileage_unit) values ($1, $2, $3, $4, plate_norm($4), $5)")
        .bind(id)
        .bind(w.branch)
        .bind(party)
        .bind(plate)
        .bind(unit)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Строка записи: строка книжки, текст, услуга и цель кассира (пробег, единица, км, дата).
struct Item<'a> {
    slot: &'a str,
    text: &'a str,
    service: Option<Uuid>,
    target: Option<(i32, &'a str, i32, &'a str)>,
}

fn item<'a>(slot: &'a str, text: &'a str) -> Item<'a> {
    Item {
        slot,
        text,
        service: None,
        target: None,
    }
}

/// Запись книжки (как из тетради или чека): дата, пробег в единице машины и в км, строки.
#[allow(clippy::too_many_arguments)]
async fn record(
    pool: &PgPool,
    w: &World,
    party: Uuid,
    vehicle: Uuid,
    date: &str,
    mileage: Option<(i32, &str, i32)>,
    declined: &str,
    items: &[Item<'_>],
) {
    let id = Uuid::now_v7();
    let oil = items
        .iter()
        .find(|i| i.slot == "oil")
        .map_or("", |i| i.text);
    sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_value, mileage_unit, mileage_km, oil_text, declined, user_id) values ($1, $2, $3, $4, $5::date, $6, $7, $8, $9, $10, $11)")
        .bind(id)
        .bind(w.branch)
        .bind(vehicle)
        .bind(party)
        .bind(date)
        .bind(mileage.map(|m| m.0))
        .bind(mileage.map_or("km", |m| m.1))
        .bind(mileage.map(|m| m.2))
        .bind(oil)
        .bind(declined)
        .bind(w.owner)
        .execute(pool)
        .await
        .unwrap();
    for (n, i) in items.iter().enumerate() {
        let t = i.target;
        sqlx::query("insert into oil_change_items (id, branch_id, oil_change_id, slot, text, position, service_id, target_set, next_value, next_unit, next_km, next_date) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::date)")
            .bind(Uuid::now_v7())
            .bind(w.branch)
            .bind(id)
            .bind(i.slot)
            .bind(i.text)
            .bind(i32::try_from(n).unwrap())
            .bind(i.service)
            .bind(t.is_some())
            .bind(t.map(|t| t.0))
            .bind(t.map(|t| t.1))
            .bind(t.map(|t| t.2))
            .bind(t.map(|t| t.3))
            .execute(pool)
            .await
            .unwrap();
    }
}

async fn press(pool: &PgPool, chat: i64, data: &str) -> Reply {
    let mut conn = pool.acquire().await.unwrap();
    handle_callback(&mut conn, chat, data).await.unwrap()
}

async fn say(pool: &PgPool, chat: i64, text: &str) -> Reply {
    let mut conn = pool.acquire().await.unwrap();
    handle_message(&mut conn, chat, chat, Some(text), None)
        .await
        .unwrap()
}

/// Кнопки под сообщением: (текст, данные).
fn buttons(r: &Reply) -> Vec<(String, String)> {
    r.inline
        .iter()
        .flatten()
        .map(|b| (b.text.clone(), b.data.clone()))
        .collect()
}

fn no_either_or(text: &str) {
    assert!(
        !text.contains("наступит раньше") && !text.contains(" или до") && !text.contains("или на"),
        "{text}"
    );
}

#[test]
fn next_change_text_is_mileage_only() {
    // ADR-115: только пробег; дата — только у строки без пробега.
    let slot = |mileage: Option<i32>, unit: &str, date: Option<NaiveDate>| SlotNext {
        slot: "oil".into(),
        label: "Масло двигателя".into(),
        interval_km: Some(8000),
        interval_days: Some(31),
        own_interval: false,
        last_date: None,
        last_km: None,
        last_text: None,
        next_km: mileage,
        next_date: date,
        unit: unit.into(),
        last_mileage: None,
        next_mileage: mileage,
    };
    let d = NaiveDate::from_ymd_opt(2027, 1, 12);
    assert_eq!(next_text(&slot(Some(93_000), "km", d)), "на 93 000 км");
    assert_eq!(next_text(&slot(Some(57_790), "mi", d)), "на 57 790 миль");
    assert_eq!(next_text(&slot(None, "km", d)), "по сроку до 12.01.2027");
    assert_eq!(next_text(&slot(None, "km", None)), "");

    let line = |mileage: Option<i32>, unit: Option<&str>, date: Option<NaiveDate>| BookLine {
        slot: "oil".into(),
        text: "Масло".into(),
        target_set: true,
        next_mileage: mileage,
        next_unit: unit.map(String::from),
        next_km: mileage,
        next_date: date,
        ..BookLine::default()
    };
    assert_eq!(
        line_next_text(&line(Some(225_000), Some("km"), d)),
        "следующая замена на 225 000 км"
    );
    assert_eq!(
        line_next_text(&line(Some(140_000), Some("mi"), d)),
        "следующая замена на 140 000 миль"
    );
    assert_eq!(
        line_next_text(&line(None, None, d)),
        "следующая замена по сроку до 12.01.2027"
    );
    assert_eq!(line_next_text(&line(None, None, None)), "");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_shows_now_in_car_and_past_visits_by_date(pool: PgPool) {
    let w = world(&pool).await;
    let chat = 760_001i64;
    let me = customer(&pool, &w, "Эрлан", chat).await;
    let car_id = car(&pool, &w, me, "09KG999ZXZ", "km").await;
    // Первая замена — из тетради.
    record(
        &pool,
        &w,
        me,
        car_id,
        "2026-01-10",
        Some((70_000, "km", 70_000)),
        "",
        &[item("oil", "Тетрадное масло")],
    )
    .await;
    // Две записи за одну дату — одно ТО: масло клиента и фильтры.
    let mut own = item("oil", "Замена масла (масло клиента)");
    own.service = Some(w.own_oil);
    record(
        &pool,
        &w,
        me,
        car_id,
        "2026-05-01",
        Some((80_000, "km", 80_000)),
        "салонный фильтр",
        &[own, item("oil_filter", "Фильтр W712 · 1 шт")],
    )
    .await;
    record(
        &pool,
        &w,
        me,
        car_id,
        "2026-05-01",
        Some((80_000, "km", 80_000)),
        "",
        &[item("air_filter", "Фильтр воздушный A1 · 1 шт")],
    )
    .await;
    // Последнее ТО: масло с целью кассира и антифриз.
    let mut oil = item("oil", "Mobil 5W-30 · 1 кан. × 4 л");
    oil.target = Some((258_000, "km", 258_000, "2026-11-26"));
    record(
        &pool,
        &w,
        me,
        car_id,
        "2026-10-26",
        Some((250_000, "km", 250_000)),
        "",
        &[oil, item("antifreeze", "Sintec G12 · 2 шт")],
    )
    .await;

    // Книжка — «Сейчас в машине»: что, когда, пробег и следующая — только пробег.
    let mut conn = pool.acquire().await.unwrap();
    let book = customer_book(&mut conn, me).await.unwrap();
    drop(conn);
    assert!(book.contains("Сейчас в машине:"), "{book}");
    assert!(
        book.contains(
            "  Масло двигателя: Mobil 5W-30 · 1 кан. × 4 л — 26.10.2026, 250 000 км → следующая на 258 000 км"
        ),
        "{book}"
    );
    assert!(
        book.contains(
            "  Фильтр масляный: Фильтр W712 · 1 шт — 01.05.2026, 80 000 км → следующая на 88 000 км"
        ),
        "{book}"
    );
    assert!(
        book.contains("  Фильтр воздушный: Фильтр воздушный A1 · 1 шт — 01.05.2026, 80 000 км"),
        "{book}"
    );
    assert!(
        book.contains("  Антифриз: Sintec G12 · 2 шт — 26.10.2026, 250 000 км → следующая на"),
        "{book}"
    );
    // История — не в книжке, а кнопкой.
    assert!(
        !book.contains("Тетрадное масло") && !book.contains("Замены:"),
        "{book}"
    );
    assert!(book.contains("«📜 Прошлые ТО»"), "{book}");
    no_either_or(&book);

    // Карточка машины — то же и кнопка «📜 Прошлые ТО».
    let card = press(&pool, chat, &format!("car:{car_id}")).await;
    assert!(card.text.contains("Сейчас в машине:"), "{}", card.text);
    no_either_or(&card.text);
    assert!(buttons(&card).contains(&("📜 Прошлые ТО".into(), format!("to:{car_id}"))));

    // Кнопка меню: одна машина — сразу последнее ТО.
    let last = say(&pool, chat, "📜 Прошлые ТО").await;
    assert!(
        last.text
            .starts_with("📜 09KG999ZXZ — ТО 26.10.2026 · 250 000 км"),
        "{}",
        last.text
    );
    assert!(
        last.text
            .contains("  Масло двигателя: Mobil 5W-30 · 1 кан. × 4 л")
            && last.text.contains("  Антифриз: Sintec G12 · 2 шт"),
        "{}",
        last.text
    );
    // У строк ТО нет следующей замены, более ранних записей тоже.
    assert!(
        !last.text.contains("следующ") && !last.text.contains("01.05.2026"),
        "{}",
        last.text
    );
    let b = buttons(&last);
    let more = format!("to:{car_id}:20261026");
    assert!(more.len() <= 64);
    assert_eq!(
        b,
        vec![
            ("⬅ Показать до 26.10.2026".to_string(), more.clone()),
            ("↩ К машине".to_string(), format!("car:{car_id}")),
        ]
    );
    // Кнопка под машиной — то же.
    assert_eq!(
        press(&pool, chat, &format!("to:{car_id}")).await.text,
        last.text
    );

    // «Показать до» — ТО строго раньше: обе записи 01.05.2026 одним ТО.
    let may = press(&pool, chat, &more).await;
    assert!(
        may.text
            .starts_with("📜 09KG999ZXZ — ТО 01.05.2026 · 80 000 км"),
        "{}",
        may.text
    );
    assert!(
        may.text
            .contains("  Масло двигателя: Замена масла (масло клиента)")
            && may.text.contains("  Фильтр масляный: Фильтр W712 · 1 шт")
            && may
                .text
                .contains("  Фильтр воздушный: Фильтр воздушный A1 · 1 шт")
            && may.text.contains("  Мастер рекомендовал: салонный фильтр"),
        "{}",
        may.text
    );
    assert!(!may.text.contains("Mobil") && !may.text.contains("Тетрадное"));
    assert!(!may.text.contains("Это первая замена"));
    assert_eq!(buttons(&may)[0].1, format!("to:{car_id}:20260501"));

    // Самое раннее — «Это первая замена на точке», кнопки «Показать до» нет.
    let first = press(&pool, chat, &format!("to:{car_id}:20260501")).await;
    assert!(
        first
            .text
            .starts_with("📜 09KG999ZXZ — ТО 10.01.2026 · 70 000 км")
            && first.text.contains("  Масло двигателя: Тетрадное масло")
            && first.text.ends_with("Это первая замена на точке."),
        "{}",
        first.text
    );
    assert_eq!(
        buttons(&first),
        vec![("↩ К машине".to_string(), format!("car:{car_id}"))]
    );
    // Раньше первой — записей нет.
    let none = press(&pool, chat, &format!("to:{car_id}:20260110")).await;
    assert!(
        none.text.contains("раньше замен на точке нет"),
        "{}",
        none.text
    );

    // Чужая машина в кнопке — отказ, ни истории, ни номера.
    let other_chat = 760_002i64;
    let other = customer(&pool, &w, "Бакыт", other_chat).await;
    let foreign = car(&pool, &w, other, "01KG111AAA", "km").await;
    record(
        &pool,
        &w,
        other,
        foreign,
        "2026-09-01",
        Some((10_000, "km", 10_000)),
        "",
        &[item("oil", "Чужое масло")],
    )
    .await;
    for data in [
        format!("to:{foreign}"),
        format!("to:{foreign}:20261231"),
        format!("car:{foreign}"),
    ] {
        let r = press(&pool, chat, &data).await;
        assert!(
            r.text.contains("Машина не найдена")
                && !r.text.contains("Чужое")
                && !r.text.contains("01KG111AAA"),
            "{data}: {}",
            r.text
        );
    }

    // Несколько машин — сначала выбор машины; машина с маслом клиента — «масло клиента».
    let second = car(&pool, &w, me, "01KG222BBB", "km").await;
    let mut own = item("oil", "Замена масла (масло клиента)");
    own.service = Some(w.own_oil);
    record(
        &pool,
        &w,
        me,
        second,
        "2026-10-20",
        Some((120_000, "km", 120_000)),
        "",
        &[own],
    )
    .await;
    let pick = say(&pool, chat, "📜 Прошлые ТО").await;
    assert!(pick.text.contains("выберите машину"), "{}", pick.text);
    let data: Vec<String> = buttons(&pick).into_iter().map(|b| b.1).collect();
    assert_eq!(data, vec![format!("to:{second}"), format!("to:{car_id}")]);
    let mut conn = pool.acquire().await.unwrap();
    let book = customer_book(&mut conn, me).await.unwrap();
    assert!(
        book.contains(
            "  Масло двигателя: масло клиента — 20.10.2026, 120 000 км → следующая на 128 000 км"
        ),
        "{book}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_in_miles_and_without_mileage(pool: PgPool) {
    let w = world(&pool).await;
    let chat = 760_101i64;
    let me = customer(&pool, &w, "Азиз", chat).await;
    // Машина в милях: пробег и следующая — в милях (ADR-084).
    let usa = car(&pool, &w, me, "01KG333CCC", "mi").await;
    let mut oil = item("oil", "Mobil 5W-30 · 1 кан. × 4 л");
    oil.target = Some((55_000, "mi", 88_514, "2026-11-26"));
    record(
        &pool,
        &w,
        me,
        usa,
        "2026-10-26",
        Some((50_000, "mi", 80_467)),
        "",
        &[oil],
    )
    .await;
    // Машина без пробега: следующая — «по сроку до …».
    let old = car(&pool, &w, me, "01KG444DDD", "km").await;
    record(
        &pool,
        &w,
        me,
        old,
        "2026-10-01",
        None,
        "",
        &[item("oil", "Тетрадное масло")],
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    let book = customer_book(&mut conn, me).await.unwrap();
    assert!(
        book.contains(
            "Mobil 5W-30 · 1 кан. × 4 л — 26.10.2026, 50 000 миль → следующая на 55 000 миль"
        ),
        "{book}"
    );
    assert!(
        book.contains(
            "  Масло двигателя: Тетрадное масло — 01.10.2026 → следующая по сроку до 01.11.2026"
        ),
        "{book}"
    );
    no_either_or(&book);
    let visit = press(&pool, chat, &format!("to:{usa}")).await;
    assert!(
        visit
            .text
            .starts_with("📜 01KG333CCC — ТО 26.10.2026 · 50 000 миль"),
        "{}",
        visit.text
    );
    // Книжки нет — ТО тоже нет.
    let empty = car(&pool, &w, me, "01KG555EEE", "km").await;
    let card = press(&pool, chat, &format!("car:{empty}")).await;
    assert!(
        card.text.contains("Замен на точке пока не было"),
        "{}",
        card.text
    );
    assert!(!buttons(&card).iter().any(|b| b.1.starts_with("to:")));
    let none = press(&pool, chat, &format!("to:{empty}")).await;
    assert!(
        none.text.contains("замен на точке пока не было"),
        "{}",
        none.text
    );
}
