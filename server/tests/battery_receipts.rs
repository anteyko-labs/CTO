//! Приём аккумуляторов в списке «Приход» (задача 025): приёмы за наличные из кассы и
//! аккумуляторы в оплату чека — строками вместе с накладными; отбор, поиск, отмена, права.
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

fn rows<'a>(list: &'a Value, kind: &str) -> Vec<&'a Value> {
    list.as_array()
        .unwrap()
        .iter()
        .filter(|r| r["kind"] == json!(kind))
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_intakes_and_tradeins_are_listed_in_receipts(pool: PgPool) {
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

    // Два приёма за наличные: 12,5 кг по 100 с и 2 кг по 80 с; второй — ошибочный, отменён.
    let first = ok(
        &app,
        "POST",
        "/api/v1/batteries/intake",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "grams": 12_500, "price_per_kg_tyiyn": 10_000, "comment": "Нексия" }),
    )
    .await;
    let second = ok(
        &app,
        "POST",
        "/api/v1/batteries/intake",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "grams": 2_000, "price_per_kg_tyiyn": 8_000 }),
    )
    .await;
    let second_id = second["id"].as_str().unwrap().to_string();
    ok(
        &app,
        "POST",
        &format!("/api/v1/batteries/intakes/{second_id}/reverse"),
        &admin,
        json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись весом" }),
    )
    .await;

    // Чек с аккумулятором в оплату: 3 кг по цене приёма (100 с) — 300 с.
    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        json!({ "name": "Разное", "kind": "other" }),
    )
    .await;
    let p = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Щётка", "sale_price_tyiyn": 50_000 }),
    )
    .await;
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
                "lines": [{ "product_id": p["id"], "qty": 5, "cost_tyiyn": 100_000 }] }),
    )
    .await;
    let sale = ok(
        &app,
        "POST",
        "/api/v1/sales",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": p["id"], "qty": 1, "unit_price_tyiyn": 50_000 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 20_000 },
                             { "method": "battery", "amount_tyiyn": 30_000, "grams": 3_000 }] }),
    )
    .await;
    let sale_id = sale["id"].as_str().unwrap().to_string();

    // «Все»: накладная, два приёма и аккумулятор в оплату чека.
    let all = get(&app, "/api/v1/receipts?kind=all", &admin).await;
    let intakes = rows(&all, "battery_intake");
    assert_eq!(intakes.len(), 2, "{all}");
    let one = intakes.iter().find(|r| r["id"] == first["id"]).unwrap();
    assert_eq!(one["grams"], json!(12_500));
    assert_eq!(one["price_per_kg_tyiyn"], json!(10_000));
    assert_eq!(one["total_tyiyn"], json!(125_000));
    assert_eq!(one["comment"], json!("Нексия"));
    assert_eq!(one["reversed"], json!(false));
    assert_eq!(one["user_name"], json!("Админ"));
    let two = intakes.iter().find(|r| r["id"] == second["id"]).unwrap();
    assert_eq!(two["reversed"], json!(true));
    let trade = rows(&all, "battery_tradein");
    assert_eq!(trade.len(), 1, "{all}");
    assert_eq!(trade[0]["id"], json!(sale_id));
    assert_eq!(trade[0]["number"], sale["number"]);
    assert_eq!(trade[0]["grams"], json!(3_000));
    assert_eq!(trade[0]["total_tyiyn"], json!(30_000));
    assert_eq!(rows(&all, "receipt").len(), 1);

    // «Приём аккумуляторов» — только они; накладные без них, как раньше.
    let only = get(&app, "/api/v1/receipts?kind=battery", &admin).await;
    assert_eq!(only.as_array().unwrap().len(), 3, "{only}");
    let plain = get(&app, "/api/v1/receipts", &admin).await;
    assert_eq!(plain.as_array().unwrap().len(), 1);
    // С фото, вне периода — приёмов нет.
    let photo = get(&app, "/api/v1/receipts?kind=battery&photo=true", &admin).await;
    assert_eq!(photo.as_array().unwrap().len(), 0);
    let old = get(
        &app,
        "/api/v1/receipts?kind=battery&from=2020-01-01&to=2020-01-31",
        &admin,
    )
    .await;
    assert_eq!(old.as_array().unwrap().len(), 0);
    // Поиск: по комментарию и по сумме в сомах.
    let by_text = get(
        &app,
        "/api/v1/receipts?kind=all&q=%D0%BD%D0%B5%D0%BA%D1%81",
        &admin,
    )
    .await;
    assert_eq!(by_text.as_array().unwrap().len(), 1, "{by_text}");
    assert_eq!(by_text[0]["id"], first["id"]);
    let by_sum = get(&app, "/api/v1/receipts?kind=battery&q=300", &admin).await;
    assert_eq!(by_sum.as_array().unwrap().len(), 1, "{by_sum}");
    assert_eq!(by_sum[0]["kind"], json!("battery_tradein"));

    // Карточка приёма: касса, кто отменил и почему.
    let card = get(
        &app,
        &format!("/api/v1/batteries/intakes/{second_id}"),
        &admin,
    )
    .await;
    assert_eq!(card["grams"], json!(2_000));
    assert_eq!(card["amount_tyiyn"], json!(16_000));
    assert_eq!(card["reversed_by"], json!("Админ"));
    assert_eq!(card["reverse_comment"], json!("ошиблись весом"));
    assert!(card["account_name"].as_str().is_some_and(|n| !n.is_empty()));
    let (s, _) = call(
        &app,
        "GET",
        &format!("/api/v1/batteries/intakes/{}", Uuid::now_v7()),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Возврат чека: строка возврата с минусом — итог за период по лому сходится.
    ok(
        &app,
        "POST",
        &format!("/api/v1/sales/{sale_id}/return"),
        &admin,
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 20_000 },
                             { "method": "battery", "amount_tyiyn": 30_000 }] }),
    )
    .await;
    let after = get(&app, "/api/v1/receipts?kind=battery", &admin).await;
    let trade = rows(&after, "battery_tradein");
    assert_eq!(trade.len(), 2, "{after}");
    let back = trade.iter().find(|r| r["id"] != json!(sale_id)).unwrap();
    assert_eq!(back["grams"], json!(-3_000));
    assert_eq!(back["total_tyiyn"], json!(-30_000));

    // Владелец видит то же.
    let mine = get(&app, "/api/v1/receipts?kind=battery", &owner).await;
    assert_eq!(mine.as_array().unwrap().len(), 4);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_rows_need_receipts_or_stock_in_right(pool: PgPool) {
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
    ok(
        &app,
        "POST",
        "/api/v1/shifts/open",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": e["id"], "cashier_pin": "1234" }),
    )
    .await;
    let intake = ok(
        &app,
        "POST",
        "/api/v1/batteries/intake",
        &admin,
        json!({ "op_id": Uuid::now_v7(), "grams": 1_000, "price_per_kg_tyiyn": 10_000 }),
    )
    .await;
    let id = intake["id"].as_str().unwrap().to_string();

    // Без «Приход товара» и «Оприходования» — 403; карточку приёма видит и с «Приёмом аккумуляторов».
    sqlx::query("delete from user_permissions where permission in ('receipts', 'stock_in')")
        .execute(&pool)
        .await
        .unwrap();
    for uri in ["/api/v1/receipts?kind=battery", "/api/v1/receipts?kind=all"] {
        let (s, v) = call(&app, "GET", uri, Some(&admin), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}: {v}");
    }
    get(&app, &format!("/api/v1/batteries/intakes/{id}"), &admin).await;
    sqlx::query("delete from user_permissions where permission = 'battery_intake'")
        .execute(&pool)
        .await
        .unwrap();
    let (s, _) = call(
        &app,
        "GET",
        &format!("/api/v1/batteries/intakes/{id}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Только «Оприходование» — приёмы видны.
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select id, branch_id, 'stock_in' from users where login = 'admin'")
        .execute(&pool)
        .await
        .unwrap();
    let list = get(&app, "/api/v1/receipts?kind=battery", &admin).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
}
