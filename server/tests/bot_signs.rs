//! Подключение клиента в боте по двум признакам из трёх (задача 046).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sqlx::PgPool;
use uuid::Uuid;

use avtodom_server::api::bot_signs::BTN_SIGNS;
use avtodom_server::api::telegram::handle_message;
use avtodom_server::bootstrap;

async fn branch(pool: &PgPool) -> Uuid {
    bootstrap::ensure_owner(pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    sqlx::query_scalar("select id from branches limit 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn party(pool: &PgPool, b: Uuid, name: &str, inn: &str, plate: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone, inn) values ($1, $2, 'customer', 'person', $3, '+996700000001', $4)")
        .bind(id).bind(b).bind(name).bind(inn)
        .execute(pool).await.unwrap();
    if !plate.is_empty() {
        sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) values ($1, $2, $3, $4, plate_norm($4))")
            .bind(Uuid::now_v7()).bind(b).bind(id).bind(plate)
            .execute(pool).await.unwrap();
    }
    id
}

async fn say(pool: &PgPool, chat: i64, text: &str) -> String {
    let mut conn = pool.acquire().await.unwrap();
    handle_message(&mut conn, chat, chat, Some(text), None)
        .await
        .unwrap()
        .text
}

async fn linked(pool: &PgPool, chat: i64) -> Option<Uuid> {
    sqlx::query_scalar("select party_id from telegram_customers where chat_id = $1")
        .bind(chat)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn two_signs_link_one_sign_refuses(pool: PgPool) {
    let b = branch(&pool).await;
    let azis = party(&pool, b, "Касымов Азиз", "12345678901234", "01KG123ABC").await;
    assert!(say(&pool, 701, BTN_SIGNS).await.contains("два признака"));
    // Один признак — отказ без подробностей.
    let r = say(&pool, 701, "касымов   азиз").await;
    assert!(r.contains("Не нашли"), "{r}");
    assert_eq!(linked(&pool, 701).await, None);
    // Без кнопки сообщение не разбирается как признаки.
    let r = say(&pool, 701, "Касымов Азиз, 01 kg 123 авс").await;
    assert!(!r.contains("вы подключены"), "{r}");
    say(&pool, 701, BTN_SIGNS).await;
    let r = say(&pool, 701, "Касымов Азиз, 01 kg 123 авс").await;
    assert!(r.contains("вы подключены"), "{r}");
    assert_eq!(linked(&pool, 701).await, Some(azis));
    let via: String = sqlx::query_scalar(
        "select data->>'via' from audit_log where action = 'telegram.customer_link'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(via, "signs");
    // ИНН и госномер, без запятой.
    party(&pool, b, "Бек", "22345678901234", "02KG555XYZ").await;
    say(&pool, 702, BTN_SIGNS).await;
    assert!(
        say(&pool, 702, "22345678901234 02KG555XYZ")
            .await
            .contains("вы подключены")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn signs_of_different_cards_refuse(pool: PgPool) {
    let b = branch(&pool).await;
    party(&pool, b, "Касымов Азиз", "", "01KG123ABC").await;
    party(&pool, b, "Осмонов Бек", "12345678901234", "").await;
    say(&pool, 703, BTN_SIGNS).await;
    let r = say(&pool, 703, "Касымов Азиз, 12345678901234").await;
    assert!(r.contains("Не нашли"), "{r}");
    // Два одинаковых по двум признакам — не угадываем.
    party(&pool, b, "Касымов Азиз", "", "01KG123ABC").await;
    say(&pool, 703, BTN_SIGNS).await;
    assert!(
        say(&pool, 703, "Касымов Азиз, 01KG123ABC")
            .await
            .contains("Не нашли")
    );
    assert_eq!(linked(&pool, 703).await, None);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn five_failures_a_day(pool: PgPool) {
    let b = branch(&pool).await;
    party(&pool, b, "Касымов Азиз", "", "01KG123ABC").await;
    for _ in 0..5 {
        say(&pool, 704, BTN_SIGNS).await;
        assert!(
            say(&pool, 704, "Кто-то, 09KG000AAA")
                .await
                .contains("Не нашли")
        );
    }
    assert!(say(&pool, 704, BTN_SIGNS).await.contains("Слишком много"));
    // Даже верные признаки после лимита не подключают.
    say(&pool, 704, "Касымов Азиз, 01KG123ABC").await;
    assert_eq!(linked(&pool, 704).await, None);
    // Другой чат не задет.
    say(&pool, 705, BTN_SIGNS).await;
    assert!(
        say(&pool, 705, "Касымов Азиз, 01KG123ABC")
            .await
            .contains("вы подключены")
    );
}
