//! Бухгалтер (задача 039, ADR-201): всё как владелец на чтение, запись и прибыль — 403.
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn accountant_reads_like_owner_but_cannot_write_or_see_profit(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    ok(
        &app,
        "POST",
        "/api/v1/users",
        &owner,
        Some(json!({ "login": "buh", "password": "buh-pass-12", "full_name": "Бухгалтер", "role": "accountant" })),
    )
    .await;
    let buh = login(&app, "buh", "buh-pass-12").await;

    let me = ok(&app, "GET", "/api/v1/auth/me", &buh, None).await;
    assert_eq!(me["role"], json!("owner"), "{me}");
    assert_eq!(me["accountant"], json!(true));
    let me_owner = ok(&app, "GET", "/api/v1/auth/me", &owner, None).await;
    assert_eq!(me_owner["accountant"], json!(false));

    // Чтение — как у владельца.
    ok(&app, "GET", "/api/v1/expenses", &buh, None).await;
    ok(&app, "GET", "/api/v1/sales", &buh, None).await;
    ok(&app, "GET", "/api/v1/users", &buh, None).await;
    let dash = ok(&app, "GET", "/api/v1/owner/dashboard", &buh, None).await;
    assert!(!dash.to_string().contains("net_tyiyn"), "{dash}");
    let dash_owner = ok(&app, "GET", "/api/v1/owner/dashboard", &owner, None).await;
    assert!(dash_owner.to_string().contains("net_tyiyn"));

    // Прибыль — 403.
    for uri in ["/api/v1/reports/profit", "/api/v1/settings/profit-report"] {
        let (s, _, _) = call(&app, "GET", uri, Some(&buh), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}");
    }
    ok(&app, "GET", "/api/v1/reports/profit", &owner, None).await;

    // Запись — 403.
    for (m, uri, body) in [
        (
            "POST",
            "/api/v1/users",
            json!({ "login": "x1", "password": "x-pass-123", "full_name": "x" }),
        ),
        ("POST", "/api/v1/expenses", json!({})),
        ("POST", "/api/v1/sales", json!({})),
        ("PUT", "/api/v1/settings/profit-report", json!({})),
    ] {
        let (s, _, _) = call(&app, m, uri, Some(&buh), Some(body)).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{m} {uri}");
    }

    // Свой пароль и выход — можно.
    ok(
        &app,
        "POST",
        "/api/v1/auth/password",
        &buh,
        Some(json!({ "op_id": Uuid::now_v7(), "old_password": "buh-pass-12", "new_password": "buh-pass-34" })),
    )
    .await;
}
