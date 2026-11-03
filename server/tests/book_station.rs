//! Задача 027: станция книжки — задание «книжка» в очереди печати, берёт его только станция `book`.
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

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, cookie, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_job_goes_to_book_station_only(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let party = ok(
        &app,
        "POST",
        "/api/v1/parties",
        &owner,
        json!({ "kind": "person", "first_name": "Азиз", "last_name": "Книжкин", "phone": "0700123456" }),
    )
    .await;
    let vehicle = ok(
        &app,
        "POST",
        &format!("/api/v1/parties/{}/vehicles", party["id"].as_str().unwrap()),
        &owner,
        json!({ "plate": "01KG777AAA", "brand": "Toyota", "model": "Camry" }),
    )
    .await;
    let book_station = Uuid::now_v7();
    let big_station = Uuid::now_v7();
    // Станция книжки отметилась — касса видит её в сети.
    let take = |station: Uuid, size: &'static str| json!({ "station_id": station, "size": size });
    let v = ok(
        &app,
        "POST",
        "/api/v1/label-jobs/take",
        &owner,
        take(book_station, "book"),
    )
    .await;
    assert!(v["job"].is_null());

    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "vehicle_id": vehicle["id"], "client": " Азиз ", "master": "Бакыт", "qr": true });
    let created = ok(
        &app,
        "POST",
        "/api/v1/label-jobs/book",
        &owner,
        body.clone(),
    )
    .await;
    assert_eq!(created["status"], json!("queued"));
    assert_eq!(created["station_online"], json!(true));
    // Повтор op_id — то же задание.
    let again = ok(&app, "POST", "/api/v1/label-jobs/book", &owner, body).await;
    assert_eq!(again["id"], created["id"]);

    // Станция этикеток книжку не берёт.
    let v = ok(
        &app,
        "POST",
        "/api/v1/label-jobs/take",
        &owner,
        take(big_station, "big"),
    )
    .await;
    assert!(v["job"].is_null(), "{v}");
    // Станция книжки берёт — с машиной и подписями.
    let v = ok(
        &app,
        "POST",
        "/api/v1/label-jobs/take",
        &owner,
        take(book_station, "book"),
    )
    .await;
    assert_eq!(v["job"]["id"], created["id"]);
    assert_eq!(v["job"]["book"]["vehicle_id"], vehicle["id"]);
    assert_eq!(v["job"]["book"]["client"], json!("Азиз"));
    assert_eq!(v["job"]["book"]["qr"], json!(true));
    let list = ok(
        &app,
        "GET",
        "/api/v1/label-jobs?size=book",
        &owner,
        json!(null),
    )
    .await;
    assert_eq!(list["jobs"][0]["what"], json!("Книжка 01KG777AAA"));
    ok(
        &app,
        "POST",
        &format!(
            "/api/v1/label-jobs/{}/done",
            created["id"].as_str().unwrap()
        ),
        &owner,
        json!({ "station_id": book_station }),
    )
    .await;

    // Машины нет — 404.
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/label-jobs/book",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "vehicle_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Неизвестный вид станции — 422.
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/label-jobs/take",
        &owner,
        Some(take(big_station, "huge")),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}
