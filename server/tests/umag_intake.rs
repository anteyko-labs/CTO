//! Разбор остатков из Умага (задача 042): «Новый товар», «Это он», «Пропустить», «Отменить»,
//! количество — в черновик ревизии «весь склад», проведение — остатки.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, bootstrap, build_state};

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie);
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
    let (s, v) = call(app, method, uri, cookie, body).await;
    assert!(s.is_success(), "{method} {uri}: {s} {v}");
    v
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "login": login, "password": password }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "вход {login}");
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

fn row_id(list: &Value, name: &str) -> String {
    list.as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("нет строки {name}: {list}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn stock(pool: &PgPool, product: &str) -> i64 {
    sqlx::query_scalar("select stock_qty from branch_products where product_id = $1::text::uuid")
        .bind(product)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn intake_new_link_undo_and_post(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let existing = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Фильтр масляный Mann W712",
                     "article": "W712", "barcodes": ["4011558000001"], "sale_price_tyiyn": 50_000 })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "PUT",
        "/api/v1/old-catalog",
        &owner,
        Some(json!([
            { "name": "Свеча NGK BKR6", "barcode": "4900000000017", "article": "BKR6",
              "price_tyiyn": 30_000, "cost_tyiyn": 20_000, "unit": "шт", "category": "Фильтры", "qty": 4.0 },
            { "name": "MANN W 712 масл.", "barcode": "4011558000001", "price_tyiyn": 50_000,
              "cost_tyiyn": 35_000, "unit": "шт", "qty": 3.0 },
            { "name": "Услуга мойка", "price_tyiyn": 10_000, "qty": 1.0 },
        ])),
    )
    .await;
    let stats = ok(&app, "GET", "/api/v1/old-catalog", &owner, None).await;
    assert_eq!(stats["intake_todo"], 3);
    let list = ok(&app, "GET", "/api/v1/old-catalog/intake", &owner, None).await;
    assert_eq!(list.as_array().unwrap().len(), 3);
    let spark = row_id(&list, "Свеча NGK BKR6");
    let mann = row_id(&list, "MANN W 712 масл.");
    let wash = row_id(&list, "Услуга мойка");

    // Похожие: по коду строки — существующий товар сверху.
    let sim = ok(
        &app,
        "GET",
        &format!("/api/v1/old-catalog/{mann}/similar"),
        &owner,
        None,
    )
    .await;
    assert_eq!(sim[0]["id"], existing.as_str());
    assert_eq!(sim[0]["reason"], "code");

    // «Новый товар»: товар, коды строки, количество в черновике ревизии.
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "category_id": cat, "name": "Свеча NGK BKR6", "article": "BKR6",
                       "barcodes": ["4900000000017"], "sale_price_tyiyn": 30_000, "initial_cost_tyiyn": 20_000 });
    let n = ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{spark}/intake/new"),
        &owner,
        Some(body.clone()),
    )
    .await;
    assert_eq!(n["intake_status"], "new");
    assert_eq!(n["intake_counted"], 4);
    let rev = n["revision_id"].as_str().unwrap().to_string();
    let new_id = n["product"]["id"].as_str().unwrap().to_string();
    // Повтор `op_id` — тот же ответ, второго товара нет.
    let again = ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{spark}/intake/new"),
        &owner,
        Some(body),
    )
    .await;
    assert_eq!(again["product"]["id"], new_id.as_str());
    let count: i64 =
        sqlx::query_scalar("select count(*) from products where name = 'Свеча NGK BKR6'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1);

    // Товар уже посчитан в черновике: «Это он» прибавляет (ADR-059).
    ok(
        &app,
        "PUT",
        &format!("/api/v1/revisions/{rev}/lines"),
        &owner,
        Some(json!({ "product_id": existing, "counted_qty": 2, "mode": "add", "op_id": Uuid::now_v7() })),
    )
    .await;
    let link = json!({ "op_id": Uuid::now_v7(), "product_id": existing });
    let l = ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{mann}/intake/link"),
        &owner,
        Some(link.clone()),
    )
    .await;
    assert_eq!(l["intake_status"], "linked");
    assert_eq!(l["revision_id"], rev.as_str());
    ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{mann}/intake/link"),
        &owner,
        Some(link),
    )
    .await;
    let r = ok(
        &app,
        "GET",
        &format!("/api/v1/revisions/{rev}"),
        &owner,
        None,
    )
    .await;
    let line = |r: &Value, p: &str| {
        r["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["product_id"] == p)
            .and_then(|l| l["counted_qty"].as_i64())
    };
    assert_eq!(line(&r, &existing), Some(5));
    assert_eq!(line(&r, &new_id), Some(4));

    // «Пропустить» и «Отменить»: строка снова не разобрана, количество убрано из черновика.
    ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{wash}/intake/skip"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{mann}/intake/undo"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    let r = ok(
        &app,
        "GET",
        &format!("/api/v1/revisions/{rev}"),
        &owner,
        None,
    )
    .await;
    assert_eq!(line(&r, &existing), Some(2));
    let todo = ok(
        &app,
        "GET",
        "/api/v1/old-catalog/intake?status=todo&limit=50&offset=0",
        &owner,
        None,
    )
    .await;
    assert_eq!(todo.as_array().unwrap().len(), 1);
    assert_eq!(todo[0]["name"], "MANN W 712 масл.");
    assert!(todo[0]["product_id"].is_null());
    // Уже разобранную строку второй раз не разобрать.
    let (s, _) = call(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{wash}/intake/skip"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Снова «Это он», проведение — остатки встают.
    ok(
        &app,
        "POST",
        &format!("/api/v1/old-catalog/{mann}/intake/link"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": existing })),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/revisions/{rev}/post"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "force": true, "comment": "Остатки из Умага" })),
    )
    .await;
    assert_eq!(stock(&pool, &existing).await, 5);
    // У привязанного товара закупок нет — излишек по закупке строки Умага, а не по 0.
    let value: i64 = sqlx::query_scalar(
        "select stock_value_tyiyn from branch_products where product_id = $1::text::uuid",
    )
    .bind(&existing)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(value, 5 * 35_000);
    assert_eq!(stock(&pool, &new_id).await, 4);
    let stats = ok(&app, "GET", "/api/v1/old-catalog", &owner, None).await;
    assert_eq!(stats["intake_todo"], 0);

    // Без права «Товары» — 403.
    let emp = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        Some(json!({ "full_name": "Бакыт", "is_cashier": true, "is_master": false })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let u = ok(
        &app,
        "POST",
        "/api/v1/users",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "login": "bakyt", "password": "bakyt-pass-1", "employee_id": emp })),
    )
    .await;
    ok(
        &app,
        "PUT",
        &format!("/api/v1/users/{}/permissions", u["id"].as_str().unwrap()),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": ["view_sales"] })),
    )
    .await;
    let bakyt = login(&app, "bakyt", "bakyt-pass-1").await;
    for (m, uri) in [
        ("GET", "/api/v1/old-catalog/intake".to_string()),
        ("GET", format!("/api/v1/old-catalog/{wash}/similar")),
        ("POST", format!("/api/v1/old-catalog/{wash}/intake/undo")),
    ] {
        let body = (m == "POST").then(|| json!({ "op_id": Uuid::now_v7() }));
        let (s, _) = call(&app, m, &uri, &bakyt, body).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {uri}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn similar_by_words(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool, false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Фильтр салона Toyota Camry",
                     "sale_price_tyiyn": 50_000 })),
    )
    .await;
    ok(
        &app,
        "PUT",
        "/api/v1/old-catalog",
        &owner,
        Some(json!([{ "name": "Салонный фильтр Camry Toyota", "qty": 1.0 }])),
    )
    .await;
    let list = ok(&app, "GET", "/api/v1/old-catalog/intake", &owner, None).await;
    let id = row_id(&list, "Салонный фильтр Camry Toyota");
    let sim = ok(
        &app,
        "GET",
        &format!("/api/v1/old-catalog/{id}/similar"),
        &owner,
        None,
    )
    .await;
    assert_eq!(sim[0]["name"], "Фильтр салона Toyota Camry");
    assert_eq!(sim[0]["reason"], "name");
}
