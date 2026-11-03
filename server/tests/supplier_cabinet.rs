//! Кабинет поставщика (задача 028, ADR-188): вход по ИНН, свои накладные с оплатой и файлами,
//! деньги, возвраты, акт сверки как у владельца, файл от поставщика, закрытые чужие разделы,
//! один ИНН — один кабинет, сообщение о накладной.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::client_messages::{self, Channels};
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string());
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    if !content_type.is_empty() {
        req = req.header(header::CONTENT_TYPE, content_type);
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, cookie, bytes)
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
    let (ct, b) = match body {
        Some(b) => ("application/json", b.to_string().into_bytes()),
        None => ("", Vec::new()),
    };
    let (s, c, bytes) = raw(app, method, uri, cookie, ct, b).await;
    (s, c, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, _, v) = call(app, method, uri, Some(cookie), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn get(app: &Router, uri: &str, cookie: &str) -> Value {
    let (s, _, v) = call(app, "GET", uri, Some(cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn status(app: &Router, method: &str, uri: &str, cookie: &str) -> StatusCode {
    call(app, method, uri, Some(cookie), None).await.0
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c, _) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    c.unwrap()
}

/// Вход в кабинет по ИНН с начальным паролем и смена пароля: кука готового кабинета.
async fn cabinet(app: &Router, inn: &str) -> String {
    let (s, c, me) = call(
        app,
        "POST",
        "/api/v1/client/login",
        None,
        Some(json!({ "inn": inn, "password": "avtodom2026" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["must_change"], json!(true));
    let c = c.unwrap();
    assert_eq!(
        status(app, "GET", "/api/v1/supplier-cabinet/invoices", &c).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    ok(
        app,
        "POST",
        "/api/v1/client/password",
        &c,
        json!({ "old_password": "avtodom2026", "new_password": "свой-пароль-1" }),
    )
    .await;
    c
}

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    item: String,
}

impl World {
    async fn new(pool: PgPool) -> Self {
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
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        let cat = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
        let p = ok(
            &app,
            "POST",
            "/api/v1/products",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 50_000 }),
        )
        .await;
        World {
            app,
            pool,
            owner,
            admin,
            item: p["id"].as_str().unwrap().to_string(),
        }
    }

    async fn supplier(&self, name: &str, inn: &str) -> String {
        let s = ok(
            &self.app,
            "POST",
            "/api/v1/suppliers",
            &self.owner,
            json!({ "name": name, "kind": "company", "inn": inn }),
        )
        .await;
        s["id"].as_str().unwrap().to_string()
    }

    /// Приход в долг: `qty` фильтров по `price`.
    async fn receipt(&self, supplier: &str, qty: i64, price: i64, doc: &str) -> String {
        let r = ok(
            &self.app,
            "POST",
            "/api/v1/receipts",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt", "supplier_doc": doc,
                    "lines": [{ "product_id": self.item, "qty": qty, "cost_tyiyn": qty * price }] }),
        )
        .await;
        r["id"].as_str().unwrap().to_string()
    }

    async fn pay(&self, supplier: &str, amount: i64) {
        ok(
            &self.app,
            "POST",
            "/api/v1/debts/repayments",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "party_id": supplier, "amount_tyiyn": amount, "method": "transfer" }),
        )
        .await;
    }

    async fn upload(
        &self,
        cookie: &str,
        receipt: &str,
        op: Uuid,
        data: Vec<u8>,
    ) -> (StatusCode, Value) {
        let (s, _, b) = raw(
            &self.app,
            "POST",
            &format!("/api/v1/supplier-cabinet/invoices/{receipt}/files?op_id={op}&name=scan.png"),
            Some(cookie),
            "image/png",
            data,
        )
        .await;
        (s, serde_json::from_slice(&b).unwrap_or(Value::Null))
    }
}

/// В ответе кабинета нет нашей себестоимости, цен продажи и остатков (инвариант 13).
fn no_secrets(v: &Value) {
    let text = v.to_string();
    for key in [
        "cost",
        "avg",
        "stock",
        "sale_price",
        "margin",
        "profit",
        "user_name",
        "account",
    ] {
        assert!(
            !text.contains(&format!("\"{key}")),
            "{key} в ответе: {text}"
        );
    }
}

#[sqlx::test]
async fn supplier_sees_own_invoices_money_files_and_act(pool: PgPool) {
    let w = World::new(pool).await;
    let sup = w.supplier("ОсОО Автоснаб", "01111111111111").await;
    let other = w.supplier("ОсОО Чужой", "02222222222222").await;
    let r1 = w.receipt(&sup, 10, 30_000, "15").await;
    let r2 = w.receipt(&sup, 5, 40_000, "16").await;
    let foreign = w.receipt(&other, 1, 10_000, "99").await;
    w.pay(&sup, 350_000).await;

    // Владелец видит кабинет в карточке; администратор пароль не сбрасывает.
    let st = get(&w.app, &format!("/api/v1/parties/{sup}/cabinet"), &w.owner).await;
    assert_eq!(st["available"], json!(true));
    assert_eq!(st["login"], json!("01111111111111"));
    assert_eq!(
        status(
            &w.app,
            "POST",
            &format!("/api/v1/parties/{sup}/cabinet/reset"),
            &w.admin
        )
        .await,
        StatusCode::FORBIDDEN
    );

    let c = cabinet(&w.app, "01111111111111").await;
    let me = get(&w.app, "/api/v1/client/me", &c).await;
    assert_eq!(me["role"], json!("supplier"));
    // Наш долг: 300 000 + 200 000 − 350 000 = 150 000, не оплачена одна накладная.
    assert_eq!(me["supplier"]["debt_tyiyn"], json!(150_000));
    assert_eq!(me["supplier"]["unpaid_count"], json!(1));
    assert_eq!(me["supplier"]["unpaid_tyiyn"], json!(150_000));

    // Накладные — только свои; оплата совпадает с карточкой поставщика у владельца.
    let list = get(&w.app, "/api/v1/supplier-cabinet/invoices", &c).await;
    no_secrets(&list);
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|i| i["id"] != json!(foreign)));
    let ours = get(&w.app, &format!("/api/v1/parties/{sup}/invoices"), &w.owner).await;
    for inv in ours["invoices"].as_array().unwrap() {
        let mine = list.iter().find(|i| i["id"] == inv["receipt_id"]).unwrap();
        assert_eq!(mine["left_tyiyn"], inv["left_tyiyn"]);
        assert_eq!(mine["status"], inv["status"]);
    }
    let first = list.iter().find(|i| i["id"] == json!(r1)).unwrap();
    assert_eq!(first["status"], json!("paid"));
    assert_eq!(first["lines"][0]["amount_tyiyn"], json!(300_000));
    let second = list.iter().find(|i| i["id"] == json!(r2)).unwrap();
    assert_eq!(second["status"], json!("partial"));
    assert_eq!(second["paid_tyiyn"], json!(50_000));
    assert_eq!(second["left_tyiyn"], json!(150_000));

    // Деньги: оплата и какие накладные закрыла — без кассы и того, кто провёл.
    let money = get(&w.app, "/api/v1/supplier-cabinet/payments", &c).await;
    no_secrets(&money);
    assert_eq!(money[0]["kind"], json!("payment"));
    assert_eq!(money[0]["amount_tyiyn"], json!(350_000));
    assert_eq!(money[0]["applied"].as_array().unwrap().len(), 2);
    let rets = get(&w.app, "/api/v1/supplier-cabinet/returns", &c).await;
    assert_eq!(rets, json!([]));

    // Акт сверки — тот же, что у владельца.
    let act = get(&w.app, "/api/v1/client/reconciliation", &c).await;
    let owner_act = get(
        &w.app,
        &format!("/api/v1/parties/{sup}/reconciliation"),
        &w.owner,
    )
    .await;
    assert_eq!(act, owner_act);

    // Файл точки: свой отдаётся, скрытый и чужой — 404.
    let (s, _, f) = raw(
        &w.app,
        "POST",
        &format!("/api/v1/receipts/{r1}/files?op_id={}", Uuid::now_v7()),
        Some(&w.owner),
        "image/png",
        PNG.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let staff_file = serde_json::from_slice::<Value>(&f).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _, f) = raw(
        &w.app,
        "POST",
        &format!("/api/v1/receipts/{foreign}/files?op_id={}", Uuid::now_v7()),
        Some(&w.owner),
        "image/png",
        PNG.to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let foreign_file = serde_json::from_slice::<Value>(&f).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _, body) = raw(
        &w.app,
        "GET",
        &format!("/api/v1/supplier-cabinet/invoices/{r1}/files/{staff_file}"),
        Some(&c),
        "",
        vec![],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, PNG);
    assert_eq!(
        status(
            &w.app,
            "GET",
            &format!("/api/v1/supplier-cabinet/invoices/{foreign}/files/{foreign_file}"),
            &c
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(
            &w.app,
            "GET",
            &format!("/api/v1/supplier-cabinet/invoices/{r1}/files/{foreign_file}"),
            &c
        )
        .await,
        StatusCode::NOT_FOUND
    );
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-files/{staff_file}/hide"),
        &w.owner,
        json!({ "reason": "не та накладная" }),
    )
    .await;
    assert_eq!(
        status(
            &w.app,
            "GET",
            &format!("/api/v1/supplier-cabinet/invoices/{r1}/files/{staff_file}"),
            &c
        )
        .await,
        StatusCode::NOT_FOUND
    );

    // Файл от поставщика: к своей — да, повтор op_id — тот же файл, к чужой — 404, больше 5 МБ — 413.
    let op = Uuid::now_v7();
    let (s, a) = w.upload(&c, &r2, op, PNG.to_vec()).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    let (s, b) = w.upload(&c, &r2, op, PNG.to_vec()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(a["id"], b["id"]);
    assert_eq!(
        w.upload(&c, &foreign, Uuid::now_v7(), PNG.to_vec()).await.0,
        StatusCode::NOT_FOUND
    );
    let mut big = PNG.to_vec();
    big.resize(5 * 1024 * 1024 + 1, 0);
    assert_eq!(
        w.upload(&c, &r2, Uuid::now_v7(), big).await.0,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let n: i64 =
        sqlx::query_scalar("select count(*) from receipt_files where receipt_id = $1::uuid")
            .bind(&r2)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // У точки файл виден с пометкой «от поставщика».
    let files = get(&w.app, &format!("/api/v1/receipts/{r2}/files"), &w.owner).await;
    assert_eq!(files[0]["source"], json!("supplier"));
    assert_eq!(files[0]["user_name"], json!("от поставщика"));
    let audit: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'receipt_file.add_supplier'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);

    // Разделы клиента поставщику закрыты.
    for uri in [
        "/api/v1/client/sales",
        "/api/v1/client/contacts",
        "/api/v1/client/oil-book",
        "/api/v1/client/messages",
    ] {
        assert_eq!(
            status(&w.app, "GET", uri, &c).await,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }

    // Сброс пароля владельцем: прежний вход закрыт, снова начальный пароль.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/parties/{sup}/cabinet/reset"),
        &w.owner,
        json!({}),
    )
    .await;
    assert_eq!(
        status(&w.app, "GET", "/api/v1/client/me", &c).await,
        StatusCode::UNAUTHORIZED
    );
    cabinet(&w.app, "01111111111111").await;
}

#[sqlx::test]
async fn customer_cabinet_is_closed_to_supplier_sections_and_inn_is_one_cabinet(pool: PgPool) {
    let w = World::new(pool).await;
    let inn = "03333333333333";
    ok(
        &w.app,
        "POST",
        "/api/v1/parties",
        &w.owner,
        json!({ "name": "ОсОО Такси", "kind": "company", "inn": inn }),
    )
    .await;
    let sup = w.supplier("ОсОО Такси-снаб", inn).await;
    // ИНН занят клиентом: поставщику кабинет не выдаётся.
    let st = get(&w.app, &format!("/api/v1/parties/{sup}/cabinet"), &w.owner).await;
    assert_eq!(st["available"], json!(false));
    assert_eq!(st["reason"], json!("ИНН занят кабинетом клиента"));
    assert_eq!(
        status(
            &w.app,
            "POST",
            &format!("/api/v1/parties/{sup}/cabinet/reset"),
            &w.owner
        )
        .await,
        StatusCode::CONFLICT
    );
    // Вход по этому ИНН — кабинет клиента; разделы поставщика ему закрыты.
    let c = cabinet_customer(&w.app, inn).await;
    let me = get(&w.app, "/api/v1/client/me", &c).await;
    assert_eq!(me["role"], json!("customer"));
    for uri in [
        "/api/v1/supplier-cabinet/invoices",
        "/api/v1/supplier-cabinet/payments",
        "/api/v1/supplier-cabinet/returns",
    ] {
        assert_eq!(
            status(&w.app, "GET", uri, &c).await,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
    assert_eq!(
        status(&w.app, "GET", "/api/v1/client/sales", &c).await,
        StatusCode::OK
    );
}

async fn cabinet_customer(app: &Router, inn: &str) -> String {
    let (s, c, _) = call(
        app,
        "POST",
        "/api/v1/client/login",
        None,
        Some(json!({ "inn": inn, "password": "avtodom2026" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let c = c.unwrap();
    ok(
        app,
        "POST",
        "/api/v1/client/password",
        &c,
        json!({ "old_password": "avtodom2026", "new_password": "свой-пароль-1" }),
    )
    .await;
    c
}

#[sqlx::test]
async fn invoice_message_to_supplier_unless_switched_off(pool: PgPool) {
    let w = World::new(pool).await;
    sqlx::query("update bot_state set value = to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')) where key = 'client_messages_since'")
        .execute(&w.pool)
        .await
        .unwrap();
    let sup = w.supplier("ОсОО Автоснаб", "01111111111111").await;
    let quiet = w.supplier("ОсОО Тихий", "04444444444444").await;
    ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/parties/{quiet}"),
        &w.owner,
        json!({ "no_purchase_messages": true }),
    )
    .await;
    w.receipt(&sup, 2, 10_000, "77").await;
    w.receipt(&quiet, 1, 10_000, "78").await;
    let mut conn = w.pool.acquire().await.unwrap();
    client_messages::pass(&mut conn, &Channels::default(), 12, false)
        .await
        .unwrap();
    // Повторный проход не пишет второй раз.
    client_messages::pass(&mut conn, &Channels::default(), 12, false)
        .await
        .unwrap();
    let rows: Vec<(String, String, String)> = sqlx::query_as("select party_id::text, status, text from client_messages where kind = 'invoice' order by created_at")
        .fetch_all(&w.pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, sup);
    // Без бота — строка журнала «не отправлено» с текстом.
    assert_eq!(rows[0].1, "not_sent");
    assert!(
        rows[0].2.starts_with("Принята накладная № 77"),
        "{}",
        rows[0].2
    );
}
