//! Вход, сессии и права на уровне HTTP (SPEC-01).
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
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Админ')")
        .bind(Uuid::now_v7())
        .bind(branch)
        .bind(hash)
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn wrong_password_and_unknown_login_look_the_same(pool: PgPool) {
    let app = setup(pool).await;
    let a = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": "owner", "password": "nope-nope" })),
    )
    .await;
    let b = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": "ghost", "password": "nope-nope" })),
    )
    .await;
    assert_eq!(a.0, StatusCode::UNAUTHORIZED);
    assert_eq!((a.0, a.2), (b.0, b.2));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn roles_are_enforced_on_server(pool: PgPool) {
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (status, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["role"], "admin");
    assert_eq!(
        call(&app, "GET", "/api/v1/users", Some(&admin), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/stock/verify", Some(&admin), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/products", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );

    let owner = login(&app, "owner", "owner-pass-1").await;
    let (status, _, users) = call(&app, "GET", "/api/v1/users", Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(users.as_array().map(Vec::len), Some(2));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn last_owner_cannot_be_disabled(pool: PgPool) {
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&owner), None).await;
    let uri = format!("/api/v1/users/{}", me["id"].as_str().unwrap());
    let (status, _, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&owner),
        Some(json!({ "active": false })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn catalog_rules(pool: PgPool) {
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Масла", "kind": "oil" })),
    )
    .await;
    let cat_id = cat["id"].as_str().unwrap().to_string();

    // Масло без объёма канистры.
    let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Mobil 5W-30" });
    assert_eq!(
        call(&app, "POST", "/api/v1/products", Some(&admin), Some(body))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Администратор не задаёт цену розлива.
    let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Mobil 5W-30", "container_ml": 4000, "pour_price_per_l_tyiyn": 50000 });
    assert_eq!(
        call(&app, "POST", "/api/v1/products", Some(&admin), Some(body))
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let body = json!({
        "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Mobil Super 3000 5W-30", "article": "MB-153",
        "container_ml": 4000, "barcodes": ["5055107433591"], "attrs": { "viscosity": "5W-30" }
    });
    let (status, _, p) = call(&app, "POST", "/api/v1/products", Some(&admin), Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(p.get("avg_cost_tyiyn").is_none());
    let pid = p["id"].as_str().unwrap().to_string();

    // Повторный штрихкод — 409 с названием товара.
    let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Другое", "container_ml": 1000, "barcodes": ["5055107433591"] });
    let (status, _, err) = call(&app, "POST", "/api/v1/products", Some(&admin), Some(body)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Mobil Super")
    );

    // Поиск по части артикула, по штрихкоду и по характеристике.
    let (_, _, found) = call(&app, "GET", "/api/v1/products?q=b-15", Some(&admin), None).await;
    assert_eq!(found.as_array().map(Vec::len), Some(1));
    let (status, _, _) = call(
        &app,
        "GET",
        "/api/v1/products/by-barcode/5055107433591",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, found) = call(
        &app,
        "GET",
        "/api/v1/products?attr.viscosity=5W-40",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(found.as_array().map(Vec::len), Some(0));

    // Похожие при опечатке.
    let (_, _, similar) = call(
        &app,
        "GET",
        "/api/v1/products/similar?name=Mobil%20Super%203000%205W30",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(similar[0]["id"].as_str(), Some(pid.as_str()));

    // Внутренний штрихкод.
    let (status, _, code) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/barcodes"),
        Some(&admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(code["code"], "2200000000019");
}
