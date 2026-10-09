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

    // Наличными из кассы — только в открытой смене: деньги уходят из кассы (SPEC-10, SPEC-05).
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 20_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    // Мимо кассы платит только владелец.
    let outside = json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 20_000, "method": "outside" });
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(outside.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Частичная оплата уменьшает наш долг.
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (status, _, r) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(outside),
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
    assert_eq!(first["title"], "Изменена цена: Фильтр");
    assert!(first["details"].as_str().unwrap().starts_with("цена: было"));
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
    let cash_in = || {
        call(
            &app,
            "POST",
            "/api/v1/cash/movements",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 100_000, "comment": "размен" }),
            ),
        )
    };
    // Без открытой смены деньги через кассу не проходят: их не с чем сверить (SPEC-05).
    assert_eq!(cash_in().await.0, StatusCode::CONFLICT);
    // Новый кассир сразу получает оплату по умолчанию: 2 % с валовой и оклад 30 000 (SPEC-07).
    let (status, _, emp) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&admin),
        Some(json!({ "full_name": "Кассир", "is_cashier": true, "is_master": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let eid = emp["id"].as_str().unwrap();
    let (_, _, rules) = call(
        &app,
        "GET",
        &format!("/api/v1/employees/{eid}/pay-rules"),
        Some(&owner),
        None,
    )
    .await;
    let rules = rules.as_array().unwrap();
    assert!(
        rules
            .iter()
            .any(|r| r["kind"] == json!("revenue_percent") && r["rate_bp"] == json!(200))
    );
    assert!(
        rules
            .iter()
            .any(|r| r["kind"] == json!("monthly_salary") && r["amount_tyiyn"] == json!(3_000_000))
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/shifts/open",
            Some(&admin),
            Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": eid })),
        )
        .await
        .0,
        StatusCode::OK
    );
    // Кладём в кассу деньги, чтобы было из чего платить.
    assert_eq!(cash_in().await.0, StatusCode::OK);

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
async fn offline_snapshot_has_no_cost(pool: PgPool) {
    // Снимок каталога для кассы: без себестоимости, с кодами и остатком (SPEC-09).
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
    call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр офлайн",
            "barcodes": ["4600000000031"], "sale_price_tyiyn": 50_000
        })),
    )
    .await;

    let (status, _, snap) = call(&app, "GET", "/api/v1/offline/snapshot", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let product = &snap["products"][0];
    assert_eq!(product["name"], "Фильтр офлайн");
    assert_eq!(product["barcodes"][0], "4600000000031");
    assert!(product.get("avg_cost_tyiyn").is_none());
    assert!(product.get("stock_value_tyiyn").is_none());
    assert!(snap["version"].as_str().is_some_and(|v| !v.is_empty()));
    assert_eq!(snap["oil_change_master_fee_tyiyn"], 3000);
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_close_handover_and_cash_reversals(pool: PgPool) {
    // Закрытие с недостачей, сдача кассы в сейф, сторно и права на сейфы (SPEC-05, ADR-037).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let cashier = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_cashier) values ($1, $2, 'Айбек', true)")
        .bind(cashier)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let acc = |pred: &dyn Fn(&Value) -> bool| {
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| pred(a))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let till = acc(&|a| a["kind"] == json!("register"));
    let shop_safe = acc(&|a| a["kind"] == json!("safe") && a["owner_only"] == json!(false));
    let owner_safe = acc(&|a| a["owner_only"] == json!(true));
    let post = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let op = || Uuid::now_v7();

    let (s, _, shift) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": op(), "cashier_employee_id": cashier }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(
        &admin,
        "/api/v1/cash/movements".into(),
        json!({ "op_id": op(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Ошибочное внесение отменяется один раз, второе сторно — 409.
    let (_, _, moves) = call(
        &app,
        "GET",
        &format!("/api/v1/cash/accounts/{till}"),
        Some(&admin),
        None,
    )
    .await;
    let mid = moves[0]["id"].as_str().unwrap().to_string();
    assert_eq!(moves[0]["reversible"], json!(true));
    let (s, _, wrong) = post(
        &admin,
        "/api/v1/cash/movements".into(),
        json!({ "op_id": op(), "kind": "cash_in", "amount_tyiyn": 1_000, "comment": "опечатка" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(wrong["balance_tyiyn"], 501_000);
    let (_, _, moves) = call(
        &app,
        "GET",
        &format!("/api/v1/cash/accounts/{till}"),
        Some(&admin),
        None,
    )
    .await;
    let wid = moves[0]["id"].as_str().unwrap().to_string();
    assert_ne!(wid, mid);
    let rev = json!({ "op_id": op(), "comment": "опечатка" });
    let (s, _, back) = post(&admin, format!("/api/v1/cash/movements/{wid}/reverse"), rev).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(back["balance_tyiyn"], 500_000);
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/cash/movements/{wid}/reverse"),
        json!({ "op_id": op(), "comment": "ещё раз" }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Администратор не двигает деньги из сейфов и не видит движения сейфа владельца.
    let (s, _, _) = post(
        &admin,
        "/api/v1/cash/transfers".into(),
        json!({ "op_id": op(), "from_account_id": shop_safe, "to_account_id": till, "amount_tyiyn": 1 }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/api/v1/cash/accounts/{owner_safe}"),
            Some(&admin),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // Закрытие с недостачей 1 000 с: комментарий обязателен, удержание с кассира.
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": op(), "counted_tyiyn": 400_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, closed) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": op(), "counted_tyiyn": 400_000, "comment": "не нашли" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(closed["diff_tyiyn"], -100_000);
    assert_eq!(closed["expected_tyiyn"], 500_000);
    assert_eq!(closed["handover_pending"], json!(true));
    let shortage: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1 and kind = 'shortage'",
    )
    .bind(cashier)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(shortage, -100_000);

    // Сдача кассы: 350 000 в сейф магазина, 50 000 остаются на размен; второй раз — 409.
    let (s, _, handed) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/handover"),
        json!({ "op_id": op(), "amount_tyiyn": 350_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(handed["handover_pending"], json!(false));
    assert_eq!(handed["to_safe_tyiyn"], 350_000);
    assert_eq!(handed["left_tyiyn"], 50_000);
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/handover"),
        json!({ "op_id": op(), "amount_tyiyn": 0 }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Сдачу кассы после закрытия отменяет только владелец.
    let tid: Uuid =
        sqlx::query_scalar("select id from cash_transfers order by created_at desc limit 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/cash/transfers/{tid}/reverse"),
        json!({ "op_id": op(), "comment": "не туда" }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = post(
        &owner,
        format!("/api/v1/cash/transfers/{tid}/reverse"),
        json!({ "op_id": op(), "comment": "не туда" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let balance = |id: &str| {
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["id"] == json!(id))
            .unwrap()["balance_tyiyn"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(balance(&till), 400_000);
    assert_eq!(balance(&shop_safe), 0);

    // Владелец получил итог смены уведомлением.
    let (_, _, notes) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    assert!(
        notes["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["action"] == json!("shift.handover"))
    );

    // Сумма движений каждой кассы равна её остатку (инвариант 6).
    let drift: i64 = sqlx::query_scalar(
        "select count(*) from cash_accounts a where a.balance_tyiyn <>
           coalesce((select sum(m.amount_tyiyn) from cash_movements m where m.account_id = a.id), 0)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(drift, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_repayment_goes_through_the_till(pool: PgPool) {
    // Погашение: наличные — в кассу смены, карта — на счёт с комиссией; прибыль не растёт (SPEC-10).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let post = |cookie: &str, uri: &str, body: Value| {
        let app = app.clone();
        let (cookie, uri) = (cookie.to_string(), uri.to_string());
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let (_, _, emp) = post(
        &admin,
        "/api/v1/employees",
        json!({ "full_name": "Кассир", "is_cashier": true }),
    )
    .await;
    let (s, _, _) = post(
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": emp["id"] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, party) = post(
        &admin,
        "/api/v1/parties",
        json!({ "name": "Эрлан", "phone": "+996555000111" }),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(
        &owner,
        "/api/v1/debts/adjust",
        json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 1_000_000, "comment": "из тетради" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, r) = post(
        &admin,
        "/api/v1/debts/repayments",
        json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 600_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["balance_tyiyn"], 400_000);
    let (s, _, r) = post(
        &admin,
        "/api/v1/debts/repayments",
        json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 400_000, "method": "card" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["balance_tyiyn"], 0);
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let bal = |kind: &str| {
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == json!(kind))
            .unwrap()["balance_tyiyn"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(bal("register"), 600_000);
    // 0,5 % банку с 4 000 с = 20 с.
    assert_eq!(bal("bank"), 400_000 - 2_000);
    let (_, _, shift) = call(&app, "GET", "/api/v1/shifts/current", Some(&admin), None).await;
    assert_eq!(shift["expected_tyiyn"], 600_000);
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["gross_tyiyn"], 0);
    assert_eq!(profit["totals"]["bank_fee_tyiyn"], 2_000);

    // Акт сверки: долг из тетради, оплата наличными и картой, сальдо ноль.
    let (s, _, act) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}/reconciliation"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rows = act["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1]["document"], "Оплата наличными");
    assert_eq!(rows[2]["document"], "Оплата безналичная");
    assert_eq!(act["debit_total_tyiyn"], 1_000_000);
    assert_eq!(act["credit_total_tyiyn"], 1_000_000);
    assert_eq!(act["opening_tyiyn"], 0);
    assert_eq!(act["closing_tyiyn"], 0);

    // Сторно оплаты наличными: деньги уходят из кассы, долг возвращается, второй раз — 409.
    let (_, _, card) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}/card"),
        Some(&owner),
        None,
    )
    .await;
    let cash_row = card["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == json!("repayment") && t["amount_tyiyn"] == json!(-600_000))
        .unwrap()
        .clone();
    assert_eq!(cash_row["reversible"], json!(true));
    let lid = cash_row["ledger_id"].as_str().unwrap();
    let rev = json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись клиентом" });
    let (s, _, _) = post(
        &admin,
        &format!("/api/v1/debts/ledger/{lid}/reverse"),
        rev.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, r) = post(&owner, &format!("/api/v1/debts/ledger/{lid}/reverse"), rev).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["balance_tyiyn"], 600_000);
    let (s, _, _) = post(
        &owner,
        &format!("/api/v1/debts/ledger/{lid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let till = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == json!("register"))
        .unwrap()["balance_tyiyn"]
        .as_i64()
        .unwrap();
    assert_eq!(till, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn hostile_inputs_and_payroll_reversals(pool: PgPool) {
    // Огромные суммы, кривой JSON, неверный способ выплаты — 4xx по-русски; сторно удержания
    // и выплаты владельцем; кассу сдают только в сейф (SPEC-05, SPEC-07).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let post = |cookie: &str, uri: &str, body: Value| {
        let app = app.clone();
        let (cookie, uri) = (cookie.to_string(), uri.to_string());
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let (_, _, emp) = post(
        &admin,
        "/api/v1/employees",
        json!({ "full_name": "Кассир", "is_cashier": true }),
    )
    .await;
    let eid = emp["id"].as_str().unwrap().to_string();
    let (_, _, shift) = post(
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": eid }),
    )
    .await;
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, e) = post(
        &admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 5_000_000_000_000_000_000i64, "comment": "x" }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("100 000 000")
    );
    let (s, _, _) = post(
        &admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 100_000, "comment": "размен" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Кривые запросы — тот же формат ошибки, что и остальные.
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cash/movements")
                .header(header::COOKIE, &admin)
                .header("x-device-id", Uuid::now_v7().to_string())
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{not json"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(res.status().is_client_error());
    let body: Value =
        serde_json::from_slice(&res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["error"]["code"], "bad_request");

    let (s, _, _) = post(
        &owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": eid, "amount_tyiyn": 100, "source": "foo" }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Закрытие с недостачей 100 с: удержание с кассира, владелец его сторнирует.
    let (s, _, _) = post(
        &admin,
        &format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 90_000, "comment": "не хватает" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let shortage: Uuid =
        sqlx::query_scalar("select id from payroll_accruals where kind = 'shortage'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let rev = json!({ "op_id": Uuid::now_v7(), "comment": "нашли деньги" });
    let (s, _, _) = post(
        &admin,
        &format!("/api/v1/payroll/accruals/{shortage}/reverse"),
        rev.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = post(
        &owner,
        &format!("/api/v1/payroll/accruals/{shortage}/reverse"),
        rev,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = post(
        &owner,
        &format!("/api/v1/payroll/accruals/{shortage}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let net: i64 = sqlx::query_scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'shortage'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(net, 0);

    // Сдать кассу в саму кассу нельзя.
    let till: Uuid = sqlx::query_scalar("select id from cash_accounts where kind = 'register'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (s, _, _) = post(
        &admin,
        &format!("/api/v1/shifts/{sid}/handover"),
        json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 100, "to_account_id": till }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Выплата мимо кассы и её сторно: долг перед сотрудником вернулся.
    let (s, _, paid) = post(
        &owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": eid, "amount_tyiyn": 5_000, "source": "outside", "advance": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let pid = paid["id"].as_str().unwrap().to_string();
    let (s, _, back) = post(
        &owner,
        &format!("/api/v1/payouts/{pid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        back["balance_tyiyn"],
        paid["balance_tyiyn"].as_i64().unwrap() + 5_000
    );
    let (s, _, one) = call(
        &app,
        "GET",
        &format!("/api/v1/shifts/{sid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(one["diff_tyiyn"], -10_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn parallel_first_visit_and_garbage_input(pool: PgPool) {
    // 20 одновременных первых обращений: кассы заведены один раз, сервер не замирает;
    // мусор в тексте — 422; администратор не изымает из сейфа; отклонённый офлайн-чек — владельцу.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let mut tasks = Vec::new();
    for i in 0..20 {
        let app = app.clone();
        let cookie = if i % 2 == 0 {
            owner.clone()
        } else {
            admin.clone()
        };
        let uri = if i % 3 == 0 {
            "/api/v1/expense-articles"
        } else {
            "/api/v1/cash/accounts"
        };
        tasks.push(tokio::spawn(async move {
            call(&app, "GET", uri, Some(&cookie), None).await.0
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), StatusCode::OK);
    }
    let accounts: i64 = sqlx::query_scalar("select count(*) from cash_accounts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(accounts, 4);

    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&admin),
        Some(json!({ "full_name": "Имя\u{0}с нулём" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(e["error"]["code"], "validation");

    let safe: Uuid =
        sqlx::query_scalar("select id from cash_accounts where kind = 'safe' and not owner_only")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "account_id": safe, "kind": "cash_out", "amount_tyiyn": 1, "comment": "x" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let op = Uuid::now_v7();
    for _ in 0..2 {
        let (s, _, _) = call(
            &app,
            "POST",
            "/api/v1/sales/offline-rejected",
            Some(&admin),
            Some(json!({ "op_id": op, "error": "товар в архиве", "body": { "lines": [] } })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (_, _, notes) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let rejected = notes["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["action"] == json!("sale.offline_rejected"))
        .count();
    assert_eq!(rejected, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn overdue_debt_is_highlighted(pool: PgPool) {
    // Фирма платит в течение 14 дней: свежий долг не просрочен, долг месячной давности — да (SPEC-10).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "ОсОО Просрочка", "kind": "company", "inn": "01234567890123" })),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    assert_eq!(party["due_days"], 14);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/adjust",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 100_000, "comment": "из тетради" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(p["overdue_tyiyn"], 0);
    // Долг, взятый месяц назад и не оплаченный, — просрочен.
    let (branch, user): (Uuid, Uuid) =
        sqlx::query_as("select branch_id, id from users where login = 'owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date, user_id, device_id)
         values ($1, $2, $3, 'debt', 50000, (now() at time zone 'Asia/Bishkek')::date - 30, $4, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(branch)
    .bind(Uuid::parse_str(&pid).unwrap())
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update parties set balance_tyiyn = balance_tyiyn + 50000 where id = $1")
        .bind(Uuid::parse_str(&pid).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(p["overdue_tyiyn"], 50_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn company_cabinet_by_inn(pool: PgPool) {
    // Кабинет юрлица: логин — ИНН, начальный пароль avtodom2026 с обязательной сменой;
    // сбрасывает пароль только владелец (SPEC-12, ADR-049).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let inn = "01204201910123";
    let (_, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "ОсОО Бишкек Такси", "kind": "company", "inn": inn })),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/adjust",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 250_000, "comment": "из тетради" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let client_login = |password: &'static str| {
        let app = app.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/v1/client/login",
                None,
                Some(json!({ "inn": inn, "password": password })),
            )
            .await
        }
    };
    assert_eq!(client_login("wrong-pass").await.0, StatusCode::UNAUTHORIZED);
    let (s, cookie, me) = client_login("avtodom2026").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["must_change"], json!(true));
    let c = cookie.unwrap();
    // Кабинет — не вход в кассу.
    assert_eq!(
        call(&app, "GET", "/api/v1/products", Some(&c), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    // До смены пароля данные закрыты.
    assert_eq!(
        call(&app, "GET", "/api/v1/client/sales", Some(&c), None)
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/client/password",
        Some(&c),
        Some(json!({ "old_password": "avtodom2026", "new_password": "avtodom2026" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/client/password",
        Some(&c),
        Some(json!({ "old_password": "avtodom2026", "new_password": "taxi-secret-9" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, me) = call(&app, "GET", "/api/v1/client/me", Some(&c), None).await;
    assert_eq!(me["must_change"], json!(false));
    assert_eq!(me["balance_tyiyn"], 250_000);
    let (s, _, act) = call(&app, "GET", "/api/v1/client/reconciliation", Some(&c), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(act["closing_tyiyn"], 250_000);
    assert_eq!(
        call(&app, "GET", "/api/v1/client/sales", Some(&c), None)
            .await
            .0,
        StatusCode::OK
    );
    // Начальный пароль больше не подходит.
    assert_eq!(
        client_login("avtodom2026").await.0,
        StatusCode::UNAUTHORIZED
    );

    // Сбросить пароль может только владелец; старые входы фирмы закрываются.
    let reset = format!("/api/v1/parties/{pid}/cabinet/reset");
    assert_eq!(
        call(&app, "POST", &reset, Some(&admin), Some(json!({})))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &reset, Some(&owner), Some(json!({})))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/client/me", Some(&c), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _, me) = client_login("avtodom2026").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(me["must_change"], json!(true));
    let (_, _, st) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}/cabinet"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(st["available"], json!(true));
    assert_eq!(st["login"], json!(inn));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stranger_cannot_lock_out_owner(pool: PgPool) {
    // Подбор пароля с чужого адреса закрывает вход только этому адресу (ADR-016, ADR-049).
    let app = setup(pool).await;
    let attempt = |password: &'static str, addr: Option<&'static str>| {
        let app = app.clone();
        async move {
            let mut req = Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(a) = addr {
                req = req.header("cf-connecting-ip", a);
            }
            let body = json!({ "login": "owner", "password": password }).to_string();
            app.oneshot(req.body(Body::from(body)).unwrap())
                .await
                .unwrap()
                .status()
        }
    };
    for _ in 0..5 {
        assert_eq!(
            attempt("wrong-pass", Some("203.0.113.7")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        attempt("owner-pass-1", Some("203.0.113.7")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Владелец со своего адреса входит.
    assert_eq!(
        attempt("owner-pass-1", Some("198.51.100.20")).await,
        StatusCode::OK
    );
}
