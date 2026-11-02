//! Килограммы у любого товара; лом аккумуляторов — по пометке `system_role` (ADR-174):
//! карточка в кг, приход по цене за кг, ревизия дробью двумя вводами, продажа на вес, приём
//! аккумулятора на карточку лома, смена единицы шт → кг только без остатка.
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

async fn setup(pool: PgPool) -> Router {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
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
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
    .bind(admin)
    .bind(branch)
    .bind(keys)
    .execute(&pool)
    .await
    .unwrap();
    app(build_state(pool, false).await.unwrap(), None)
}

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

async fn stock(pool: &PgPool, product: &str) -> (i64, i64) {
    sqlx::query_as(
        "select stock_qty, stock_value_tyiyn from branch_products where product_id = $1::uuid",
    )
    .bind(product)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn category(app: &Router, owner: &str) -> Value {
    ok(
        app,
        "POST",
        "/api/v1/categories",
        owner,
        json!({ "name": "Разное", "kind": "other" }),
    )
    .await
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn kg_product_receipt_revision_sale_and_battery(pool: PgPool) {
    let app = setup(pool.clone()).await;
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
    ok(
        &app,
        "POST",
        "/api/v1/shifts/open",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    let cat = category(&app, &owner).await;
    // Обычный товар в кг заведён раньше карточки лома: прежнее правило «первый товар в граммах»
    // приняло бы аккумулятор на него.
    let p = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Солидол на вес",
                "unit": "g", "sale_price_tyiyn": 30_000 }),
    )
    .await;
    assert_eq!(p["unit"], json!("g"));
    assert_eq!(p["container_ml"], Value::Null);
    let pid = p["id"].as_str().unwrap().to_string();

    // Приход 2,5 кг по 200 с за кг: строка — 2 500 г на 500 с.
    let sup = ok(
        &app,
        "POST",
        "/api/v1/suppliers",
        &admin,
        json!({ "name": "Поставщик" }),
    )
    .await;
    ok(
        &app,
        "POST",
        "/api/v1/receipts",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "supplier_id": sup["id"], "payment": "debt",
                "lines": [{ "product_id": pid, "qty": 2_500, "cost_tyiyn": 50_000 }] }),
    )
    .await;
    assert_eq!(stock(&pool, &pid).await, (2_500, 50_000));

    // Продажа 1,5 кг по 300 с за кг: сумма 450 с, себестоимость — 1,5 × 200 с.
    let sale = ok(
        &app,
        "POST",
        "/api/v1/sales",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "weight", "product_id": pid, "qty": 1_500, "unit_price_tyiyn": 30_000 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 45_000 }] }),
    )
    .await;
    let (amount, cost): (i64, i64) =
        sqlx::query_as("select amount_tyiyn, cost_tyiyn from sale_lines where sale_id = $1::uuid")
            .bind(sale["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((amount, cost), (45_000, 30_000));
    assert_eq!(stock(&pool, &pid).await, (1_000, 20_000));

    // Ревизия: на полке 1 кг и ещё 0,25 кг — два ввода, итог 1 250 г; излишек 250 г по средней.
    let rev = ok(&app, "POST", "/api/v1/revisions", &owner, json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/lines");
    for q in [1_000, 250] {
        ok(
            &app,
            "PUT",
            &uri,
            &owner,
            json!({ "product_id": pid, "counted_qty": q, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    }
    ok(
        &app,
        "POST",
        &format!("/api/v1/revisions/{rid}/post"),
        &owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт", "force": true }),
    )
    .await;
    assert_eq!(stock(&pool, &pid).await, (1_250, 25_000));

    // Аккумулятор 2 кг — на служебную карточку лома, не на солидол.
    ok(
        &app,
        "POST",
        "/api/v1/batteries/intake",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "grams": 2_000, "price_per_kg_tyiyn": 10_000 }),
    )
    .await;
    let info = get(&app, "/api/v1/batteries", &admin).await;
    let scrap = info["product_id"].as_str().unwrap().to_string();
    assert_ne!(scrap, pid);
    assert_eq!(info["stock_g"], json!(2_000));
    assert_eq!(stock(&pool, &pid).await, (1_250, 25_000));
    let role: Option<String> =
        sqlx::query_scalar("select system_role from products where id = $1::uuid")
            .bind(&scrap)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(role.as_deref(), Some("battery_scrap"));

    // Единица карточки лома не меняется.
    let (s, v) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{scrap}/unit"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "unit": "piece" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn piece_to_kg_only_without_stock(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cat = category(&app, &owner).await;
    let mk = |name: &str| {
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
                "sale_price_tyiyn": 40_000 })
    };
    let empty = ok(&app, "POST", "/api/v1/products", &owner, mk("Хомут")).await;
    let full = ok(&app, "POST", "/api/v1/products", &owner, mk("Ветошь")).await;
    assert_eq!(empty["unit"], json!("piece"));
    let (empty, full) = (
        empty["id"].as_str().unwrap().to_string(),
        full["id"].as_str().unwrap().to_string(),
    );
    ok(
        &app,
        "POST",
        "/api/v1/stock-ins",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "остатки",
                "lines": [{ "product_id": full, "qty": 3, "price_tyiyn": 10_000 }] }),
    )
    .await;

    // Администратор единицу не меняет (I12).
    let to_kg = || json!({ "op_id": Uuid::now_v7(), "unit": "g" });
    let (s, _) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{empty}/unit"),
        Some(&admin),
        Some(to_kg()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Без остатка — сразу: цена штуки становится ценой кг.
    let out = ok(
        &app,
        "POST",
        &format!("/api/v1/products/{empty}/unit"),
        &owner,
        to_kg(),
    )
    .await;
    assert_eq!(out["product"]["unit"], json!("g"));
    assert_eq!(out["product"]["sale_price_tyiyn"], json!(40_000));

    // С остатком — понятный отказ, ничего не меняется.
    let (s, v) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{full}/unit"),
        Some(&owner),
        Some(to_kg()),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("Обнулите остаток ревизией"), "{m}");
    assert_eq!(stock(&pool, &full).await, (3, 30_000));
    let unit: String = sqlx::query_scalar("select unit from products where id = $1::uuid")
        .bind(&full)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unit, "piece");
}
