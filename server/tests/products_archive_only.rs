//! «Товары» → «Показать архив» — только товары в архиве (задача 020): `archived=only` в списке
//! товаров; без отметки — только в продаже, `archived=true` — все, как раньше. Касса архив не ищет.
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

/// В категории «Фильтры»: «Фильтр в продаже» и «Фильтр архивный»; в «Масла» — «Масло архивное».
async fn world(pool: PgPool) -> (Router, String, String) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool, false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let filters = ok(
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
    let oils = ok(
        &app,
        "POST",
        "/api/v1/categories",
        &owner,
        Some(json!({ "name": "Масла", "kind": "other" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    for (cat, name, archived) in [
        (&filters, "Фильтр в продаже", false),
        (&filters, "Фильтр архивный", true),
        (&oils, "Масло архивное", true),
    ] {
        let p = ok(
            &app,
            "POST",
            "/api/v1/products",
            &owner,
            Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": name, "sale_price_tyiyn": 50_000 })),
        )
        .await;
        if archived {
            ok(
                &app,
                "PATCH",
                &format!("/api/v1/products/{}", p["id"].as_str().unwrap()),
                &owner,
                Some(json!({ "archived": true })),
            )
            .await;
        }
    }
    (app, owner, filters)
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn archive_only_lists_just_archived_products(pool: PgPool) {
    let (app, owner, filters) = world(pool).await;
    let list = |q: String| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            ok(
                &app,
                "GET",
                &format!("/api/v1/products/list{q}"),
                &owner,
                None,
            )
            .await
        }
    };

    let active = list(String::new()).await;
    assert_eq!(names(&active), ["Фильтр в продаже"]);
    let only = list("?archived=only".into()).await;
    assert_eq!(names(&only), ["Масло архивное", "Фильтр архивный"]);
    assert_eq!(only["totals"]["count"], 2);
    assert_eq!(only["totals"]["counts"]["all"], 2);
    let all = list("?archived=true".into()).await;
    assert_eq!(
        names(&all),
        ["Масло архивное", "Фильтр архивный", "Фильтр в продаже"]
    );

    // Поиск и категория — внутри архива.
    let by_text = list("?archived=only&q=%D0%A4%D0%B8%D0%BB%D1%8C%D1%82%D1%80".into()).await;
    assert_eq!(names(&by_text), ["Фильтр архивный"]);
    let by_cat = list(format!("?archived=only&category_id={filters}")).await;
    assert_eq!(names(&by_cat), ["Фильтр архивный"]);
    assert_eq!(by_cat["totals"]["count"], 1);

    // «Все филиалы» — тоже только архив.
    let branches = list("?archived=only&branch=all".into()).await;
    assert_eq!(names(&branches), ["Масло архивное", "Фильтр архивный"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn till_search_still_skips_archive(pool: PgPool) {
    let (app, owner, _) = world(pool).await;
    let found = ok(
        &app,
        "GET",
        "/api/v1/products?q=%D0%A4%D0%B8%D0%BB%D1%8C%D1%82%D1%80",
        &owner,
        None,
    )
    .await;
    assert_eq!(names(&found), ["Фильтр в продаже"]);
}
