//! Сообщения клиентам (SPEC-32, ADR-107): канал (Телеграм, WhatsApp, «не отправлено»), ссылка
//! на бот с кодом клиента, напоминание о замене по среднему пробегу, сообщение фирме о закупе.
//! Телеграм и WhatsApp — подставные HTTP-серверы здесь же; настоящие в тестах не вызываются.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::client_messages::{self, Channels, WhatsApp};
use avtodom_server::api::telegram::{self, Api};
use avtodom_server::config::SecretKey;
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const WA_TOKEN: &str = "wa-test-token-5k1";
const WA_PHONE_ID: &str = "1065403522";
const TG_TOKEN: &str = "123456:tg-secret-token-cm1";

// ---------- Подставные серверы ----------

#[derive(Clone, Default)]
struct FakeTg {
    got: Arc<Mutex<Vec<(i64, String)>>>,
}

impl FakeTg {
    fn texts(&self, chat: i64) -> Vec<String> {
        self.got
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| *c == chat)
            .map(|(_, t)| t.clone())
            .collect()
    }
}

async fn tg_send(
    State(f): State<FakeTg>,
    axum::Json(body): axum::Json<Value>,
) -> axum::Json<Value> {
    f.got.lock().unwrap().push((
        body["chat_id"].as_i64().unwrap(),
        body["text"].as_str().unwrap().to_string(),
    ));
    axum::Json(json!({ "ok": true, "result": {} }))
}

#[derive(Clone, Default)]
struct FakeWa {
    /// Ключ из заголовка и тело.
    got: Arc<Mutex<Vec<(String, Value)>>>,
    /// Ответить ошибкой Meta.
    fail: Arc<Mutex<Option<(u16, Value)>>>,
}

impl FakeWa {
    fn bodies(&self) -> Vec<Value> {
        self.got
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect()
    }
}

async fn wa_send(
    State(f): State<FakeWa>,
    Path(phone_id): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, axum::Json<Value>) {
    assert_eq!(phone_id, WA_PHONE_ID);
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if auth != format!("Bearer {WA_TOKEN}") {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(
                json!({ "error": { "message": "Invalid OAuth access token", "code": 190 } }),
            ),
        );
    }
    if let Some((code, v)) = f.fail.lock().unwrap().clone() {
        return (StatusCode::from_u16(code).unwrap(), axum::Json(v));
    }
    let n = {
        let mut g = f.got.lock().unwrap();
        g.push((auth, body.clone()));
        g.len()
    };
    (
        StatusCode::OK,
        axum::Json(json!({
            "messaging_product": "whatsapp",
            "contacts": [{ "input": body["to"], "wa_id": "996555000000" }],
            "messages": [{ "id": format!("wamid.{n}") }]
        })),
    )
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

async fn fakes() -> (Api, FakeTg, WhatsApp, FakeWa) {
    let tg = FakeTg::default();
    let tg_url = serve(
        Router::new()
            .route("/{bot}/sendMessage", axum::routing::post(tg_send))
            .with_state(tg.clone()),
    )
    .await;
    let api =
        Api::new(&tg_url, TG_TOKEN).with_pace(std::time::Duration::ZERO, std::time::Duration::ZERO);
    let wa = FakeWa::default();
    let wa_url = serve(
        Router::new()
            .route("/{phone_id}/messages", axum::routing::post(wa_send))
            .with_state(wa.clone()),
    )
    .await;
    let whatsapp = WhatsApp::new(
        SecretKey(WA_TOKEN.into()),
        WA_PHONE_ID.into(),
        Some(format!("{wa_url}/")),
    );
    (api, tg, whatsapp, wa)
}

// ---------- Программа ----------

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string());
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let mut json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(c) = cookie
        && uri.ends_with("/auth/login")
    {
        json = json!(c);
    }
    (status, json)
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, Some(cookie), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn get(app: &Router, uri: &str, cookie: &str) -> Value {
    let (s, v) = call(app, "GET", uri, Some(cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    c.as_str().unwrap().to_string()
}

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    owner_id: Uuid,
    branch: Uuid,
    cashier: String,
    product: String,
    ch: Channels,
    tg: FakeTg,
    wa: FakeWa,
}

impl World {
    async fn new(pool: PgPool, public_url: Option<&str>) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let branch: Uuid = sqlx::query_scalar("select id from branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let hash = hash_password("admin-pass-1".into()).await.unwrap();
        let admin = Uuid::now_v7();
        sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Админ')")
            .bind(admin)
            .bind(branch)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
        let keys: Vec<String> = avtodom_server::auth::default_permissions()
            .into_iter()
            .map(String::from)
            .collect();
        sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
            .bind(admin)
            .bind(branch)
            .bind(keys)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into bot_state (key, value) values ('username', '\"avtodom_test_bot\"')",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Чеки «после установки»: начало рассылки закупов — час назад.
        sqlx::query("update bot_state set value = to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')) where key = 'client_messages_since'")
            .execute(&pool)
            .await
            .unwrap();
        let (api, tg, whatsapp, wa) = fakes().await;
        let mut state = build_state(pool.clone(), false).await.unwrap();
        state.telegram_api = Some(api);
        state.whatsapp = Some(whatsapp);
        state.public_url = public_url.map(str::to_string);
        let ch = Channels::from_state(&state);
        let app = app(state, None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        let e = ok(
            &app,
            "POST",
            "/api/v1/employees",
            &owner,
            json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" }),
        )
        .await;
        let cashier = e["id"].as_str().unwrap().to_string();
        let accounts = get(&app, "/api/v1/cash/accounts", &owner).await;
        let safe = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["owner_only"] == json!(true))
            .unwrap()["id"]
            .clone();
        let cat = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            json!({ "name": "Аксессуары", "kind": "other" }),
        )
        .await;
        let p = ok(
            &app,
            "POST",
            "/api/v1/products",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Щётка", "sale_price_tyiyn": 100_000 }),
        )
        .await;
        let product = p["id"].as_str().unwrap().to_string();
        ok(
            &app,
            "POST",
            "/api/v1/receipts",
            &owner,
            json!({
                "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": product, "qty": 100, "cost_tyiyn": 100 * 50_000 }]
            }),
        )
        .await;
        World {
            app,
            pool,
            owner,
            admin,
            owner_id,
            branch,
            cashier,
            product,
            ch,
            tg,
            wa,
        }
    }

    async fn settings(&self, oil: &str, purchase: &str) {
        ok(
            &self.app,
            "PUT",
            "/api/v1/settings/client-messages",
            &self.owner,
            json!({ "whatsapp_enabled": true, "oil_template": oil, "oil_language": "ru",
                    "purchase_template": purchase, "purchase_language": "ru", "km_per_day": 50 }),
        )
        .await;
    }

    async fn person(&self, first: &str, last: &str, phone: &str) -> Uuid {
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/parties",
            &self.owner,
            json!({ "role": "customer", "kind": "person", "first_name": first, "last_name": last, "phone": phone }),
        )
        .await;
        p["id"].as_str().unwrap().parse().unwrap()
    }

    async fn vehicle(
        &self,
        party: Uuid,
        plate: &str,
        brand: &str,
        model: &str,
        unit: &str,
    ) -> Uuid {
        let v = ok(
            &self.app,
            "POST",
            &format!("/api/v1/parties/{party}/vehicles"),
            &self.owner,
            json!({ "plate": plate, "brand": brand, "model": model, "mileage_unit": unit }),
        )
        .await;
        v["id"].as_str().unwrap().parse().unwrap()
    }

    /// Интервал строки книжки у машины.
    async fn interval(&self, vehicle: Uuid, slot: &str, km: i32, days: i32) {
        sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) values ($1, $2, $3, $4, $5)")
            .bind(vehicle)
            .bind(self.branch)
            .bind(slot)
            .bind(km)
            .bind(days)
            .execute(&self.pool)
            .await
            .unwrap();
    }

    /// Запись книжки `days_ago` дней назад: пробег в км (и как вписали — в единице машины).
    async fn change(
        &self,
        vehicle: Uuid,
        party: Uuid,
        days_ago: i32,
        (km, value, unit): (i32, i32, &str),
        slots: &[&str],
    ) {
        let id = Uuid::now_v7();
        sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km, mileage_value, mileage_unit, oil_text, user_id) values ($1, $2, $3, $4, (now() at time zone 'Asia/Bishkek')::date - $5, $6, $7, $8, 'масло', $9)")
            .bind(id)
            .bind(self.branch)
            .bind(vehicle)
            .bind(party)
            .bind(days_ago)
            .bind(km)
            .bind(value)
            .bind(unit)
            .bind(self.owner_id)
            .execute(&self.pool)
            .await
            .unwrap();
        for (i, s) in slots.iter().enumerate() {
            sqlx::query("insert into oil_change_items (id, branch_id, oil_change_id, slot, text, position) values ($1, $2, $3, $4, 'замена', $5)")
                .bind(Uuid::now_v7())
                .bind(self.branch)
                .bind(id)
                .bind(*s)
                .bind(i32::try_from(i).unwrap())
                .execute(&self.pool)
                .await
                .unwrap();
        }
    }

    async fn link_chat(&self, chat: i64, party: Uuid, phone: &str) {
        sqlx::query(
            "insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, $3)",
        )
        .bind(chat)
        .bind(party)
        .bind(phone)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn pass(&self, hour: u32) {
        let mut conn = self.pool.acquire().await.unwrap();
        client_messages::pass(&mut conn, &self.ch, hour, true)
            .await
            .unwrap();
    }

    async fn messages(&self) -> Vec<Value> {
        get(&self.app, "/api/v1/client-messages", &self.owner)
            .await
            .as_array()
            .unwrap()
            .clone()
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by created_at, id")
            .bind(action)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

fn params(body: &Value) -> Vec<String> {
    body["template"]["components"][0]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            assert_eq!(p["type"], "text");
            p["text"].as_str().unwrap().to_string()
        })
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_reminder_by_average_mileage_and_channel(pool: PgPool) {
    let w = World::new(pool, None).await;
    w.settings("avtodom_oil", "avtodom_purchase").await;

    // 1. Подключён к боту — Телеграм. Средний пробег: 8 000 км за 100 дней = 80 км в день;
    // масло — интервал 1 760 км от последнего заезда 20 дней назад → через 2 дня; коробка —
    // 2 000 км → через 5 дней (в те же 14 дней — тем же сообщением); воздушный — 5 000 км →
    // через 42 дня (не в этом сообщении).
    let azis = w.person("Азиз", "Касымов", "0555 11 22 33").await;
    let camry = w.vehicle(azis, "01KG123ABC", "Toyota", "Camry", "km").await;
    w.interval(camry, "oil", 1760, 365).await;
    w.interval(camry, "gear_oil", 2000, 730).await;
    w.interval(camry, "air_filter", 5000, 365).await;
    w.change(camry, azis, 120, (100_000, 100_000, "km"), &["oil"])
        .await;
    w.change(
        camry,
        azis,
        20,
        (108_000, 108_000, "km"),
        &["oil", "gear_oil", "air_filter"],
    )
    .await;
    w.link_chat(501, azis, "0555 11 22 33").await;

    // 2. Не подключён, есть телефон — WhatsApp по шаблону. Одна запись — 50 км в день по
    // умолчанию: интервал 600 км от заезда 10 дней назад → через 2 дня. Машина в милях.
    let bek = w.person("Бек", "Осмонов", "+996 700 99 88 77").await;
    let ford = w.vehicle(bek, "01KG777AAA", "Ford", "F-150", "mi").await;
    w.interval(ford, "oil", 600, 365).await;
    // 50 000 км = 31 069 миль; цель 50 600 км = 31 441 миля.
    w.change(ford, bek, 10, (50_000, 31_069, "mi"), &["oil"])
        .await;

    // 3. Ни бота, ни телефона — «не отправлено».
    let nobody = w.person("Нурлан", "Без", "").await;
    let golf = w.vehicle(nobody, "01KG000NNN", "VW", "Golf", "km").await;
    w.interval(golf, "oil", 600, 365).await;
    w.change(golf, nobody, 10, (70_000, 70_000, "km"), &["oil"])
        .await;

    // Ночью ничего не составляется и не уходит.
    w.pass(22).await;
    assert!(w.messages().await.is_empty());

    w.pass(10).await;
    let tg = w.tg.texts(501);
    assert_eq!(tg.len(), 1, "{tg:?}");
    assert_eq!(
        tg[0],
        "Азиз, здравствуйте! Магазин Avtodom. Не забудьте поменять масло на Toyota Camry 01KG123ABC на пробеге 109 760 км. А масло в коробке — на 110 000 км."
    );
    // В Телеграме нет ссылки и кода.
    assert!(!tg[0].contains("t.me"));

    let wa = w.wa.bodies();
    assert_eq!(wa.len(), 1, "{wa:?}");
    let code: String = sqlx::query_scalar("select bot_code from parties where id = $1")
        .bind(bek)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(wa[0]["messaging_product"], "whatsapp");
    assert_eq!(wa[0]["to"], "+996700998877");
    assert_eq!(wa[0]["type"], "template");
    assert_eq!(wa[0]["template"]["name"], "avtodom_oil");
    assert_eq!(wa[0]["template"]["language"]["code"], "ru");
    assert_eq!(
        params(&wa[0]),
        vec![
            "Бек".to_string(),
            "масло".into(),
            "Ford F-150 01KG777AAA".into(),
            "31 441 миля".into(),
            format!("https://t.me/avtodom_test_bot?start={code}"),
        ]
    );

    let list = w.messages().await;
    assert_eq!(list.len(), 3);
    let by = |party: Uuid| {
        list.iter()
            .find(|m| m["party_id"] == json!(party))
            .unwrap()
            .clone()
    };
    assert_eq!(by(azis)["channel"], "telegram");
    assert_eq!(by(azis)["status"], "sent");
    assert_eq!(by(bek)["channel"], "whatsapp");
    assert_eq!(by(bek)["status"], "sent");
    assert_eq!(by(bek)["template"], "avtodom_oil");
    assert_eq!(by(nobody)["channel"], "none");
    assert_eq!(by(nobody)["status"], "not_sent");
    assert!(
        by(nobody)["error"]
            .as_str()
            .unwrap()
            .contains("телефона нет")
    );

    // Не дважды: ни на следующем проходе, ни через час.
    w.pass(11).await;
    w.pass(12).await;
    assert_eq!(w.tg.texts(501).len(), 1);
    assert_eq!(w.wa.bodies().len(), 1);
    assert_eq!(w.messages().await.len(), 3);

    // Прежнее напоминание бота по дням для этих машин не уходит: срок считается по пробегу.
    let mut conn = w.pool.acquire().await.unwrap();
    let old = telegram::due_reminders(&mut conn).await.unwrap();
    assert!(
        old.iter().all(|r| r.vehicle_id != camry),
        "{:?}",
        old.iter().map(|r| &r.text).collect::<Vec<_>>()
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn whatsapp_off_or_error_is_written(pool: PgPool) {
    let w = World::new(pool, None).await;
    // Шаблон не задан — WhatsApp не используется.
    w.settings("", "").await;
    let bek = w.person("Бек", "Осмонов", "+996 700 99 88 77").await;
    let car = w.vehicle(bek, "01KG777AAA", "Ford", "Focus", "km").await;
    w.interval(car, "oil", 600, 365).await;
    w.change(car, bek, 10, (50_000, 50_000, "km"), &["oil"])
        .await;
    w.pass(10).await;
    let list = w.messages().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["status"], "not_sent");
    assert!(list[0]["error"].as_str().unwrap().contains("шаблон"));
    assert!(w.wa.bodies().is_empty());

    // Шаблон не одобрен Meta — «ошибка» с причиной, без повторов.
    w.settings("avtodom_oil", "avtodom_purchase").await;
    *w.wa.fail.lock().unwrap() = Some((
        400,
        json!({ "error": { "message": "Template name does not exist in the translation", "code": 132001,
                           "error_data": { "details": "template name (avtodom_oil) does not exist in ru" } } }),
    ));
    let other = w.vehicle(bek, "01KG778AAA", "Ford", "Fiesta", "km").await;
    w.interval(other, "oil", 600, 365).await;
    w.change(other, bek, 10, (20_000, 20_000, "km"), &["oil"])
        .await;
    w.pass(10).await;
    let list = w.messages().await;
    let failed = list.iter().find(|m| m["channel"] == "whatsapp").unwrap();
    assert_eq!(failed["status"], "error");
    assert!(failed["error"].as_str().unwrap().contains("132001"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_link_with_client_code(pool: PgPool) {
    let w = World::new(pool, None).await;
    let azis = w.person("Азиз", "Касымов", "").await;
    let link = get(
        &w.app,
        &format!("/api/v1/parties/{azis}/bot-link"),
        &w.owner,
    )
    .await;
    let url = link["url"].as_str().unwrap().to_string();
    let code = url
        .strip_prefix("https://t.me/avtodom_test_bot?start=")
        .unwrap()
        .to_string();
    assert_eq!(code.len(), 32);
    // Код — не номер по порядку: у другого клиента совсем другой.
    let bek = w.person("Бек", "Осмонов", "").await;
    let other = get(&w.app, &format!("/api/v1/parties/{bek}/bot-link"), &w.owner).await;
    assert_ne!(other["url"], link["url"]);
    // Ссылку видит только владелец; в карточке клиента и кабинете кода нет.
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/parties/{azis}/bot-link"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let card = get(&w.app, &format!("/api/v1/parties/{azis}/card"), &w.admin).await;
    assert!(!card.to_string().contains(&code));

    // «Старт» по ссылке привязывает чат сразу, без номера.
    let mut conn = w.pool.acquire().await.unwrap();
    let r = telegram::handle_message(&mut conn, 601, 601, Some(&format!("/start {code}")), None)
        .await
        .unwrap();
    assert!(r.text.contains("вы подключены"), "{}", r.text);
    // В перечне кнопок — и «📜 Прошлые ТО» (SPEC-36 §7).
    assert!(
        r.text
            .contains("«🚗 Мои машины», «📜 Прошлые ТО», «⭐ Баллы», «💳 Мой долг»"),
        "{}",
        r.text
    );
    assert!(!r.text.contains(&code));
    let linked: Uuid =
        sqlx::query_scalar("select party_id from telegram_customers where chat_id = 601")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(linked, azis);
    // Подключение держится и дальше: книжка клиента по любому тексту.
    let r = telegram::handle_message(&mut conn, 601, 601, Some("⭐ Баллы"), None)
        .await
        .unwrap();
    assert!(!r.text.contains("Поделиться номером"), "{}", r.text);

    // «Новая ссылка»: старая больше не работает, подключённый чат остаётся.
    let fresh = ok(
        &w.app,
        "POST",
        &format!("/api/v1/parties/{azis}/bot-link/reset"),
        &w.owner,
        json!({}),
    )
    .await;
    assert_ne!(fresh["url"], link["url"]);
    let r = telegram::handle_message(&mut conn, 602, 602, Some(&format!("/start {code}")), None)
        .await
        .unwrap();
    assert!(r.text.contains("Ссылка устарела"), "{}", r.text);
    let n: i64 = sqlx::query_scalar("select count(*) from telegram_customers where chat_id = 602")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert_eq!(w.audit("client.bot_link_reset").await.len(), 1);
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/parties/{azis}/bot-link/reset"),
        Some(&w.admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

impl World {
    async fn firm(&self, name: &str, phone: &str) -> (Uuid, Uuid, Uuid) {
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/parties",
            &self.owner,
            json!({ "role": "customer", "kind": "company", "name": name, "phone": phone, "inn": "01234567890123" }),
        )
        .await;
        let firm: Uuid = p["id"].as_str().unwrap().parse().unwrap();
        let c = ok(
            &self.app,
            "POST",
            &format!("/api/v1/parties/{firm}/contacts"),
            &self.owner,
            json!({ "full_name": "Чынтемир" }),
        )
        .await;
        let driver: Uuid = c["id"].as_str().unwrap().parse().unwrap();
        let v = self.vehicle(firm, "09KG111JJJ", "", "Sprinter", "km").await;
        (firm, driver, v)
    }

    /// Чек фирмы в долг на `amount` (штук по 1 000 сом).
    async fn firm_sale(&self, firm: Uuid, driver: Uuid, car: Uuid, qty: i64) -> Value {
        let amount = qty * 100_000;
        ok(
            &self.app,
            "POST",
            "/api/v1/sales",
            &self.admin,
            json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
                "party_id": firm, "contact_id": driver, "vehicle_id": car,
                "lines": [{ "kind": "piece", "product_id": self.product, "qty": qty, "unit_price_tyiyn": 100_000 }],
                "payments": [{ "method": "debt", "amount_tyiyn": amount }]
            }),
        )
        .await
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn firm_purchase_message_now_or_morning(pool: PgPool) {
    let w = World::new(pool, Some("https://avtodom.example")).await;
    w.settings("avtodom_oil", "avtodom_purchase").await;
    let (firm, driver, car) = w.firm("ОсОО Бишкек такси", "0555 44 55 66").await;
    // Руководитель фирмы — его имя в обращении.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/parties/{firm}/contacts"),
        &w.owner,
        json!({ "full_name": "Азиз", "phone": "0700 12 34 56", "is_head": true }),
    )
    .await;

    // Чек — сообщение сразу (в окно), долг на сейчас, ссылка на кабинет по PUBLIC_URL.
    w.firm_sale(firm, driver, car, 1).await;
    w.pass(10).await;
    let wa = w.wa.bodies();
    assert_eq!(wa.len(), 1, "{wa:?}");
    assert_eq!(wa[0]["to"], "+996555445566");
    assert_eq!(wa[0]["template"]["name"], "avtodom_purchase");
    assert_eq!(
        params(&wa[0]),
        vec![
            "Азиз (ОсОО Бишкек такси)".to_string(),
            "Сегодня был закуп".into(),
            "1 000 сом".into(),
            "Чынтемир".into(),
            "Sprinter 09KG111JJJ".into(),
            "1 000 сом".into(),
            "https://avtodom.example/cabinet".into(),
        ]
    );

    // Второй чек — долг уже общий.
    let sale = w.firm_sale(firm, driver, car, 2).await;
    w.pass(10).await;
    let wa = w.wa.bodies();
    assert_eq!(wa.len(), 2);
    assert_eq!(params(&wa[1])[2], "2 000 сом");
    assert_eq!(params(&wa[1])[5], "3 000 сом");
    // Тот же чек второй раз не уходит.
    w.pass(11).await;
    assert_eq!(w.wa.bodies().len(), 2);

    // Ночью — не сразу, а утром.
    let sale_number = sale["number"].clone();
    w.firm_sale(firm, driver, car, 1).await;
    w.pass(21).await;
    assert_eq!(w.wa.bodies().len(), 2);
    w.pass(9).await;
    let wa = w.wa.bodies();
    assert_eq!(wa.len(), 3);
    assert_eq!(params(&wa[2])[5], "4 000 сом");

    // Руководитель подключил свою карточку к боту — фирме в Телеграм, текстом как у заказчика.
    let head = w.person("Азиз", "Руководитель", "0700 12 34 56").await;
    w.link_chat(701, head, "0700 12 34 56").await;
    w.firm_sale(firm, driver, car, 1).await;
    w.pass(10).await;
    let tg = w.tg.texts(701);
    assert_eq!(tg.len(), 1);
    assert_eq!(
        tg[0],
        "Азиз (ОсОО Бишкек такси), здравствуйте! Сегодня был закуп на сумму 1 000 сом. Водитель Чынтемир, машина Sprinter 09KG111JJJ. Общий долг на сегодня — 5 000 сом. Для полного ознакомления пройдите в личный кабинет: https://avtodom.example/cabinet"
    );
    assert_eq!(w.wa.bodies().len(), 3);

    // Флажок «Не присылать сообщения о закупах» отключает.
    let p = ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/parties/{firm}"),
        &w.admin,
        json!({ "no_purchase_messages": true }),
    )
    .await;
    assert_eq!(p["no_purchase_messages"], true);
    w.firm_sale(firm, driver, car, 1).await;
    w.pass(10).await;
    assert_eq!(w.tg.texts(701).len(), 1);
    assert_eq!(w.wa.bodies().len(), 3);

    let list = w.messages().await;
    assert_eq!(list.iter().filter(|m| m["kind"] == "purchase").count(), 4);
    assert!(list.iter().any(|m| m["sale_number"] == sale_number));
    let _ = driver;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn firm_link_to_bot_without_public_url_and_test_send(pool: PgPool) {
    let w = World::new(pool, None).await;
    w.settings("avtodom_oil", "avtodom_purchase").await;
    let (firm, driver, car) = w.firm("ОсОО Ала-Тоо", "0555 00 11 22").await;
    w.firm_sale(firm, driver, car, 1).await;
    w.pass(10).await;
    let code: String = sqlx::query_scalar("select bot_code from parties where id = $1")
        .bind(firm)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let wa = w.wa.bodies();
    assert_eq!(params(&wa[0])[0], "ОсОО Ала-Тоо");
    assert_eq!(
        params(&wa[0])[6],
        format!("https://t.me/avtodom_test_bot?start={code}")
    );

    // «Отправить пробное» — сразу, в журнале с отметкой; только владелец.
    let sent = ok(
        &w.app,
        "POST",
        "/api/v1/client-messages/test",
        &w.owner,
        json!({ "party_id": firm, "kind": "purchase" }),
    )
    .await;
    assert_eq!(sent[0]["status"], "sent");
    assert_eq!(sent[0]["test"], true);
    assert_eq!(w.wa.bodies().len(), 2);
    assert_eq!(w.audit("client_message.send").await.len(), 1);
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/client-messages/test",
        Some(&w.admin),
        Some(json!({ "party_id": firm, "kind": "oil" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "GET",
        "/api/v1/client-messages",
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(w.audit("client_message.settings").await.len(), 1);
    let st = get(&w.app, "/api/v1/settings/client-messages", &w.owner).await;
    assert_eq!(st["whatsapp_configured"], true);
    assert_eq!(st["oil_params"].as_array().unwrap().len(), 5);
    assert_eq!(st["purchase_params"].as_array().unwrap().len(), 7);
}
