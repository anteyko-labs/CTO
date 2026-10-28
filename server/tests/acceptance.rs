//! Приёмка товара на планшете (SPEC-39, ADR-134) и чат администратора в боте (ADR-135).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

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
    assert_eq!(s, StatusCode::OK, "{login}");
    c.as_str().unwrap().to_string()
}

fn sid(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// Подпись: начало PNG и немного байтов — сервер проверяет только вид и размер.
fn png() -> String {
    // 89 50 4E 47 0D 0A 1A 0A + 4 байта → base64.
    "iVBORw0KGgoAAAAA".into()
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    /// Администратор с правом «Приход товара» — готовит накладные.
    admin: String,
    admin_id: Uuid,
    /// Планшет: только «Приёмка товара».
    tablet: String,
    cashier: String,
    supplier: String,
    filter: String,
    oil: String,
}

async fn user(pool: &PgPool, branch: Uuid, login: &str, perms: &[&str]) -> Uuid {
    let hash = hash_password("pass-12345".into()).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $3)")
        .bind(id)
        .bind(branch)
        .bind(login)
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();
    let perms: Vec<String> = perms.iter().map(|p| p.to_string()).collect();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
    .bind(id)
    .bind(branch)
    .bind(perms)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn world(pool: PgPool) -> W {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let admin_id = user(&pool, branch, "admin", &["receipts"]).await;
    user(&pool, branch, "tablet", &["acceptance"]).await;
    user(&pool, branch, "nobody", &["products"]).await;
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "pass-12345").await;
    let tablet = login(&app, "tablet", "pass-12345").await;
    let e = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" }),
    )
    .await;
    let supplier = sid(&ok(
        &app,
        "POST",
        "/api/v1/suppliers",
        &owner,
        json!({ "name": "Автоснаб" }),
    )
    .await);
    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        json!({ "name": "Фильтры", "kind": "filter" }),
    )
    .await;
    let filter = sid(&ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр W712/75",
                "sale_price_tyiyn": 90_000, "barcodes": ["4011558"] }),
    )
    .await);
    let oils = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        json!({ "name": "Масла", "kind": "oil" }),
    )
    .await;
    let oil = sid(&ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": oils["id"], "name": "Shell 4 л",
                "container_ml": 4000, "sale_price_tyiyn": 200_000 }),
    )
    .await);
    W {
        app,
        pool,
        owner,
        admin,
        admin_id,
        tablet,
        cashier: sid(&e),
        supplier,
        filter,
        oil,
    }
}

impl W {
    /// «На приёмку»: фильтр 20 шт на 100 000 и масло 5 канистр (20 л) на 333 333.
    async fn send(&self, doc: &str) -> String {
        sid(&ok(
            &self.app,
            "POST",
            "/api/v1/acceptances",
            &self.admin,
            json!({ "op_id": Uuid::now_v7(), "supplier_id": self.supplier, "supplier_doc": doc,
                    "lines": [
                        { "product_id": self.filter, "qty": 20, "cost_tyiyn": 100_000, "sale_price_tyiyn": 95_000 },
                        { "product_id": self.oil, "qty": 20_000, "cost_tyiyn": 333_333 }
                    ] }),
        )
        .await)
    }

    fn accept_body(&self, lines: Value) -> Value {
        json!({ "op_id": Uuid::now_v7(), "lines": lines, "employee_id": self.cashier,
                "pin": "1234", "signature_png": png() })
    }

    async fn stock(&self, product: &str) -> i64 {
        sqlx::query_scalar(
            "select coalesce(sum(qty_delta), 0)::bigint from stock_movements where product_id = $1::uuid",
        )
        .bind(product)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// Наш долг поставщику (в книге — с минусом).
    async fn debt(&self) -> i64 {
        sqlx::query_scalar(
            "select (-coalesce(sum(amount_tyiyn), 0))::bigint from party_ledger where party_id = $1::uuid",
        )
        .bind(&self.supplier)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by created_at")
            .bind(action)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    async fn receipt_lines(&self, receipt: &str) -> Vec<(String, i64, i64)> {
        sqlx::query_as::<_, (Uuid, i64, i64)>(
            "select product_id, qty, cost_tyiyn from receipt_lines where receipt_id = $1::uuid order by line_no",
        )
        .bind(receipt)
        .fetch_all(&self.pool)
        .await
        .unwrap()
        .into_iter()
        .map(|(p, q, c)| (p.to_string(), q, c))
        .collect()
    }
}

#[sqlx::test]
async fn send_changes_nothing_and_tablet_sees_no_prices(pool: PgPool) {
    let w = world(pool).await;
    let id = w.send("15").await;
    assert_eq!(w.stock(&w.filter).await, 0);
    assert_eq!(w.debt().await, 0);
    // Та же накладная второй раз — 409.
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/acceptances",
        Some(&w.admin),
        Some(
            json!({ "op_id": Uuid::now_v7(), "supplier_id": w.supplier, "supplier_doc": "15",
                     "lines": [{ "product_id": w.filter, "qty": 1, "cost_tyiyn": 1 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // Планшет: без цен и сумм.
    let list = get(&w.app, "/api/v1/acceptances?status=waiting", &w.tablet).await;
    assert_eq!(list["prices"], json!(false));
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["items"][0]["total_tyiyn"].is_null());
    let one = get(&w.app, &format!("/api/v1/acceptances/{id}"), &w.tablet).await;
    assert!(one["lines"][0]["line_cost_tyiyn"].is_null());
    assert_eq!(one["lines"][0]["barcodes"], json!(["4011558"]));
    // С правом «Приход» — с суммами.
    let full = get(&w.app, &format!("/api/v1/acceptances/{id}"), &w.admin).await;
    assert_eq!(full["total_tyiyn"], json!(433_333));
    // Ни того ни другого — 403; планшет не отправляет на приёмку и не видит подпись.
    let nobody = login(&w.app, "nobody", "pass-12345").await;
    let (s, _) = call(&w.app, "GET", "/api/v1/acceptances", Some(&nobody), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/acceptances",
        Some(&w.tablet),
        Some(json!({ "op_id": Uuid::now_v7(), "supplier_id": w.supplier,
                     "lines": [{ "product_id": w.filter, "qty": 1, "cost_tyiyn": 1 }] })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(w.audit("acceptance.send").await.len(), 1);
}

#[sqlx::test]
async fn accept_by_invoice_posts_receipt_on_debt(pool: PgPool) {
    let w = world(pool).await;
    let id = w.send("15").await;
    let body = w.accept_body(json!([
        { "line_no": 1, "accepted_qty": 20 },
        { "line_no": 2, "accepted_qty": 20_000 }
    ]));
    let r = ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        &w.tablet,
        body.clone(),
    )
    .await;
    let receipt = r["receipt_id"].as_str().unwrap().to_string();
    // Повтор той же операции — тот же приход.
    let again = ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        &w.tablet,
        body,
    )
    .await;
    assert_eq!(again["receipt_id"], r["receipt_id"]);
    assert_eq!(w.stock(&w.filter).await, 20);
    assert_eq!(w.stock(&w.oil).await, 20_000);
    assert_eq!(w.debt().await, 433_333);
    // Новая цена продажи — у принятой строки.
    let price: i64 =
        sqlx::query_scalar("select sale_price_tyiyn from products where id = $1::uuid")
            .bind(&w.filter)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(price, 95_000);
    // Приёмка закрыта, подпись — владельцу, в накладной — приёмка.
    let a = get(&w.app, &format!("/api/v1/acceptances/{id}"), &w.owner).await;
    assert_eq!(a["status"], "accepted");
    assert_eq!(a["employee_name"], "Айбек");
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/acceptances/{id}/signature"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/acceptances/{id}/signature"),
        Some(&w.tablet),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let of = get(
        &w.app,
        &format!("/api/v1/receipts/{receipt}/acceptance"),
        &w.admin,
    )
    .await;
    assert_eq!(of["id"].as_str(), Some(id.as_str()));
    // Владельцу — «по накладной, без расхождений».
    let feed = get(&w.app, "/api/v1/notifications?limit=50", &w.owner).await;
    let text = feed.to_string();
    assert!(text.contains("принял Айбек"), "{text}");
    assert!(text.contains("без расхождений"), "{text}");
    // Вторая приёмка той же — 409.
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        Some(&w.tablet),
        Some(w.accept_body(json!([
            { "line_no": 1, "accepted_qty": 20 },
            { "line_no": 2, "accepted_qty": 20_000 }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Сторно прихода — у приёмки отметка «приход сторнирован».
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/receipts/{receipt}/reverse"),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    let a = get(&w.app, &format!("/api/v1/acceptances/{id}"), &w.owner).await;
    assert_eq!(a["receipt_state"], "reversed");
}

#[sqlx::test]
async fn discrepancies_are_proportional_and_reported(pool: PgPool) {
    let w = world(pool).await;
    let id = w.send("16").await;
    let (s, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        Some(&w.tablet),
        Some(w.accept_body(json!([
            { "line_no": 1, "accepted_qty": 18 },
            { "line_no": 2, "accepted_qty": 24_000 }
        ]))),
    )
    .await;
    // Меньше накладной без причины — 422.
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let mut body = w.accept_body(json!([
        { "line_no": 1, "accepted_qty": 18, "reason": "missing" },
        { "line_no": 2, "accepted_qty": 24_000 }
    ]));
    body["extras"] = json!([{ "name": "Щётки", "qty": 2 }]);
    let r = ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        &w.tablet,
        body,
    )
    .await;
    let receipt = r["receipt_id"].as_str().unwrap();
    // 18 из 20: 100 000 × 18 / 20 = 90 000; масло 24 л из 20 л: 333 333 × 24 000 / 20 000 =
    // 399 999,6 → 400 000 (одно округление).
    assert_eq!(
        w.receipt_lines(receipt).await,
        vec![
            (w.filter.clone(), 18, 90_000),
            (w.oil.clone(), 24_000, 400_000)
        ]
    );
    assert_eq!(w.debt().await, 490_000);
    let feed = get(&w.app, "/api/v1/notifications?limit=50", &w.owner).await;
    let text = feed.to_string();
    assert!(text.contains("18 из 20"), "{text}");
    assert!(text.contains("Щётки"), "{text}");
    let log = w.audit("receipt.accept").await;
    assert_eq!(log.len(), 1);
}

#[sqlx::test]
async fn zero_lines_refuse_and_cancel(pool: PgPool) {
    let w = world(pool).await;
    let id = w.send("17").await;
    // Все строки 0 — 422.
    let (s, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        Some(&w.tablet),
        Some(w.accept_body(json!([
            { "line_no": 1, "accepted_qty": 0, "reason": "missing" },
            { "line_no": 2, "accepted_qty": 0, "reason": "missing" }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Строка 0 — её нет в приходе.
    let r = ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/accept"),
        &w.tablet,
        w.accept_body(json!([
            { "line_no": 1, "accepted_qty": 20 },
            { "line_no": 2, "accepted_qty": 0, "reason": "damaged" }
        ])),
    )
    .await;
    let lines = w.receipt_lines(r["receipt_id"].as_str().unwrap()).await;
    assert_eq!(lines, vec![(w.filter.clone(), 20, 100_000)]);

    // Отказ от приёмки: прихода нет, владельцу уведомление.
    let id = w.send("18").await;
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/refuse"),
        &w.tablet,
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "pin": "1234",
                "reason": "привезли не тот фильтр" }),
    )
    .await;
    let a = get(&w.app, &format!("/api/v1/acceptances/{id}"), &w.owner).await;
    assert_eq!(a["status"], "refused");
    assert!(a["receipt_id"].is_null());
    let feed = get(&w.app, "/api/v1/notifications?limit=50", &w.owner).await;
    assert!(feed.to_string().contains("привезли не тот фильтр"));

    // «Убрать с приёмки» — только с правом «Приход», с причиной; ту же накладную можно снова.
    let id = w.send("19").await;
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/cancel"),
        Some(&w.tablet),
        Some(json!({ "op_id": Uuid::now_v7(), "reason": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/acceptances/{id}/cancel"),
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "reason": "ошибка в количестве" }),
    )
    .await;
    w.send("19").await;
    let list = get(&w.app, "/api/v1/acceptances?status=waiting", &w.tablet).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn pin_signature_and_sender_checks(pool: PgPool) {
    let w = world(pool).await;
    let id = w.send("20").await;
    let uri = format!("/api/v1/acceptances/{id}/accept");
    let lines = json!([
        { "line_no": 1, "accepted_qty": 20 },
        { "line_no": 2, "accepted_qty": 20_000 }
    ]);
    // Неверный PIN — 422 и событие с пометкой «приёмка».
    let mut body = w.accept_body(lines.clone());
    body["pin"] = json!("0000");
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.tablet), Some(body)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let failed = w.audit("shift.pin_failed").await;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["for"], "acceptance");
    // Подпись не PNG и больше 200 КБ — 422, попытка PIN не тратится.
    let mut body = w.accept_body(lines.clone());
    body["signature_png"] = json!("R0lGODlhAQABAAAAACw=");
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.tablet), Some(body)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let mut body = w.accept_body(lines.clone());
    body["signature_png"] = json!(format!("iVBORw0KGgo{}", "A".repeat(280_000)));
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.tablet), Some(body)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(w.audit("shift.pin_failed").await.len(), 1);
    // Не кассир — 422.
    let master = ok(
        &w.app,
        "POST",
        "/api/v1/employees",
        &w.owner,
        json!({ "full_name": "Мастер", "is_cashier": false, "is_master": true }),
    )
    .await;
    let mut body = w.accept_body(lines.clone());
    body["employee_id"] = master["id"].clone();
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.tablet), Some(body)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Отправивший потерял право «Приход товара» — 409 «отправьте на приёмку заново».
    sqlx::query("delete from user_permissions where user_id = $1 and permission = 'receipts'")
        .bind(w.admin_id)
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, v) = call(
        &w.app,
        "POST",
        &uri,
        Some(&w.tablet),
        Some(w.accept_body(lines)),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(w.stock(&w.filter).await, 0);
}

#[sqlx::test]
async fn owner_links_admin_chat_for_invoice_photos_only(pool: PgPool) {
    let w = world(pool).await;
    let uri = format!("/api/v1/users/{}/telegram/code", w.admin_id);
    // Код даёт только владелец.
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.admin), Some(json!({}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let code = ok(&w.app, "POST", &uri, &w.owner, json!({})).await;
    let code = code["code"].as_str().unwrap().to_string();
    let mut conn = w.pool.acquire().await.unwrap();
    let r = avtodom_server::api::telegram::handle_message(
        &mut conn,
        7001,
        7001,
        Some(&format!("/start {code}")),
        None,
    )
    .await
    .unwrap();
    assert!(!r.text.is_empty());
    let users = get(&w.app, "/api/v1/users", &w.owner).await;
    let admin = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["login"] == "admin")
        .unwrap();
    assert!(admin["telegram_linked_at"].is_string(), "{admin}");
    // Чат администратора — не владельца: на текст только подсказка про фото накладной.
    let r =
        avtodom_server::api::telegram::handle_message(&mut conn, 7001, 7001, Some("/day"), None)
            .await
            .unwrap();
    assert_eq!(r.text, avtodom_server::api::telegram::ADMIN_HELP);
    let feed = get(&w.app, "/api/v1/notifications?limit=50", &w.owner).await;
    assert!(feed.to_string().contains("Телеграм привязан"));
    // Отключение администратора — отвязка в той же транзакции.
    ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/users/{}", w.admin_id),
        &w.owner,
        json!({ "full_name": "admin", "role": "admin", "active": false }),
    )
    .await;
    let linked: i64 = sqlx::query_scalar("select count(*) from telegram_links where user_id = $1")
        .bind(w.admin_id)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(linked, 0);
}
