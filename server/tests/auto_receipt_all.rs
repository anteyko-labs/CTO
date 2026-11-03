//! Задача 023: настройка «Товарный чек после любого чека — печатать сам» (`auto_receipt_all` в
//! «Масляная книжка и печать»): по умолчанию нет, сохраняется, старое значение без ключа — нет,
//! меняет только владелец.
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

async fn login(app: &Router, login: &str, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "login": login, "password": password }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn auto_receipt_all_setting(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let hash = hash_password("staff-pass-1".into()).await.unwrap();
    sqlx::query(
        "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin1', $3, 'admin', 'admin1')",
    )
    .bind(Uuid::now_v7())
    .bind(branch)
    .bind(hash)
    .execute(&pool)
    .await
    .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin1", "staff-pass-1").await;
    const URI: &str = "/api/v1/settings/oil-book";

    // По умолчанию — нет.
    let (s, v) = call(&app, "GET", URI, &owner, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["auto_receipt_all"], json!(false));

    // Старое значение без ключа читается как «нет».
    sqlx::query(
        "insert into settings (branch_id, key, value) values ($1, 'oil_book', '{\"auto_receipt\": true}'::jsonb)
         on conflict (branch_id, key) do update set value = excluded.value",
    )
    .bind(branch)
    .execute(&pool)
    .await
    .unwrap();
    let (_, v) = call(&app, "GET", URI, &owner, None).await;
    assert_eq!(v["auto_receipt"], json!(true));
    assert_eq!(v["auto_receipt_all"], json!(false));

    // Владелец включает — сохраняется.
    let mut body = v.clone();
    body["auto_receipt_all"] = json!(true);
    body["op_id"] = json!(Uuid::now_v7());
    let (s, v) = call(&app, "PUT", URI, &owner, Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (_, v) = call(&app, "GET", URI, &owner, None).await;
    assert_eq!(v["auto_receipt_all"], json!(true));

    // Администратор не меняет — 403, значение прежнее.
    body["auto_receipt_all"] = json!(false);
    body["op_id"] = json!(Uuid::now_v7());
    let (s, _) = call(&app, "PUT", URI, &admin, Some(body)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (_, v) = call(&app, "GET", URI, &owner, None).await;
    assert_eq!(v["auto_receipt_all"], json!(true));
}
