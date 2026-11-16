//! Свой товар на нужды магазина (задача 038, ADR-198): остаток по себестоимости, касса не
//! меняется, строка прибыли «Товар на нужды магазина», сторно, права.
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

async fn add_admin(pool: &PgPool, login: &str, perms: Vec<String>) {
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(pool)
        .await
        .unwrap();
    let hash = hash_password("admin-pass-1".into()).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $3)")
        .bind(id)
        .bind(branch)
        .bind(login)
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
    .bind(id)
    .bind(branch)
    .bind(perms)
    .execute(pool)
    .await
    .unwrap();
}

async fn setup(pool: PgPool) -> Router {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let all: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    let no_expenses: Vec<String> = all.iter().filter(|k| *k != "expenses").cloned().collect();
    add_admin(&pool, "admin", all).await;
    add_admin(&pool, "noexp", no_expenses).await;
    app(build_state(pool, false).await.unwrap(), None)
}

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

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
    let (s, _, v) = call(app, method, uri, Some(cookie), body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
    v
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

async fn cash_total(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from cash_movements")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn amount_of(lines: &Value, key: &str) -> i64 {
    lines
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["key"] == json!(key))
        .map_or(0, |l| l["amount_tyiyn"].as_i64().unwrap())
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn own_use_takes_stock_at_cost_into_its_own_profit_line(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let noexp = login(&app, "noexp", "admin-pass-1").await;

    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        Some(json!({ "name": "Тряпки", "kind": "other" })),
    )
    .await;
    let item = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Тряпка",
                     "sale_price_tyiyn": 20_000 }),
        ),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // 10 шт по 100 с = 1 000 с.
    ok(
        &app,
        "POST",
        "/api/v1/stock-ins",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "начальный",
                     "lines": [{ "product_id": item, "qty": 10, "price_tyiyn": 10_000 }] })),
    )
    .await;
    let before_net =
        ok(&app, "GET", "/api/v1/reports/profit", &owner, None).await["totals"]["net_tyiyn"]
            .as_i64()
            .unwrap();
    let cash_before = cash_total(&pool).await;

    // Без права «Расходы» — 403.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/own-use",
        Some(&noexp),
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": 1 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Больше остатка — 409.
    let (s, _, v) = call(
        &app,
        "POST",
        "/api/v1/own-use",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": 11 })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // Администратор списывает 3 шт: остаток 7, стоимость −300 с, касса та же.
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "product_id": item, "qty": 3, "comment": "протирать машины" });
    let w = ok(&app, "POST", "/api/v1/own-use", &admin, Some(body.clone())).await;
    assert_eq!(w["qty"], 3);
    assert!(
        w["cost_tyiyn"].is_null(),
        "себестоимость администратору не видна"
    );
    // Повтор op_id — тот же ответ, без второго списания.
    let again = ok(&app, "POST", "/api/v1/own-use", &admin, Some(body)).await;
    assert_eq!(again["id"], w["id"]);
    assert_eq!(stock(&pool, &item).await, (7, 70_000));
    assert_eq!(cash_total(&pool).await, cash_before);

    // Прибыль: строка «Товар на нужды магазина» 300 с, чистая меньше на 300 с, по дням — так же.
    let p = ok(&app, "GET", "/api/v1/reports/profit?detail=1", &owner, None).await;
    assert_eq!(p["totals"]["own_use_tyiyn"], 30_000);
    assert_eq!(
        p["totals"]["expenses_tyiyn"], 0,
        "не расход по статье (ответ 2б)"
    );
    assert_eq!(
        p["totals"]["net_tyiyn"].as_i64().unwrap(),
        before_net - 30_000
    );
    let days: i64 = p["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["own_use_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(days, 30_000);
    let net_days: i64 = p["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["net_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(net_days, p["totals"]["net_tyiyn"].as_i64().unwrap());
    let rows = p["detail"]["own_use"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["product"], "Тряпка");
    assert_eq!(rows[0]["cost_tyiyn"], 30_000);
    // Владелец видит себестоимость в списке.
    let list = ok(&app, "GET", "/api/v1/own-use", &owner, None).await;
    assert_eq!(list[0]["cost_tyiyn"], 30_000);

    // Сторно без причины — 422; с причиной — остаток и прибыль вернулись.
    let wid = w["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/own-use/{wid}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let r = ok(
        &app,
        "POST",
        &format!("/api/v1/own-use/{wid}/reverse"),
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись товаром" })),
    )
    .await;
    assert_eq!(r["qty"], -3);
    assert_eq!(stock(&pool, &item).await, (10, 100_000));
    let p = ok(&app, "GET", "/api/v1/reports/profit", &owner, None).await;
    assert_eq!(p["totals"]["own_use_tyiyn"], 0);
    assert_eq!(p["totals"]["net_tyiyn"].as_i64().unwrap(), before_net);
    // Повторное сторно — 409.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/own-use/{wid}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Чужое списание администратор не сторнирует.
    let mine = ok(
        &app,
        "POST",
        "/api/v1/own-use",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": 1 })),
    )
    .await;
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/own-use/{}/reverse", mine["id"].as_str().unwrap()),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "не моё" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Журнал действий.
    let n: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action in ('stock.own_use', 'stock.own_use_reverse')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 3);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn own_use_shows_in_shift_profit_without_money(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cat = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        Some(json!({ "name": "Химия", "kind": "other" })),
    )
    .await;
    let item = ok(
        &app,
        "POST",
        "/api/v1/products",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Очиститель",
                     "sale_price_tyiyn": 50_000 }),
        ),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "POST",
        "/api/v1/stock-ins",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "начальный",
                     "lines": [{ "product_id": item, "qty": 2, "price_tyiyn": 25_000 }] })),
    )
    .await;
    let cashier = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        Some(json!({ "full_name": "Кассир", "is_cashier": true, "pin": "1234" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let shift = ok(
        &app,
        "POST",
        "/api/v1/shifts/open",
        &admin,
        Some(
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234",
                     "counted_tyiyn": 0 }),
        ),
    )
    .await;
    let sid = shift["id"].as_str().unwrap().to_string();
    let expected_before =
        ok(&app, "GET", "/api/v1/shifts/current", &admin, None).await["expected_tyiyn"]
            .as_i64()
            .unwrap();
    ok(
        &app,
        "POST",
        "/api/v1/own-use",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": 1 })),
    )
    .await;
    // Деньги в кассе смены те же; в прибыли смены — строка «Товар на нужды магазина».
    let expected_after =
        ok(&app, "GET", "/api/v1/shifts/current", &admin, None).await["expected_tyiyn"]
            .as_i64()
            .unwrap();
    assert_eq!(expected_after, expected_before);
    let r = ok(
        &app,
        "GET",
        &format!("/api/v1/shifts/{sid}/report"),
        &owner,
        None,
    )
    .await;
    assert_eq!(
        amount_of(&r["revenue"]["profit"], "own_use"),
        -25_000,
        "{r}"
    );
}
