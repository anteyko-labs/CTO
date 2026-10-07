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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn every_product_has_a_barcode(pool: PgPool) {
    // Товара без штрихкода не бывает, кодов может быть несколько (ADR-026).
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let cat_id = cat["id"].as_str().unwrap().to_string();

    // Код не указан — система выдала свой.
    let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Фильтр без кода" });
    let (status, _, p) = call(&app, "POST", "/api/v1/products", Some(&admin), Some(body)).await;
    assert_eq!(status, StatusCode::OK);
    let codes = p["barcodes"].as_array().unwrap();
    assert_eq!(codes.len(), 1);
    let own = codes[0].as_str().unwrap().to_string();
    assert!(own.starts_with("22"));
    let pid = p["id"].as_str().unwrap().to_string();

    // Единственный код не отвязать.
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/products/{pid}/barcodes/{own}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Заводской код этого же товара — второй код в той же карточке, без дубля.
    let (status, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/barcodes"),
        Some(&admin),
        Some(json!({ "code": "4006381333931" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for code in ["4006381333931", own.as_str()] {
        let (status, _, found) = call(
            &app,
            "GET",
            &format!("/api/v1/products/by-barcode/{code}"),
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(found["id"].as_str(), Some(pid.as_str()));
    }

    // Теперь кодов два, лишний отвязывается.
    let (status, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/products/{pid}/barcodes/4006381333931"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{pid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["barcodes"].as_array().map(Vec::len), Some(1));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_change_fee_is_a_setting(pool: PgPool) {
    // Ставку замены меняет только владелец (ADR-019, ADR-027).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (status, _, v) = call(&app, "GET", "/api/v1/settings/sales", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["oil_change_master_fee_tyiyn"], 3000);
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/settings/sales",
            Some(&admin),
            Some(json!({ "oil_change_master_fee_tyiyn": 5000 })),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            "/api/v1/settings/sales",
            Some(&owner),
            Some(json!({ "oil_change_master_fee_tyiyn": 5000 })),
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, _, v) = call(&app, "GET", "/api/v1/settings/sales", Some(&admin), None).await;
    assert_eq!(v["oil_change_master_fee_tyiyn"], 5000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_card_shows_what_was_delivered(pool: PgPool) {
    // Карточка поставщика: его накладные и что он привозил (SPEC-10).
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let (_, _, product) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр" })),
    )
    .await;
    let (_, _, supplier) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(json!({ "name": "Поставщик А", "phone": "+996700000000" })),
    )
    .await;
    let (_, _, other) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(json!({ "name": "Поставщик Б" })),
    )
    .await;
    let sid = supplier["id"].as_str().unwrap().to_string();
    let line = |qty: i64, cost: i64| json!({ "product_id": product["id"], "qty": qty, "cost_tyiyn": cost });
    for (who, qty, cost) in [(&sid, 10, 30_000), (&sid, 5, 20_000)] {
        let (status, _, _) = call(
            &app,
            "POST",
            "/api/v1/receipts",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": who, "lines": [line(qty, cost)] }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "supplier_id": other["id"], "lines": [line(1, 1000)] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Накладные фильтруются по поставщику: чужая не попадает.
    let (status, _, docs) = call(
        &app,
        "GET",
        &format!("/api/v1/receipts?supplier_id={sid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(docs.as_array().map(Vec::len), Some(2));

    // Что привозил: количество и сумма сложены, цена — из последней поставки.
    let (status, _, supplies) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}/supplies"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(supplies.as_array().map(Vec::len), Some(1));
    let row = &supplies[0];
    assert_eq!(row["receipts"], 2);
    assert_eq!(row["qty"], 15);
    assert_eq!(row["amount_tyiyn"], 50_000);
    assert_eq!(row["last_price_tyiyn"], 4000);

    let (status, _, one) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(one["name"], "Поставщик А");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_debt_is_tracked_and_paid(pool: PgPool) {
    // Накладная в долг: мы должны поставщику, оплата долг гасит (SPEC-10).
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let (_, _, product) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр" })),
    )
    .await;
    let (_, _, supplier) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(json!({ "name": "Поставщик В" })),
    )
    .await;
    let sid = supplier["id"].as_str().unwrap().to_string();

    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "supplier_id": sid, "payment": "debt",
            "lines": [{ "product_id": product["id"], "qty": 10, "cost_tyiyn": 50_000 }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, party) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(party["balance_tyiyn"], -50_000);
    assert_eq!(party["role"], "supplier");

    // Частичная оплата уменьшает наш долг.
    let (status, _, r) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 20_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(r["balance_tyiyn"], -30_000);

    // В списке долгов он виден как «должны мы».
    let (_, _, debtors) = call(
        &app,
        "GET",
        "/api/v1/parties?only_debtors=true",
        Some(&admin),
        None,
    )
    .await;
    let rows = debtors.as_array().unwrap();
    assert!(
        rows.iter()
            .any(|p| p["id"] == json!(sid) && p["balance_tyiyn"] == -30_000)
    );

    // Накладная в долг без поставщика не проводится.
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "payment": "debt",
            "lines": [{ "product_id": product["id"], "qty": 1, "cost_tyiyn": 100 }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn price_changes_are_owner_only_and_notify(pool: PgPool) {
    // Цену заведённого товара меняет только владелец, и он же об этом узнаёт (ADR-032).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let (_, _, product) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр",
            "sale_price_tyiyn": 50_000
        })),
    )
    .await;
    let pid = product["id"].as_str().unwrap().to_string();

    // Администратор цену не меняет, минимальный остаток — меняет.
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/v1/products/{pid}/prices"),
            Some(&admin),
            Some(json!({ "sale_price_tyiyn": 60_000 })),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/v1/products/{pid}/prices"),
            Some(&admin),
            Some(json!({ "min_stock": 3 })),
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &format!("/api/v1/products/{pid}/prices"),
            Some(&owner),
            Some(json!({ "sale_price_tyiyn": 60_000 })),
        )
        .await
        .0,
        StatusCode::OK
    );

    // Владельцу видно, что цену меняли; администратору уведомления не отдаются.
    assert_eq!(
        call(&app, "GET", "/api/v1/notifications", Some(&admin), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(list["unseen"].as_i64().unwrap() >= 1);
    let first = &list["items"][0];
    assert_eq!(first["title"], "Изменена цена товара");
    assert_eq!(first["new"], json!(true));

    // Открыли экран — уведомления больше не новые.
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/notifications/seen",
            Some(&owner),
            Some(json!({}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    assert_eq!(list["unseen"], 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn duplicate_products_can_be_merged(pool: PgPool) {
    // Дубль сливается в основной товар: коды и остаток переходят, дубль в архив (SPEC-02).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let make = |name: &'static str, code: &'static str| {
        let app = app.clone();
        let owner = owner.clone();
        let cat_id = cat["id"].clone();
        async move {
            let (_, _, p) = call(
                &app,
                "POST",
                "/api/v1/products",
                Some(&owner),
                Some(json!({
                    "op_id": Uuid::now_v7(), "category_id": cat_id, "name": name,
                    "barcodes": [code], "sale_price_tyiyn": 50_000
                })),
            )
            .await;
            p["id"].as_str().unwrap().to_string()
        }
    };
    let main = make("Фильтр основной", "4600000000017").await;
    let dup = make("Фильтр дубль", "4600000000024").await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "lines": [{ "product_id": dup, "qty": 4, "cost_tyiyn": 20_000 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, merged) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{dup}/merge"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "into_product_id": main })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(merged["stock_qty"], 4);
    assert_eq!(merged["stock_value_tyiyn"], 20_000);
    let codes = merged["barcodes"].as_array().unwrap();
    assert_eq!(codes.len(), 2);

    // Код дубля теперь ведёт на основной товар, сам дубль в архиве.
    let (_, _, found) = call(
        &app,
        "GET",
        "/api/v1/products/by-barcode/4600000000024",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(found["id"].as_str(), Some(main.as_str()));
    let (_, _, old) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{dup}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(old["archived"], json!(true));
    assert_eq!(old["stock_qty"], 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expenses_take_money_from_the_till(pool: PgPool) {
    // Расход из кассы уменьшает наличные, сторно их возвращает (SPEC-06).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let till = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["is_default"] == json!(true))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Кладём в кассу деньги, чтобы было из чего платить.
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/cash/movements",
            Some(&admin),
            Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 100_000, "comment": "размен" })),
        )
        .await
        .0,
        StatusCode::OK
    );

    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&admin), None).await;
    let list = articles.as_array().unwrap();
    // Личные расходы администратору не видны.
    assert!(list.iter().all(|a| a["name"] != json!("Личные расходы")));
    let household = list
        .iter()
        .find(|a| a["name"] == json!("Хозтовары"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (status, _, exp) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 30_000, "comment": "швабра" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let balance = |cookie: &str| {
        let app = app.clone();
        let cookie = cookie.to_string();
        let till = till.clone();
        async move {
            let (_, _, accs) =
                call(&app, "GET", "/api/v1/cash/accounts", Some(&cookie), None).await;
            accs.as_array()
                .unwrap()
                .iter()
                .find(|a| a["id"] == json!(till))
                .unwrap()["balance_tyiyn"]
                .as_i64()
                .unwrap()
        }
    };
    assert_eq!(balance(&admin).await, 70_000);

    // Расход задним числом и по статье владельца администратору недоступен.
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/expenses",
            Some(&admin),
            Some(json!({
                "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1000,
                "expense_date": "2026-01-01"
            })),
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );

    // Сторно возвращает деньги в кассу.
    let eid = exp["id"].as_str().unwrap();
    let (status, _, back) = call(
        &app,
        "POST",
        &format!("/api/v1/expenses/{eid}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(back["amount_tyiyn"], -30_000);
    assert_eq!(balance(&admin).await, 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn profit_is_owner_only(pool: PgPool) {
    // Прибыль и сводка — только владельцу (инвариант 13, SPEC-08).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    for path in ["/api/v1/reports/profit", "/api/v1/owner/dashboard"] {
        assert_eq!(
            call(&app, "GET", path, Some(&admin), None).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(&app, "GET", path, Some(&owner), None).await.0,
            StatusCode::OK
        );
    }
    let (_, _, report) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(report["totals"]["net_tyiyn"], 0);
    assert_eq!(report["totals"]["margin_bp"], json!(null));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn brute_force_is_locked_out(pool: PgPool) {
    let app = setup(pool).await;
    let attempt = |login: &'static str, password: &'static str| {
        let app = app.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/v1/auth/login",
                None,
                Some(json!({ "login": login, "password": password })),
            )
            .await
            .0
        }
    };
    for _ in 0..5 {
        assert_eq!(
            attempt("owner", "wrong-pass").await,
            StatusCode::UNAUTHORIZED
        );
    }
    // Даже верный пароль отклоняется, пока действует блокировка.
    assert_eq!(
        attempt("OWNER", "owner-pass-1").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Неизвестный логин блокируется так же, не выдавая отсутствие пользователя.
    for _ in 0..5 {
        assert_eq!(
            attempt("ghost", "wrong-pass").await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        attempt("ghost", "wrong-pass").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Другие пользователи входят как обычно, успешный вход сбрасывает счётчик.
    assert_eq!(
        attempt("admin", "wrong-pass").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(attempt("admin", "admin-pass-1").await, StatusCode::OK);
}
