//! Метры у любого товара (задача 006, ADR-178): карточка в метрах (остаток в мм), приход и
//! продажа по цене за метр, вид строки `length` только у товара в метрах, ревизия дробью двумя
//! вводами, смена штуки → метры с коэффициентом «1 шт = X мм», кг ↔ м с остатком — отказ.
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

async fn product(app: &Router, owner: &str, cat: &Value, name: &str, unit: &str) -> String {
    let p = ok(
        app,
        "POST",
        "/api/v1/products",
        owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
                "unit": unit, "sale_price_tyiyn": 30_000 }),
    )
    .await;
    assert_eq!(p["unit"], json!(unit));
    p["id"].as_str().unwrap().to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn meters_receipt_sale_kinds_and_revision(pool: PgPool) {
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
    let hose = product(&app, &owner, &cat, "Шланг в метрах", "mm").await;
    let grease = product(&app, &owner, &cat, "Солидол на вес", "g").await;

    // Приход 3,5 м по 200 с за метр: строка — 3 500 мм на 700 с.
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
                "lines": [{ "product_id": hose, "qty": 3_500, "cost_tyiyn": 70_000 },
                          { "product_id": grease, "qty": 1_000, "cost_tyiyn": 20_000 }] }),
    )
    .await;
    assert_eq!(stock(&pool, &hose).await, (3_500, 70_000));

    // Вид строки не подходит к единице — 409, остаток не тронут.
    let sale = |kind: &str, pid: &str| {
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": kind, "product_id": pid, "qty": 1_500, "unit_price_tyiyn": 30_000 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 45_000 }] })
    };
    for (kind, pid) in [("piece", &hose), ("weight", &hose), ("length", &grease)] {
        let (s, v) = call(
            &app,
            "POST",
            "/api/v1/sales",
            Some(&admin),
            Some(sale(kind, pid)),
        )
        .await;
        assert_eq!(s, StatusCode::CONFLICT, "{kind}: {v}");
    }
    assert_eq!(stock(&pool, &hose).await, (3_500, 70_000));

    // Продажа 1,5 м по 300 с за метр: сумма 450 с, себестоимость — 1,5 × 200 с.
    let sold = ok(&app, "POST", "/api/v1/sales", &admin, sale("length", &hose)).await;
    let (amount, cost): (i64, i64) =
        sqlx::query_as("select amount_tyiyn, cost_tyiyn from sale_lines where sale_id = $1::uuid")
            .bind(sold["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((amount, cost), (45_000, 30_000));
    assert_eq!(stock(&pool, &hose).await, (2_000, 40_000));

    // Ревизия: 1,25 м и ещё 0,75 м — два ввода, итог 2 000 мм, расхождения нет.
    let rev = ok(&app, "POST", "/api/v1/revisions", &owner, json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/lines");
    for q in [1_250, 750] {
        ok(
            &app,
            "PUT",
            &uri,
            &owner,
            json!({ "product_id": hose, "counted_qty": q, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    }
    ok(
        &app,
        "PUT",
        &uri,
        &owner,
        json!({ "product_id": grease, "counted_qty": 1_000, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/revisions/{rid}/post"),
        &owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт", "force": true }),
    )
    .await;
    assert_eq!(stock(&pool, &hose).await, (2_000, 40_000));

    // Кг ↔ м с остатком — отказ, единица прежняя.
    let (s, v) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{grease}/unit"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "unit": "mm" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let unit: String = sqlx::query_scalar("select unit from products where id = $1::uuid")
        .bind(&grease)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unit, "g");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn piece_to_meters_with_coefficient(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let cat = category(&app, &owner).await;
    let p = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Бухта шланга",
                "sale_price_tyiyn": 50_000 }),
    )
    .await;
    let pid = p["id"].as_str().unwrap().to_string();
    ok(
        &app,
        "POST",
        "/api/v1/stock-ins",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "остатки",
                "lines": [{ "product_id": pid, "qty": 3, "price_tyiyn": 20_000 }] }),
    )
    .await;

    // Без коэффициента с остатком — отказ.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/unit"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "unit": "mm" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // 1 шт = 2,5 м: 3 шт → 7,5 м, стоимость та же, цена 500 с за шт → 200 с за метр.
    let out = ok(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/unit"),
        &owner,
        json!({ "op_id": Uuid::now_v7(), "unit": "mm", "piece_qty": 2_500 }),
    )
    .await;
    assert_eq!(out["product"]["unit"], json!("mm"));
    assert_eq!(out["product"]["sale_price_tyiyn"], json!(20_000));
    assert_eq!(stock(&pool, &pid).await, (7_500, 60_000));
}
