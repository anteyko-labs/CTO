//! «Похожие в базе» в карточке товара (задача 041 §5–7): `GET /products/{id}/similar`.
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

/// Названия товаров ответа: у списка — `items`, у поиска — массив.
fn names(v: &Value) -> Vec<String> {
    let rows = v.get("items").unwrap_or(v).as_array().unwrap();
    let mut out: Vec<String> = rows
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    out.sort();
    out
}

async fn product(
    app: &Router,
    owner: &str,
    cat: &str,
    name: &str,
    article: &str,
    code: &str,
) -> String {
    let codes: Vec<&str> = if code.is_empty() { vec![] } else { vec![code] };
    ok(
        app,
        "POST",
        "/api/v1/products",
        owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": name, "article": article,
                     "barcodes": codes, "sale_price_tyiyn": 50_000 }),
        ),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn category(app: &Router, owner: &str, name: &str) -> String {
    ok(
        app,
        "POST",
        "/api/v1/categories",
        owner,
        Some(json!({ "name": name, "kind": "filter" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn similar_by_article_words_and_rights(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool, false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let oil = category(&app, &owner, "Масляные").await;
    let excel = category(&app, &owner, "Из Excel").await;

    let me = product(
        &app,
        &owner,
        &excel,
        "ФИЛЬТР W 712/75 масляный",
        "W712/75",
        "4011558738005",
    )
    .await;
    product(&app, &owner, &oil, "Масляный фильтр Mann", "w-712-75", "").await;
    product(&app, &owner, &oil, "Фильтр масляный Toyota Camry", "", "").await;
    product(&app, &owner, &oil, "Фильтр воздушный", "", "").await;

    let v = ok(
        &app,
        "GET",
        &format!("/api/v1/products/{me}/similar"),
        &owner,
        None,
    )
    .await;
    let rows = v.as_array().unwrap();
    // По артикулу в любом написании; одного общего слова мало, «фильтр» не слово; себя нет.
    assert_eq!(rows[0]["name"], "Масляный фильтр Mann");
    assert_eq!(rows[0]["reason"], "code");
    assert_eq!(rows[0]["category_name"], "Масляные");
    assert!(rows[0]["stocks"].is_array());
    let found = names(&v);
    assert!(
        !found.contains(&"ФИЛЬТР W 712/75 масляный".to_string()),
        "{found:?}"
    );
    assert!(
        !found.contains(&"Фильтр воздушный".to_string()),
        "{found:?}"
    );

    // По словам названия: два общих слова.
    let camry = product(&app, &owner, &excel, "Toyota Camry фильтр масла", "", "").await;
    let v = ok(
        &app,
        "GET",
        &format!("/api/v1/products/{camry}/similar"),
        &owner,
        None,
    )
    .await;
    assert_eq!(names(&v), ["Фильтр масляный Toyota Camry"]);
    assert_eq!(v[0]["reason"], "name");

    // Без права «products» — отказ.
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
    let (s, _) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{me}/similar"),
        &bakyt,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
