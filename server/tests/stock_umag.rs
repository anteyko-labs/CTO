//! «Склад → Остатки» (SPEC-36 §14, ADR-125): список с ненулевым остатком и подытоги по категориям;
//! выключатель старой базы Умаг (SPEC-36 §15, ADR-126).
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
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
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

async fn send(app: &Router, method: &str, who: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, method, uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn get(app: &Router, who: &str, uri: &str) -> Value {
    let (s, _, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn category(app: &Router, owner: &str, name: &str, kind: &str) -> String {
    let c = send(
        app,
        "POST",
        owner,
        "/api/v1/categories",
        json!({ "name": name, "kind": kind }),
    )
    .await;
    c["id"].as_str().unwrap().to_string()
}

async fn product(app: &Router, owner: &str, body: Value) -> String {
    let mut body = body;
    body["op_id"] = json!(Uuid::now_v7());
    let p = send(app, "POST", owner, "/api/v1/products", body).await;
    p["id"].as_str().unwrap().to_string()
}

async fn stock_in(app: &Router, who: &str, product: &str, qty: i64, price: i64) {
    send(
        app,
        "POST",
        who,
        "/api/v1/stock-ins",
        json!({ "op_id": Uuid::now_v7(), "comment": "остатки",
                "lines": [{ "product_id": product, "qty": qty, "price_tyiyn": price }] }),
    )
    .await;
}

fn group<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["totals"]["category_totals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["name"] == name)
        .unwrap_or_else(|| panic!("нет группы {name}: {list}"))
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stock_list_keeps_nonzero_and_sums_groups(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let oils = category(&app, &owner, "Моторные масла", "oil").await;
    let filters = category(&app, &owner, "Фильтры", "filter").await;
    let other = category(&app, &owner, "Разное", "other").await;
    let oil_a = product(
        &app,
        &owner,
        json!({ "category_id": oils, "name": "Масло А 5W-30",
        "container_ml": 4000, "sale_price_tyiyn": 200_000, "attrs": { "viscosity": "5W-30" } }),
    )
    .await;
    let oil_b = product(
        &app,
        &owner,
        json!({ "category_id": oils, "name": "Масло Б 5W-40",
        "container_ml": 4000, "sale_price_tyiyn": 240_000 }),
    )
    .await;
    let filter = product(
        &app,
        &owner,
        json!({ "category_id": filters, "name": "Фильтр масляный",
        "sale_price_tyiyn": 50_000 }),
    )
    .await;
    let empty = product(
        &app,
        &owner,
        json!({ "category_id": filters, "name": "Фильтр пустой",
        "sale_price_tyiyn": 40_000 }),
    )
    .await;
    let short = product(
        &app,
        &owner,
        json!({ "category_id": other, "name": "Лампа в минусе",
        "sale_price_tyiyn": 10_000 }),
    )
    .await;
    // 8 кан. + 2,5 л и ровно 1 канистра; 3 фильтра; лампа ушла в минус.
    stock_in(&app, &owner, &oil_a, 34_500, 150_000).await;
    stock_in(&app, &owner, &oil_b, 4_000, 180_000).await;
    stock_in(&app, &owner, &filter, 3, 30_000).await;
    sqlx::query(
        "insert into branch_products (branch_id, product_id, stock_qty) select id, $1::uuid, -2 from branches
         on conflict (branch_id, product_id) do update set stock_qty = -2",
    )
    .bind(&short)
    .execute(&pool)
    .await
    .unwrap();

    let uri = "/api/v1/products/list?stock=in_stock&group=category&limit=2000";
    let list = get(&app, &owner, uri).await;
    let ids: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&oil_a.as_str()) && ids.contains(&short.as_str()),
        "{list}"
    );
    assert!(
        !ids.contains(&empty.as_str()),
        "нулевой остаток не показывается: {list}"
    );
    assert_eq!(list["totals"]["count"], 4);

    // Подытог группы равен сумме её строк.
    let groups = list["totals"]["category_totals"].as_array().unwrap();
    assert_eq!(groups.len(), 3, "{list}");
    let mut all_sale = 0;
    let mut all_purchase = 0;
    for g in groups {
        let rows: Vec<&Value> = list["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["category_id"] == g["category_id"])
            .collect();
        assert_eq!(g["count"], rows.len());
        let sum = |k: &str| rows.iter().map(|r| r[k].as_i64().unwrap()).sum::<i64>();
        assert_eq!(g["sale_tyiyn"], sum("stock_sale_tyiyn"), "{g}");
        assert_eq!(g["purchase_tyiyn"], sum("stock_purchase_tyiyn"), "{g}");
        all_sale += g["sale_tyiyn"].as_i64().unwrap();
        all_purchase += g["purchase_tyiyn"].as_i64().unwrap();
    }
    assert_eq!(list["totals"]["sale_tyiyn"], all_sale);
    assert_eq!(list["totals"]["purchase_tyiyn"], all_purchase);
    let oil = group(&list, "Моторные масла");
    assert_eq!(oil["ml"], 38_500);
    assert_eq!(oil["container_ml"], 4000);
    assert_eq!(oil["count"], 2);
    // 8,625 кан. × 2 000 с + 1 × 2 400 с
    assert_eq!(oil["sale_tyiyn"], 1_725_000 + 240_000);
    let f = group(&list, "Фильтры");
    assert_eq!(
        (f["pieces"].as_i64(), f["count"].as_i64()),
        (Some(3), Some(1))
    );
    assert_eq!(group(&list, "Разное")["pieces"], -2);

    // Администратору закупки нет — ни в строках, ни в подытогах (ADR-008).
    let list = get(&app, &admin, uri).await;
    assert!(list["totals"].get("purchase_tyiyn").is_none(), "{list}");
    for g in list["totals"]["category_totals"].as_array().unwrap() {
        assert!(g.get("purchase_tyiyn").is_none(), "{g}");
        assert!(g["sale_tyiyn"].as_i64().is_some());
    }
    for r in list["items"].as_array().unwrap() {
        assert!(r.get("stock_purchase_tyiyn").is_none(), "{r}");
        assert!(r.get("purchase_price_tyiyn").is_none(), "{r}");
        assert!(r["last_purchase_price_tyiyn"].is_null(), "{r}");
    }

    // Без `group` подытогов нет; отбор по категории и поиск работают.
    let plain = get(&app, &owner, "/api/v1/products/list?stock=in_stock").await;
    assert!(plain["totals"].get("category_totals").is_none());
    let one = get(
        &app,
        &owner,
        &format!("/api/v1/products/list?stock=in_stock&group=category&category_id={oils}&q=5W-40"),
    )
    .await;
    assert_eq!(one["totals"]["count"], 1, "{one}");
    assert_eq!(one["items"][0]["id"], json!(oil_b));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn umag_switch_hides_and_blocks(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cat = category(&app, &owner, "Фильтры", "filter").await;
    let item = product(
        &app,
        &owner,
        json!({ "category_id": cat, "name": "Фильтр учёта",
        "sale_price_tyiyn": 50_000 }),
    )
    .await;
    send(
        &app,
        "PUT",
        &owner,
        "/api/v1/old-catalog",
        json!([
            { "name": "Фильтр Умаг связан", "barcode": "4600000000011", "extra_code": "4600000000028" },
            { "name": "Фильтр Умаг свободен", "barcode": "4600000000035" }
        ]),
    )
    .await;
    let hits = get(
        &app,
        &owner,
        "/api/v1/old-catalog/search?barcode=4600000000011",
    )
    .await;
    let row = hits[0]["id"].as_str().unwrap().to_string();
    let free = get(
        &app,
        &owner,
        "/api/v1/old-catalog/search?barcode=4600000000035",
    )
    .await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    send(
        &app,
        "POST",
        &owner,
        &format!("/api/v1/old-catalog/{row}/link"),
        json!({ "op_id": Uuid::now_v7(), "product_id": item }),
    )
    .await;

    // Включено по умолчанию; администратор видит, но не переключает.
    let s = get(&app, &admin, "/api/v1/settings/old-catalog").await;
    assert_eq!(s, json!({ "enabled": true, "rows": 2, "linked": 1 }));
    let (st, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/old-catalog",
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let off = send(
        &app,
        "PUT",
        &owner,
        "/api/v1/settings/old-catalog",
        json!({ "enabled": false }),
    )
    .await;
    assert_eq!(off, json!({ "enabled": false, "rows": 2, "linked": 1 }));
    assert_eq!(
        get(&app, &admin, "/api/v1/old-catalog").await["enabled"],
        false
    );

    // Поиск пуст, связывание и пары — 409 «Умаг выключен».
    for uri in [
        "/api/v1/old-catalog/search?barcode=4600000000035",
        "/api/v1/old-catalog/search?q=%D0%A4%D0%B8%D0%BB%D1%8C%D1%82%D1%80",
    ] {
        assert_eq!(get(&app, &owner, uri).await, json!([]), "{uri}");
    }
    let body = || Some(json!({ "op_id": Uuid::now_v7(), "product_id": item }));
    for (method, uri, b) in [
        ("POST", format!("/api/v1/old-catalog/{free}/link"), body()),
        ("POST", format!("/api/v1/old-catalog/{free}/reject"), body()),
        (
            "POST",
            "/api/v1/old-catalog/auto-link".to_string(),
            Some(json!({ "op_id": Uuid::now_v7() })),
        ),
        ("GET", "/api/v1/old-catalog/pairs".to_string(), None),
    ] {
        let (st, _, out) = call(&app, method, &uri, Some(&owner), b).await;
        assert_eq!(st, StatusCode::CONFLICT, "{uri}: {out}");
        assert!(out.to_string().contains("Умаг выключен"), "{out}");
    }
    // Новый товар из строки Умага — тоже 409.
    let (st, _, _) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Из Умага",
                     "sale_price_tyiyn": 1_000, "old_catalog_id": free }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Блока «Умаг» в карточке нет, а перенесённые коды остались — товар находится по старой наклейке.
    let card = get(&app, &owner, &format!("/api/v1/products/{item}")).await;
    assert!(
        card["umag"].is_null() && card["umag_more"] == json!([]),
        "{card}"
    );
    let found = get(&app, &admin, "/api/v1/products/by-barcode/4600000000028").await;
    assert_eq!(found["id"], json!(item));

    // Журнал и лента владельца.
    let actions: Vec<String> = sqlx::query_scalar(
        "select action from audit_log where action like 'old_catalog.%able' order by created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actions, ["old_catalog.disable"]);
    let feed = get(&app, &owner, "/api/v1/notifications?limit=20").await;
    assert!(
        feed.to_string().contains("Старая база Умаг выключена"),
        "{feed}"
    );

    // Повтор той же настройки журнал не пишет; включается обратно одной кнопкой — связи на месте.
    send(
        &app,
        "PUT",
        &owner,
        "/api/v1/settings/old-catalog",
        json!({ "enabled": false }),
    )
    .await;
    let on = send(
        &app,
        "PUT",
        &owner,
        "/api/v1/settings/old-catalog",
        json!({ "enabled": true }),
    )
    .await;
    assert_eq!(on, json!({ "enabled": true, "rows": 2, "linked": 1 }));
    let hits = get(
        &app,
        &owner,
        "/api/v1/old-catalog/search?barcode=4600000000035",
    )
    .await;
    assert_eq!(hits[0]["id"], json!(free));
    let card = get(&app, &owner, &format!("/api/v1/products/{item}")).await;
    assert_eq!(card["umag"]["id"], json!(row));
    let n: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action like 'old_catalog.%able'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 2);
}
