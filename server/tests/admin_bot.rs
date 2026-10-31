//! Администратор в боте (SPEC-40): меню по правам, «Нет права», смена без прибыли, остаток без
//! себестоимости, расход из кассы смены тем же путём, что черновик владельца, лента — только
//! включённое владельцем; ни один ответ администратору не несёт денег владельца (I13).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::bot_admin::{
    self, BTN_BOOK, BTN_EXPENSE, BTN_SHIFT_NOW, BTN_STOCK, ITEMS, leaks,
};
use avtodom_server::api::bot_debtors::{BTN_ADD_DEBT, BTN_DEBTORS};
use avtodom_server::api::telegram::{
    self, BTN_INVOICE, BTN_MY_INVOICES, Keys, Outgoing, Reply, mark_sent,
};
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const OWNER_CHAT: i64 = 710_001;
const ADMIN_CHAT: i64 = 710_002;
/// Себестоимость тестового товара за штуку — 777,77 с; приход 10 шт на 7 777,70 с.
const COST_UNIT: &str = "777,77";
const COST_TOTAL: &str = "7 777,7";

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
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
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, cookie, json)
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (status, cookie, _) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    cookie.unwrap()
}

async fn ok(app: &Router, who: &str, method: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, method, uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// Мир: владелец и администратор (права по умолчанию) с привязанными чатами, кассир, поставщик,
/// товар с приходом 10 шт по 777,77 с (цена продажи 1 500 с), машина клиента, открытая смена и
/// чек наличными.
struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    admin_id: Uuid,
    cashier: String,
    supplier: String,
    item: String,
}

async fn world(pool: PgPool) -> W {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let hash = hash_password("admin-pass-1".into()).await.unwrap();
    let admin_id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Эрбол')")
        .bind(admin_id)
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
        .bind(admin_id)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
    for (chat, login) in [(OWNER_CHAT, "owner"), (ADMIN_CHAT, "admin")] {
        sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, linked_at, tg_user_id) select $1, id, branch_id, now() - interval '1 hour', $1 from users where login = $2")
            .bind(chat)
            .bind(login)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into bot_state (key, value) values
           ('cursor', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))),
           ('feed_since', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')))
         on conflict (key) do update set value = excluded.value",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = id_of(
        &ok(
            &app,
            &owner,
            "POST",
            "/api/v1/employees",
            json!({ "full_name": "Кассир Айбек", "is_cashier": true, "pin": "1234" }),
        )
        .await,
    );
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Масла", "kind": "other" })),
    )
    .await;
    let item = id_of(
        &ok(
            &app,
            &owner,
            "POST",
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Присадка Тест",
                    "article": "PT-77", "sale_price_tyiyn": 150_000 }),
        )
        .await,
    );
    let supplier = id_of(
        &ok(
            &app,
            &owner,
            "POST",
            "/api/v1/suppliers",
            json!({ "name": "Поставщик Тест" }),
        )
        .await,
    );
    let party = id_of(
        &ok(
            &app,
            &owner,
            "POST",
            "/api/v1/parties",
            json!({ "name": "Бакыт", "kind": "person", "phone": "+996 555 111 222" }),
        )
        .await,
    );
    sqlx::query(
        "insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, brand, model)
         values ($1, $2, $3::uuid, '01KG777ABC', plate_norm('01KG777ABC'), 'Toyota', 'Camry')",
    )
    .bind(Uuid::now_v7())
    .bind(branch)
    .bind(&party)
    .execute(&pool)
    .await
    .unwrap();
    let w = W {
        app,
        pool,
        owner,
        admin,
        admin_id,
        cashier,
        supplier,
        item,
    };
    w.receipt().await;
    ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": w.cashier, "cashier_pin": "1234" }),
    )
    .await;
    ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/sales",
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
            "lines": [{ "kind": "piece", "product_id": w.item, "qty": 1, "unit_price_tyiyn": 150_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 150_000 }]
        }),
    )
    .await;
    w
}

impl W {
    /// Приход 10 шт на 7 777,70 с — себестоимость 777,77 с за штуку.
    async fn receipt(&self) -> Value {
        ok(
            &self.app,
            &self.owner,
            "POST",
            "/api/v1/receipts",
            json!({ "op_id": Uuid::now_v7(), "supplier_id": self.supplier, "payment": "debt",
                    "supplier_doc": "Н-15",
                    "lines": [{ "product_id": self.item, "qty": 10, "cost_tyiyn": 777_770 }] }),
        )
        .await
    }

    async fn say(&self, text: &str) -> Reply {
        let mut conn = self.pool.acquire().await.unwrap();
        telegram::handle_message(&mut conn, ADMIN_CHAT, ADMIN_CHAT, Some(text), None)
            .await
            .unwrap()
    }

    async fn press(&self, data: &str) -> Reply {
        let mut conn = self.pool.acquire().await.unwrap();
        telegram::handle_callback_from(&mut conn, None, ADMIN_CHAT, ADMIN_CHAT, data)
            .await
            .unwrap()
    }

    async fn grant(&self, keys: &[&str]) {
        ok(
            &self.app,
            &self.owner,
            "PUT",
            &format!("/api/v1/users/{}/permissions", self.admin_id),
            json!({ "op_id": Uuid::now_v7(), "permissions": keys }),
        )
        .await;
    }

    async fn defaults_without(&self, off: &[&str]) {
        let keys: Vec<&str> = avtodom_server::auth::default_permissions()
            .into_iter()
            .filter(|k| !off.contains(k))
            .collect();
        self.grant(&keys).await;
    }

    /// Лента администраторов сейчас; всё отмечается доставленным.
    async fn admin_feed(&self) -> Vec<Outgoing> {
        let mut conn = self.pool.acquire().await.unwrap();
        let out = bot_admin::pending(&mut conn).await.unwrap();
        for m in &out {
            mark_sent(&mut conn, m).await.unwrap();
        }
        out
    }

    /// Лента владельцев сейчас; всё отмечается доставленным.
    async fn owner_feed(&self) -> Vec<Outgoing> {
        let mut conn = self.pool.acquire().await.unwrap();
        let out = telegram::pending(&mut conn).await.unwrap();
        for m in &out {
            mark_sent(&mut conn, m).await.unwrap();
        }
        out
    }

    async fn close_shift(&self) {
        let (_, _, shift) = call(
            &self.app,
            "GET",
            "/api/v1/shifts/current",
            Some(&self.admin),
            None,
        )
        .await;
        let expected = shift["expected_tyiyn"].as_i64().unwrap();
        ok(
            &self.app,
            &self.admin,
            "POST",
            &format!("/api/v1/shifts/{}/close", id_of(&shift)),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "", "payouts": [] }),
        )
        .await;
    }
}

/// I13: в тексте для администратора нет слов о прибыли и сумм себестоимости тестового товара.
fn clean(what: &str, text: &str) {
    assert!(!leaks(text), "{what}: деньги владельца\n{text}");
    for bad in [
        "прибыл",
        "себестоим",
        "маржа",
        "валов",
        "чистая",
        COST_UNIT,
        COST_TOTAL,
    ] {
        assert!(
            !text.to_lowercase().contains(bad),
            "{what}: «{bad}»\n{text}"
        );
    }
}

fn clean_reply(what: &str, r: &Reply) {
    clean(what, &r.text);
    for b in r.inline.iter().flatten() {
        clean(what, &b.text);
    }
    if let Some(f) = &r.follow {
        clean(what, &f.text);
    }
}

fn menu_of(r: &Reply) -> bot_admin::Menu {
    match r.keys {
        Keys::Admin(m) => m,
        other => panic!("не кнопки администратора: {other:?}"),
    }
}

fn data_of(r: &Reply, text: &str) -> String {
    r.inline
        .iter()
        .flatten()
        .find(|b| b.text.starts_with(text))
        .unwrap_or_else(|| panic!("нет кнопки «{text}»: {:?}", r.inline.len()))
        .data
        .clone()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_menu_follows_rights_and_hides_owner_money(pool: PgPool) {
    let w = world(pool).await;
    // Права по умолчанию: всё, кроме «Старые долги из тетради».
    let r = w.say("привет").await;
    let menu = menu_of(&r);
    for key in [
        "invoice",
        "my_invoices",
        "shift",
        "stock",
        "debtors",
        "expense",
        "book",
    ] {
        assert!(menu.has(key), "{key}");
    }
    assert!(!menu.has("add_debt"));
    assert!(r.text.starts_with("Что можно в боте:"), "{}", r.text);
    assert!(r.text.contains(BTN_STOCK) && !r.text.contains(BTN_ADD_DEBT));
    let keyboard = r.keys.markup()["keyboard"].to_string();
    assert!(
        keyboard.contains(BTN_SHIFT_NOW) && !keyboard.contains(BTN_ADD_DEBT),
        "{keyboard}"
    );

    // Каждый пункт меню отвечает без денег владельца.
    let shift = w.say(BTN_SHIFT_NOW).await;
    clean_reply("смена", &shift);
    for part in [
        "🧾 Смена № 1 · кассир Кассир Айбек",
        "Выручка: 1 500 с · чеков 1",
        "наличные: 1 500 с",
        "должно быть:",
    ] {
        assert!(shift.text.contains(part), "{part}\n{}", shift.text);
    }
    // Владельцу та же смена — с прибылью: проверка не пустая.
    let mut conn = w.pool.acquire().await.unwrap();
    let owner_shift =
        telegram::handle_message(&mut conn, OWNER_CHAT, OWNER_CHAT, Some("💵 Смена"), None)
            .await
            .unwrap();
    assert!(
        owner_shift.text.contains("валовая прибыль"),
        "{}",
        owner_shift.text
    );
    // Функции ответов сами по себе — без защиты `guard` — не несут денег владельца.
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let admin_user = avtodom_server::auth::CurrentUser {
        id: w.admin_id,
        branch_id: branch,
        login: "admin".into(),
        full_name: "Эрбол".into(),
        role: avtodom_server::auth::Role::Admin,
    };
    clean(
        "смена без защиты",
        &bot_admin::shift_now(&mut conn, branch).await.unwrap(),
    );
    clean(
        "остаток без защиты",
        &bot_admin::stock_text(&mut conn, &admin_user, "присадка")
            .await
            .unwrap(),
    );
    clean(
        "книжка без защиты",
        &bot_admin::book_text(&mut conn, "01KG777ABC").await.unwrap(),
    );
    drop(conn);

    let r = w.say(BTN_STOCK).await;
    assert!(r.text.starts_with("📦 Остаток: напишите"), "{}", r.text);
    let stock = w.say("присадка").await;
    clean_reply("остаток", &stock);
    assert!(
        stock
            .text
            .contains("• Присадка Тест (арт. PT-77) — 9 шт · цена 1 500 с"),
        "{}",
        stock.text
    );
    // Поиск остаётся открытым; по коду — тоже.
    let again = w.say("PT-77").await;
    assert!(again.text.contains("Присадка Тест"), "{}", again.text);
    let direct = w.say("/остаток присадка").await;
    assert!(direct.text.contains("9 шт"), "{}", direct.text);

    let r = w.say(BTN_BOOK).await;
    assert!(r.text.starts_with("📒 Книжка"), "{}", r.text);
    let book = w.say("01 kg 777").await;
    clean_reply("книжка", &book);
    assert!(
        book.text
            .starts_with("🚗 01KG777ABC (Toyota Camry) · Бакыт"),
        "{}",
        book.text
    );

    for b in [BTN_INVOICE, BTN_MY_INVOICES, BTN_DEBTORS, "/day", "что-то"] {
        let r = w.say(b).await;
        clean_reply(b, &r);
        assert!(!r.text.starts_with("Нет права"), "{b}: {}", r.text);
    }

    // «💸 Расход» — статья, сумма, без комментария → черновик → «✅ Провести».
    let r = w.say(BTN_EXPENSE).await;
    clean_reply("расход", &r);
    assert!(r.text.starts_with("💸 Расход из кассы смены"), "{}", r.text);
    assert!(
        r.inline
            .iter()
            .flatten()
            .all(|b| b.text != "Личные расходы"),
        "статьи владельца администратору не показываются"
    );
    let r = w.press(&data_of(&r, "Хозтовары")).await;
    assert!(r.text.contains("Сумма"), "{}", r.text);
    let r = w.say("250").await;
    assert!(r.text.starts_with("Комментарий"), "{}", r.text);
    let r = w.press("ax:skip").await;
    clean_reply("черновик", &r);
    assert!(
        r.text
            .starts_with("Расход\nСтатья: Хозтовары\nСумма: 250 с"),
        "{}",
        r.text
    );
    let go = data_of(&r, "✅ Провести");
    // Нажать может только тот, кто привязал чат.
    let mut conn = w.pool.acquire().await.unwrap();
    let other = telegram::handle_callback_from(&mut conn, None, ADMIN_CHAT, 1, &go)
        .await
        .unwrap();
    drop(conn);
    assert!(other.text.contains("только тот"), "{}", other.text);
    let done = w.press(&go).await;
    assert!(
        done.text
            .starts_with("Проведено: расход № 1 — Хозтовары, 250 с"),
        "{}",
        done.text
    );
    // Повтор нажатия — тот же расход, не второй (I8).
    let again = w.press(&go).await;
    assert!(again.text.starts_with("Уже проведено"), "{}", again.text);
    let (n, by, till): (i64, Uuid, bool) = sqlx::query_as(
        "select count(*) over (), e.user_id, a.is_default and a.kind = 'register'
         from expenses e join cash_accounts a on a.id = e.account_id",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!((n, by, till), (1, w.admin_id, true));
    let shift = w.say(BTN_SHIFT_NOW).await;
    assert!(
        shift.text.contains("💸 Расходы смены: 250 с (1)"),
        "{}",
        shift.text
    );

    // Права сняли — меню со следующего сообщения, команда — «Нет права: …».
    w.say(BTN_STOCK).await;
    w.defaults_without(&["view_stock", "expenses", "clients", "receipts"])
        .await;
    let r = w.say("присадка").await;
    let menu = menu_of(&r);
    assert!(!menu.has("stock") && !menu.has("expense") && !menu.has("book"));
    assert!(!menu.has("invoice") && menu.has("shift"));
    assert!(r.text.starts_with("Что можно в боте:"), "{}", r.text);
    let keyboard = r.keys.markup()["keyboard"].to_string();
    for gone in [BTN_STOCK, BTN_EXPENSE, BTN_BOOK, BTN_INVOICE] {
        assert!(!keyboard.contains(gone), "{gone}: {keyboard}");
    }
    for (b, label) in [
        (BTN_STOCK, "Нет права: Остатки и продаваемые"),
        ("/расход", "Нет права: Расходы"),
        (BTN_BOOK, "Нет права: Клиенты и машины"),
        (BTN_MY_INVOICES, "Нет права: Приход товара"),
    ] {
        let r = w.say(b).await;
        assert!(r.text.starts_with(label), "{b}: {}", r.text);
    }
    // Шаги расхода без права не продолжаются.
    w.defaults_without(&[]).await;
    let r = w.say(BTN_EXPENSE).await;
    w.defaults_without(&["expenses"]).await;
    let r = w.press(&data_of(&r, "Хозтовары")).await;
    assert!(r.text.starts_with("Нет права: Расходы"), "{}", r.text);

    // Каждый пункт меню со всеми правами — ещё раз через защиту: ни одного слова о прибыли.
    w.grant(
        &avtodom_server::auth::PERMISSIONS
            .iter()
            .map(|p| p.key)
            .collect::<Vec<_>>(),
    )
    .await;
    for item in &ITEMS {
        let r = w.say(item.button).await;
        clean_reply(item.button, &r);
        assert!(
            !r.text.starts_with("Нет права"),
            "{}: {}",
            item.button,
            r.text
        );
        w.say("/отмена").await;
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_feed_has_only_enabled_events_without_owner_money(pool: PgPool) {
    let w = world(pool).await;
    // По умолчанию лента администратору не идёт (ADR-135).
    w.owner_feed().await;
    assert!(w.admin_feed().await.is_empty());
    w.receipt().await;
    assert!(w.admin_feed().await.is_empty());

    // Владелец включает события; чужое событие — 422; повтор `op_id` — одна запись журнала.
    let uri = format!("/api/v1/users/{}/bot-feed", w.admin_id);
    let (s, _, _) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "on": ["sale.post"] })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let body = json!({ "op_id": Uuid::now_v7(),
                       "on": ["shift.close", "receipt.post", "expense.post", "shift.open"] });
    for _ in 0..2 {
        ok(&w.app, &w.owner, "PUT", &uri, body.clone()).await;
    }
    let logged: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'user.bot_feed'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(logged, 1);

    // Приход — номер, поставщик, строки, без сумм.
    let rec = w.receipt().await;
    let msgs = w.admin_feed().await;
    assert_eq!(
        msgs.len(),
        1,
        "{:?}",
        msgs.iter().map(|m| &m.text).collect::<Vec<_>>()
    );
    assert_eq!(msgs[0].chat_id, ADMIN_CHAT);
    let text = &msgs[0].text;
    clean("приход", text);
    assert!(
        text.starts_with(&format!(
            "📦 Приход № {}\nПоставщик Тест, накладная № Н-15 — 1 строка",
            rec["number"].as_i64().unwrap()
        )),
        "{text}"
    );
    // Расход владельца со счёта — не из кассы смены — администратору не приходит.
    let articles: Value = {
        let (_, _, v) = call(
            &w.app,
            "GET",
            "/api/v1/expense-articles",
            Some(&w.owner),
            None,
        )
        .await;
        v
    };
    let personal = articles
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"] == "Личные расходы"))
        .map(id_of);
    if let Some(article) = personal {
        ok(
            &w.app,
            &w.owner,
            "POST",
            "/api/v1/expenses",
            json!({ "op_id": Uuid::now_v7(), "article_id": article, "amount_tyiyn": 10_000,
                    "comment": "обед" }),
        )
        .await;
        assert!(w.admin_feed().await.is_empty(), "личный расход владельца");
    }

    // Закрытие смены: владельцу — с прибылью, администратору — без.
    w.owner_feed().await;
    w.close_shift().await;
    let owner = w.owner_feed().await;
    assert!(
        owner
            .iter()
            .any(|m| m.text.contains("🔴 Смена № 1 закрыта") && m.text.contains("валовая")),
        "владельцу — с прибылью"
    );
    let msgs = w.admin_feed().await;
    let closed = msgs
        .iter()
        .find(|m| m.text.starts_with("🔴 Смена № 1 закрыта"))
        .unwrap_or_else(|| panic!("нет закрытия: {msgs:?}", msgs = msgs.len()));
    clean("закрытие смены", &closed.text);
    for part in ["Выручка: 1 500 с", "пересчитали:", "сошлось"] {
        assert!(closed.text.contains(part), "{part}\n{}", closed.text);
    }

    // Право события сняли — событие не приходит и не держит ленту.
    w.defaults_without(&["receipts"]).await;
    w.receipt().await;
    assert!(w.admin_feed().await.is_empty());
    assert!(w.admin_feed().await.is_empty());

    // Экран «Что видит в боте»: пункты по правам и выключатели.
    let (s, _, v) = call(
        &w.app,
        "GET",
        &format!("/api/v1/users/{}/bot", w.admin_id),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["linked"], true);
    let item = |key: &str| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["key"] == key)
            .cloned()
            .unwrap()
    };
    assert_eq!(item("shift")["on"], true);
    assert_eq!(item("invoice")["on"], false);
    assert_eq!(
        item("invoice")["need"],
        "Нет права: Приход товара и оплата поставщику"
    );
    let receipt_ev = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["events"].as_array().unwrap().iter())
        .find(|e| e["action"] == "receipt.post")
        .cloned()
        .unwrap();
    assert_eq!(
        (&receipt_ev["on"], &receipt_ev["allowed"]),
        (&json!(true), &json!(false))
    );
    clean("экран", &v.to_string());

    // Тексты событий администратора — без защиты [`guard`]: сами по себе без денег владельца.
    let rows: Vec<(String, Value, Option<Uuid>, Uuid)> = sqlx::query_as(
        "select action, data, entity_id, branch_id from audit_log where action = any($1) order by created_at",
    )
    .bind(
        bot_admin::EVENTS
            .iter()
            .map(|e| e.action.to_string())
            .collect::<Vec<_>>(),
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for (action, data, entity, branch) in rows {
        let t = bot_admin::event_text(&mut conn, &action, &data, entity, branch)
            .await
            .unwrap();
        if let Some((title, details)) = t {
            clean(&action, &format!("{title}\n{details}"));
            seen.insert(action);
        }
    }
    for a in ["shift.open", "shift.close", "receipt.post"] {
        assert!(seen.contains(a), "{a}: {seen:?}");
    }
}
