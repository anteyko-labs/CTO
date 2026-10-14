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
                // В долг: оплата из кассы проверяется отдельно (ADR-062).
                json!({ "op_id": Uuid::now_v7(), "supplier_id": who, "payment": "debt", "lines": [line(qty, cost)] }),
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
        Some(json!({ "op_id": Uuid::now_v7(), "supplier_id": other["id"], "payment": "debt", "lines": [line(1, 1000)] })),
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
    let app = setup(pool.clone()).await;
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
        Some(json!({ "op_id": Uuid::now_v7(), "paid_from": "outside", "lines": [{ "product_id": dup, "qty": 4, "cost_tyiyn": 20_000 }] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // Кросс-номер дубля должен перейти основной карточке (SPEC-17).
    sqlx::query("insert into product_cross_numbers (product_id, code, code_norm) values ($1::uuid, 'W 712/75', 'W71275')")
        .bind(&dup)
        .execute(&pool)
        .await
        .unwrap();

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
    let cross: Vec<(String,)> = sqlx::query_as(
        "select product_id::text from product_cross_numbers where code_norm = 'W71275'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(cross, vec![(main.clone(),)]);
    let (data,): (Value,) =
        sqlx::query_as("select data from audit_log where action = 'product.merge'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(data["barcodes"], json!(["4600000000024"]));
    assert_eq!(data["cross_numbers"], json!(["W 712/75"]));
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
        // Кассира заводит владелец: вместе с ним назначается оплата (ADR-059).
        Some(&owner),
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
    // Отчёт смены сходится: на начало + движения наличных = должно быть (пересчёт при закрытии не входит).
    let ladder = |v: &Value| {
        v["opening_expected_tyiyn"].as_i64().unwrap()
            + v["breakdown"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["sum_tyiyn"].as_i64().unwrap())
                .sum::<i64>()
    };
    assert_eq!(ladder(&closed), 500_000);
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
    // Сдача кассы — после закрытия, в отчёт смены не попадает.
    assert_eq!(ladder(&handed), 500_000);
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
        &owner,
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
        &owner,
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
            // Адрес клиента ставит наш прокси (Caddy) в X-Forwarded-For (ADR-058).
            if let Some(a) = addr {
                req = req.header("x-forwarded-for", a);
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn filter_analogs_by_cross_numbers(pool: PgPool) {
    // Кросс-номер связывает фильтры в обе стороны и находится поиском в любом написании (SPEC-17).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let post = |uri: &str, body: Value| {
        let (app, owner, uri) = (app.clone(), owner.clone(), uri.to_string());
        async move { call(&app, "POST", &uri, Some(&owner), Some(body)).await }
    };
    let (_, _, cat) = post(
        "/api/v1/categories",
        json!({ "name": "Фильтры", "kind": "filter" }),
    )
    .await;
    let product = |name: &'static str, article: &'static str| {
        let cat = cat["id"].clone();
        async move {
            post(
                "/api/v1/products",
                json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": name, "article": article, "sale_price_tyiyn": 60000 }),
            )
            .await
            .2["id"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let toyota = product("Фильтр Toyota", "90915-YZZE1").await;
    let mann = product("Фильтр Mann", "W 712/75").await;
    let (s, _, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/products/{mann}/cross"),
        Some(&owner),
        Some(json!({ "codes": ["90915 yzze1", "", "90915-YZZE1"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, a) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{toyota}/analogs"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(a.as_array().unwrap().len(), 1);
    assert_eq!(a[0]["id"], json!(mann));
    assert!(a[0].get("avg_cost_tyiyn").is_none());
    let (_, _, b) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{mann}/analogs"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(b[0]["id"], json!(toyota));
    let (_, _, found) = call(
        &app,
        "GET",
        "/api/v1/products?q=90915YZZE",
        Some(&owner),
        None,
    )
    .await;
    let ids: Vec<&str> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&mann.as_str()));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn favorites_heatmap_and_compare(pool: PgPool) {
    // Плитки кассы задаёт владелец, видят все; тепловая карта и сравнение — только владельцу (ADR-055).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Масла", "kind": "oil" })),
    )
    .await;
    let (_, _, p) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Totachi 5W-30", "container_ml": 4000, "sale_price_tyiyn": 250_000 })),
    )
    .await;
    let (_, _, svc) = call(
        &app,
        "POST",
        "/api/v1/services",
        Some(&owner),
        Some(json!({ "name": "Замена масла", "price_tyiyn": 40_000 })),
    )
    .await;
    let tiles =
        json!([{ "kind": "product", "id": p["id"] }, { "kind": "service", "id": svc["id"] }]);
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/favorites",
        Some(&admin),
        Some(tiles.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/favorites",
        Some(&owner),
        Some(tiles),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, got) = call(
        &app,
        "GET",
        "/api/v1/settings/favorites",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(got.as_array().unwrap().len(), 2);
    assert_eq!(got[0]["product"]["name"], "Totachi 5W-30");
    assert_eq!(got[1]["service"]["name"], "Замена масла");

    let (s, _, _) = call(&app, "GET", "/api/v1/reports/heatmap", Some(&admin), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, heat) = call(
        &app,
        "GET",
        "/api/v1/reports/heatmap?days=7",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(heat.is_array());
    let (_, _, dash) = call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None).await;
    assert!(dash["compare"]["yesterday"]["revenue_tyiyn"].is_i64());
    assert!(dash["compare"]["week_ago"]["sales_count"].is_i64());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_catalog_import_search_and_initial_cost(pool: PgPool) {
    // Список старой программы — только подсказка; товар заводится с закупкой для оценки ревизии (SPEC-20).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let rows = json!([
        { "name": "ГАЗПРОМ ЛИТОЛ 24", "barcode": "4670026845129", "cost_tyiyn": 13000, "price_tyiyn": 17000, "unit": "шт" },
        { "name": "Новый продукт", "barcode": "6942687231640" },
        { "name": "  ", "barcode": "1" }
    ]);
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/old-catalog",
        Some(&admin),
        Some(rows.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, r) = call(&app, "PUT", "/api/v1/old-catalog", Some(&owner), Some(rows)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["rows"], 1);
    let (_, _, hits) = call(
        &app,
        "GET",
        "/api/v1/old-catalog/search?barcode=4670026845129",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(hits[0]["exact"], json!(true));
    assert_eq!(hits[0]["price_tyiyn"], 17000);
    let (_, _, hits) = call(
        &app,
        "GET",
        "/api/v1/old-catalog/search?q=литол",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(hits.as_array().unwrap().len(), 1);

    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Смазки", "kind": "other" })),
    )
    .await;
    let (s, _, p) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "ГАЗПРОМ ЛИТОЛ 24",
                     "barcodes": ["4670026845129"], "sale_price_tyiyn": 17000, "initial_cost_tyiyn": 13000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["last_purchase_price_tyiyn"], 13000);
    // Позиция теперь отмечена как заведённая.
    let (_, _, hits) = call(
        &app,
        "GET",
        "/api/v1/old-catalog/search?barcode=4670026845129",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(hits[0]["linked_product_id"], p["id"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn many_addresses_cannot_lock_out_owner(pool: PgPool) {
    // Подбор с многих адресов не закрывает вход владельцу: общий счётчик только уведомляет
    // (ADR-059). Предел на свой адрес остаётся, и попытки во время блокировки её не продлевают.
    let app = setup(pool.clone()).await;
    let attempt = |addr: String, password: &str| {
        let app = app.clone();
        let body = json!({ "login": "owner", "password": password }).to_string();
        async move {
            let req = Request::builder()
                .method("POST")
                .uri("/api/v1/auth/login")
                .header("x-device-id", Uuid::now_v7().to_string())
                .header("x-forwarded-for", addr)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap();
            app.oneshot(req).await.unwrap().status()
        }
    };
    // 60 неверных паролей с 30 разных адресов (по два с каждого): каждый — просто «неверно».
    for n in 0..60 {
        let s = attempt(format!("10.1.0.{}", n % 30), "wrong-password").await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "попытка {n}");
    }
    // Владелец со своего адреса входит верным паролем.
    assert_eq!(
        attempt("198.51.100.20".into(), "owner-pass-1").await,
        StatusCode::OK
    );
    // Владельцу пришло одно уведомление о подборе (в окне — не чаще одного).
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let floods: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["action"] == "auth.login_flood")
        .collect();
    assert_eq!(floods.len(), 1);
    assert!(floods[0]["title"].as_str().unwrap().contains("owner"));

    // Предел на адрес работает: пять неверных — и с этого адреса даже верный пароль ждёт.
    let bad = "203.0.113.9".to_string();
    for _ in 0..5 {
        assert_eq!(
            attempt(bad.clone(), "wrong-password").await,
            StatusCode::UNAUTHORIZED
        );
    }
    for _ in 0..10 {
        assert_eq!(
            attempt(bad.clone(), "wrong-password").await,
            StatusCode::TOO_MANY_REQUESTS
        );
    }
    assert_eq!(
        attempt(bad.clone(), "owner-pass-1").await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // Попытки во время блокировки счётчик не копят: когда она истечёт, счёт начнётся заново.
    let failures: i32 =
        sqlx::query_scalar("select failures from login_attempts where login = 'owner|203.0.113.9'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(failures, 0);
    sqlx::query("update login_attempts set locked_until = now() - interval '1 second', last_failed_at = now() - interval '16 minutes' where login = 'owner|203.0.113.9'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        attempt(bad.clone(), "wrong-password").await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(attempt(bad, "owner-pass-1").await, StatusCode::OK);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn only_owner_makes_a_cashier(pool: PgPool) {
    // Кассир сразу получает 2 % и оклад — назначает это только владелец (SPEC-07, ADR-059).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&admin),
        Some(json!({ "full_name": "Сам себе кассир", "is_cashier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Мастера администратор заводит, но кассиром его не сделает.
    let (s, _, m) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&admin),
        Some(json!({ "full_name": "Мастер", "is_master": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let mid = m["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/employees/{mid}"),
        Some(&admin),
        Some(json!({ "is_cashier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let rules: i64 = sqlx::query_scalar("select count(*) from employee_pay_rules")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rules, 0);
    // Владелец — можно, оплата по умолчанию назначена.
    let (s, _, c) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Кассир", "is_cashier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let cid = c["id"].as_str().unwrap().to_string();
    // Администратор правит имя кассира, признак шлёт без изменения — это можно.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/employees/{cid}"),
        Some(&admin),
        Some(json!({ "full_name": "Кассир Айбек", "is_cashier": true, "is_master": false, "active": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rules: i64 = sqlx::query_scalar("select count(*) from employee_pay_rules")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rules, 2);
    // Владелец узнаёт о новых сотрудниках и правках.
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let actions: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["action"].as_str())
        .collect();
    assert_eq!(
        actions.iter().filter(|a| **a == "employee.create").count(),
        2
    );
    assert!(actions.contains(&"employee.update"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_count_adds_or_replaces(pool: PgPool) {
    // Товар на двух полках: 5 и 3 — в ревизии 8, а не 3; «заменить» записывает как есть (ADR-059).
    let app = setup(pool.clone()).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр на двух полках" });
    let (_, _, p) = call(&app, "POST", "/api/v1/products", Some(&admin), Some(body)).await;
    let pid = p["id"].as_str().unwrap().to_string();
    let (s, _, rev) = call(
        &app,
        "POST",
        "/api/v1/revisions",
        Some(&admin),
        Some(json!({ "category_id": null })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let lines = format!("/api/v1/revisions/{rid}/lines");
    let one = format!("/api/v1/revisions/{rid}/lines/{pid}");
    let (_, _, got) = call(&app, "GET", &one, Some(&admin), None).await;
    assert_eq!(got["counted_qty"], Value::Null);
    let put_count = |qty: i64, mode: Option<&'static str>| {
        let (app, admin, lines, pid) = (app.clone(), admin.clone(), lines.clone(), pid.clone());
        async move {
            let mut body = json!({ "product_id": pid, "counted_qty": qty });
            if let Some(m) = mode {
                body["mode"] = json!(m);
            }
            call(&app, "PUT", &lines, Some(&admin), Some(body)).await
        }
    };
    let (s, _, r) = put_count(5, Some("add")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(r["counted_qty"], 5);
    // Между полками остаток изменился — «должно быть» остаётся с первого пересчёта.
    sqlx::query("update branch_products set stock_qty = stock_qty - 1 where product_id = $1")
        .bind(Uuid::parse_str(&pid).unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let first: Option<i64> =
        sqlx::query_scalar("select stock_at_count from revision_lines where product_id = $1")
            .bind(Uuid::parse_str(&pid).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    let (_, _, r) = put_count(3, Some("add")).await;
    assert_eq!(r["counted_qty"], 8);
    let (_, _, got) = call(&app, "GET", &one, Some(&admin), None).await;
    assert_eq!(got["counted_qty"], 8);
    let kept: Option<i64> =
        sqlx::query_scalar("select stock_at_count from revision_lines where product_id = $1")
            .bind(Uuid::parse_str(&pid).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(kept, first);
    // «Заменить» и запрос без режима (экран «Ревизия») записывают число как есть.
    let (_, _, r) = put_count(4, Some("set")).await;
    assert_eq!(r["counted_qty"], 4);
    let (_, _, r) = put_count(6, None).await;
    assert_eq!(r["counted_qty"], 6);
    let (s, _, _) = put_count(1, Some("plus")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_questions_usage_is_owner_only(pool: PgPool) {
    // Расход на вопросы боту текстом: только владельцу, за сегодня и за месяц (SPEC-21).
    let app = setup(pool.clone()).await;
    let (owner_id, branch): (Uuid, Uuid) =
        sqlx::query_as("select id, branch_id from users where login = 'owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "insert into bot_questions (id, chat_id, user_id, branch_id, question, input_tokens, output_tokens, answer, error)
         values ($1, 1, $3, $4, 'а', 1000, 200, 'б', null),
                ($2, 1, $3, $4, 'в', 0, 0, null, 'лимит вопросов')",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(owner_id)
    .bind(branch)
    .execute(&pool)
    .await
    .unwrap();
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (status, _, _) = call(
        &app,
        "GET",
        "/api/v1/settings/telegram/questions",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (status, _, body) = call(
        &app,
        "GET",
        "/api/v1/settings/telegram/questions",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["enabled"], false);
    assert_eq!(body["today"]["questions"], 1);
    assert_eq!(body["today"]["input_tokens"], 1000);
    assert_eq!(body["month"]["output_tokens"], 200);
}

// ---------- Исправления по ревью (ADR-062) ----------

/// Счета филиала по виду: касса, счёт, сейф владельца.
async fn accounts_of(app: &Router, owner: &str) -> (String, String, String) {
    let (_, _, accounts) = call(app, "GET", "/api/v1/cash/accounts", Some(owner), None).await;
    let find = |pred: &dyn Fn(&Value) -> bool| {
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
    (
        find(&|a| a["kind"] == json!("register")),
        find(&|a| a["kind"] == json!("bank")),
        find(&|a| a["owner_only"] == json!(true)),
    )
}

async fn balance_of(pool: &PgPool, account: &str) -> i64 {
    sqlx::query_scalar("select balance_tyiyn from cash_accounts where id = $1::uuid")
        .bind(account)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Категория и товар; возвращает id товара.
async fn product(app: &Router, owner: &str, name: &str, price: i64, min_stock: i64) -> String {
    let (_, _, cat) = call(
        app,
        "POST",
        "/api/v1/categories",
        Some(owner),
        Some(json!({ "name": format!("Кат {name}"), "kind": "filter" })),
    )
    .await;
    let (status, _, p) = call(
        app,
        "POST",
        "/api/v1/products",
        Some(owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
            "sale_price_tyiyn": price, "min_stock": min_stock
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{p}");
    p["id"].as_str().unwrap().to_string()
}

async fn cashier_employee(app: &Router, owner: &str) -> String {
    let (status, _, e) = call(
        app,
        "POST",
        "/api/v1/employees",
        Some(owner),
        Some(json!({ "full_name": "Айбек", "is_cashier": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{e}");
    e["id"].as_str().unwrap().to_string()
}

async fn actions(pool: &PgPool, action: &str) -> Vec<Value> {
    sqlx::query_scalar("select data from audit_log where action = $1 order by created_at")
        .bind(action)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn paid_receipt_takes_money_from_chosen_register(pool: PgPool) {
    // Оплаченная накладная списывает деньги из выбранной кассы; сторно их возвращает (ADR-062).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (till, _bank, safe) = accounts_of(&app, &owner).await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let receipt = |who: &str, total: i64, paid_from: Option<&str>| {
        let app = app.clone();
        let who = who.to_string();
        let mut body = json!({
            "op_id": Uuid::now_v7(), "payment": "paid",
            "lines": [{ "product_id": item, "qty": 1, "cost_tyiyn": total }]
        });
        if let Some(p) = paid_from {
            body["paid_from"] = json!(p);
        }
        async move { call(&app, "POST", "/api/v1/receipts", Some(&who), Some(body)).await }
    };

    // Из кассы — только в открытой смене.
    let (s, _, _) = receipt(&admin, 10_000, None).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Старый клиент без поля: администратор платит из «Кассы».
    let (s, _, paid) = receipt(&admin, 100_000, None).await;
    assert_eq!(s, StatusCode::OK, "{paid}");
    assert_eq!(paid["paid_from"], "Касса");
    assert_eq!(balance_of(&pool, &till).await, 400_000);
    // Сейф, счёт и «из своих» администратору недоступны.
    let (s, _, _) = receipt(&admin, 1_000, Some(&safe)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = receipt(&admin, 1_000, Some("outside")).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // В минус касса не уходит.
    let (s, _, _) = receipt(&admin, 1_000_000, Some(&till)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(balance_of(&pool, &till).await, 400_000);

    // Владелец обязан выбрать источник; «из своих» кассы не трогает.
    let (s, _, _) = receipt(&owner, 1_000, None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, outside) = receipt(&owner, 70_000, Some("outside")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(outside["paid_from"], "outside");
    assert_eq!(balance_of(&pool, &till).await, 400_000);
    assert_eq!(balance_of(&pool, &safe).await, 0);

    // Сторно возвращает деньги в ту же кассу.
    let rid = paid["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/receipts/{rid}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(balance_of(&pool, &till).await, 500_000);

    // Из сейфа владельца: деньги туда переводит владелец, платит из сейфа он же.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/transfers",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "from_account_id": till, "to_account_id": safe, "amount_tyiyn": 200_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, from_safe) = receipt(&owner, 50_000, Some(&safe)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(from_safe["paid_from"], "Сейф владельца");
    assert_eq!(balance_of(&pool, &safe).await, 150_000);
    let sid = from_safe["id"].as_str().unwrap();
    let rev = |who: &str| {
        let app = app.clone();
        let who = who.to_string();
        let uri = format!("/api/v1/receipts/{sid}/reverse");
        async move {
            call(
                &app,
                "POST",
                &uri,
                Some(&who),
                Some(json!({ "op_id": Uuid::now_v7(), "comment": "возврат поставщику" })),
            )
            .await
        }
    };
    let (s, _, _) = rev(&admin).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = rev(&owner).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(balance_of(&pool, &safe).await, 200_000);

    // Владелец видит оплату в уведомлениях; прибыль оплата поставщику не трогает.
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let titles: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["action"] == "receipt.supplier_payment")
        .map(|i| i["details"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles.len(), 3);
    assert!(titles.iter().any(|t| t.contains("мимо касс")), "{titles:?}");
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["expenses_tyiyn"], 0);
    assert_eq!(profit["totals"]["net_tyiyn"], 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_without_purchase_price_waits_for_owner(pool: PgPool) {
    // Товар без цены закупки продаётся, но 2 % кассира с этой строки ждут цены от владельца (ADR-062).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let fresh = product(&app, &owner, "Новый фильтр", 50_000, 0).await;
    let known = product(&app, &owner, "Старый фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": "outside",
            "lines": [{ "product_id": known, "qty": 10, "cost_tyiyn": 200_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, sale) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [
                { "kind": "piece", "product_id": fresh, "qty": 2, "unit_price_tyiyn": 50_000 },
                { "kind": "piece", "product_id": known, "qty": 1, "unit_price_tyiyn": 50_000 }
            ],
            "payments": [{ "method": "cash", "amount_tyiyn": 150_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let percent = || async {
        let v: i64 = sqlx::query_scalar(
            "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'revenue_percent'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        v
    };
    // Только с известной строки: 2 % от (500 − 200) с.
    assert_eq!(percent().await, 600);
    let flags: Vec<bool> = sqlx::query_scalar(
        "select cost_unknown from sale_lines where sale_id = $1::uuid order by line_no",
    )
    .bind(sale["id"].as_str().unwrap())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(flags, vec![true, false]);
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert!(
        profit["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w == "есть продажи без себестоимости: 1"),
        "{profit}"
    );
    let (_, _, stock) = call(&app, "GET", "/api/v1/stock?q=Новый", Some(&owner), None).await;
    assert_eq!(stock[0]["cost_pending"], true);
    assert_eq!(actions(&pool, "sale.cost_unknown").await.len(), 1);

    // Отметку «проверить» без цены не снять; цену указывает только владелец.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/stock/{fresh}/review-done"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let op = Uuid::now_v7();
    let price = json!({ "op_id": op, "price_tyiyn": 30_000 });
    let uri = format!("/api/v1/stock/{fresh}/purchase-price");
    let (s, _, _) = call(&app, "POST", &uri, Some(&admin), Some(price.clone())).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, done) = call(&app, "POST", &uri, Some(&owner), Some(price.clone())).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    // 2 % от (1000 − 2 × 300) с = 8 с; повтор того же op_id второй раз не начисляет.
    assert_eq!(done, json!({ "lines": 1, "accrued_tyiyn": 800 }));
    let (_, _, again) = call(&app, "POST", &uri, Some(&owner), Some(price)).await;
    assert_eq!(again, done);
    assert_eq!(percent().await, 1_400);
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/stock/{fresh}/review-done"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, stock) = call(&app, "GET", "/api/v1/stock?q=Новый", Some(&owner), None).await;
    assert_eq!(stock[0]["cost_pending"], false);
    assert!(stock[0]["last_purchase_price_tyiyn"] == 30_000);

    // Возврат одной штуки после цены снимает ровно её процент: 2 % от (500 − 300) с.
    let sid = sale["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/sales/{sid}/return"),
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(percent().await, 1_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cashier_rules_are_not_recreated_and_rule_close_is_logged(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let id = cashier_employee(&app, &owner).await;
    let (_, _, rules) = call(
        &app,
        "GET",
        &format!("/api/v1/employees/{id}/pay-rules"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(rules.as_array().unwrap().len(), 2);
    for r in rules.as_array().unwrap() {
        let (s, _, _) = call(
            &app,
            "DELETE",
            &format!(
                "/api/v1/employees/{id}/pay-rules/{}",
                r["id"].as_str().unwrap()
            ),
            Some(&owner),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    assert_eq!(actions(&pool, "payroll.rule_close").await.len(), 2);
    for flag in [false, true] {
        let (s, _, _) = call(
            &app,
            "PATCH",
            &format!("/api/v1/employees/{id}"),
            Some(&owner),
            Some(json!({ "is_cashier": flag })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    // Снова кассир — но закрытые владельцем ставки не возвращаются.
    let open: i64 = sqlx::query_scalar(
        "select count(*) from employee_pay_rules where employee_id = $1::uuid and active_to is null",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(open, 0);
    let updates = actions(&pool, "employee.update").await;
    assert_eq!(updates.last().unwrap()["pay_rules_added"], false);
    // Ставка мастера — настройка продаж, правилом сотрудника не задаётся.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/employees/{id}/pay-rules"),
        Some(&owner),
        Some(json!({ "kind": "per_service", "role": "master", "amount_tyiyn": 3000 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn vehicles_are_not_duplicated_and_removed_ones_come_back(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let party = |name: &str| {
        let app = app.clone();
        let admin = admin.clone();
        let body = json!({ "name": name });
        async move {
            let (_, _, p) = call(&app, "POST", "/api/v1/parties", Some(&admin), Some(body)).await;
            p["id"].as_str().unwrap().to_string()
        }
    };
    let me = party("Эрлан").await;
    let other = party("Бакыт").await;
    let add = |pid: &str, plate: &str| {
        let app = app.clone();
        let admin = admin.clone();
        let uri = format!("/api/v1/parties/{pid}/vehicles");
        let body = json!({ "plate": plate });
        async move { call(&app, "POST", &uri, Some(&admin), Some(body)).await }
    };
    let (s, _, car) = add(&me, "01 kg 777 abc").await;
    assert_eq!(s, StatusCode::OK);
    let car_id = car["id"].as_str().unwrap().to_string();
    // Тот же номер в другом написании — та же машина.
    let (_, _, same) = add(&me, "01KG-777 АВС").await;
    assert_eq!(same["id"], car["id"]);
    // Клиент убрал машину — повторное добавление возвращает её с историей.
    sqlx::query("update party_vehicles set active = false where id = $1::uuid")
        .bind(&car_id)
        .execute(&pool)
        .await
        .unwrap();
    let book = |removed: bool| {
        let app = app.clone();
        let admin = admin.clone();
        let uri = format!("/api/v1/parties/{me}/oil-book?removed={removed}");
        async move { call(&app, "GET", &uri, Some(&admin), None).await.2 }
    };
    assert_eq!(book(false).await.as_array().unwrap().len(), 0);
    let all = book(true).await;
    assert_eq!(all[0]["active"], false);
    let (_, _, back) = add(&me, "01KG777ABC").await;
    assert_eq!(
        (back["id"].clone(), back["active"].clone()),
        (car["id"].clone(), json!(true))
    );
    assert_eq!(actions(&pool, "vehicle.restore").await.len(), 1);
    // Номер другого клиента: новая машина и уведомление владельцу.
    let (_, _, foreign) = add(&other, "01KG777ABC").await;
    assert_ne!(foreign["id"], car["id"]);
    let conflicts = actions(&pool, "vehicle.plate_conflict").await;
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0]["source"], "staff");

    // Продажа на убранную машину: онлайн — 409, кнопка «Вернуть» — машина снова в списке.
    sqlx::query("update party_vehicles set active = false where id = $1::uuid")
        .bind(&car_id)
        .execute(&pool)
        .await
        .unwrap();
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "party_id": me, "vehicle_id": car_id,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, restored) = call(
        &app,
        "POST",
        &format!("/api/v1/parties/{me}/vehicles/{car_id}/restore"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(restored["active"], true);
    assert_eq!(book(false).await.as_array().unwrap().len(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn edits_keep_old_and_new_values(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, p) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "ОсОО Ромашка", "kind": "company", "phone": "0700" })),
    )
    .await;
    let pid = p["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/parties/{pid}"),
        Some(&owner),
        Some(json!({ "credit_limit_tyiyn": 1_000_000, "phone": "0555" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let upd = actions(&pool, "party.update").await;
    assert_eq!(
        upd[0]["changes"]["phone"],
        json!({ "old": "0700", "new": "0555" })
    );
    assert_eq!(
        upd[0]["changes"]["credit_limit_tyiyn"],
        json!({ "old": null, "new": 1_000_000 })
    );
    // Правка лимита — отдельным уведомлением, правка телефона — нет.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/parties/{pid}"),
        Some(&owner),
        Some(json!({ "phone": "0777" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let limits: Vec<&Value> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["action"] == "party.limit_change")
        .collect();
    assert_eq!(limits.len(), 1);
    assert!(
        limits[0]["details"]
            .as_str()
            .unwrap()
            .contains("было без лимита, стало 10 000"),
        "{}",
        limits[0]["details"]
    );

    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{item}"),
        Some(&owner),
        Some(json!({ "name": "Фильтр W712", "brand": "Mann" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let upd = actions(&pool, "product.update").await;
    assert_eq!(
        upd[0]["changes"],
        json!({
            "name": { "old": "Фильтр", "new": "Фильтр W712" },
            "brand": { "old": "", "new": "Mann" }
        })
    );

    let (_, _, art) = call(
        &app,
        "POST",
        "/api/v1/expense-articles",
        Some(&owner),
        Some(json!({ "name": "Хозтовары" })),
    )
    .await;
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/expense-articles/{}", art["id"].as_str().unwrap()),
        Some(&owner),
        Some(json!({ "name": "Хозяйственные товары" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let upd = actions(&pool, "expense.article_update").await;
    assert_eq!(
        upd[0]["changes"]["name"],
        json!({ "old": "Хозтовары", "new": "Хозяйственные товары" })
    );

    let (_, _, sup) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&owner),
        Some(json!({ "name": "Поставщик", "phone": "1" })),
    )
    .await;
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/suppliers/{}", sup["id"].as_str().unwrap()),
        Some(&owner),
        Some(json!({ "phone": "2" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let upd = actions(&pool, "supplier.update").await;
    assert_eq!(
        upd[0]["changes"],
        json!({ "phone": { "old": "1", "new": "2" } })
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn dashboard_low_stock_skips_archived(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 5).await;
    let low = || async {
        call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None)
            .await
            .2["low_stock"]
            .clone()
    };
    assert_eq!(low().await, 1);
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{item}"),
        Some(&owner),
        Some(json!({ "archived": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(low().await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn posted_sale_is_found_by_op_id(pool: PgPool) {
    // Касса узнаёт, что чек уже проведён, даже если его отправил другой пользователь (ADR-062).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let op = Uuid::now_v7();
    let body = json!({
        "op_id": op, "sale_type": "takeaway", "cashier_id": cashier,
        "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
        "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }]
    });
    let (s, _, sale) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Тот же op_id от другого пользователя — конфликт, но чек находится по op_id.
    let (s, _, _) = call(&app, "POST", "/api/v1/sales", Some(&owner), Some(body)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, found) = call(
        &app,
        "GET",
        &format!("/api/v1/sales/by-op/{op}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(found["number"], sale["number"]);
    let (s, _, _) = call(
        &app,
        "GET",
        &format!("/api/v1/sales/by-op/{}", Uuid::now_v7()),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_add_is_idempotent_and_delete_is_logged(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, rev) = call(
        &app,
        "POST",
        "/api/v1/revisions",
        Some(&owner),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{rev}");
    let id = rev["head"]["id"].as_str().unwrap();
    let uri = format!("/api/v1/revisions/{id}/lines");
    let op = Uuid::now_v7();
    let body = json!({ "product_id": item, "counted_qty": 5, "mode": "add", "op_id": op });
    for _ in 0..2 {
        let (s, _, r) = call(&app, "PUT", &uri, Some(&owner), Some(body.clone())).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(r["counted_qty"], 5);
    }
    let (_, _, r) = call(
        &app,
        "PUT",
        &uri,
        Some(&owner),
        Some(
            json!({ "product_id": item, "counted_qty": 2, "mode": "add", "op_id": Uuid::now_v7() }),
        ),
    )
    .await;
    assert_eq!(r["counted_qty"], 7);
    let (s, _, _) = call(
        &app,
        "DELETE",
        &format!("/api/v1/revisions/{id}/lines/{item}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let del = actions(&pool, "revision.line_delete").await;
    assert_eq!(del.len(), 1);
    assert_eq!(del[0]["counted_qty"], 7);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn disabled_owner_loses_telegram_chat(pool: PgPool) {
    // Владельца отключили или сделали администратором — его чат отвязывается, в журнале запись
    // (ADR-063).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let mut ids = Vec::new();
    for login_name in ["owner2", "owner3"] {
        let (status, _, user) = call(
            &app,
            "POST",
            "/api/v1/users",
            Some(&owner),
            Some(json!({ "login": login_name, "password": "owner-pass-2", "full_name": login_name, "role": "owner" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{user}");
        let id: Uuid = user["id"].as_str().unwrap().parse().unwrap();
        sqlx::query("insert into telegram_links (chat_id, user_id, branch_id) select $1, $2, branch_id from users where id = $2")
            .bind(i64::try_from(ids.len()).unwrap() + 501)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        ids.push(id);
    }
    let links = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from telegram_links")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(links().await, 2);
    for (id, body) in [
        (ids[0], json!({ "active": false })),
        (ids[1], json!({ "role": "admin" })),
    ] {
        let (status, _, _) = call(
            &app,
            "PATCH",
            &format!("/api/v1/users/{id}"),
            Some(&owner),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert_eq!(links().await, 0);
    let unlinks: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'telegram.unlink'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unlinks, 2);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn loyalty_lookup_is_per_branch_and_unambiguous(pool: PgPool) {
    // Поиск клиента для баллов — только в своём филиале и ровно одна карточка (ADR-063).
    let app = setup(pool.clone()).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let other = Uuid::now_v7();
    sqlx::query("insert into branches (id, name) values ($1, 'Другая точка')")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    let client = |b: Uuid, name: &'static str, chat: i64| {
        let pool = pool.clone();
        async move {
            let id = Uuid::now_v7();
            sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', $3, '0555 12 34 56')")
                .bind(id)
                .bind(b)
                .bind(name)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, '0555123456')")
                .bind(chat)
                .bind(id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let uri = "/api/v1/loyalty/lookup?phone=%2B996555123456";
    // Клиент другого филиала не находится.
    client(other, "Чужой", 1).await;
    assert_eq!(
        call(&app, "GET", uri, Some(&admin), None).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    client(branch, "Эрлан", 2).await;
    let (status, _, found) = call(&app, "GET", uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["name"], "Эрлан");
    // Две карточки с номером — не угадываем.
    client(branch, "Эрлан дубль", 3).await;
    let (status, _, err) = call(&app, "GET", uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(err.to_string().contains("несколько карточек"), "{err}");
}
