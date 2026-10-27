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
    let admin = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Админ')")
        .bind(admin)
        .bind(branch)
        .bind(hash)
        .execute(&pool)
        .await
        .unwrap();
    grant_default_permissions(&pool, admin, branch).await;
    app(build_state(pool, false).await.unwrap(), None)
}

/// Права администратора по умолчанию (SPEC-30 §5): тестовый администратор заводится мимо
/// `POST /users`, поэтому права ему выдаются здесь — как новому администратору.
async fn grant_default_permissions(pool: &PgPool, user: Uuid, branch: Uuid) {
    // База до миграции 0056 (проверки миграций): права выдаст сама миграция, как на точке.
    let ready: bool = sqlx::query_scalar(
        "select exists (select 1 from pg_constraint where conname = 'user_permissions_permission_check'
                          and pg_get_constraintdef(oid) like '%sale_return%')",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    if !ready {
        return;
    }
    let keys: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
    .bind(user)
    .bind(branch)
    .bind(keys)
    .execute(pool)
    .await
    .unwrap();
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

/// Вид замены для чеков «В сервис» (ADR-096): бесплатно с нашим маслом, мастеру 30 с.
async fn oil_change_kind(app: &Router, owner: &str) -> String {
    let (s, _, sv) = call(
        app,
        "POST",
        "/api/v1/services",
        Some(owner),
        Some(json!({ "name": "Замена масла в моторе", "price_tyiyn": 0, "master_fee_tyiyn": 3000, "free_with_oil": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sv}");
    sv["id"].as_str().unwrap().to_string()
}

/// Клиент для чека «В сервис» (ADR-100): человек с именем, фамилией и телефоном и его машина.
async fn service_client(app: &Router, admin: &str, plate: &str) -> (String, String) {
    let (s, _, p) = call(
        app,
        "POST",
        "/api/v1/parties",
        Some(admin),
        Some(json!({ "kind": "person", "first_name": "Бакыт", "last_name": "Асанов", "phone": "0700 11-22-33" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let pid = p["id"].as_str().unwrap().to_string();
    let (s, _, car) = call(
        app,
        "POST",
        &format!("/api/v1/parties/{pid}/vehicles"),
        Some(admin),
        Some(json!({ "plate": plate })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{car}");
    (pid, car["id"].as_str().unwrap().to_string())
}

/// Право администратора сверх роли (ADR-082, ADR-097) — прямо в базе: ограничение списка прав
/// снимается, тест не зависит от того, какие права в нём уже перечислены.
async fn grant_admin(pool: &PgPool, permission: &str) {
    sqlx::query(
        "alter table user_permissions drop constraint if exists user_permissions_permission_check",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission, granted_by)
         select a.id, a.branch_id, $1, o.id from users a join users o on o.branch_id = a.branch_id and o.role = 'owner'
         where a.login = 'admin' on conflict do nothing",
    )
    .bind(permission)
    .execute(pool)
    .await
    .unwrap();
}

/// «Сейф владельца»: из него тесты платят за приходы для склада — «из своих» больше нет (ADR-079).
async fn owner_safe(app: &Router, owner: &str) -> String {
    let (s, _, list) = call(app, "GET", "/api/v1/cash/accounts", Some(owner), None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    list.as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф владельца")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
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

    // Отбор по виду категории: перелив берёт все масла, сколько бы ни было товаров.
    let has = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"].as_str() == Some(pid.as_str()))
    };
    let (_, _, oils) = call(&app, "GET", "/api/v1/products?kind=oil", Some(&admin), None).await;
    assert!(has(&oils));
    assert!(oils.as_array().unwrap().iter().all(|x| x["unit"] == "ml"));
    let (_, _, other) = call(
        &app,
        "GET",
        "/api/v1/products?kind=battery",
        Some(&admin),
        None,
    )
    .await;
    assert!(!has(&other));

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
    // «Из своих, мимо касс» больше нет ни у кого (ADR-079); со счёта платит только владелец.
    let outside = json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 20_000, "method": "outside" });
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(outside.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let transfer = json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 20_000, "method": "transfer" });
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(transfer.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(outside),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // Частичная оплата уменьшает наш долг.
    let (status, _, r) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(transfer),
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
async fn oil_has_one_price(pool: PgPool) {
    // У масла в канистре одна цена — канистры; розлив за литр считается от неё (ADR-074).
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Масла", "kind": "oil" })),
    )
    .await;
    let new_oil = |pour: Option<i64>| {
        json!({
            "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Масло 4 л",
            "container_ml": 4000, "sale_price_tyiyn": 200_000, "pour_price_per_l_tyiyn": pour
        })
    };
    let (status, _, err) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(new_oil(Some(60_000))),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("цена канистры")
    );
    let (status, _, p) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(new_oil(None)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["pour_price_per_l_tyiyn"], 50_000);
    let pid = p["id"].as_str().unwrap().to_string();
    let prices = format!("/api/v1/products/{pid}/prices");

    // Отдельную цену розлива владельцу тоже не задать: 422 с понятным текстом.
    let (status, _, err) = call(
        &app,
        "PATCH",
        &prices,
        Some(&owner),
        Some(json!({ "pour_price_per_l_tyiyn": 60_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("розлив за литр считается")
    );
    // Новая цена канистры — новый розлив: 1 999,99 с / 4 л = 499,9975 с → 500,00 с.
    let (status, _, p) = call(
        &app,
        "PATCH",
        &prices,
        Some(&owner),
        Some(json!({ "sale_price_tyiyn": 199_999 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["pour_price_per_l_tyiyn"], 50_000);
    let (status, _, p) = call(
        &app,
        "PATCH",
        &prices,
        Some(&owner),
        Some(json!({ "sale_price_tyiyn": 240_000 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(p["pour_price_per_l_tyiyn"], 60_000);
    // Касса без сети видит ту же цену розлива.
    let (_, _, snap) = call(&app, "GET", "/api/v1/offline/snapshot", Some(&owner), None).await;
    let oil = snap["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(pid))
        .unwrap();
    assert_eq!(oil["pour_price_per_l_tyiyn"], 60_000);

    // Лента владельца: с ценой канистры поменялся и розлив за литр.
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let details = list["items"][0]["details"].as_str().unwrap();
    assert!(
        details.contains("розлив за литр: было 500 с, стало 600 с"),
        "{details}"
    );
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
        Some(json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await, "lines": [{ "product_id": dup, "qty": 4, "cost_tyiyn": 20_000 }] })),
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
        Some(
            json!({ "full_name": "Кассир", "is_cashier": true, "pin": "1234", "is_master": false }),
        ),
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
            Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": eid, "cashier_pin": "1234" })),
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
    sqlx::query("insert into employees (id, branch_id, full_name, is_cashier, pin_hash, pin_set_at) values ($1, $2, 'Айбек', true, $3, now())")
        .bind(cashier)
        .bind(branch)
        .bind(hash_password("1234".into()).await.unwrap())
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
        json!({ "op_id": op(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
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

    // После сторно сдачи смена снова ждёт сдачи, и кассу сдают заново (ADR-088).
    let (s, _, shift) = call(
        &app,
        "GET",
        &format!("/api/v1/shifts/{sid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(shift["handover_pending"], json!(true), "{shift}");
    let (s, _, again) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/handover"),
        json!({ "op_id": op(), "amount_tyiyn": 300_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["to_safe_tyiyn"], 300_000);
    assert_eq!(again["handover_pending"], json!(false));
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
        json!({ "full_name": "Кассир", "is_cashier": true, "pin": "1234" }),
    )
    .await;
    let (s, _, _) = post(
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": emp["id"], "cashier_pin": "1234" }),
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
        json!({ "full_name": "Кассир", "is_cashier": true, "pin": "1234" }),
    )
    .await;
    let eid = emp["id"].as_str().unwrap().to_string();
    let (_, _, shift) = post(
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": eid, "cashier_pin": "1234" }),
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

    // Выплата из сейфа и её сторно: долг перед сотрудником вернулся.
    let (s, _, paid) = post(
        &owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": eid, "amount_tyiyn": 5_000, "source": "account",
                "account_id": owner_safe(&app, &owner).await, "advance": true }),
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
    assert!(dash["compare"]["yesterday_net_tyiyn"].is_i64());
    assert!(dash["totals"]["salary_tyiyn"].is_i64());
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
        Some(json!({ "full_name": "Сам себе кассир", "is_cashier": true, "pin": "1234" })),
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
        Some(json!({ "full_name": "Кассир", "is_cashier": true, "pin": "1234" })),
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
        Some(json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" })),
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
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
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
    // Сейф и счёт администратору недоступны; «из своих» больше нет ни у кого (ADR-079).
    let (s, _, _) = receipt(&admin, 1_000, Some(&safe)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = receipt(&admin, 1_000, Some("outside")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Владелец запретил кассе минус (по умолчанию разрешён, ADR-077) — сверх остатка нельзя.
    let negative = |on: bool| {
        let app = app.clone();
        let owner = owner.clone();
        let uri = format!("/api/v1/cash/accounts/{till}");
        async move {
            call(
                &app,
                "PATCH",
                &uri,
                Some(&owner),
                Some(json!({ "allow_negative": on })),
            )
            .await
            .0
        }
    };
    assert_eq!(negative(false).await, StatusCode::OK);
    let (s, _, _) = receipt(&admin, 1_000_000, Some(&till)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(balance_of(&pool, &till).await, 400_000);
    assert_eq!(negative(true).await, StatusCode::OK);

    // Владелец обязан выбрать источник; «из своих» — 422 и кассы не трогает (ADR-079).
    let (s, _, _) = receipt(&owner, 1_000, None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, outside) = receipt(&owner, 70_000, Some("outside")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        outside["error"]["message"]
            .as_str()
            .unwrap()
            .contains("больше нет"),
        "{outside}"
    );
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
    assert_eq!(titles.len(), 2);
    assert!(
        titles.iter().all(|t| !t.contains("мимо касс")),
        "{titles:?}"
    );
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
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
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
    // Наличные возвращаются только в открытую смену (ADR-065).
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
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
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "per_service", "role": "master", "amount_tyiyn": 3000 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Повтор формы с тем же op_id не заводит второе правило (ADR-070).
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "kind": "monthly_salary", "role": "cashier", "amount_tyiyn": 3_100_000 });
    let (s1, _, first) = call(
        &app,
        "POST",
        &format!("/api/v1/employees/{id}/pay-rules"),
        Some(&owner),
        Some(body.clone()),
    )
    .await;
    let (s2, _, second) = call(
        &app,
        "POST",
        &format!("/api/v1/employees/{id}/pay-rules"),
        Some(&owner),
        Some(body),
    )
    .await;
    assert_eq!((s1, s2), (StatusCode::OK, StatusCode::OK));
    assert_eq!(first["id"], second["id"]);
    let same: i64 = sqlx::query_scalar(
        "select count(*) from employee_pay_rules where employee_id = $1::uuid and kind = 'monthly_salary' and amount_tyiyn = 3100000",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(same, 1);
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
        Some(json!({ "name": "Мелочи" })),
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
        json!({ "old": "Мелочи", "new": "Хозяйственные товары" })
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
async fn loyalty_lookup_is_shared_and_unambiguous(pool: PgPool) {
    // Поиск клиента для баллов — по всем филиалам (клиенты общие, SPEC-33) и ровно одна карточка
    // (ADR-063).
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
    // Клиент, заведённый в другом филиале, находится: клиенты общие.
    client(other, "Эрлан", 1).await;
    let (status, _, found) = call(&app, "GET", uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found["name"], "Эрлан");
    // Две карточки с номером — не угадываем.
    client(branch, "Эрлан дубль", 3).await;
    let (status, _, err) = call(&app, "GET", uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(err.to_string().contains("несколько карточек"), "{err}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cash_shortage_and_surplus_enter_profit(pool: PgPool) {
    // Недостача и излишек кассы — строка прибыли; удержание с кассира её уравновешивает (ADR-065).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let post = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let totals = || {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            let (s, _, p) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
            assert_eq!(s, StatusCode::OK, "{p}");
            p
        }
    };
    let net = |p: &Value| p["totals"]["net_tyiyn"].as_i64().unwrap();
    let (s, _, shift) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(
        &admin,
        "/api/v1/cash/movements".into(),
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let start = totals().await;
    assert_eq!(start["totals"]["cash_diff_tyiyn"], 0);

    // Недостача 1 000 с удержана с кассира: прибыль не меняется.
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 400_000, "comment": "не нашли" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let withheld = totals().await;
    assert_eq!(withheld["totals"]["cash_diff_tyiyn"], -100_000);
    assert_eq!(net(&withheld), net(&start));
    assert_eq!(withheld["days"][0]["cash_diff_tyiyn"], -100_000);
    assert_eq!(withheld["days"][0]["net_tyiyn"], net(&withheld));

    // Владелец прощает недостачу — сторно удержания: прибыль меньше на 1 000 с.
    let today: String =
        sqlx::query_scalar("select ((now() at time zone 'Asia/Bishkek')::date)::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let uri = format!("/api/v1/payroll/entries?employee_id={cashier}&from={today}&to={today}");
    assert_eq!(
        call(&app, "GET", &uri, Some(&admin), None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _, entries) = call(&app, "GET", &uri, Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK, "{entries}");
    let shortage = entries["accruals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "shortage")
        .unwrap()
        .clone();
    assert_eq!(shortage["reversed"], false);
    let aid = shortage["id"].as_str().unwrap();
    let (s, _, _) = post(
        &owner,
        format!("/api/v1/payroll/accruals/{aid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "прощаем" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let forgiven = totals().await;
    assert_eq!(net(&forgiven), net(&start) - 100_000);
    let (_, _, entries) = call(&app, "GET", &uri, Some(&owner), None).await;
    let list = entries["accruals"].as_array().unwrap();
    assert!(
        list.iter()
            .any(|a| a["id"] == json!(aid) && a["reversed"] == json!(true))
    );
    assert!(list.iter().any(|a| a["reversal_of"] == json!(aid)));

    // Излишек 300 с при повторном закрытии — доход: прибыль больше на 300 с.
    let (s, _, _) = post(
        &owner,
        format!("/api/v1/shifts/{sid}/reopen"),
        json!({ "op_id": Uuid::now_v7(), "reason": "продолжаем" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 430_000, "comment": "лишние" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let surplus = totals().await;
    assert_eq!(surplus["totals"]["cash_diff_tyiyn"], -70_000);
    assert_eq!(net(&surplus), net(&forgiven) + 30_000);
    let (_, _, dash) = call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None).await;
    assert_eq!(dash["totals"]["net_tyiyn"], net(&surplus));
    // Удержание уже прощено — напоминать владельцу не о чем.
    assert!(actions(&pool, "shift.reclose_surplus").await.is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn reclose_surplus_reminds_owner_about_withheld_shortage(pool: PgPool) {
    // Удержание при повторном закрытии само не снимается (ADR-064), но владелец узнаёт, что
    // его можно простить (ADR-070).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let post = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let (s, _, shift) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(
        &admin,
        "/api/v1/cash/movements".into(),
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let close = |counted: i64| {
        post(
            &admin,
            format!("/api/v1/shifts/{sid}/close"),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": counted, "comment": "пересчёт" }),
        )
    };
    assert_eq!(close(400_000).await.0, StatusCode::OK);
    assert!(actions(&pool, "shift.reclose_surplus").await.is_empty());
    let (s, _, _) = post(
        &owner,
        format!("/api/v1/shifts/{sid}/reopen"),
        json!({ "op_id": Uuid::now_v7(), "reason": "нашли деньги" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(close(430_000).await.0, StatusCode::OK);
    let notes = actions(&pool, "shift.reclose_surplus").await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["number"], shift["number"]);
    assert_eq!(notes[0]["surplus"], 30_000);
    assert_eq!(notes[0]["withheld"], 100_000);
    // Само удержание не тронуто.
    let shortage: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'shortage'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(shortage, -100_000);
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let title = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["action"] == "shift.reclose_surplus")
        .unwrap()["title"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        title,
        format!(
            "После переоткрытия смены № {} излишек 300 с — удержание 1 000 с можно простить в «Расчёте»",
            shift["number"]
        )
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cash_refund_needs_open_shift(pool: PgPool) {
    // Возврат наличными из «Кассы» — только при открытой смене; на карту — без смены (ADR-065).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": item, "qty": 5, "cost_tyiyn": 50_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Продажа без смены разрешена, как прежде.
    let (s, _, sale) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 3, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }, { "method": "card", "amount_tyiyn": 100_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let uri = format!("/api/v1/sales/{}/return", sale["id"].as_str().unwrap());
    let refund = |method: &str| {
        json!({
            "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
            "payments": [{ "method": method, "amount_tyiyn": 50_000 }]
        })
    };
    let (s, _, err) = call(&app, "POST", &uri, Some(&admin), Some(refund("cash"))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("откройте смену"),
        "{err}"
    );
    let (s, _, _) = call(&app, "POST", &uri, Some(&owner), Some(refund("cash"))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    // На карту — без смены.
    let (s, _, back) = call(&app, "POST", &uri, Some(&admin), Some(refund("card"))).await;
    assert_eq!(s, StatusCode::OK, "{back}");
    // Смену открыли — наличными можно.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, back) = call(&app, "POST", &uri, Some(&admin), Some(refund("cash"))).await;
    assert_eq!(s, StatusCode::OK, "{back}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bank_fee_rate_has_history_by_business_date(pool: PgPool) {
    // Ставка комиссии действует с даты; чек получает ставку своего учётного дня (SPEC-08, ADR-065).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": item, "qty": 5, "cost_tyiyn": 50_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Ставка — у безналичного счёта (ADR-077): у «Счёта» прежние 0,5 %.
    let (till, bank, _) = accounts_of(&app, &owner).await;
    let uri = format!("/api/v1/cash/accounts/{bank}/fees");
    assert_eq!(
        call(&app, "GET", &uri, Some(&admin), None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _, fees) = call(&app, "GET", &uri, Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK, "{fees}");
    assert_eq!(fees["current_bp"], 50);
    assert_eq!(fees["history"].as_array().unwrap().len(), 1);
    let today: String =
        sqlx::query_scalar("select ((now() at time zone 'Asia/Bishkek')::date)::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let yesterday: String =
        sqlx::query_scalar("select ((now() at time zone 'Asia/Bishkek')::date - 1)::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let set = |who: &str, acc: &str, rate: i64, from: &str| {
        let app = app.clone();
        let who = who.to_string();
        let account = format!("/api/v1/cash/accounts/{acc}");
        let body = json!({ "fee_rate_bp": rate, "fee_from": from });
        async move { call(&app, "PATCH", &account, Some(&who), Some(body)).await }
    };
    assert_eq!(
        set(&admin, &bank, 100, &today).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        set(&owner, &bank, 10_001, &today).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Комиссия — только у безналичного счёта.
    assert_eq!(
        set(&owner, &till, 100, &today).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Задним числом ставку не поставить: комиссия проведённых чеков не меняется (ADR-070).
    let (s, _, err) = set(&owner, &bank, 100, &yesterday).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("задним числом"),
        "{err}"
    );
    // 1 % с сегодняшнего дня: прежняя ставка закрыта этим днём.
    let (s, _, acc) = set(&owner, &bank, 100, &today).await;
    assert_eq!(s, StatusCode::OK, "{acc}");
    assert_eq!(acc["fee_rate_bp"], 100);
    // Повтор той же правки историю не множит.
    assert_eq!(set(&owner, &bank, 100, &today).await.0, StatusCode::OK);
    let (_, _, fees) = call(&app, "GET", &uri, Some(&owner), None).await;
    let history = fees["history"].as_array().unwrap();
    assert_eq!(history.len(), 2, "{fees}");
    assert_eq!(history[0]["rate_bp"], 100);
    assert!(history[0]["user_name"].is_string(), "{fees}");
    assert_eq!(history[1]["active_to"], json!(today));
    assert_eq!(fees["current_bp"], 100);
    // Раньше действующей ставки новую не поставить: история только дописывается.
    assert_eq!(
        set(&owner, &bank, 70, &yesterday).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let notes = actions(&pool, "settings.payment_fee").await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["account"], "Счёт");

    let sale = |offline: bool, client_time: Option<String>| {
        let app = app.clone();
        let admin = admin.clone();
        let body = json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "offline": offline, "client_time": client_time,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "card", "amount_tyiyn": 50_000 }]
        });
        async move {
            let (s, _, out) = call(&app, "POST", "/api/v1/sales", Some(&admin), Some(body)).await;
            assert_eq!(s, StatusCode::OK, "{out}");
            out["id"].as_str().unwrap().to_string()
        }
    };
    let fee_of = |id: String| {
        let pool = pool.clone();
        async move {
            let v: i64 = sqlx::query_scalar(
                "select fee_tyiyn from sale_payments where sale_id = $1::uuid and method = 'card'",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
            v
        }
    };
    // Сегодняшний чек — 1 %: 5 с.
    assert_eq!(fee_of(sale(false, None).await).await, 500);
    // Чек без сети вчерашнего учётного дня — вчерашняя ставка 0,5 %: 2,50 с.
    let day_ago = (chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339();
    let old = sale(true, Some(day_ago)).await;
    let date: String =
        sqlx::query_scalar("select business_date::text from sales where id = $1::uuid")
            .bind(&old)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(date, yesterday);
    assert_eq!(fee_of(old).await, 250);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expense_articles_are_managed_by_owner(pool: PgPool) {
    // Статьи расходов: добавить, переименовать, «только владелец», скрыть — только владелец (SPEC-06).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let uri = "/api/v1/expense-articles";
    let body = json!({ "name": "Реклама", "owner_only": false });
    // Первая статья владельца не отменяет начальные статьи филиала.
    assert_eq!(
        call(&app, "POST", uri, Some(&admin), Some(body.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (s, _, a) = call(&app, "POST", uri, Some(&owner), Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(
        call(&app, "POST", uri, Some(&owner), Some(body)).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(actions(&pool, "expense.article_create").await.len(), 1);
    let id = a["id"].as_str().unwrap();
    let (s, _, a) = call(
        &app,
        "PATCH",
        &format!("{uri}/{id}"),
        Some(&owner),
        Some(json!({ "name": "Реклама и вывески", "owner_only": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(a["owner_only"], true);
    // Статья владельца администратору не видна.
    let (_, _, list) = call(&app, "GET", uri, Some(&admin), None).await;
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == json!(id))
    );
    let (s, _, a) = call(
        &app,
        "PATCH",
        &format!("{uri}/{id}"),
        Some(&owner),
        Some(json!({ "active": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(a["active"], false);
    // Переименовать в занятое имя — 409.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("{uri}/{id}"),
        Some(&owner),
        Some(json!({ "name": "Налоги" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

// ---------- Кабинет работника в боте (SPEC-23) ----------

/// Ответ бота на сообщение или контакт из чата `chat`.
async fn bot(
    pool: &PgPool,
    chat: i64,
    text: Option<&str>,
    phone: Option<&str>,
) -> avtodom_server::api::telegram::Reply {
    let mut conn = pool.acquire().await.unwrap();
    let contact = phone.map(|p| avtodom_server::api::telegram::SharedContact {
        phone: p.into(),
        user_id: Some(chat),
    });
    avtodom_server::api::telegram::handle_message(&mut conn, chat, chat, text, contact)
        .await
        .unwrap()
}

/// Слова о выплатах: в кабинете работника их быть не должно (SPEC-23).
fn no_payout_words(text: &str) {
    let low = text.to_lowercase();
    for w in ["выплат", "выдан", "выдач", "аванс", "к выплате", "остаток"]
    {
        assert!(!low.contains(w), "«{w}» в ответе работнику: {text}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_employee_cabinet_matches_payroll_screen(pool: PgPool) {
    // Телефон задаёт только владелец; «Сегодня» и «Месяц» в боте — до тыйына как экран «Расчёт»,
    // со сторно; выплат в боте нет; отключение и смена телефона отвязывают чат сразу (SPEC-23).
    use avtodom_server::api::telegram::Keys;
    use avtodom_server::domain::money::format_som;
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let post = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let patch = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "PATCH", &uri, Some(&cookie), Some(body)).await }
    };
    let cashier = cashier_employee(&app, &owner).await;
    let (s, _, m) = post(
        &admin,
        "/api/v1/employees".into(),
        json!({ "full_name": "Бакыт", "is_master": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let master = m["id"].as_str().unwrap().to_string();

    // Телефон: администратор не задаёт (403), владелец — да; правка в журнале с прежним значением.
    let (s, _, _) = patch(
        &admin,
        format!("/api/v1/employees/{cashier}"),
        json!({ "phone": "+996 555 12 34 56" }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = patch(
        &owner,
        format!("/api/v1/employees/{cashier}"),
        json!({ "phone": "12-34" }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, e) = patch(
        &owner,
        format!("/api/v1/employees/{cashier}"),
        json!({ "phone": "+996 555 12 34 56" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert_eq!(e["phone"], "+996 555 12 34 56");
    let (s, _, _) = patch(
        &owner,
        format!("/api/v1/employees/{master}"),
        json!({ "phone": "0700 11 22 33" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Администратор правит имя, телефон шлёт тем же — это можно; сам телефон ему не виден.
    let (s, _, e) = patch(
        &admin,
        format!("/api/v1/employees/{master}"),
        json!({ "full_name": "Бакыт", "phone": "0700 11 22 33" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert!(e.get("phone").is_none());
    let (_, _, list) = call(&app, "GET", "/api/v1/employees", Some(&admin), None).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .all(|e| e.get("phone").is_none())
    );
    let (_, _, list) = call(&app, "GET", "/api/v1/employees", Some(&owner), None).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|e| e["phone"] == "0700 11 22 33")
    );
    let edits = actions(&pool, "employee.update").await;
    let first = edits
        .iter()
        .find(|d| d["phone"] == "+996 555 12 34 56")
        .unwrap();
    assert_eq!(first["before"]["phone"], "");
    assert_eq!(first["phone_changed"], true);

    // Работники подключаются своим номером.
    let (cchat, mchat) = (950_101i64, 950_102i64);
    let r = bot(&pool, cchat, None, Some("+996555123456")).await;
    assert_eq!(r.keys, Keys::Employee, "{}", r.text);
    assert!(r.text.contains("Айбек, вы подключены"), "{}", r.text);
    let r = bot(&pool, mchat, None, Some("+996700112233")).await;
    assert_eq!(r.keys, Keys::Employee, "{}", r.text);
    let linked = actions(&pool, "telegram.employee_link").await;
    assert_eq!(linked.len(), 2);

    // День: продажа кассира (2 % с валовой), две замены мастера, премия со сторно, удержание,
    // выплата и недостача при закрытии смены.
    let item = product(&app, &owner, "Масло", 50_000, 0).await;
    let (s, _, _) = post(
        &owner,
        "/api/v1/receipts".into(),
        json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 30_000 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, shift) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{shift}");
    let sid = shift["id"].as_str().unwrap().to_string();
    let mut totals = Vec::new();
    // Замены — с видом замены (мастеру 30 с) и данными клиента (ADR-096, ADR-100).
    let kind_id = oil_change_kind(&app, &owner).await;
    let (pid, vid) = service_client(&app, &admin, "01KG404CAB").await;
    for (kind, qty) in [("takeaway", 2), ("service", 1), ("service", 1)] {
        let mut body = json!({
            "op_id": Uuid::now_v7(), "sale_type": kind, "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": qty, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 * qty }]
        });
        if kind == "service" {
            body["master_id"] = json!(master);
            body["party_id"] = json!(pid);
            body["vehicle_id"] = json!(vid);
            body["mileage"] = json!(100_000);
            body["lines"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "kind": "service", "service_id": kind_id, "qty": 1, "unit_price_tyiyn": 0 }));
        }
        let (s, _, sale) = post(&admin, "/api/v1/sales".into(), body).await;
        assert_eq!(s, StatusCode::OK, "{sale}");
        totals.push(50_000 * qty);
    }
    for (kind, amount, comment) in [
        ("bonus", 50_000, "за чистоту"),
        ("penalty", 10_000, "опоздание"),
    ] {
        let (s, _, _) = post(
            &owner,
            "/api/v1/payroll/accruals".into(),
            json!({ "op_id": Uuid::now_v7(), "employee_id": cashier, "kind": kind,
                    "amount_tyiyn": amount, "comment": comment }),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let bonus: Uuid = sqlx::query_scalar("select id from payroll_accruals where kind = 'bonus'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (s, _, _) = post(
        &owner,
        format!("/api/v1/payroll/accruals/{bonus}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, p) = post(
        &owner,
        "/api/v1/payouts".into(),
        json!({ "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 1_000, "source": "account",
                "account_id": owner_safe(&app, &owner).await }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let (till, _, _) = accounts_of(&app, &owner).await;
    let counted = balance_of(&pool, &till).await - 20_000;
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": counted, "comment": "не хватает" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Экран «Расчёт»: день и месяц.
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&owner), None).await;
    let (_, _, month) = call(&app, "GET", "/api/v1/payroll/month", Some(&owner), None).await;
    let of = |v: &Value, id: &str, key: &str| {
        v.as_array()
            .unwrap()
            .iter()
            .find(|r| r["employee_id"] == id)
            .unwrap()[key]
            .as_i64()
            .unwrap()
    };
    let c_day = of(&day, &cashier, "accrued_tyiyn");
    let m_day = of(&day, &master, "accrued_tyiyn");
    let c_month = of(&month["rows"], &cashier, "accrued_tyiyn");
    let m_month = of(&month["rows"], &master, "accrued_tyiyn");
    // 2 % с валовой 4 × 470 с → 37,60 с; удержание 100 с; недостача 200 с; премия сторнирована.
    assert_eq!(c_day, 3_760 - 10_000 - 20_000);
    assert_eq!(m_day, 6_000);

    let mut replies = Vec::new();
    let r = bot(&pool, cchat, Some("💼 Сегодня"), None).await;
    assert!(
        r.text
            .contains(&format!("Итого за день: {}", format_som(c_day))),
        "{}",
        r.text
    );
    assert!(
        r.text.contains("% с чеков: 37,60 с") && r.text.contains("удержание недостачи: −200 с")
    );
    replies.push(r.text);
    let r = bot(&pool, cchat, Some("📅 Месяц"), None).await;
    assert!(
        r.text
            .contains(&format!("Итого за месяц: {}", format_som(c_month))),
        "{}",
        r.text
    );
    assert!(r.text.contains(&format!("— {}", format_som(c_day))));
    assert!(r.text.contains("Оклад за этот месяц ещё не начислен"));
    // Листается назад, вперёд дальше текущего — нет.
    let texts: Vec<&str> = r.inline.iter().flatten().map(|b| b.text.as_str()).collect();
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert!(texts[0].starts_with('←'));
    replies.push(r.text);
    let r = bot(&pool, mchat, Some("💼 Сегодня"), None).await;
    assert!(
        r.text.contains("за замены: 2 × 30 с = 60 с")
            // Заработок мастера по услугам (SPEC-30 §2).
            && r.text.contains("— Замена масла в моторе: 2 × 30 с = 60 с")
            && r.text
                .contains(&format!("Итого за день: {}", format_som(m_day))),
        "{}",
        r.text
    );
    replies.push(r.text);
    let r = bot(&pool, mchat, Some("/месяц"), None).await;
    assert!(
        r.text.contains("— Замена масла в моторе: 2 × 30 с = 60 с"),
        "{}",
        r.text
    );
    assert!(
        r.text
            .contains(&format!("Итого за месяц: {}", format_som(m_month)))
    );
    replies.push(r.text);
    let r = bot(&pool, cchat, Some("🧾 Удержания и премии"), None).await;
    assert!(
        r.text.contains("премия +500 с — за чистоту")
            && r.text.contains("отменено: ошибка")
            && r.text.contains("удержание −100 с — опоздание")
            && r.text.contains("удержание недостачи −200 с"),
        "{}",
        r.text
    );
    replies.push(r.text);
    // Мастер видит свои замены без сумм чеков; кассир — свои смены.
    let r = bot(&pool, mchat, Some("🔧 Мои работы"), None).await;
    assert_eq!(r.text.matches(" · Масло").count(), 2, "{}", r.text);
    for t in &totals {
        assert!(!r.text.contains(&format_som(*t)), "{}", r.text);
    }
    replies.push(r.text);
    let r = bot(&pool, cchat, Some("🔧 Мои работы"), None).await;
    assert!(
        r.text.contains("смена № 1") && r.text.contains("недостача 200 с"),
        "{}",
        r.text
    );
    replies.push(r.text);
    replies.push(
        bot(&pool, cchat, Some("а когда зарплата?"), None)
            .await
            .text,
    );
    for t in &replies {
        no_payout_words(t);
    }

    // Отключение сотрудника и смена телефона отвязывают чат сразу, в той же правке.
    let (s, _, _) = patch(
        &owner,
        format!("/api/v1/employees/{master}"),
        json!({ "active": false }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let r = bot(&pool, mchat, Some("💼 Сегодня"), None).await;
    assert_eq!(r.keys, Keys::Contact);
    assert!(!r.text.contains("Итого"), "{}", r.text);
    let (s, _, _) = patch(
        &owner,
        format!("/api/v1/employees/{cashier}"),
        json!({ "phone": "0555 99 88 77" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let r = bot(&pool, cchat, Some("📅 Месяц"), None).await;
    assert_eq!(r.keys, Keys::Contact);
    assert!(!r.text.contains("Итого"), "{}", r.text);
    let left: i64 = sqlx::query_scalar("select count(*) from telegram_employees")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    let unlinked = actions(&pool, "telegram.employee_unlink").await;
    assert_eq!(unlinked.len(), 2);
    // Тот же номер в другой записи — чат не отвязывается.
    let r = bot(&pool, cchat, None, Some("+996 555 998 877")).await;
    assert_eq!(r.keys, Keys::Employee);
    let (s, _, _) = patch(
        &owner,
        format!("/api/v1/employees/{cashier}"),
        json!({ "phone": "+996555998877" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        bot(&pool, cchat, Some("💼 Сегодня"), None).await.keys,
        Keys::Employee
    );
}

// ---------- Операции и отчёты владельца через бота (SPEC-24) ----------

/// Поддельный сервис разбора вопросов: отдаёт заготовленные ответы по очереди; `None` —
/// вернуть текстом результаты инструментов из последнего сообщения.
#[derive(Clone, Default)]
struct FakeModel {
    steps: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Option<Value>>>>,
}

async fn fake_model_messages(
    axum::extract::State(f): axum::extract::State<FakeModel>,
    axum::Json(body): axum::Json<Value>,
) -> axum::Json<Value> {
    let step = f.steps.lock().unwrap().pop_front().flatten();
    axum::Json(step.unwrap_or_else(|| {
        let last = body["contents"].as_array().unwrap().last().unwrap().clone();
        let text = last["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                let r = &p["functionResponse"]["response"];
                r["result"]
                    .as_str()
                    .or_else(|| r["error"].as_str())
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        model_text(&text)
    }))
}

fn model_text(text: &str) -> Value {
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": text }] },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
    })
}

fn model_tool(name: &str, input: Value) -> Value {
    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{ "functionCall": { "name": name, "args": input }, "thoughtSignature": "sig" }]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
    })
}

async fn fake_model() -> (String, FakeModel) {
    let fake = FakeModel::default();
    let app = Router::new()
        .route(
            "/v1beta/models/{*rest}",
            axum::routing::post(fake_model_messages),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_operations_and_reports_match_the_screens(pool: PgPool) {
    // Расход через бота — такой же, как внесённый на экране; отчёты бота — те же числа, что
    // отдают экраны владельца (SPEC-24).
    use avtodom_server::api::assistant::Assistant;
    use avtodom_server::api::telegram::{handle_callback, handle_message_with};
    use avtodom_server::domain::money::format_som;
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (owner_id, branch): (Uuid, Uuid) =
        sqlx::query_as("select id, branch_id from users where login = 'owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let (till, _, _) = accounts_of(&app, &owner).await;
    let eid = cashier_employee(&app, &owner).await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": eid, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 2_000_000, "comment": "размен" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let pid = product(&app, &owner, "Фильтр C-1821", 50_000, 10).await;
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": pid, "qty": 4, "cost_tyiyn": 120_000 }]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&owner), None).await;
    let household = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == json!("Хозтовары"))
        .unwrap()["id"]
        .clone();
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 150_000,
            "source": "account", "account_id": till, "comment": "ведро"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Тот же расход через бота.
    let chat = 980_001i64;
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $1)")
        .bind(chat)
        .bind(owner_id)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    let (url, fake) = fake_model().await;
    let a = Assistant::new("test-key".into(), Some(url), None, 100).with_retry_pauses(Vec::new());
    let mut conn = pool.acquire().await.unwrap();
    let ask = |steps: Vec<Option<Value>>| {
        fake.steps.lock().unwrap().extend(steps);
    };
    async fn say_in(
        conn: &mut sqlx::PgConnection,
        a: &Assistant,
        chat: i64,
        text: &str,
    ) -> avtodom_server::api::telegram::Reply {
        handle_message_with(conn, Some(a), chat, chat, Some(text), None)
            .await
            .unwrap()
    }
    say_in(&mut conn, &a, chat, "💬 Спросить").await;
    ask(vec![
        Some(model_tool(
            "draft_expense",
            json!({ "article": "Хозтовары", "amount": "1 500", "comment": "ведро" }),
        )),
        Some(model_text("Проверьте черновик и нажмите «✅ Провести».")),
    ]);
    let r = say_in(
        &mut conn,
        &a,
        chat,
        "расход 1500 на хозтовары из кассы, ведро",
    )
    .await;
    let ok = r.follow.as_ref().unwrap().inline[0][0].data.clone();
    let done = handle_callback(&mut conn, chat, &ok).await.unwrap().text;
    assert!(done.starts_with("Проведено: расход № 2"), "{done}");
    let (_, _, list) = call(&app, "GET", "/api/v1/expenses", Some(&owner), None).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    for key in [
        "article_id",
        "article_name",
        "amount_tyiyn",
        "source",
        "account_name",
        "expense_date",
        "comment",
        "reversed",
        "user_name",
    ] {
        assert_eq!(list[0][key], list[1][key], "{key}");
    }

    // Цена через бота — на экране «Уведомления» с пометкой «через бот».
    ask(vec![
        Some(model_tool(
            "draft_product",
            json!({ "operation": "цена продажи", "product": "C-1821", "value": "550" }),
        )),
        Some(model_text("Проверьте черновик.")),
    ]);
    let r = say_in(&mut conn, &a, chat, "цена C-1821 теперь 550").await;
    let ok = r.follow.as_ref().unwrap().inline[0][0].data.clone();
    let done = handle_callback(&mut conn, chat, &ok).await.unwrap().text;
    assert!(
        done.starts_with("Проведено: «Фильтр C-1821»: цена 550 с"),
        "{done}"
    );
    let (_, _, events) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let price = events["items"]
        .as_array()
        .or(events.as_array())
        .unwrap()
        .iter()
        .find(|n| n["action"] == json!("product.prices"))
        .unwrap()
        .clone();
    assert!(
        price["details"].as_str().unwrap().ends_with("через бот"),
        "{price}"
    );

    // Отчёты бота — те же числа, что экраны.
    let (_, _, accs) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let till_balance = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(till))
        .unwrap()["balance_tyiyn"]
        .as_i64()
        .unwrap();
    assert_eq!(till_balance, 1_700_000);
    ask(vec![
        Some(model_tool(
            "money_report",
            json!({ "report": "movements", "account": "Касса" }),
        )),
        None,
    ]);
    let text = say_in(&mut conn, &a, chat, "движения по кассе").await.text;
    assert!(
        text.contains(&format!("Касса: сейчас {}", format_som(till_balance))),
        "{text}"
    );

    let (_, _, dash) = call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None).await;
    ask(vec![
        Some(model_tool("sales_report", json!({ "report": "dashboard" }))),
        None,
    ]);
    let text = say_in(&mut conn, &a, chat, "сводка").await.text;
    for want in [
        format!(
            "Деньги всего: {}",
            format_som(dash["money_total_tyiyn"].as_i64().unwrap())
        ),
        format!(
            "чистая прибыль {}",
            format_som(dash["totals"]["net_tyiyn"].as_i64().unwrap())
        ),
        format!("кончается {}", dash["low_stock"]),
    ] {
        assert!(text.contains(&want), "{want}: {text}");
    }

    let day = dash["date"].as_str().unwrap().to_string();
    let (_, _, exps) = call(
        &app,
        "GET",
        &format!("/api/v1/expenses?from={day}&to={day}"),
        Some(&owner),
        None,
    )
    .await;
    let total: i64 = exps
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["amount_tyiyn"].as_i64().unwrap())
        .sum();
    ask(vec![
        Some(model_tool(
            "money_report",
            json!({ "report": "expenses_list" }),
        )),
        None,
    ]);
    let text = say_in(&mut conn, &a, chat, "расходы сегодня").await.text;
    assert!(
        text.contains(&format!("итого {}", format_som(total))),
        "{text}"
    );

    let (_, _, shifts) = call(&app, "GET", "/api/v1/shifts", Some(&owner), None).await;
    let expected = shifts[0]["expected_tyiyn"].as_i64().unwrap();
    ask(vec![
        Some(model_tool("money_report", json!({ "report": "shifts" }))),
        None,
    ]);
    let text = say_in(&mut conn, &a, chat, "смены").await.text;
    assert!(
        text.contains(&format!("в кассе должно быть {}", format_som(expected))),
        "{text}"
    );

    let (_, _, stock) = call(&app, "GET", "/api/v1/stock", Some(&owner), None).await;
    let value: i64 = stock
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["stock_value_tyiyn"].as_i64().unwrap_or(0))
        .sum();
    assert_eq!(value, 120_000);
    ask(vec![
        Some(model_tool("stock_report", json!({ "report": "stock" }))),
        None,
    ]);
    let text = say_in(&mut conn, &a, chat, "склад").await.text;
    assert!(
        text.contains(&format!("стоимость остатков {}", format_som(value))),
        "{text}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defects_and_supplier_deliveries(pool: PgPool) {
    // Брак (SPEC-25) и поставки в карточке поставщика (SPEC-26) через HTTP: права, уведомление,
    // строки прибыли, оценка остатка «первым продаётся пришедшее раньше».
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let mut suppliers = vec![];
    for name in ["Автоснаб", "Колёса"] {
        let (_, _, s) = call(
            &app,
            "POST",
            "/api/v1/suppliers",
            Some(&admin),
            Some(json!({ "name": name })),
        )
        .await;
        suppliers.push(s["id"].as_str().unwrap().to_string());
    }
    let (a, b) = (suppliers[0].clone(), suppliers[1].clone());
    // Автоснаб: 10 шт, потом Колёса: 5 шт, потом Автоснаб: 3 шт и сторнированная накладная.
    let mut receipts = vec![];
    for (sid, qty, cost) in [
        (&a, 10, 1_000_000),
        (&b, 5, 600_000),
        (&a, 3, 360_000),
        (&a, 7, 700_000),
    ] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/receipts",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": sid, "payment": "debt",
                         "lines": [{ "product_id": item, "qty": qty, "cost_tyiyn": cost }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
        receipts.push(r["id"].as_str().unwrap().to_string());
    }
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/receipts/{}/reverse", receipts[3]),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Подсказка поставщика — из последнего непогашенного прихода.
    let (_, _, hint) = call(
        &app,
        "GET",
        &format!("/api/v1/defects/supplier-hint?product_id={item}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(hint["supplier_id"], json!(a));

    // Администратор отмечает брак без причины — нельзя, с причиной — можно.
    let mark = |reason: &str, qty: i64| json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": qty, "reason": reason });
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/defects",
        Some(&admin),
        Some(mark("  ", 1)),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, d) = call(
        &app,
        "POST",
        "/api/v1/defects",
        Some(&admin),
        Some(mark("битый корпус", 2)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["cost_tyiyn"], Value::Null);
    let id = d["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/defects",
        Some(&admin),
        Some(mark("много", 100)),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Владельцу — уведомление.
    let (_, _, notes) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let note = notes["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["action"] == "defect.mark")
        .unwrap()
        .clone();
    assert_eq!(note["title"], "В брак: Фильтр × 2 шт");
    assert!(
        note["details"].as_str().unwrap().contains("битый корпус"),
        "{note}"
    );

    // Решение и его сторно — только владелец.
    let resolve = |kind: &str| json!({ "op_id": Uuid::now_v7(), "kind": kind });
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{id}/resolve"),
        Some(&admin),
        Some(resolve("write_off")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, r) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{id}/resolve"),
        Some(&owner),
        Some(resolve("credit")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "returned");
    let cost = r["cost_tyiyn"].as_i64().unwrap();
    // Средняя: 19 600 с за 18 шт, брак 2 шт.
    assert_eq!(cost, 217_778);
    assert_eq!(r["resolution"]["refund_tyiyn"], cost);
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{id}/resolution/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Списки: ожидающие и решённые; администратору стоимость не отдаётся.
    let (_, _, pending) = call(
        &app,
        "GET",
        "/api/v1/defects?status=pending",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(pending.as_array().map(Vec::len), Some(0));
    let (_, _, done) = call(
        &app,
        "GET",
        "/api/v1/defects?status=resolved",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(done[0]["resolution"]["kind"], "credit");
    assert_eq!(done[0]["resolution"]["refund_tyiyn"], Value::Null);

    // Второй брак списываем: отчёт о прибыли показывает строку «Списание брака».
    let (_, _, d2) = call(
        &app,
        "POST",
        "/api/v1/defects",
        Some(&owner),
        Some(mark("течёт", 1)),
    )
    .await;
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{}/resolve", d2["id"].as_str().unwrap()),
        Some(&owner),
        Some(resolve("write_off")),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["defect_writeoff_tyiyn"], d2["cost_tyiyn"]);
    assert_eq!(profit["totals"]["defect_diff_tyiyn"], 0);
    assert_eq!(profit["days"][0]["defect_writeoff_tyiyn"], d2["cost_tyiyn"]);
    let (_, _, dash) = call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None).await;
    assert_eq!(dash["totals"]["defect_writeoff_tyiyn"], d2["cost_tyiyn"]);

    // Остаток: 18 − 2 − 1 = 15. Сходится с движениями.
    let (_, _, verify) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!(verify, json!([]));

    // Поставки Автоснаба: привёз 13 (сторнированная не считается), вернули 2.
    let (s, _, del) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{a}/deliveries"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{del}");
    let row = &del["rows"][0];
    assert_eq!(row["delivered_qty"], 13);
    assert_eq!(row["returned_qty"], 2);
    assert_eq!(row["amount_tyiyn"], 1_360_000);
    assert_eq!(row["last_price_tyiyn"], 120_000);
    assert_eq!(row["stock_qty"], 15);
    // От новых к старым: 3 от Автоснаба, 5 от Колёс, оставшиеся 7 — от Автоснаба: 10.
    assert_eq!(row["estimate_qty"], 10);
    assert!(row["estimate_value_tyiyn"].is_i64());
    assert_eq!(del["amount_tyiyn"], 1_360_000);
    // Наш долг минус возмещение брака.
    assert_eq!(del["balance_tyiyn"], -1_360_000 + cost);
    let (_, _, del_b) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{b}/deliveries"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(del_b["rows"][0]["estimate_qty"], 5);
    assert_eq!(del_b["rows"][0]["returned_qty"], 0);
    // Оценки не больше остатка в сумме.
    assert!(row["estimate_qty"].as_i64().unwrap() + 5 <= 15);

    // Администратор видит количества и суммы закупок, но не себестоимость остатка.
    let (s, _, del_admin) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{a}/deliveries"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(del_admin["rows"][0]["amount_tyiyn"], 1_360_000);
    assert_eq!(del_admin["rows"][0]["estimate_value_tyiyn"], Value::Null);
    assert_eq!(del_admin["pending_defects_tyiyn"], Value::Null);

    // Накладные поставщика с товаром: сторнированная помечена.
    let (_, _, list) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{a}/deliveries/{item}"),
        Some(&admin),
        None,
    )
    .await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list.iter().filter(|r| r["reversed"] == true).count(), 1);

    // Сторно решения по возврату: запись ждёт решения, долг поставщика вернулся, возврат пропал.
    let (s, _, back) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{id}/resolution/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "поставщик не принял" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["status"], "pending");
    let (_, _, del) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{a}/deliveries"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(del["rows"][0]["returned_qty"], 0);
    assert_eq!(del["balance_tyiyn"], -1_360_000);
    assert_eq!(del["pending_defects"], 1);
    let (_, _, verify) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!(verify, json!([]));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn employee_with_phone_is_reactivated_only_by_owner(pool: PgPool) {
    // Телефон открывает работнику кабинет в боте: включить такого снова может только владелец
    // (ADR-070). Без телефона — и администратор.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let id = cashier_employee(&app, &owner).await;
    let uri = format!("/api/v1/employees/{id}");
    let patch = |who: &str, body: Value| {
        let (app, who, uri) = (app.clone(), who.to_string(), uri.clone());
        async move { call(&app, "PATCH", &uri, Some(&who), Some(body)).await }
    };
    assert_eq!(
        patch(&admin, json!({ "active": false })).await.0,
        StatusCode::OK
    );
    assert_eq!(
        patch(&admin, json!({ "active": true })).await.0,
        StatusCode::OK
    );
    assert_eq!(
        patch(&owner, json!({ "phone": "+996 555 12 34 56" }))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        patch(&admin, json!({ "active": false })).await.0,
        StatusCode::OK
    );
    let (s, _, err) = patch(&admin, json!({ "active": true })).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("включает владелец"),
        "{err}"
    );
    let (s, _, e) = patch(&owner, json!({ "active": true })).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(e["active"], true);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn product_with_pending_defect_is_not_archived(pool: PgPool) {
    // Брак, ждущий решения, вернулся бы на карточку в архиве (ADR-070).
    let app = setup(pool.clone()).await;
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
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр" })),
    )
    .await;
    let pid = product["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": pid, "qty": 1, "cost_tyiyn": 10_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, d) = call(
        &app,
        "POST",
        "/api/v1/defects",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "product_id": pid, "qty": 1, "reason": "течёт", "no_supplier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let archive = |v: bool| {
        let (app, owner, uri) = (
            app.clone(),
            owner.clone(),
            format!("/api/v1/products/{pid}"),
        );
        async move {
            call(
                &app,
                "PATCH",
                &uri,
                Some(&owner),
                Some(json!({ "archived": v })),
            )
            .await
        }
    };
    let (s, _, err) = archive(true).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("№ {}", d["number"])),
        "{err}"
    );
    // Списали брак — карточку без остатка можно убрать в архив.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/defects/{}/resolve", d["id"].as_str().unwrap()),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "write_off" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(archive(true).await.0, StatusCode::OK);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_deliveries_follow_merged_cards(pool: PgPool) {
    // Приход на дубль, потом влитый в основную карточку, — поставка основной (ADR-070).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let main = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let dup = product(&app, &owner, "Фильтр дубль", 50_000, 0).await;
    let (_, _, s) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(json!({ "name": "Автоснаб" })),
    )
    .await;
    let sid = s["id"].as_str().unwrap().to_string();
    for (item, qty, cost) in [(&dup, 4, 400_000), (&main, 2, 200_000)] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/receipts",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": sid, "payment": "debt",
                         "lines": [{ "product_id": item, "qty": qty, "cost_tyiyn": cost }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    let (s, _, m) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{dup}/merge"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "into_product_id": main })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    let (s, _, del) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}/deliveries"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{del}");
    let rows = del["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{del}");
    assert_eq!(rows[0]["product_id"], json!(main));
    assert_eq!(rows[0]["delivered_qty"], 6);
    assert_eq!(rows[0]["stock_qty"], 6);
    assert_eq!(rows[0]["estimate_qty"], 6);
    let (_, _, list) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}/deliveries/{main}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(list.as_array().map(Vec::len), Some(2));
}

// ---------- Смены и кассир (SPEC-27, ADR-071) ----------

/// Ещё один кассир с заданным именем.
async fn named_cashier(app: &Router, owner: &str, name: &str) -> String {
    let (status, _, e) = call(
        app,
        "POST",
        "/api/v1/employees",
        Some(owner),
        Some(json!({ "full_name": name, "is_cashier": true, "pin": "1234" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{e}");
    e["id"].as_str().unwrap().to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn several_shifts_a_day_each_with_own_count(pool: PgPool) {
    // Две смены в один день по очереди: у каждой свой пересчёт, отчёт дня складывает обе,
    // переоткрыть можно только последнюю.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let first = named_cashier(&app, &owner, "Айбек").await;
    let second = named_cashier(&app, &owner, "Бермет").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": item, "qty": 5, "cost_tyiyn": 30_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let post = |cookie: &str, uri: String, body: Value| {
        let app = app.clone();
        let cookie = cookie.to_string();
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let sale = |cashier: String| {
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }]
        })
    };

    // Первая смена: продажа и закрытие с недостачей 100 с.
    let (s, _, one) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": first, "cashier_pin": "1234", "counted_tyiyn": 0 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{one}");
    let one_id = one["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(&admin, "/api/v1/sales".into(), sale(first.clone())).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, closed) = post(
        &admin,
        format!("/api/v1/shifts/{one_id}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 40_000, "comment": "не нашли" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{closed}");
    assert_eq!(closed["expected_tyiyn"], 50_000);
    assert_eq!(closed["diff_tyiyn"], -10_000);

    // Вторая смена того же дня: свой пересчёт при открытии (нашлись 50 с) и при закрытии.
    let (s, _, two) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": second, "cashier_pin": "1234", "counted_tyiyn": 45_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{two}");
    assert_eq!(two["business_date"], one["business_date"]);
    assert_eq!(two["opening_expected_tyiyn"], 40_000);
    assert_eq!(two["expected_tyiyn"], 45_000);
    assert_ne!(two["number"], one["number"]);
    let two_id = two["id"].as_str().unwrap().to_string();
    // Пока открыта вторая, третью не открыть.
    let (s, _, _) = post(
        &admin,
        "/api/v1/shifts/open".into(),
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": first, "cashier_pin": "1234" }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, _) = post(&admin, "/api/v1/sales".into(), sale(second.clone())).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, closed2) = post(
        &admin,
        format!("/api/v1/shifts/{two_id}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 95_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{closed2}");
    assert_eq!(closed2["expected_tyiyn"], 95_000);
    assert_eq!(closed2["diff_tyiyn"], 0);
    // Отчёт смены — только свои чеки.
    assert_eq!(closed["sales_count"], 1);
    assert_eq!(closed2["sales_count"], 1);

    let (_, _, list) = call(&app, "GET", "/api/v1/shifts", Some(&owner), None).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["id"], json!(two_id));

    // Отчёт дня складывает обе смены: выручка двух чеков и оба пересчёта (−100 с + 50 с).
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["goods_tyiyn"], 100_000, "{profit}");
    assert_eq!(profit["totals"]["cash_diff_tyiyn"], -5_000);
    assert_eq!(profit["days"][0]["cash_diff_tyiyn"], -5_000);

    // Первую не переоткрыть: после неё была вторая. Вторую — можно, и только владельцу.
    let (s, _, err) = post(
        &owner,
        format!("/api/v1/shifts/{one_id}/reopen"),
        json!({ "op_id": Uuid::now_v7(), "reason": "проверка" }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("только последнюю"),
        "{err}"
    );
    let (s, _, _) = post(
        &admin,
        format!("/api/v1/shifts/{two_id}/reopen"),
        json!({ "op_id": Uuid::now_v7(), "reason": "проверка" }),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, back) = post(
        &owner,
        format!("/api/v1/shifts/{two_id}/reopen"),
        json!({ "op_id": Uuid::now_v7(), "reason": "проверка" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["closed_at"], Value::Null);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_takes_the_cashier_of_the_open_shift(pool: PgPool) {
    // При открытой смене кассир чека — кассир смены, даже если прислали другого (ADR-071).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let on_shift = named_cashier(&app, &owner, "Айбек").await;
    let other = named_cashier(&app, &owner, "Бермет").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": item, "qty": 5, "cost_tyiyn": 30_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sale = |offline: bool| {
        let mut body = json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": other,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "card", "amount_tyiyn": 50_000 }]
        });
        if offline {
            body["offline"] = json!(true);
            body["client_time"] =
                json!((chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339());
        }
        body
    };
    // Без смены — кассир, выбранный на кассе.
    let (s, _, before) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(false)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{before}");
    assert_eq!(before["cashier_id"], json!(other));
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": on_shift, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Смена открыта — присланный другой кассир не учитывается.
    let (s, _, during) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(false)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{during}");
    assert_eq!(during["cashier_id"], json!(on_shift));
    assert_eq!(during["cashier_name"], "Айбек");
    // Чек без сети, пробитый до открытия смены, остаётся за кассиром с устройства.
    let (s, _, offline) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(true)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{offline}");
    assert_eq!(offline["cashier_id"], json!(other));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn close_with_payouts_pays_from_the_till(pool: PgPool) {
    // «Выплатить отмеченным и закрыть»: выплаты из кассы смены и закрытие одной операцией,
    // «должно быть» — уже за вычетом выплат; повтор не платит второй раз (SPEC-27).
    // Администратор выплачивает только мастерам, кассиру — владелец (SPEC-30 §6).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = named_cashier(&app, &owner, "Айбек").await;
    let (_, _, master) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Мастер Нурлан", "is_master": true })),
    )
    .await;
    let master = master["id"].as_str().unwrap().to_string();
    for (emp, amount) in [(&cashier, 30_000), (&master, 20_000)] {
        let (s, _, _) = call(
            &app,
            "POST",
            "/api/v1/payroll/accruals",
            Some(&owner),
            Some(json!({
                "op_id": Uuid::now_v7(), "employee_id": emp, "kind": "bonus",
                "amount_tyiyn": amount, "comment": "за неделю"
            })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (till, _, _) = accounts_of(&app, &owner).await;
    let (s, _, shift) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 100_000, "comment": "размен" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // В расчёте дня видно, кому сколько к выдаче: владельцу — всем, администратору — мастерам.
    let owed = |day: &Value, id: &str| {
        day.as_array()
            .unwrap()
            .iter()
            .find(|r| r["employee_id"] == json!(id))
            .map(|r| r["balance_tyiyn"].as_i64().unwrap())
    };
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&owner), None).await;
    assert_eq!(owed(&day, &cashier), Some(30_000));
    assert_eq!(owed(&day, &master), Some(20_000));
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&admin), None).await;
    assert_eq!(owed(&day, &cashier), None, "{day}");
    assert_eq!(owed(&day, &master), Some(20_000));

    // Кассиру администратор не выплачивает — вся операция отклоняется, смена открыта.
    let (s, _, err) = call(
        &app,
        "POST",
        &format!("/api/v1/shifts/{sid}/close"),
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "counted_tyiyn": 50_000,
            "payouts": [
                { "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 15_000 },
                { "op_id": Uuid::now_v7(), "employee_id": cashier, "amount_tyiyn": 30_000 }
            ]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{err}");
    assert_eq!(err["error"]["message"], "Кассиру выплачивает владелец");
    // Не выдаёт и больше заработанного.
    let (s, _, err) = call(
        &app,
        "POST",
        &format!("/api/v1/shifts/{sid}/close"),
        Some(&admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "counted_tyiyn": 50_000,
            "payouts": [{ "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 25_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{err}");
    assert_eq!(balance_of(&pool, &till).await, 100_000);
    let paid: i64 = sqlx::query_scalar("select count(*) from payouts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(paid, 0);

    // Кассиру всё выдаёт владелец из кассы смены, мастеру часть — администратор при закрытии;
    // в кассе остаётся 100 − 30 − 15 = 55 с.
    let (s, _, out) = call(
        &app,
        "POST",
        "/api/v1/payouts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "employee_id": cashier, "amount_tyiyn": 30_000,
            "account_id": till, "comment": "за неделю"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let body = json!({
        "op_id": Uuid::now_v7(), "counted_tyiyn": 55_000,
        "payouts": [
            { "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 15_000 }
        ]
    });
    let uri = format!("/api/v1/shifts/{sid}/close");
    let (s, _, closed) = call(&app, "POST", &uri, Some(&admin), Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{closed}");
    assert_eq!(closed["expected_tyiyn"], 55_000);
    assert_eq!(closed["diff_tyiyn"], 0);
    let payouts = closed["breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["kind"] == "payout")
        .unwrap()["sum_tyiyn"]
        .clone();
    assert_eq!(payouts, -45_000);
    // Отчёт смены сходится: на начало + движения (с выплатами) = должно быть.
    let ladder: i64 = closed["opening_expected_tyiyn"].as_i64().unwrap()
        + closed["breakdown"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["sum_tyiyn"].as_i64().unwrap())
            .sum::<i64>();
    assert_eq!(ladder, 55_000);
    // Повтор того же запроса (потерянный ответ) — тот же ответ, без второй выплаты.
    let (s, _, again) = call(&app, "POST", &uri, Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], closed["id"]);
    let rows: Vec<(String, i64, String)> = sqlx::query_as(
        "select employee_id::text, amount_tyiyn, comment from payouts order by amount_tyiyn desc",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, cashier);
    assert_eq!(rows[0].1, 30_000);
    assert_eq!(rows[1].0, master);
    assert_eq!(rows[1].1, 15_000);
    assert!(rows[1].2.contains("при закрытии смены"));
    assert_eq!(balance_of(&pool, &till).await, 55_000);
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&admin), None).await;
    let left = day
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == json!(master))
        .unwrap()["balance_tyiyn"]
        .clone();
    assert_eq!(left, 5_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_edits_every_employee_field_with_journal(pool: PgPool) {
    // Владелец меняет ФИО, роли, телефон и активность в одной карточке; в журнале — было и стало.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Айбек", "is_master": true, "phone": "+996 555 12 34 56" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert_eq!(e["phone"], "+996 555 12 34 56");
    let id = e["id"].as_str().unwrap().to_string();
    // Администратор телефон не задаёт и кассиром не делает.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&admin),
        Some(json!({ "full_name": "Чужой", "is_master": true, "phone": "+996 555 00 00 01" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/employees/{id}"),
        Some(&admin),
        Some(json!({ "is_cashier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _, after) = call(
        &app,
        "PATCH",
        &format!("/api/v1/employees/{id}"),
        Some(&owner),
        Some(json!({
            "full_name": "Айбек Садыров", "is_cashier": true, "is_master": false,
            "active": false, "phone": "+996 700 11 22 33"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{after}");
    assert_eq!(after["full_name"], "Айбек Садыров");
    assert_eq!(after["is_cashier"], true);
    assert_eq!(after["is_master"], false);
    assert_eq!(after["active"], false);
    assert_eq!(after["phone"], "+996 700 11 22 33");
    let log = actions(&pool, "employee.update").await;
    let last = log.last().unwrap();
    assert_eq!(last["before"]["full_name"], "Айбек");
    assert_eq!(last["before"]["is_master"], true);
    assert_eq!(last["before"]["active"], true);
    assert_eq!(last["before"]["phone"], "+996 555 12 34 56");
    assert_eq!(last["after"]["full_name"], "Айбек Садыров");
    assert_eq!(last["after"]["is_cashier"], true);
    assert_eq!(last["after"]["active"], false);
    assert_eq!(last["after"]["phone"], "+996 700 11 22 33");
    // Стал кассиром — получил правила оплаты по умолчанию.
    assert_eq!(last["pay_rules_added"], true);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_slot_is_set_on_category_and_overridden_per_vehicle(pool: PgPool) {
    // «Строка книжки» и её интервал задаёт владелец у категории; у машины интервал строки
    // правится и сбрасывается к категории (SPEC-27).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    // Без строки — по виду категории: фильтры — масляный фильтр.
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&admin),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    assert_eq!(cat["book_slot"], "oil_filter");
    let uri = format!("/api/v1/categories/{}", cat["id"].as_str().unwrap());
    let book =
        json!({ "book": { "slot": "cabin_filter", "interval_km": 15000, "interval_days": 180 } });
    assert_eq!(
        call(&app, "PATCH", &uri, Some(&admin), Some(book.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let bad = json!({ "book": { "slot": "cabin_filter", "interval_km": 15000 } });
    assert_eq!(
        call(&app, "PATCH", &uri, Some(&owner), Some(bad)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, cat) = call(&app, "PATCH", &uri, Some(&owner), Some(book)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (
            cat["book_slot"].clone(),
            cat["book_interval_km"].clone(),
            cat["book_interval_days"].clone()
        ),
        (json!("cabin_filter"), json!(15000), json!(180))
    );
    assert_eq!(
        actions(&pool, "category.update").await[0]["book"]["slot"]["new"],
        "cabin_filter"
    );

    // Машина: интервал масла и салонного фильтра у машины, сброс — к категории.
    let (_, _, p) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "name": "Эрлан" })),
    )
    .await;
    let (_, _, car) = call(
        &app,
        "POST",
        &format!("/api/v1/parties/{}/vehicles", p["id"].as_str().unwrap()),
        Some(&admin),
        Some(json!({ "plate": "01KG555BBB" })),
    )
    .await;
    let settings = format!(
        "/api/v1/vehicles/{}/oil-settings",
        car["id"].as_str().unwrap()
    );
    // Как раньше: без строки — масло.
    let (s, _, b) = call(
        &app,
        "PATCH",
        &settings,
        Some(&admin),
        Some(json!({ "interval_km": 10000, "interval_days": 60 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (b["interval_km"].clone(), b["interval_days"].clone()),
        (json!(10000), json!(60))
    );
    let (_, _, b) = call(
        &app,
        "PATCH",
        &settings,
        Some(&admin),
        Some(json!({ "slot": "cabin_filter", "interval_km": 9000, "interval_days": 90 })),
    )
    .await;
    let cabin = b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "cabin_filter")
        .unwrap()
        .clone();
    assert_eq!(
        (cabin["interval_km"].clone(), cabin["own_interval"].clone()),
        (json!(9000), json!(true))
    );
    let (_, _, b) = call(
        &app,
        "PATCH",
        &settings,
        Some(&admin),
        Some(json!({ "slot": "cabin_filter", "reset": true })),
    )
    .await;
    // Замен салонного фильтра не было, интервал машины снят — строки в книжке нет.
    assert!(
        !b["slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["slot"] == "cabin_filter")
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &settings,
            Some(&admin),
            Some(json!({ "slot": "brakes", "interval_km": 9000, "interval_days": 90 })),
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(actions(&pool, "vehicle.oil_interval").await.len(), 3);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn product_list_and_invoice_payment_over_http(pool: PgPool) {
    // SPEC-27: список товаров с итогами (закупка — только владельцу), оплата накладной частью.
    let app = setup(pool).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let (_, _, product) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр W712", "sale_price_tyiyn": 50_000 })),
    )
    .await;
    let (_, _, supplier) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(json!({ "name": "Автоснаб" })),
    )
    .await;
    let (status, _, receipt) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(
            json!({ "op_id": Uuid::now_v7(), "supplier_id": supplier["id"], "payment": "debt",
                     "lines": [{ "product_id": product["id"], "qty": 4, "cost_tyiyn": 120_000 }] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(receipt["debt"]["status"], "unpaid");

    // Владелец видит закупку и сумму по закупке, итоги совпадают со строками.
    let (status, _, list) = call(&app, "GET", "/api/v1/products/list", Some(&owner), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"][0]["purchase_price_tyiyn"], 30_000);
    assert_eq!(list["items"][0]["stock_purchase_tyiyn"], 120_000);
    assert_eq!(list["totals"]["purchase_tyiyn"], 120_000);
    assert_eq!(list["totals"]["sale_tyiyn"], 200_000);
    // Администратору — только по продаже.
    let (status, _, list) = call(&app, "GET", "/api/v1/products/list", Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK);
    let row = &list["items"][0];
    for key in [
        "purchase_price_tyiyn",
        "stock_purchase_tyiyn",
        "stock_value_tyiyn",
        "avg_cost_tyiyn",
    ] {
        assert!(row.get(key).is_none(), "администратору отдано {key}");
    }
    assert!(row["last_purchase_price_tyiyn"].is_null());
    assert!(list["totals"].get("purchase_tyiyn").is_none());
    assert_eq!(list["totals"]["sale_tyiyn"], 200_000);

    // Прошлая закупка видна в приходе и администратору (ADR-008).
    let (status, _, last) = call(
        &app,
        "GET",
        &format!(
            "/api/v1/products/{}/last-receipt",
            product["id"].as_str().unwrap()
        ),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(last["price_tyiyn"], 30_000);
    assert_eq!(last["supplier_name"], "Автоснаб");

    // Оплата части накладной владельцем «из своих».
    let (status, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "party_id": supplier["id"], "amount_tyiyn": 50_000,
                     "method": "transfer", "receipt_id": receipt["id"] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, again) = call(
        &app,
        "GET",
        &format!("/api/v1/receipts/{}", receipt["id"].as_str().unwrap()),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(again["debt"]["status"], "partial");
    assert_eq!(again["debt"]["left_tyiyn"], 70_000);
    assert_eq!(again["debt"]["payments"][0]["source"], "Счёт");
    let (status, _, inv) = call(
        &app,
        "GET",
        &format!(
            "/api/v1/parties/{}/invoices",
            supplier["id"].as_str().unwrap()
        ),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(inv["debt_tyiyn"], 70_000);
    assert_eq!(inv["payments"][0]["user_name"], "Владелец");
    // Продаваемые товары открыты и администратору.
    let (status, _, _) = call(
        &app,
        "GET",
        "/api/v1/reports/top-products",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

// ---------- Деньги и отчёты (SPEC-27 §6) ----------

/// Сумма поля по строкам массива.
fn sum_of(rows: &Value, key: &str) -> i64 {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r[key].as_i64().unwrap())
        .sum()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bank_screen_income_report_and_dashboard_period(pool: PgPool) {
    // «Счёт» сходится с движениями кассы «Счёт»; «Куда сколько поступило» — с отчётом о прибыли;
    // сводка за прошлый день — с прибылью за этот день; администратору «Счёт» закрыт (SPEC-27 §6).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let post = |cookie: &str, uri: &str, body: Value| {
        let app = app.clone();
        let (cookie, uri) = (cookie.to_string(), uri.to_string());
        async move { call(&app, "POST", &uri, Some(&cookie), Some(body)).await }
    };
    let get = |cookie: &str, uri: &str| {
        let app = app.clone();
        let (cookie, uri) = (cookie.to_string(), uri.to_string());
        async move { call(&app, "GET", &uri, Some(&cookie), None).await }
    };
    let (s, _, _) = post(
        &owner,
        "/api/v1/receipts",
        json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 100_000 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sale = |pays: Value, qty: i64| {
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": item, "qty": qty, "unit_price_tyiyn": 50_000 }],
                "payments": pays })
    };
    let (s, _, first) = post(
        &admin,
        "/api/v1/sales",
        sale(
            json!([{ "method": "cash", "amount_tyiyn": 50_000 }, { "method": "card", "amount_tyiyn": 100_000 }]),
            3,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{first}");
    let (s, _, _) = post(
        &admin,
        "/api/v1/sales",
        sale(
            json!([{ "method": "transfer", "amount_tyiyn": 100_000 }]),
            2,
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Возврат одной штуки на карту.
    let (s, _, back) = post(
        &admin,
        &format!("/api/v1/sales/{}/return", first["id"].as_str().unwrap()),
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "card", "amount_tyiyn": 50_000 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    // Чек без сети вчерашним днём — для сводки за прошлый день.
    let mut old = sale(json!([{ "method": "card", "amount_tyiyn": 50_000 }]), 1);
    old["offline"] = json!(true);
    old["client_time"] = json!((chrono::Utc::now() - chrono::Duration::hours(24)).to_rfc3339());
    let (s, _, old) = post(&admin, "/api/v1/sales", old).await;
    assert_eq!(s, StatusCode::OK, "{old}");
    // Погашение долга картой и переводом, перемещение со счёта в сейф владельца.
    let (_, _, party) = post(
        &owner,
        "/api/v1/parties",
        json!({ "name": "ОсОО Должник", "kind": "company", "inn": "01234567890123" }),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, _) = post(
        &owner,
        "/api/v1/debts/adjust",
        json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 300_000, "comment": "из тетради" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    for (method, amount) in [("card", 200_000), ("transfer", 50_000)] {
        let (s, _, r) = post(
            &admin,
            "/api/v1/debts/repayments",
            json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": amount, "method": method }),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    let (_, bank, safe) = accounts_of(&app, &owner).await;
    let (s, _, _) = post(
        &owner,
        "/api/v1/cash/transfers",
        json!({ "op_id": Uuid::now_v7(), "from_account_id": bank, "to_account_id": safe, "amount_tyiyn": 100_000 }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // «Счёт»: только владельцу.
    assert_eq!(
        get(&admin, "/api/v1/cash/bank").await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _, b) = get(&owner, "/api/v1/cash/bank").await;
    assert_eq!(s, StatusCode::OK, "{b}");
    let moves = &b["movements"];
    let change = sum_of(moves, "amount_tyiyn");
    assert_eq!(b["movements_count"], moves.as_array().unwrap().len());
    assert_eq!(b["opening_tyiyn"], 0);
    assert_eq!(b["closing_tyiyn"], change);
    assert_eq!(b["balance_tyiyn"], balance_of(&pool, &bank).await);
    assert_eq!(b["closing_tyiyn"], b["balance_tyiyn"]);
    // Карта: 100 000 − 50 000 возврат + 50 000 чек без сети (деньги пришли сегодня); перевод 100 000.
    assert_eq!(b["card"]["sales_tyiyn"], 100_000);
    assert_eq!(b["transfer"]["sales_tyiyn"], 100_000);
    assert_eq!(b["card"]["repayments_tyiyn"], 200_000);
    assert_eq!(b["transfer"]["repayments_tyiyn"], 50_000);
    let fee_moves: i64 = moves
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == "bank_fee")
        .map(|m| -m["amount_tyiyn"].as_i64().unwrap())
        .sum();
    assert!(fee_moves > 0);
    assert_eq!(b["fee_tyiyn"], fee_moves);
    assert_eq!(b["outgoing_tyiyn"], 100_000);
    let total = b["card"]["total_tyiyn"].as_i64().unwrap()
        + b["transfer"]["total_tyiyn"].as_i64().unwrap()
        - b["fee_tyiyn"].as_i64().unwrap()
        + b["other_in_tyiyn"].as_i64().unwrap()
        - b["outgoing_tyiyn"].as_i64().unwrap();
    assert_eq!(total, change);
    assert_eq!(
        sum_of(&b["kinds"], "in_tyiyn") - sum_of(&b["kinds"], "out_tyiyn"),
        change
    );
    assert_eq!(sum_of(&b["days"], "net_tyiyn"), change);

    // «Куда сколько поступило» за сегодня: способы оплаты в сумме — выручка отчёта о прибыли.
    let (s, _, profit) = get(&owner, "/api/v1/reports/profit").await;
    assert_eq!(s, StatusCode::OK);
    let revenue = profit["totals"]["goods_tyiyn"].as_i64().unwrap()
        + profit["totals"]["services_tyiyn"].as_i64().unwrap();
    let (s, _, inc) = get(&owner, "/api/v1/reports/income").await;
    assert_eq!(s, StatusCode::OK, "{inc}");
    assert_eq!(inc["revenue_tyiyn"], revenue);
    assert_eq!(sum_of(&inc["methods"], "net_tyiyn"), revenue);
    assert_eq!(sum_of(&inc["methods"], "fee_tyiyn"), {
        let fees: i64 = sqlx::query_scalar(
            "select coalesce(sum(p.fee_tyiyn), 0)::bigint from sale_payments p join sales s on s.id = p.sale_id
             where s.business_date = (now() at time zone 'Asia/Bishkek')::date",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        fees
    });
    let method = |m: &str| {
        inc["methods"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["method"] == m)
            .unwrap()
            .clone()
    };
    assert_eq!(method("card")["net_tyiyn"], 50_000);
    assert_eq!(method("card")["returns_tyiyn"], 50_000);
    assert_eq!(method("transfer")["net_tyiyn"], 100_000);
    assert_eq!(method("cash")["net_tyiyn"], 50_000);
    assert_eq!(method("battery")["net_tyiyn"], 0);
    assert_eq!(inc["returns_tyiyn"], 50_000);
    assert_eq!(sum_of(&inc["categories"], "revenue_tyiyn"), revenue);
    // Кассы: на конец сегодняшнего дня — остаток кассы сейчас.
    for a in inc["accounts"].as_array().unwrap() {
        let id = a["account_id"].as_str().unwrap();
        assert_eq!(a["closing_tyiyn"], balance_of(&pool, id).await, "{a}");
    }
    assert!(
        inc["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["account_id"] == json!(safe))
    );
    // По накладной: продано + осталось = привезли.
    let receipts = inc["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 1);
    let it = &receipts[0]["items"][0];
    assert_eq!(it["qty"], 10);
    assert_eq!(
        it["sold_qty"].as_i64().unwrap() + it["left_qty"].as_i64().unwrap(),
        10
    );
    assert_eq!(it["left_qty"], 5);
    // Администратор: без сейфа владельца и без накладных.
    let (s, _, inc_admin) = get(&admin, "/api/v1/reports/income").await;
    assert_eq!(s, StatusCode::OK);
    assert!(inc_admin["receipts"].is_null());
    assert!(
        !inc_admin["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a["account_id"] == json!(safe))
    );
    assert_eq!(inc_admin["revenue_tyiyn"], revenue);

    // Сводка за сегодня: безнал чеков и те же итоги, что прибыль.
    let (s, _, dash) = get(&owner, "/api/v1/owner/dashboard").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(dash["noncash"]["card_tyiyn"], 50_000);
    assert_eq!(dash["noncash"]["transfer_tyiyn"], 100_000);
    assert_eq!(dash["totals"], profit["totals"]);
    assert_eq!(dash["compare"]["until_now"], true);
    // Прошлый день: те же итоги, что отчёт о прибыли за этот день, сравнение — день целиком.
    let yesterday: String =
        sqlx::query_scalar("select ((now() at time zone 'Asia/Bishkek')::date - 1)::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    let q = format!("?from={yesterday}&to={yesterday}");
    let (s, _, dash) = get(&owner, &format!("/api/v1/owner/dashboard{q}")).await;
    assert_eq!(s, StatusCode::OK, "{dash}");
    let (_, _, profit_y) = get(&owner, &format!("/api/v1/reports/profit{q}")).await;
    assert_eq!(dash["totals"], profit_y["totals"]);
    assert_eq!(dash["totals"]["goods_tyiyn"], 50_000);
    assert_eq!(dash["noncash"]["card_tyiyn"], 50_000);
    assert_eq!(dash["compare"]["until_now"], false);
    assert_eq!(dash["compare"]["today"]["revenue_tyiyn"], 50_000);
    assert!(dash["previous"].is_null());
    // Период: итоги — прибыль за период, сравнение — с предыдущим периодом той же длины.
    let (from, prev_from, prev_to): (String, String, String) = sqlx::query_as(
        "select ((now() at time zone 'Asia/Bishkek')::date - 6)::text,
                ((now() at time zone 'Asia/Bishkek')::date - 13)::text,
                ((now() at time zone 'Asia/Bishkek')::date - 7)::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let (s, _, dash) = get(&owner, &format!("/api/v1/owner/dashboard?from={from}")).await;
    assert_eq!(s, StatusCode::OK, "{dash}");
    let (_, _, profit_w) = get(&owner, &format!("/api/v1/reports/profit?from={from}")).await;
    assert_eq!(dash["totals"], profit_w["totals"]);
    assert_eq!(dash["previous"]["from"], prev_from);
    assert_eq!(dash["previous"]["to"], prev_to);
    assert_eq!(dash["previous"]["revenue_tyiyn"], 0);
    assert!(dash["compare"].is_null());
    assert_eq!(dash["noncash"]["card_tyiyn"], 100_000);
    let (s, _, _) = get(
        &owner,
        "/api/v1/owner/dashboard?from=2024-01-01&to=2026-01-01",
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        get(&admin, &format!("/api/v1/owner/dashboard{q}")).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, _, heat) = get(&owner, &format!("/api/v1/reports/heatmap{q}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(sum_of(&heat, "revenue_tyiyn"), 50_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debts_list_has_overdue_days_last_payment_and_same_numbers(pool: PgPool) {
    // Экран «Долги»: те же суммы и просрочка, что `GET /parties`, плюс дни просрочки и дата
    // последней оплаты (SPEC-27 §6).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "ОсОО Срок", "kind": "company", "inn": "01234567890123" })),
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
    let (_, _, list) = call(&app, "GET", "/api/v1/debts/list", Some(&admin), None).await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(pid))
        .unwrap()
        .clone();
    let today: String =
        sqlx::query_scalar("select ((now() at time zone 'Asia/Bishkek')::date + 13)::text")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row["due_date"], today);
    assert_eq!(row["overdue_days"], 0);
    assert!(row["last_payment_at"].is_null());
    // Долг месячной давности: просрочен на 30 − 13 = 17 дней.
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
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 10_000, "method": "cash",
                     "account_id": owner_safe(&app, &owner).await })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let (s, _, list) = call(&app, "GET", "/api/v1/debts/list", Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, before) = call(
        &app,
        "GET",
        "/api/v1/parties?only_debtors=true",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(
        list.as_array().unwrap().len(),
        before.as_array().unwrap().len()
    );
    for (a, b) in list
        .as_array()
        .unwrap()
        .iter()
        .zip(before.as_array().unwrap())
    {
        assert_eq!(a["id"], b["id"]);
        assert_eq!(a["balance_tyiyn"], b["balance_tyiyn"]);
        assert_eq!(a["overdue_tyiyn"], b["overdue_tyiyn"]);
        assert_eq!(a["credit_limit_tyiyn"], b["credit_limit_tyiyn"]);
    }
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(pid))
        .unwrap();
    assert_eq!(row["balance_tyiyn"], 140_000);
    assert_eq!(row["overdue_tyiyn"], 40_000);
    assert_eq!(row["overdue_days"], 17);
    assert!(row["last_payment_at"].is_string());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_customer_debt_matches_client_card(pool: PgPool) {
    // «💳 Мой долг»: сумма, срок и последние записи — те же, что в карточке клиента (SPEC-27 §7).
    use avtodom_server::api::telegram::Keys;
    use avtodom_server::domain::money::format_som;
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                     "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 100_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "Эрлан", "phone": "+996 555 12-34-56", "inn": "21201199000123", "due_days": 10 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{party}");
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, sale) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier, "party_id": pid,
                     "lines": [{ "kind": "piece", "product_id": item, "qty": 2, "unit_price_tyiyn": 50_000 }],
                     "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 30_000, "method": "card" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let chat = 555_123_456i64;
    let linked = bot(&pool, chat, None, Some("+996555123456")).await;
    assert_eq!(linked.keys, Keys::Customer);
    assert!(linked.text.contains("💳 Мой долг"), "{}", linked.text);
    let reply = bot(&pool, chat, Some("💳 Мой долг"), None).await;
    assert_eq!(reply.keys, Keys::Customer);
    let (_, _, card) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}/card"),
        Some(&owner),
        None,
    )
    .await;
    let balance = card["party"]["balance_tyiyn"].as_i64().unwrap();
    assert_eq!(balance, 70_000);
    assert!(
        reply
            .text
            .contains(&format!("Ваш долг: {}", format_som(balance))),
        "{}",
        reply.text
    );
    for t in card["timeline"].as_array().unwrap() {
        if t["ledger_id"].is_string() {
            let amount = t["amount_tyiyn"].as_i64().unwrap().abs();
            assert!(reply.text.contains(&format_som(amount)), "{}", reply.text);
        }
    }
    assert!(
        reply.text.contains(&format!("чек № {}", sale["number"])),
        "{}",
        reply.text
    );
    let due: String = sqlx::query_scalar(
        "select to_char((now() at time zone 'Asia/Bishkek')::date + 9, 'DD.MM.YYYY')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        reply.text.contains(&format!("Оплатить до {due}")),
        "{}",
        reply.text
    );
    // Словом «долг» — то же.
    assert_eq!(bot(&pool, chat, Some("долг"), None).await.text, reply.text);

    // Клиент без долга видит кнопку и ответ «долга нет».
    let (_, _, other) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "Айбек", "phone": "+996700111222" })),
    )
    .await;
    assert!(other["id"].is_string());
    let chat2 = 700_111_222i64;
    bot(&pool, chat2, None, Some("+996700111222")).await;
    let none = bot(&pool, chat2, Some("💳 Мой долг"), None).await;
    assert!(none.text.contains("Долга нет"), "{}", none.text);
}

/// Как `call`, но с числом строк отбора из заголовка `X-Total-Count` (SPEC-28 §2).
async fn call_total(app: &Router, uri: &str, cookie: &str) -> (StatusCode, Option<i64>, Value) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let total = res
        .headers()
        .get("x-total-count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        total,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_sets_sale_price_with_markup_journal_and_notice(pool: PgPool) {
    // Цена продажи в строке прихода: ставит и администратор, товар получает её при проведении,
    // в журнале и ленте владельца — «было / стало», приход и наценка (SPEC-28 §1, ADR-075).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let filter = product(&app, &owner, "Фильтр наценки", 180_000, 0).await;
    let kept = product(&app, &owner, "Фильтр без новой цены", 50_000, 0).await;
    let (_, _, supplier) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&owner),
        Some(json!({ "name": "Поставщик цен" })),
    )
    .await;
    let receipt = |lines: Value| json!({ "op_id": Uuid::now_v7(), "supplier_id": supplier["id"], "payment": "debt", "lines": lines });

    // Две строки одного товара с разными ценами продажи — ошибка ввода.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(receipt(json!([
            { "product_id": filter, "qty": 1, "cost_tyiyn": 160_000, "sale_price_tyiyn": 200_000 },
            { "product_id": filter, "qty": 1, "cost_tyiyn": 160_000, "sale_price_tyiyn": 210_000 }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(receipt(json!([
            { "product_id": filter, "qty": 1, "cost_tyiyn": 160_000, "sale_price_tyiyn": -1 }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Администратор: 10 шт по 1 600 с, новая цена продажи 2 000 с — наценка 25,0 %.
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&admin),
        Some(receipt(json!([
            { "product_id": filter, "qty": 10, "cost_tyiyn": 1_600_000, "sale_price_tyiyn": 200_000 },
            { "product_id": kept, "qty": 2, "cost_tyiyn": 60_000 }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["lines"][0]["sale_price_tyiyn"], 200_000);
    // Без новой цены строка помнит текущую цену товара.
    assert_eq!(r["lines"][1]["sale_price_tyiyn"], 50_000);
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{filter}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["sale_price_tyiyn"], 200_000);
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{kept}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["sale_price_tyiyn"], 50_000);

    // Журнал: одна смена цены — только у товара, где цена поменялась.
    let changes: Vec<Value> = actions(&pool, "product.prices")
        .await
        .into_iter()
        .filter(|d| d.get("receipt_number").is_some())
        .collect();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0]["old"]["sale"], 180_000);
    assert_eq!(changes[0]["new"]["sale"], 200_000);
    assert_eq!(changes[0]["receipt_number"], r["number"]);
    assert_eq!(changes[0]["markup_permille"], 250);
    let who: String = sqlx::query_scalar(
        "select u.login from audit_log a join users u on u.id = a.user_id
         where a.action = 'product.prices' and a.data ? 'receipt_number'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(who, "admin");

    // Владелец видит это в ленте: было / стало, приход и наценка.
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let item = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Изменена цена: Фильтр наценки")
        .unwrap()
        .clone();
    let details = item["details"].as_str().unwrap().replace('\u{a0}', " ");
    assert!(details.contains("было 1 800 с, стало 2 000 с"), "{details}");
    assert!(
        details.contains(&format!("приход № {}, наценка 25,0 %", r["number"])),
        "{details}"
    );

    // Вне прихода администратор цену по-прежнему не меняет (ADR-032).
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{filter}/prices"),
        Some(&admin),
        Some(json!({ "sale_price_tyiyn": 1 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Та же цена — не смена: в журнале ничего нового.
    let before = actions(&pool, "product.prices").await.len();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(receipt(json!([
            { "product_id": filter, "qty": 1, "cost_tyiyn": 160_000, "sale_price_tyiyn": 200_000 }
        ]))),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(actions(&pool, "product.prices").await.len(), before);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn long_lists_come_in_pages_with_total(pool: PgPool) {
    // Страницы длинных списков: `limit`/`offset`, число строк отбора в `X-Total-Count`; без
    // `limit` — список как раньше (SPEC-28 §2).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    for i in 0..30 {
        let (s, _, _) = call(
            &app,
            "POST",
            "/api/v1/parties",
            Some(&owner),
            Some(json!({ "name": format!("Клиент страниц {i:02}") })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let base = "/api/v1/parties?role=customer&q=%D0%A1%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86";
    let (s, total, page) = call_total(&app, &format!("{base}&limit=25&offset=25"), &owner).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(total, Some(30));
    let names: Vec<&str> = page
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 5);
    assert_eq!(names[0], "Клиент страниц 25");
    let (_, total, all) = call_total(&app, base, &owner).await;
    assert_eq!((total, all.as_array().map(Vec::len)), (Some(30), Some(30)));

    // Расходы: страница и всего; сумма страниц — весь список.
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&owner), None).await;
    let article = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["owner_only"] == json!(false))
        .unwrap()["id"]
        .clone();
    for _ in 0..7 {
        let (s, _, e) = call(
            &app,
            "POST",
            "/api/v1/expenses",
            Some(&owner),
            Some(json!({ "op_id": Uuid::now_v7(), "article_id": article, "amount_tyiyn": 1_000, "source": "account",
                         "account_id": owner_safe(&app, &owner).await })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{e}");
    }
    let mut seen = 0;
    for offset in [0, 3, 6] {
        let (_, total, page) = call_total(
            &app,
            &format!("/api/v1/expenses?limit=3&offset={offset}"),
            &owner,
        )
        .await;
        assert_eq!(total, Some(7));
        seen += page.as_array().unwrap().len();
    }
    assert_eq!(seen, 7);

    // Товары: итоги по всему отбору, строки — только страницы.
    for i in 0..3 {
        product(&app, &owner, &format!("Товар страниц {i}"), 10_000, 0).await;
    }
    let (_, _, all) = call(&app, "GET", "/api/v1/products/list", Some(&owner), None).await;
    let (_, _, list) = call(
        &app,
        "GET",
        "/api/v1/products/list?limit=2&offset=2",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(list["totals"], all["totals"]);
    assert_eq!(list["totals"]["count"], 3);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], all["items"][2]["id"]);

    // Уведомления: всего событий в теле ответа.
    let (_, _, n1) = call(
        &app,
        "GET",
        "/api/v1/notifications?limit=1",
        Some(&owner),
        None,
    )
    .await;
    let (_, _, n2) = call(
        &app,
        "GET",
        "/api/v1/notifications?limit=1&offset=1",
        Some(&owner),
        None,
    )
    .await;
    assert!(n1["total"].as_i64().unwrap() >= n1["items"].as_array().unwrap().len() as i64);
    assert_eq!(n1["total"], n2["total"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn top_products_by_profit_is_owner_only(pool: PgPool) {
    // «Больше всех приносят» — по валовой прибыли, только владельцу; администратору прибыль не
    // отдаётся вовсе (SPEC-28 §4, инвариант 13).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    // Дешёвый, но частый — и дорогой с большой прибылью.
    let cheap = product(&app, &owner, "Частый товар", 10_000, 0).await;
    let rich = product(&app, &owner, "Прибыльный товар", 500_000, 0).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [
                { "product_id": cheap, "qty": 10, "cost_tyiyn": 90_000 },
                { "product_id": rich, "qty": 1, "cost_tyiyn": 200_000 }
            ]
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
                { "kind": "piece", "product_id": cheap, "qty": 10, "unit_price_tyiyn": 10_000 },
                { "kind": "piece", "product_id": rich, "qty": 1, "unit_price_tyiyn": 500_000 }
            ],
            "payments": [{ "method": "cash", "amount_tyiyn": 600_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");

    let (s, _, top) = call(
        &app,
        "GET",
        "/api/v1/reports/top-products?sort=profit",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(top["items"][0]["name"], "Прибыльный товар");
    // 5 000 − 2 000 = 3 000 с; частый: 1 000 − 900 = 100 с.
    assert_eq!(top["items"][0]["profit_tyiyn"], 300_000);
    assert_eq!(top["items"][1]["profit_tyiyn"], 10_000);
    let (_, _, often) = call(
        &app,
        "GET",
        "/api/v1/reports/top-products?sort=qty",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(often["items"][0]["name"], "Частый товар");

    // Администратору — 403 на «по прибыли» и ни одного поля прибыли в обычном списке.
    let (s, _, _) = call(
        &app,
        "GET",
        "/api/v1/reports/top-products?sort=profit",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, top) = call(
        &app,
        "GET",
        "/api/v1/reports/top-products?sort=qty",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    for item in top["items"].as_array().unwrap() {
        assert!(item.get("profit_tyiyn").is_none(), "{item}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn mixed_sale_cash_and_debt_shows_in_repayment_window(pool: PgPool) {
    // Чек частью наличными, остаток — в долг клиента (уже есть, SPEC-10); окно приёма долга
    // показывает неоплаченные чеки с остатком и просрочку (SPEC-28 §3, §5).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр в долг", 100_000, 0).await;
    let (_, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "Нурлан", "inn": "21201199000777", "due_days": 10 })),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let mut numbers = Vec::new();
    for _ in 0..2 {
        let (s, _, sale) = call(
            &app,
            "POST",
            "/api/v1/sales",
            Some(&admin),
            Some(json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier, "party_id": pid,
                "lines": [{ "kind": "piece", "product_id": item, "qty": 3, "unit_price_tyiyn": 100_000 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 100_000 }, { "method": "debt", "amount_tyiyn": 200_000 }]
            })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{sale}");
        assert_eq!(sale["total_tyiyn"], 300_000);
        numbers.push(sale["number"].clone());
    }
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["balance_tyiyn"], 400_000);
    // Наличные части — в кассе.
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let till = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == json!("register"))
        .unwrap()["balance_tyiyn"]
        .clone();
    assert_eq!(till, 200_000);

    // Оплата 300 с гасит старый чек целиком и 100 с нового — в окне остаётся новый, 100 с.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 300_000, "method": "card" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, open) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{pid}/open-debts"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{open}");
    assert_eq!(open["balance_tyiyn"], 100_000);
    let items = open["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{open}");
    assert_eq!(items[0]["sale_number"], numbers[1]);
    assert_eq!(items[0]["amount_tyiyn"], 200_000);
    assert_eq!(items[0]["left_tyiyn"], 100_000);
    assert_eq!(items[0]["overdue"], false);
    assert_eq!(open["overdue_tyiyn"], 0);
}

/// Сумма движений каждой кассы равна её остатку (инвариант 6): ни одной расходящейся кассы.
async fn balances_match_movements(pool: &PgPool) {
    let bad: i64 = sqlx::query_scalar(
        "select count(*) from cash_accounts a where a.balance_tyiyn <>
           coalesce((select sum(m.amount_tyiyn) from cash_movements m where m.account_id = a.id), 0)",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(bad, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn own_accounts_created_edited_archived_with_negative_flag(pool: PgPool) {
    // «Обзор счетов» (SPEC-28 §5, ADR-076): счета заводит и правит только владелец, архив — только
    // с нулевым остатком, «отрицательный баланс» проверяется на каждом расходе.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let req = |cookie: &str, method: &str, uri: &str, body: Option<Value>| {
        let app = app.clone();
        let (cookie, method, uri) = (cookie.to_string(), method.to_string(), uri.to_string());
        async move { call(&app, &method, &uri, Some(&cookie), body).await }
    };
    let create = |cookie: &str, name: &str, kind: &str, negative: bool, owner_only: bool| {
        req(
            cookie,
            "POST",
            "/api/v1/cash/accounts",
            Some(json!({ "op_id": Uuid::now_v7(), "name": name, "kind": kind,
                         "allow_negative": negative, "owner_only": owner_only })),
        )
    };
    // Администратор счета не заводит и не правит.
    assert_eq!(
        create(&admin, "Оптима", "bank", false, false).await.0,
        StatusCode::FORBIDDEN
    );
    let (till, bank, _) = accounts_of(&app, &owner).await;
    assert_eq!(
        req(
            &admin,
            "PATCH",
            &format!("/api/v1/cash/accounts/{bank}"),
            Some(json!({ "name": "Банк" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (s, _, optima) = create(&owner, "ОПТИМА БИЗНЕС", "bank", false, false).await;
    assert_eq!(s, StatusCode::OK, "{optima}");
    assert_eq!(optima["is_default"], false);
    assert_eq!(optima["allow_negative"], false);
    // Второй с тем же названием — 409; вторую кассу смены не заводят — 422.
    assert_eq!(
        create(&owner, "ОПТИМА БИЗНЕС", "bank", false, false)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        create(&owner, "Касса-3", "register", false, false).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (_, _, lead) = create(&owner, "СВИНЕЦ", "other", false, false).await;
    let (_, _, cabinet) = create(&owner, "Кабинет касса", "other", true, true).await;
    let lead = lead["id"].as_str().unwrap().to_string();
    let cabinet = cabinet["id"].as_str().unwrap().to_string();

    // Администратор видит счета без «только владелец»; владелец — все.
    let (_, _, seen) = req(&admin, "GET", "/api/v1/cash/accounts", None).await;
    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(names(&seen).contains(&"СВИНЕЦ".to_string()));
    assert!(!names(&seen).contains(&"Кабинет касса".to_string()));
    let (_, _, all) = req(&owner, "GET", "/api/v1/cash/accounts", None).await;
    assert!(names(&all).contains(&"Кабинет касса".to_string()));

    // Отрицательный баланс не разрешён — 422 с остатком; разрешён — уходит ниже нуля.
    let out = |acc: &str, amount: i64| {
        req(
            &owner,
            "POST",
            "/api/v1/cash/movements",
            Some(
                json!({ "op_id": Uuid::now_v7(), "account_id": acc, "kind": "cash_out",
                         "amount_tyiyn": amount, "comment": "проверка" }),
            ),
        )
    };
    let (s, _, e) = out(&lead, 10_000).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        e.to_string().contains("на счёте «СВИНЕЦ» только 0 с"),
        "{e}"
    );
    let (s, _, e) = out(&cabinet, 10_000).await;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert_eq!(balance_of(&pool, &cabinet).await, -10_000);
    // Перемещение из счёта с разрешённым минусом проходит, в «СВИНЕЦ» — поступление.
    let (s, _, e) = req(
        &owner,
        "POST",
        "/api/v1/cash/transfers",
        Some(json!({ "op_id": Uuid::now_v7(), "from_account_id": cabinet, "to_account_id": lead, "amount_tyiyn": 5_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    assert_eq!(balance_of(&pool, &cabinet).await, -15_000);
    // У кассы смены минус по факту разрешён по умолчанию (ADR-077); владелец может выключить.
    let (_, _, accs) = req(&owner, "GET", "/api/v1/cash/accounts", None).await;
    let till_row = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == json!(till))
        .unwrap()
        .clone();
    assert_eq!(till_row["allow_negative"], true);
    for on in [false, true] {
        let (s, _, t) = req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{till}"),
            Some(json!({ "allow_negative": on })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{t}");
        assert_eq!(t["allow_negative"], on);
    }
    // Новый счёт без признака — минус разрешён.
    let (s, _, plain) = req(
        &owner,
        "POST",
        "/api/v1/cash/accounts",
        Some(json!({ "op_id": Uuid::now_v7(), "name": "Ящик", "kind": "other" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{plain}");
    assert_eq!(plain["allow_negative"], true);

    // Архив — только с нулевым остатком; основной счёт и кассу смены в архив не убрать.
    let archive = |acc: &str| {
        req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{acc}"),
            Some(json!({ "active": false })),
        )
    };
    let (s, _, e) = archive(&lead).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(e.to_string().contains("остаток 50 с"), "{e}");
    assert_eq!(archive(&till).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(archive(&bank).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = req(
        &owner,
        "POST",
        "/api/v1/cash/transfers",
        Some(json!({ "op_id": Uuid::now_v7(), "from_account_id": lead, "to_account_id": cabinet, "amount_tyiyn": 5_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, archived) = archive(&lead).await;
    assert_eq!(s, StatusCode::OK, "{archived}");
    assert_eq!(archived["active"], false);
    // Архивный — не в списке выбора, но в обзоре с архивом и с историей.
    let (_, _, active) = req(&owner, "GET", "/api/v1/cash/accounts", None).await;
    assert!(!names(&active).contains(&"СВИНЕЦ".to_string()));
    let (_, _, with_archive) =
        req(&owner, "GET", "/api/v1/cash/accounts?archived=true", None).await;
    assert!(names(&with_archive).contains(&"СВИНЕЦ".to_string()));
    let (s, _, moves) = req(
        &owner,
        "GET",
        &format!("/api/v1/cash/accounts/{lead}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(moves.as_array().unwrap().len(), 2);
    // Из архивного не платят.
    assert_eq!(out(&lead, 1).await.0, StatusCode::UNPROCESSABLE_ENTITY);

    // Правка названия — в журнал «было / стало»; счёт по умолчанию для карты меняется выбором
    // другого, для перевода остаётся прежний (ADR-077).
    let opt = optima["id"].as_str().unwrap().to_string();
    let (s, _, renamed) = req(
        &owner,
        "PATCH",
        &format!("/api/v1/cash/accounts/{opt}"),
        Some(json!({ "name": "Оптима", "default_card": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{renamed}");
    assert_eq!(renamed["default_card"], true);
    assert_eq!(renamed["default_transfer"], false);
    assert_eq!(renamed["is_default"], false);
    let (_, _, accs) = req(&owner, "GET", "/api/v1/cash/accounts", None).await;
    let old_bank = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == json!(bank))
        .unwrap()
        .clone();
    assert_eq!(old_bank["default_card"], false);
    assert_eq!(old_bank["default_transfer"], true);
    // Отметку «по умолчанию» получает только безналичный счёт.
    assert_eq!(
        req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{cabinet}"),
            Some(json!({ "default_transfer": true }))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let audit: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'cash.account_update' and entity_id = $1::uuid order by created_at desc limit 1",
    )
    .bind(&opt)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit["before"]["name"], "ОПТИМА БИЗНЕС");
    assert_eq!(audit["after"]["name"], "Оптима");
    assert_eq!(audit["changes"]["default_card"]["new"], true);
    // «Только владелец» счёту по умолчанию нельзя; снять отметку напрямую — тоже.
    assert_eq!(
        req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{opt}"),
            Some(json!({ "owner_only": true }))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{opt}"),
            Some(json!({ "default_card": false }))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Прежний счёт — по умолчанию для перевода: в архив не убрать.
    assert_eq!(archive(&bank).await.0, StatusCode::UNPROCESSABLE_ENTITY);
    // Итоги истории за период сходятся с остатком: на конец = сумма движений.
    let (s, _, sum) = req(
        &owner,
        "GET",
        &format!("/api/v1/cash/accounts/{cabinet}/summary"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sum}");
    assert_eq!(sum["closing_tyiyn"], balance_of(&pool, &cabinet).await);
    // Сейф владельца и его история администратору не видны.
    assert_eq!(
        req(
            &admin,
            "GET",
            &format!("/api/v1/cash/accounts/{cabinet}/summary"),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    balances_match_movements(&pool).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn card_sale_goes_to_chosen_bank_with_fee_and_refund_returns_there(pool: PgPool) {
    // Кассир выбирает банк для карты и перевода (ADR-076): деньги и комиссия — на этот счёт,
    // возврат — с него же; чек без сети с архивным счётом ложится на основной с отметкой владельцу.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let req = |cookie: &str, method: &str, uri: &str, body: Option<Value>| {
        let app = app.clone();
        let (cookie, method, uri) = (cookie.to_string(), method.to_string(), uri.to_string());
        async move { call(&app, &method, &uri, Some(&cookie), body).await }
    };
    let (s, _, _) = req(
        &owner,
        "POST",
        "/api/v1/receipts",
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                     "lines": [{ "product_id": item, "qty": 20, "cost_tyiyn": 10_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, bank, _) = accounts_of(&app, &owner).await;
    let new_bank = |name: &str, owner_only: bool| {
        req(
            &owner,
            "POST",
            "/api/v1/cash/accounts",
            Some(
                json!({ "op_id": Uuid::now_v7(), "name": name, "kind": "bank", "owner_only": owner_only }),
            ),
        )
    };
    let (_, _, optima) = new_bank("Оптима", false).await;
    assert_eq!(optima["fee_rate_bp"], 0);
    let optima = optima["id"].as_str().unwrap().to_string();
    // Новый банк — 0 %; ставку терминала владелец ставит в карточке счёта (ADR-077).
    let (s, _, o) = req(
        &owner,
        "PATCH",
        &format!("/api/v1/cash/accounts/{optima}"),
        Some(json!({ "fee_rate_bp": 50 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{o}");
    assert_eq!(o["fee_rate_bp"], 50);
    let sale = |pays: Value, qty: i64| {
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": item, "qty": qty, "unit_price_tyiyn": 50_000 }],
                "payments": pays })
    };
    // Картой 100 000 на «Оптиму»: комиссия 0,5 % — там же, «Счёт» не тронут.
    let (s, _, sold) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "card", "amount_tyiyn": 100_000, "account_id": optima }]),
            2,
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sold}");
    assert_eq!(sold["payments"][0]["account_id"], json!(optima));
    assert_eq!(sold["payments"][0]["account_name"], "Оптима");
    assert_eq!(balance_of(&pool, &optima).await, 99_500);
    assert_eq!(balance_of(&pool, &bank).await, 0);
    // Перевод без выбора — на счёт по умолчанию для перевода, «Счёт».
    let (s, _, _) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "transfer", "amount_tyiyn": 50_000 }]),
            1,
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(balance_of(&pool, &bank).await, 49_750);

    // «Счёт» по выбранному банку и по основному; сводка и смена — по счетам.
    let (s, _, b) = req(
        &owner,
        "GET",
        &format!("/api/v1/cash/bank?account_id={optima}"),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["name"], "Оптима");
    assert_eq!(b["card"]["sales_tyiyn"], 100_000);
    assert_eq!(b["transfer"]["sales_tyiyn"], 0);
    assert_eq!(b["fee_tyiyn"], 500);
    assert_eq!(b["closing_tyiyn"], 99_500);
    let (_, _, main) = req(&owner, "GET", "/api/v1/cash/bank", None).await;
    assert_eq!(main["card"]["sales_tyiyn"], 0);
    assert_eq!(main["transfer"]["sales_tyiyn"], 50_000);
    let (_, _, dash) = req(&owner, "GET", "/api/v1/owner/dashboard", None).await;
    let split = dash["noncash"]["by_account"].as_array().unwrap();
    assert_eq!(split.len(), 2, "{dash}");
    let of = |name: &str| split.iter().find(|a| a["name"] == name).unwrap().clone();
    assert_eq!(of("Оптима")["card_tyiyn"], 100_000);
    assert_eq!(of("Оптима")["fee_tyiyn"], 500);
    assert_eq!(of("Счёт")["transfer_tyiyn"], 50_000);
    assert_eq!(dash["noncash"]["total_tyiyn"], 150_000);
    let (_, _, income) = req(&owner, "GET", "/api/v1/reports/income", None).await;
    let flows = income["accounts"].as_array().unwrap();
    let flow = flows.iter().find(|a| a["name"] == "Оптима").unwrap();
    assert_eq!(flow["closing_tyiyn"], 99_500);

    // Возврат на карту — с того счёта, куда пришли деньги, даже если касса прислала другой;
    // комиссия возвращается туда же.
    let (s, _, back) = req(
        &admin,
        "POST",
        &format!("/api/v1/sales/{}/return", sold["id"].as_str().unwrap()),
        Some(json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                     "payments": [{ "method": "card", "amount_tyiyn": 50_000, "account_id": bank }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["payments"][0]["account_id"], json!(optima));
    assert_eq!(balance_of(&pool, &optima).await, 99_500 - 50_000 + 250);
    assert_eq!(balance_of(&pool, &bank).await, 49_750);

    // Погашение долга картой на выбранный банк — с комиссией на нём же.
    let (_, _, party) = req(
        &owner,
        "POST",
        "/api/v1/parties",
        Some(json!({ "name": "ОсОО Должник", "kind": "company", "inn": "01234567890123" })),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, _) = req(
        &owner,
        "POST",
        "/api/v1/debts/adjust",
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 100_000, "comment": "из тетради" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let before = balance_of(&pool, &optima).await;
    let (s, _, r) = req(
        &admin,
        "POST",
        "/api/v1/debts/repayments",
        Some(
            json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 20_000,
                     "method": "card", "account_id": optima }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(balance_of(&pool, &optima).await, before + 20_000 - 100);

    // Счёт «только владелец» администратор выбрать не может; архивный — тоже.
    let (_, _, private) = new_bank("Банк владельца", true).await;
    let private = private["id"].as_str().unwrap().to_string();
    let (s, _, e) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "card", "amount_tyiyn": 50_000, "account_id": private }]),
            1,
        )),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let (_, _, gone) = new_bank("МБИЗНЕС", false).await;
    let gone = gone["id"].as_str().unwrap().to_string();
    let (s, _, _) = req(
        &owner,
        "PATCH",
        &format!("/api/v1/cash/accounts/{gone}"),
        Some(json!({ "active": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "card", "amount_tyiyn": 50_000, "account_id": gone }]),
            1,
        )),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Чек без сети с архивным счётом не теряется: деньги — на основной, владельцу — причина.
    let mut offline = sale(
        json!([{ "method": "card", "amount_tyiyn": 50_000, "account_id": gone }]),
        1,
    );
    offline["offline"] = json!(true);
    offline["client_time"] =
        json!((chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339());
    let main_before = balance_of(&pool, &bank).await;
    let (s, _, off) = req(&admin, "POST", "/api/v1/sales", Some(offline)).await;
    assert_eq!(s, StatusCode::OK, "{off}");
    assert_eq!(off["payments"][0]["account_id"], json!(bank));
    assert_eq!(balance_of(&pool, &bank).await, main_before + 50_000 - 250);
    assert_eq!(balance_of(&pool, &gone).await, 0);
    let reasons: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
    )
    .bind(off["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        reasons.to_string().contains("счёт «МБИЗНЕС» в архиве"),
        "{reasons}"
    );
    // Снимок кассы без сети знает действующие безналичные счета, без архивных и чужих.
    let (_, _, snap) = req(&admin, "GET", "/api/v1/offline/snapshot", None).await;
    let banks: Vec<&str> = snap["bank_accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(banks, vec!["Счёт", "Оптима"]);
    balances_match_movements(&pool).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expense_over_balance_goes_negative_with_one_notice(pool: PgPool) {
    // Деньги уходят в минус, когда так было на деле (ADR-077): расход больше остатка кассы
    // проводится, владельцу — одно уведомление на переход через ноль; признак выключен — 422.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let (till, _, _) = accounts_of(&app, &owner).await;
    let (s, _, shift) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{shift}");
    let cash_in = |amount: i64| {
        let app = app.clone();
        let admin = admin.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/v1/cash/movements",
                Some(&admin),
                Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": amount, "comment": "размен" })),
            )
            .await
            .0
        }
    };
    assert_eq!(cash_in(100_000).await, StatusCode::OK);
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&admin), None).await;
    let household = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == json!("Хозтовары"))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let expense = |amount: i64| {
        let app = app.clone();
        let admin = admin.clone();
        let body = json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": amount, "comment": "ремонт" });
        async move { call(&app, "POST", "/api/v1/expenses", Some(&admin), Some(body)).await }
    };
    // В кассе 1 000 с, расход 5 000 с — проводится, касса −4 000 с.
    let (s, _, exp) = expense(500_000).await;
    assert_eq!(s, StatusCode::OK, "{exp}");
    assert_eq!(balance_of(&pool, &till).await, -400_000);
    let number: i64 = sqlx::query_scalar("select number from expenses where id = $1::uuid")
        .bind(exp["id"].as_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    // Ещё расход ниже нуля — уведомление не повторяется.
    assert_eq!(expense(10_000).await.0, StatusCode::OK);
    assert_eq!(actions(&pool, "cash.negative").await.len(), 1);
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let titles: Vec<&str> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["title"].as_str().unwrap())
        .collect();
    let want = format!("Счёт «Касса» ушёл в минус: −4 000 с (расход № {number})");
    assert!(titles.contains(&want.as_str()), "{titles:?}");

    // Закрытие смены с отрицательным «должно быть»: пересчитали 0 — излишек 4 100 с.
    let sid = shift["id"].as_str().unwrap();
    let (s, _, closed) = call(
        &app,
        "POST",
        &format!("/api/v1/shifts/{sid}/close"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 0, "comment": "расход из своих" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{closed}");
    let close: (i64, i64, i64) = sqlx::query_as(
        "select expected_tyiyn, counted_tyiyn, diff_tyiyn from shift_closes where shift_id = $1::uuid",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(close, (-410_000, 0, 410_000));
    assert_eq!(balance_of(&pool, &till).await, 0);
    // Сдать в сейф больше, чем лежит в кассе, нельзя.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/shifts/{sid}/handover"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 1_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Новая смена: снова в минус — новое уведомление (новый переход через ноль).
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(expense(1_000).await.0, StatusCode::OK);
    assert_eq!(actions(&pool, "cash.negative").await.len(), 2);

    // Владелец выключил минус у кассы — расход сверх остатка 422, касса не тронута.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/cash/accounts/{till}"),
        Some(&owner),
        Some(json!({ "allow_negative": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, e) = expense(1_000).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(
        e.to_string().contains("на счёте «Касса» только −10 с"),
        "{e}"
    );
    assert_eq!(balance_of(&pool, &till).await, -1_000);
    balances_match_movements(&pool).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bank_fee_per_account_and_default_bank_per_method(pool: PgPool) {
    // Комиссия — по ставке счёта, куда пришли деньги; карта и перевод по умолчанию идут на свои
    // счета, снимок кассы знает обе отметки, погашение долга — тоже (ADR-077).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let req = |cookie: &str, method: &str, uri: &str, body: Option<Value>| {
        let app = app.clone();
        let (cookie, method, uri) = (cookie.to_string(), method.to_string(), uri.to_string());
        async move { call(&app, &method, &uri, Some(&cookie), body).await }
    };
    let (s, _, _) = req(
        &owner,
        "POST",
        "/api/v1/receipts",
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                     "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 10_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, old_bank, _) = accounts_of(&app, &owner).await;
    // «Оптима» — терминал с комиссией 1 % сразу при заведении, «Карта владельца» — 0 %.
    let (s, _, optima) = req(
        &owner,
        "POST",
        "/api/v1/cash/accounts",
        Some(json!({ "op_id": Uuid::now_v7(), "name": "Оптима", "kind": "bank", "fee_rate_bp": 100 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{optima}");
    assert_eq!(optima["fee_rate_bp"], 100);
    assert_eq!(optima["default_card"], false);
    let optima = optima["id"].as_str().unwrap().to_string();
    let (_, _, card) = req(
        &owner,
        "POST",
        "/api/v1/cash/accounts",
        Some(json!({ "op_id": Uuid::now_v7(), "name": "Карта владельца", "kind": "bank" })),
    )
    .await;
    assert_eq!(card["fee_rate_bp"], 0);
    let owner_card = card["id"].as_str().unwrap().to_string();
    for (acc, mark) in [(&optima, "default_card"), (&owner_card, "default_transfer")] {
        let (s, _, a) = req(
            &owner,
            "PATCH",
            &format!("/api/v1/cash/accounts/{acc}"),
            Some(json!({ mark: true })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{a}");
        assert_eq!(a[mark], true);
    }
    let sale = |pays: Value| {
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
                "payments": pays })
    };
    // Картой без выбора — на «Оптиму» с её 1 %; переводом — на карту владельца без комиссии.
    let (s, _, by_card) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(json!([{ "method": "card", "amount_tyiyn": 50_000 }]))),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{by_card}");
    assert_eq!(by_card["payments"][0]["account_id"], json!(optima));
    assert_eq!(balance_of(&pool, &optima).await, 49_500);
    let (s, _, by_transfer) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "transfer", "amount_tyiyn": 50_000 }]),
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{by_transfer}");
    assert_eq!(by_transfer["payments"][0]["account_id"], json!(owner_card));
    assert_eq!(balance_of(&pool, &owner_card).await, 50_000);
    // Карта на выбранный другой банк — по ставке того банка («Счёт», 0,5 %).
    let (s, _, chosen) = req(
        &admin,
        "POST",
        "/api/v1/sales",
        Some(sale(
            json!([{ "method": "card", "amount_tyiyn": 50_000, "account_id": old_bank }]),
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{chosen}");
    assert_eq!(balance_of(&pool, &old_bank).await, 49_750);
    let fees: Vec<(String, i64)> = sqlx::query_as(
        "select a.name, p.fee_tyiyn from sale_payments p join cash_accounts a on a.id = p.account_id
         order by p.id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(fees.contains(&("Оптима".into(), 500)), "{fees:?}");
    assert!(fees.contains(&("Карта владельца".into(), 0)), "{fees:?}");
    assert!(fees.contains(&("Счёт".into(), 250)), "{fees:?}");

    // Погашение долга переводом без выбора — на счёт по умолчанию для перевода, без комиссии;
    // картой — на «Оптиму» с 1 %.
    let (_, _, party) = req(
        &owner,
        "POST",
        "/api/v1/parties",
        Some(json!({ "name": "ОсОО Должник", "kind": "company", "inn": "01234567890123" })),
    )
    .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let (s, _, _) = req(
        &owner,
        "POST",
        "/api/v1/debts/adjust",
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 100_000, "comment": "из тетради" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    for method in ["transfer", "card"] {
        let (s, _, r) = req(
            &admin,
            "POST",
            "/api/v1/debts/repayments",
            Some(json!({ "op_id": Uuid::now_v7(), "party_id": pid, "amount_tyiyn": 20_000, "method": method })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    assert_eq!(balance_of(&pool, &owner_card).await, 70_000);
    assert_eq!(balance_of(&pool, &optima).await, 49_500 + 20_000 - 200);

    // Снимок кассы без сети: обе отметки; без сети касса выбирает тот же счёт.
    let (_, _, snap) = req(&admin, "GET", "/api/v1/offline/snapshot", None).await;
    let banks = snap["bank_accounts"].as_array().unwrap();
    let flag = |name: &str, key: &str| {
        banks.iter().find(|b| b["name"] == name).unwrap()[key]
            .as_bool()
            .unwrap()
    };
    assert!(flag("Оптима", "default_card") && !flag("Оптима", "default_transfer"));
    assert!(
        flag("Карта владельца", "default_transfer") && !flag("Карта владельца", "default_card")
    );
    assert!(!flag("Счёт", "default_card") && !flag("Счёт", "default_transfer"));
    // Счёт по умолчанию в архив не убрать и «только владелец» не сделать.
    for body in [json!({ "active": false }), json!({ "owner_only": true })] {
        assert_eq!(
            req(
                &owner,
                "PATCH",
                &format!("/api/v1/cash/accounts/{owner_card}"),
                Some(body)
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    // В журнале — смена отметок и ставка «Оптимы» при заведении.
    let fee_notes = actions(&pool, "settings.payment_fee").await;
    assert_eq!(fee_notes.len(), 1);
    assert_eq!(fee_notes[0]["account"], "Оптима");
    balances_match_movements(&pool).await;
}

// ---------- SPEC-29: доработки по проверке на точке, часть 3 ----------

/// Запрос с телом как есть (фото накладной): тип файла — в заголовке.
async fn call_raw(
    app: &Router,
    uri: &str,
    cookie: &str,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Скачать файл: статус, тип и байты.
async fn get_raw(app: &Router, uri: &str, cookie: &str) -> (StatusCode, String, Vec<u8>) {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ct, bytes)
}

async fn new_supplier(app: &Router, owner: &str, name: &str) -> String {
    let (s, _, sup) = call(
        app,
        "POST",
        "/api/v1/suppliers",
        Some(owner),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sup}");
    sup["id"].as_str().unwrap().to_string()
}

async fn stock_of(pool: &PgPool, product: &str) -> (i64, i64) {
    sqlx::query_as(
        "select stock_qty, stock_value_tyiyn from branch_products where product_id = $1::uuid",
    )
    .bind(product)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn assert_stock_matches_movements(app: &Router, owner: &str) {
    let (s, _, bad) = call(app, "GET", "/api/v1/stock/verify", Some(owner), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bad, json!([]), "остаток разошёлся с движениями");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_part_paid_now_rest_on_debt(pool: PgPool) {
    // Приход: часть оплачена сразу из выбранного счёта, остаток — долг поставщику (SPEC-29 §4).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (till, bank, safe) = accounts_of(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let sid = new_supplier(&app, &owner, "Автоснаб").await;
    let receipt = |who: &str, body: Value| {
        let app = app.clone();
        let who = who.to_string();
        async move { call(&app, "POST", "/api/v1/receipts", Some(&who), Some(body)).await }
    };
    let body = |paid_now: i64, from: &str| {
        json!({
            "op_id": Uuid::now_v7(), "supplier_id": sid, "payment": "debt",
            "paid_now_tyiyn": paid_now, "paid_from": from,
            "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 100_000 }]
        })
    };
    // Часть — больше нуля и меньше суммы; у оплаченной сразу части не бывает.
    for bad in [100_000, 150_000, -1] {
        let (s, _, e) = receipt(&owner, body(bad, &bank)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {e}");
    }
    let (s, _, _) = receipt(
        &owner,
        json!({ "op_id": Uuid::now_v7(), "payment": "paid", "paid_from": bank, "paid_now_tyiyn": 1_000,
                "lines": [{ "product_id": item, "qty": 1, "cost_tyiyn": 10_000 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // «Из своих» — 422 и здесь.
    let (s, _, _) = receipt(&owner, body(30_000, "outside")).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Владелец: 30 000 со «Счёта», 70 000 — в долг.
    let (s, _, r) = receipt(&owner, body(30_000, &bank)).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["debt"]["status"], "partial");
    assert_eq!(r["debt"]["paid_tyiyn"], 30_000);
    assert_eq!(r["debt"]["left_tyiyn"], 70_000);
    assert_eq!(r["debt"]["payments"][0]["source"], "Счёт");
    assert_eq!(balance_of(&pool, &bank).await, -30_000);
    let (_, _, party) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(party["balance_tyiyn"], -70_000);
    // Администратор: только из кассы смены, и только в открытой смене.
    let (s, _, _) = receipt(&admin, body(10_000, &safe)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = receipt(&admin, body(10_000, &till)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let cashier = cashier_employee(&app, &owner).await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, r2) = receipt(&admin, body(10_000, &till)).await;
    assert_eq!(s, StatusCode::OK, "{r2}");
    assert_eq!(r2["debt"]["left_tyiyn"], 90_000);
    assert_eq!(balance_of(&pool, &till).await, -10_000);
    // Сторно частично оплаченной: долг снят, оплата осталась авансом поставщику.
    let rid = r["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/receipts/{rid}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, party) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(party["balance_tyiyn"], -60_000);
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn outside_money_is_refused_everywhere(pool: PgPool) {
    // «Из своих, мимо касс» убрано (ADR-079): расход, выплата и погашение — 422, история читается.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&owner), None).await;
    let article = articles.as_array().unwrap()[0]["id"].clone();
    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "article_id": article, "amount_tyiyn": 1_000, "source": "outside" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let (_, _, emp) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Мастер", "is_master": true })),
    )
    .await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/payouts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": emp["id"], "amount_tyiyn": 1_000,
                     "source": "outside", "advance": true }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let sid = new_supplier(&app, &owner, "Поставщик").await;
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": sid, "amount_tyiyn": 1_000, "method": "outside" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn invoice_photos_are_kept_and_hidden_only_by_owner(pool: PgPool) {
    // Фото накладной: загрузка, просмотр, повтор без дубля, скрытие владельцем (SPEC-29 §3, ADR-080).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
                     "lines": [{ "product_id": item, "qty": 1, "cost_tyiyn": 10_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let rid = r["id"].as_str().unwrap().to_string();
    let jpeg: Vec<u8> = [&[0xFF, 0xD8, 0xFF, 0xE0][..], &[7u8; 2000][..]].concat();
    let op = Uuid::now_v7();
    let uri = format!("/api/v1/receipts/{rid}/files?op_id={op}&name=page1.jpg");
    let (s, f) = call_raw(&app, &uri, &admin, "image/jpeg", jpeg.clone()).await;
    assert_eq!(s, StatusCode::OK, "{f}");
    assert_eq!(f["content_type"], "image/jpeg");
    assert_eq!(f["size_bytes"], 2004);
    // Повтор той же загрузки — тот же файл.
    let (s, again) = call_raw(&app, &uri, &admin, "image/jpeg", jpeg.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], f["id"]);
    // Не снимок и не PDF, или тип не совпал с содержимым — 422; больше 5 МБ — отказ.
    let bad = format!("/api/v1/receipts/{rid}/files?op_id={}", Uuid::now_v7());
    let (s, _) = call_raw(&app, &bad, &owner, "text/html", b"<html></html>".to_vec()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = call_raw(&app, &bad, &owner, "application/pdf", jpeg.clone()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let big: Vec<u8> = [
        &[0xFF, 0xD8, 0xFF][..],
        &vec![0u8; 5 * 1024 * 1024 + 10][..],
    ]
    .concat();
    let (s, _) = call_raw(&app, &bad, &owner, "image/jpeg", big).await;
    assert!(s.is_client_error(), "{s}");
    let pdf = format!(
        "/api/v1/receipts/{rid}/files?op_id={}&name=scan.pdf",
        Uuid::now_v7()
    );
    let (s, p) = call_raw(
        &app,
        &pdf,
        &owner,
        "application/pdf",
        b"%PDF-1.4 test".to_vec(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    // Список и скачивание: байты те же, тип — из содержимого.
    let (_, _, list) = call(
        &app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let fid = f["id"].as_str().unwrap();
    let (s, ct, bytes) = get_raw(&app, &format!("/api/v1/receipt-files/{fid}"), &admin).await;
    assert_eq!((s, ct.as_str()), (StatusCode::OK, "image/jpeg"));
    assert_eq!(bytes, jpeg);
    // Скрыть — только владелец и с причиной; запись остаётся, администратор её не видит.
    let hide = format!("/api/v1/receipt-files/{fid}/hide");
    let (s, _, _) = call(
        &app,
        "POST",
        &hide,
        Some(&admin),
        Some(json!({ "reason": "не та" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "POST",
        &hide,
        Some(&owner),
        Some(json!({ "reason": " " })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, h) = call(
        &app,
        "POST",
        &hide,
        Some(&owner),
        Some(json!({ "reason": "не та накладная" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{h}");
    assert_eq!(h["hidden_reason"], "не та накладная");
    // Повтор (потерянный ответ) — тот же результат, причина не меняется (инвариант 4, ADR-090).
    let (s, _, again) = call(
        &app,
        "POST",
        &hide,
        Some(&owner),
        Some(json!({ "reason": "ещё раз" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["hidden_reason"], "не та накладная");
    let (_, _, list) = call(
        &app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let (_, _, list) = call(
        &app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let (s, _, _) = get_raw(&app, &format!("/api/v1/receipt-files/{fid}"), &admin).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Файл не удаляется и не правится даже в базе.
    assert!(
        sqlx::query("delete from receipt_files")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update receipt_files set name = 'x'")
            .execute(&pool)
            .await
            .is_err()
    );
    // Сторно прихода — без своих фото: прикрепляют к самой накладной.
    let (_, _, rev) = call(
        &app,
        "POST",
        &format!("/api/v1/receipts/{rid}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    let rev_id = rev["id"].as_str().unwrap();
    let uri = format!("/api/v1/receipts/{rev_id}/files?op_id={}", Uuid::now_v7());
    let (s, _) = call_raw(&app, &uri, &owner, "image/jpeg", jpeg).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(actions(&pool, "receipt.file_hide").await.len(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stock_in_without_supplier_and_its_storno(pool: PgPool) {
    // Оприходование без поставщика: количество и цена закупки, комментарий обязателен (SPEC-29 §8).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let body = |qty: i64, price: i64, comment: &str| json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": qty, "price_tyiyn": price, "comment": comment });
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/stock-ins",
        Some(&admin),
        Some(body(3, 20_000, "  ")),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/stock-ins",
        Some(&admin),
        Some(body(0, 20_000, "нашли")),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, si) = call(
        &app,
        "POST",
        "/api/v1/stock-ins",
        Some(&admin),
        Some(body(3, 20_000, "принёс владелец со склада")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{si}");
    assert_eq!(
        (si["number"].as_i64(), si["cost_tyiyn"].as_i64()),
        (Some(1), Some(60_000))
    );
    assert_eq!(stock_of(&pool, &item).await, (3, 60_000));
    // Цена закупки видна в товаре обеим ролям (ADR-008); владельцу — уведомление.
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{item}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["last_purchase_price_tyiyn"], 20_000);
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["action"] == "stock_in.post"
                && i["title"].as_str().unwrap().contains("Фильтр")),
        "{list}"
    );
    // Непроданное администратор сторнирует сам.
    let id = si["id"].as_str().unwrap();
    let rev = |who: &str, comment: &str| {
        let app = app.clone();
        let who = who.to_string();
        let uri = format!("/api/v1/stock-ins/{id}/reverse");
        let body = json!({ "op_id": Uuid::now_v7(), "comment": comment });
        async move { call(&app, "POST", &uri, Some(&who), Some(body)).await }
    };
    let (s, _, _) = rev(&admin, "").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, back) = rev(&admin, "ошиблись количеством").await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["lines"][0]["qty"], -3);
    assert_eq!(back["reversal_of"], si["id"]);
    assert_eq!(stock_of(&pool, &item).await, (0, 0));
    let (s, _, _) = rev(&owner, "ещё раз").await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Оприходование и его сторно не правятся.
    for sql in [
        "update stock_ins set comment = 'x'",
        "update stock_in_lines set qty = 1",
        "delete from stock_in_lines",
    ] {
        assert!(sqlx::query(sql).execute(&pool).await.is_err(), "{sql}");
    }
    // Часть ушла со склада (недостача) — сторно уведёт остаток в минус: только владелец.
    let (_, _, si2) = call(
        &app,
        "POST",
        "/api/v1/stock-ins",
        Some(&owner),
        Some(body(2, 20_000, "нашли на складе")),
    )
    .await;
    sqlx::query(
        "insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
         select $1, branch_id, product_id, -1, -20000, 'revision', $1 from branch_products where product_id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&item)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update branch_products set stock_qty = stock_qty - 1, stock_value_tyiyn = stock_value_tyiyn - 20000 where product_id = $1::uuid")
        .bind(&item)
        .execute(&pool)
        .await
        .unwrap();
    let id2 = si2["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/stock-ins/{id2}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/stock-ins/{id2}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(stock_of(&pool, &item).await.0, -1);
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_return_reduces_debt_or_brings_money(pool: PgPool) {
    // Возврат поставщику, не брак: по средней себестоимости, итог — долг или деньги, разница —
    // в прибыль; сторно возвращает всё (SPEC-29 §9).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (till, bank, safe) = accounts_of(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let sid = new_supplier(&app, &owner, "Автоснаб").await;
    let other = new_supplier(&app, &owner, "Другой").await;
    // 10 шт в долг Автоснабу по 1 000 с, 10 шт другому по 2 000 с: средняя 1 500 с.
    for (sup, cost) in [(&sid, 100_000), (&other, 200_000)] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/receipts",
            Some(&owner),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": sup, "payment": "debt",
                         "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": cost }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    let (_, _, hint) = call(
        &app,
        "GET",
        &format!("/api/v1/supplier-returns/price?supplier_id={sid}&product_id={item}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(hint["price_tyiyn"], 10_000);
    let ret = |who: &str, body: Value| {
        let app = app.clone();
        let who = who.to_string();
        async move {
            call(
                &app,
                "POST",
                "/api/v1/supplier-returns",
                Some(&who),
                Some(body),
            )
            .await
        }
    };
    // Больше, чем на складе, — 422.
    let (s, _, _) = ret(
        &admin,
        json!({ "op_id": Uuid::now_v7(), "supplier_id": sid, "settle": "debt",
                "lines": [{ "product_id": item, "qty": 21 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Администратор: 4 шт в счёт долга по цене последнего прихода — 4 000 с; себестоимость 6 000 с.
    let (s, _, r) = ret(
        &admin,
        json!({ "op_id": Uuid::now_v7(), "supplier_id": sid, "settle": "debt", "comment": "не пошёл",
                "lines": [{ "product_id": item, "qty": 4 }] }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["amount_tyiyn"], 40_000);
    assert!(
        r.get("cost_tyiyn").is_none() && r.get("diff_tyiyn").is_none(),
        "{r}"
    );
    assert!(r["lines"][0].get("cost_tyiyn").is_none());
    assert_eq!(stock_of(&pool, &item).await, (16, 240_000));
    let (_, _, party) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(party["balance_tyiyn"], -60_000);
    let (_, _, inv) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}/invoices"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(inv["invoices"][0]["status"], "partial");
    assert_eq!(inv["payments"][0]["kind"], "return");
    let (_, _, full) = call(
        &app,
        "GET",
        &format!("/api/v1/supplier-returns/{}", r["id"].as_str().unwrap()),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(
        (full["cost_tyiyn"].as_i64(), full["diff_tyiyn"].as_i64()),
        (Some(60_000), Some(-20_000))
    );
    // Деньгами: администратору — только касса смены; владелец — на «Счёт» с договорной суммой.
    let money = |acc: &str, amount: i64| {
        json!({ "op_id": Uuid::now_v7(), "supplier_id": other, "settle": "money", "account_id": acc,
                "lines": [{ "product_id": item, "qty": 2, "amount_tyiyn": amount }] })
    };
    let (s, _, _) = ret(&admin, money(&safe, 40_000)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = ret(&admin, money(&till, 40_000)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _, m) = ret(&owner, money(&bank, 40_000)).await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["diff_tyiyn"], 10_000);
    assert_eq!(balance_of(&pool, &bank).await, 40_000);
    // Прибыль: −20 000 + 10 000; поставки поставщика показывают «Вернули».
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["return_diff_tyiyn"], -10_000);
    assert_eq!(profit["totals"]["net_tyiyn"], -10_000);
    let (_, _, d) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}/deliveries"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(d["rows"][0]["returned_qty"], 4);
    // Сторно: товар назад по той же стоимости, долг и деньги — обратно, разница снята.
    let rid = r["id"].as_str().unwrap();
    let (s, _, back) = call(
        &app,
        "POST",
        &format!("/api/v1/supplier-returns/{rid}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "поставщик не принял" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    let (_, _, party) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(party["balance_tyiyn"], -100_000);
    let (_, _, inv) = call(
        &app,
        "GET",
        &format!("/api/v1/parties/{sid}/invoices"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(inv["invoices"][0]["status"], "unpaid");
    let mid = m["id"].as_str().unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/supplier-returns/{mid}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/supplier-returns/{mid}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(balance_of(&pool, &bank).await, 0);
    let (_, _, profit) = call(&app, "GET", "/api/v1/reports/profit", Some(&owner), None).await;
    assert_eq!(profit["totals"]["return_diff_tyiyn"], 0);
    assert_eq!(stock_of(&pool, &item).await.0, 20);
    let (_, _, d) = call(
        &app,
        "GET",
        &format!("/api/v1/suppliers/{sid}/deliveries"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(d["rows"][0]["returned_qty"], 0);
    assert_eq!(actions(&pool, "supplier_return.post").await.len(), 2);
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_takes_a_product_not_in_the_program(pool: PgPool) {
    // В ревизию вписывают товар, которого нет в программе: карточка заводится с найденным
    // количеством, излишек оценивается по закупке (SPEC-29 §10).
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
    let (_, _, rev) = call(
        &app,
        "POST",
        "/api/v1/revisions",
        Some(&admin),
        Some(json!({ "category_id": null })),
    )
    .await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let op = Uuid::now_v7();
    let body = json!({
        "op_id": op, "counted_qty": 8000, "category_id": cat["id"], "name": "Mobil 5W-30 4л",
        "brand": "Mobil", "container_ml": 4000, "attrs": { "viscosity": "5W-30" },
        "barcodes": ["4600000000999"], "sale_price_tyiyn": 200_000, "initial_cost_tyiyn": 150_000
    });
    let uri = format!("/api/v1/revisions/{rid}/new-product");
    let (s, _, out) = call(&app, "POST", &uri, Some(&admin), Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Повтор той же операции — без второй карточки.
    let (s, _, again) = call(&app, "POST", &uri, Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["product"]["id"], out["product"]["id"]);
    let pid = out["product"]["id"].as_str().unwrap().to_string();
    let (_, _, draft) = call(
        &app,
        "GET",
        &format!("/api/v1/revisions/{rid}"),
        Some(&admin),
        None,
    )
    .await;
    let line = draft["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["product_id"] == json!(pid))
        .unwrap()
        .clone();
    assert_eq!(
        (line["expected_qty"].as_i64(), line["counted_qty"].as_i64()),
        (Some(0), Some(8000))
    );
    // Проводит владелец: остаток 8 л по 1 500 с за канистру.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/revisions/{rid}/post"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(stock_of(&pool, &pid).await, (8000, 300_000));
    // В проведённую ревизию новый товар не вписать.
    let (s, _, _) = call(
        &app,
        "POST",
        &uri,
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "counted_qty": 1, "category_id": cat["id"], "name": "Ещё",
                     "container_ml": 1000, "sale_price_tyiyn": 50_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn label_settings_have_brand_and_spec(pool: PgPool) {
    // Этикетка: бренд и характеристики (SPEC-29 §6); прежние настройки получают их включёнными.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, _, l) = call(&app, "GET", "/api/v1/settings/labels", Some(&owner), None).await;
    assert_eq!(
        (l["show_brand"].as_bool(), l["show_spec"].as_bool()),
        (Some(true), Some(true))
    );
    let old = json!({ "width_mm": 58, "height_mm": 40, "show_name": true, "show_price": true, "show_article": true });
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/labels",
        Some(&owner),
        Some(old),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, l) = call(&app, "GET", "/api/v1/settings/labels", Some(&owner), None).await;
    assert_eq!(
        (l["width_mm"].as_i64(), l["show_spec"].as_bool()),
        (Some(58), Some(true))
    );
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/labels",
        Some(&owner),
        Some(
            json!({ "width_mm": 58, "height_mm": 40, "show_name": true, "show_price": true,
                     "show_article": false, "show_brand": false, "show_spec": false }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, l) = call(&app, "GET", "/api/v1/settings/labels", Some(&owner), None).await;
    assert_eq!(l["show_brand"], false);
}

/// Заметки владельцу по действию — из экрана «Уведомления».
async fn notice_of(app: &Router, owner: &str, action: &str) -> Vec<Value> {
    let (_, _, list) = call(app, "GET", "/api/v1/notifications", Some(owner), None).await;
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["action"] == action)
        .cloned()
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_opens_with_cashier_pin_or_owner_password(pool: PgPool) {
    // PIN кассира при открытии смены (SPEC-29 §15, ADR-081): задаёт владелец, хранится хешем,
    // не отдаётся; 5 неверных — блокировка; владелец может открыть своим паролем.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Айбек", "is_cashier": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(e["has_pin"], false);
    let cashier = e["id"].as_str().unwrap().to_string();
    let open = |who: &str, extra: Value| {
        let app = app.clone();
        let who = who.to_string();
        let mut body = json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        async move { call(&app, "POST", "/api/v1/shifts/open", Some(&who), Some(body)).await }
    };
    // Без PIN у кассира — понятный отказ.
    let (s, _, err) = open(&admin, json!({ "cashier_pin": "1234" })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("нет PIN")
    );
    // PIN задаёт только владелец; 4 цифры.
    let pin_uri = format!("/api/v1/employees/{cashier}/pin");
    assert_eq!(
        call(
            &app,
            "POST",
            &pin_uri,
            Some(&admin),
            Some(json!({ "pin": "1234" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for bad in ["12", "12345", "12a4"] {
        assert_eq!(
            call(
                &app,
                "POST",
                &pin_uri,
                Some(&owner),
                Some(json!({ "pin": bad }))
            )
            .await
            .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (s, _, e) = call(
        &app,
        "POST",
        &pin_uri,
        Some(&owner),
        Some(json!({ "pin": "4321" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(e["has_pin"], true);
    let hash: String = sqlx::query_scalar("select pin_hash from employees where id = $1::uuid")
        .bind(&cashier)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(hash.starts_with("$argon2") && !hash.contains("4321"));
    let (_, _, list) = call(&app, "GET", "/api/v1/employees", Some(&owner), None).await;
    assert!(!list.to_string().contains("pin_hash") && !list.to_string().contains("4321"));
    assert!(!notice_of(&app, &owner, "employee.pin").await.is_empty());

    // Без PIN и неверный PIN — 422; смена не открыта.
    assert_eq!(
        open(&admin, json!({})).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        open(&admin, json!({ "cashier_pin": "0000" })).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Пароль владельца — только у владельца.
    assert_eq!(
        open(&admin, json!({ "owner_password": "admin-pass-1" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Ещё 4 неверных (всего 5) — дальше блокировка даже с верным PIN.
    for _ in 0..4 {
        assert_eq!(
            open(&admin, json!({ "cashier_pin": "1111" })).await.0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let (s, _, err) = open(&admin, json!({ "cashier_pin": "4321" })).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("заблокирован")
    );
    let (s, _, cur) = call(&app, "GET", "/api/v1/shifts/current", Some(&admin), None).await;
    assert_eq!((s, cur), (StatusCode::OK, Value::Null));
    // Новый PIN от владельца снимает блокировку.
    call(
        &app,
        "POST",
        &pin_uri,
        Some(&owner),
        Some(json!({ "pin": "4321" })),
    )
    .await;
    let (s, _, shift) = open(&admin, json!({ "cashier_pin": "4321" })).await;
    assert_eq!(s, StatusCode::OK, "{shift}");
    assert_eq!(actions(&pool, "shift.open").await[0]["confirmed"], "pin");
    let sid = shift["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/shifts/{sid}/close"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Владелец открывает своим паролем; неверный пароль — 422.
    assert_eq!(
        open(&owner, json!({ "owner_password": "wrong-pass" }))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, _) = open(&owner, json!({ "owner_password": "owner-pass-1" })).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        actions(&pool, "shift.open").await[1]["confirmed"],
        "owner_password"
    );
    // Снять PIN — снова «нет PIN».
    let (_, _, e) = call(
        &app,
        "POST",
        &pin_uri,
        Some(&owner),
        Some(json!({ "pin": null })),
    )
    .await;
    assert_eq!(e["has_pin"], false);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_transfer_by_admin_with_permission_hides_cost(pool: PgPool) {
    // Перелив — право отдельных администраторов (SPEC-29 §19, ADR-082): без права — 403,
    // с правом — проводит, но себестоимости не видит; владелец получает уведомление;
    // отмена — только владелец.
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
    let mut oils = Vec::new();
    for name in ["Масло 5W-30 4 л", "Масло 5W-30 розлив"] {
        let (s, _, p) = call(
            &app,
            "POST",
            "/api/v1/products",
            Some(&owner),
            Some(json!({
                "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
                "container_ml": 4000, "sale_price_tyiyn": 200_000
            })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{p}");
        oils.push(p["id"].as_str().unwrap().to_string());
    }
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": oils[0], "qty": 8000, "cost_tyiyn": 240_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let transfer = |who: &str| {
        let app = app.clone();
        let who = who.to_string();
        let body = json!({
            "op_id": Uuid::now_v7(), "from_product_id": oils[0], "to_product_id": oils[1],
            "qty_ml": 1000, "comment": "в бочку"
        });
        async move {
            call(
                &app,
                "POST",
                "/api/v1/oil/transfers",
                Some(&who),
                Some(body),
            )
            .await
        }
    };
    assert_eq!(transfer(&admin).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&app, "GET", "/api/v1/oil/transfers", Some(&admin), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Право выдаёт владелец в карточке пользователя.
    let (_, _, users) = call(&app, "GET", "/api/v1/users", Some(&owner), None).await;
    let admin_id = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["login"] == "admin")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // Права — отдельной операцией «Что можно» (ADR-097), список целиком.
    let user_uri = format!("/api/v1/users/{admin_id}/permissions");
    let with_oil = json!(["oil_transfer", "receipts"]);
    assert_eq!(
        call(
            &app,
            "PUT",
            &user_uri,
            Some(&owner),
            Some(json!({ "op_id": Uuid::now_v7(), "permissions": ["price_edit"] }))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &app,
            "PUT",
            &user_uri,
            Some(&admin),
            Some(json!({ "op_id": Uuid::now_v7(), "permissions": with_oil }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (s, _, u) = call(
        &app,
        "PUT",
        &user_uri,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": with_oil })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(u["permissions"], with_oil);
    let (_, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
    assert_eq!(me["permissions"], with_oil);
    assert!(
        notice_of(&app, &owner, "user.permissions").await[0]["details"]
            .as_str()
            .unwrap()
            .contains("включено: Перелив масла")
    );

    let (s, _, out) = transfer(&admin).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["qty_ml"], 1000);
    assert!(out.get("value_tyiyn").is_none(), "{out}");
    for key in ["value_tyiyn", "avg_per_l_tyiyn"] {
        assert!(out["from"].get(key).is_none(), "{out}");
        assert!(out["to"].get(key).is_none(), "{out}");
    }
    let (s, _, list) = call(&app, "GET", "/api/v1/oil/transfers", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(list[0].get("value_tyiyn").is_none() && list[0]["qty_ml"] == 1000);
    let (_, _, list) = call(&app, "GET", "/api/v1/oil/transfers", Some(&owner), None).await;
    assert_eq!(list[0]["value_tyiyn"], 30_000);
    // Владелец — уведомление о каждом переливе администратора, с себестоимостью.
    let notes = notice_of(&app, &owner, "oil.transfer_admin").await;
    assert_eq!(notes.len(), 1);
    assert!(notes[0]["details"].as_str().unwrap().contains("1 л"));
    // Отмена — только владелец.
    let rev = format!(
        "/api/v1/oil/transfers/{}/reverse",
        out["id"].as_str().unwrap()
    );
    let body = json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись" });
    assert_eq!(
        call(&app, "POST", &rev, Some(&admin), Some(body.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &rev, Some(&owner), Some(body)).await.0,
        StatusCode::OK
    );
    // Право сняли — снова 403.
    call(
        &app,
        "PUT",
        &user_uri,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": [] })),
    )
    .await;
    assert_eq!(transfer(&admin).await.0, StatusCode::FORBIDDEN);
    let (s, _, bad) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!((s, bad), (StatusCode::OK, json!([])));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn handover_to_chosen_account_and_payouts_from_any_account(pool: PgPool) {
    // Сдача кассы на выбранный счёт с уведомлением «Сдали кассу: … → …, кассир …»; по умолчанию —
    // куда сдавали в прошлый раз (SPEC-29 §14). Выплата при закрытии — с любого счёта владельцем,
    // администратору — только касса смены (SPEC-29 §13).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = named_cashier(&app, &owner, "Айбек").await;
    let (till, bank, owner_safe_id) = accounts_of(&app, &owner).await;
    let (_, _, def) = call(
        &app,
        "GET",
        "/api/v1/shifts/handover-target",
        Some(&admin),
        None,
    )
    .await;
    let (_, _, accs) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let shop_safe = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф магазина")
        .unwrap()["id"]
        .clone();
    assert_eq!(def["account_id"], shop_safe);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/payroll/accruals",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "employee_id": cashier, "kind": "bonus",
            "amount_tyiyn": 30_000, "comment": "за неделю"
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, shift) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let sid = shift["id"].as_str().unwrap().to_string();
    call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 2_500_000, "comment": "размен" })),
    )
    .await;
    // Администратор выплатить со счёта при закрытии не может.
    let close = |who: &str, account: &str| {
        let app = app.clone();
        let who = who.to_string();
        let body = json!({
            "op_id": Uuid::now_v7(), "counted_tyiyn": 2_500_000,
            "payouts": [{ "op_id": Uuid::now_v7(), "employee_id": cashier,
                          "amount_tyiyn": 30_000, "account_id": account }]
        });
        let uri = format!("/api/v1/shifts/{sid}/close");
        async move { call(&app, "POST", &uri, Some(&who), Some(body)).await }
    };
    assert_eq!(close(&admin, &bank).await.0, StatusCode::FORBIDDEN);
    // Владелец — со счёта: касса смены не уменьшается, пересчёт сходится.
    let (s, _, closed) = close(&owner, &bank).await;
    assert_eq!(s, StatusCode::OK, "{closed}");
    assert_eq!(closed["diff_tyiyn"], 0);
    assert_eq!(balance_of(&pool, &bank).await, -30_000);
    // Сдать в саму кассу — нельзя; в сейф владельца — можно, кассир выбирает.
    let handover = |to: Value| {
        let app = app.clone();
        let uri = format!("/api/v1/shifts/{sid}/handover");
        let admin = admin.clone();
        async move {
            call(
                &app,
                "POST",
                &uri,
                Some(&admin),
                Some(json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 2_000_000, "to_account_id": to })),
            )
            .await
        }
    };
    assert_eq!(
        handover(json!(till)).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, done) = handover(json!(owner_safe_id)).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    assert_eq!(balance_of(&pool, &owner_safe_id).await, 2_000_000);
    assert_eq!(balance_of(&pool, &till).await, 500_000);
    let note = &notice_of(&app, &owner, "shift.handover").await[0];
    let details = note["details"].as_str().unwrap();
    assert!(
        details.contains("20 000")
            && details.contains("→ Сейф владельца")
            && details.contains("кассир Айбек"),
        "{details}"
    );
    // В следующий раз первым предлагается тот же счёт.
    let (_, _, def) = call(
        &app,
        "GET",
        "/api/v1/shifts/handover-target",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(def["account_id"], json!(owner_safe_id));
    balances_match_movements(&pool).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn client_person_has_first_and_last_name(pool: PgPool) {
    // Имя и фамилия клиента — отдельные обязательные поля; полное имя «Фамилия Имя» для поиска
    // и показа (SPEC-29 §20, ADR-083). Прежние записи остаются с одним именем.
    let app = setup(pool.clone()).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let create = |body: Value| {
        let app = app.clone();
        let admin = admin.clone();
        async move { call(&app, "POST", "/api/v1/parties", Some(&admin), Some(body)).await }
    };
    assert_eq!(
        create(json!({ "kind": "person", "first_name": "Азамат", "last_name": " " }))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        create(json!({ "kind": "company", "first_name": "А", "last_name": "Б", "name": "ОсОО" }))
            .await
            .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, p) = create(json!({ "kind": "person", "first_name": " Азамат ", "last_name": "Токтогулов", "name": "игнор" })).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(
        (
            p["name"].clone(),
            p["first_name"].clone(),
            p["last_name"].clone()
        ),
        (
            json!("Токтогулов Азамат"),
            json!("Азамат"),
            json!("Токтогулов")
        )
    );
    // Ищут и «Имя Фамилия», и «Фамилия».
    for q in ["Азамат Токто", "Токтогулов"] {
        let (_, _, found) = call(
            &app,
            "GET",
            &format!("/api/v1/parties?role=customer&q={}", urlencode(q)),
            Some(&admin),
            None,
        )
        .await;
        assert_eq!(found.as_array().unwrap().len(), 1, "{q}");
    }
    // Прежняя запись — только `name`; дописывают при правке.
    let (s, old) = {
        let r = create(json!({ "name": "Эрлан" })).await;
        (r.0, r.2)
    };
    assert_eq!(s, StatusCode::OK);
    assert_eq!(old["first_name"], "");
    let uri = format!("/api/v1/parties/{}", old["id"].as_str().unwrap());
    assert_eq!(
        call(
            &app,
            "PATCH",
            &uri,
            Some(&admin),
            Some(json!({ "first_name": "Эрлан" }))
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (s, _, p) = call(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "first_name": "Эрлан", "last_name": "Садыков" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(p["name"], "Садыков Эрлан");
    assert_eq!(
        actions(&pool, "party.update").await[0]["changes"]["last_name"]["new"],
        "Садыков"
    );
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn car_from_dictionary_miles_and_interval_in_sale(pool: PgPool) {
    // Марка и модель из справочника, новая — в справочник (SPEC-29 §21); пробег в милях
    // (§22, ADR-084); «через сколько менять» в чеке — интервал машины (§23).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, makes) = call(&app, "GET", "/api/v1/car-makes", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    let toyota = makes
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "Toyota")
        .unwrap();
    assert!(
        toyota["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "Camry")
    );
    let (_, _, p) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "kind": "person", "first_name": "Нурлан", "last_name": "Абдыкадыров", "phone": "0555 10-20-30" })),
    )
    .await;
    let pid = p["id"].as_str().unwrap().to_string();
    // Написали по-своему — запишется как в справочнике.
    let (s, _, car) = call(
        &app,
        "POST",
        &format!("/api/v1/parties/{pid}/vehicles"),
        Some(&admin),
        Some(json!({ "plate": "01KG777AAA", "brand": "toyota", "model": "camry", "mileage_unit": "mi" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{car}");
    assert_eq!(
        (
            car["brand"].clone(),
            car["model"].clone(),
            car["mileage_unit"].clone()
        ),
        (json!("Toyota"), json!("Camry"), json!("mi"))
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/parties/{pid}/vehicles"),
            Some(&admin),
            Some(json!({ "plate": "01KG778AAA", "mileage_unit": "verst" })),
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Новая модель попадает в справочник.
    let (_, _, car2) = call(
        &app,
        "POST",
        &format!("/api/v1/parties/{pid}/vehicles"),
        Some(&admin),
        Some(json!({ "plate": "01KG779AAA", "brand": "Toyota", "model": "Mark X" })),
    )
    .await;
    assert_eq!(car2["mileage_unit"], "km");
    let (_, _, makes) = call(&app, "GET", "/api/v1/car-makes", Some(&admin), None).await;
    let toyota = makes
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "Toyota")
        .unwrap();
    assert!(
        toyota["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "Mark X")
    );

    // Замена в сервисе: пробег 50 000 миль, интервал масла 3 000 миль (= 4 828 км) из чека.
    let cashier = named_cashier(&app, &owner, "Айбек").await;
    let (_, _, master) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Мастер", "is_master": true })),
    )
    .await;
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Масла", "kind": "oil" })),
    )
    .await;
    let (_, _, oil) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Масло 4 л",
            "container_ml": 4000, "sale_price_tyiyn": 200_000
        })),
    )
    .await;
    call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "paid_from": owner_safe(&app, &owner).await,
            "lines": [{ "product_id": oil["id"], "qty": 8000, "cost_tyiyn": 240_000 }]
        })),
    )
    .await;
    let vid = car["id"].as_str().unwrap().to_string();
    let kind_id = oil_change_kind(&app, &owner).await;
    let sale = |intervals: Value| {
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "service", "cashier_id": cashier,
            "master_id": master["id"], "party_id": pid, "vehicle_id": vid,
            "mileage": 50_000, "book_intervals": intervals,
            "lines": [{ "kind": "container", "product_id": oil["id"], "qty": 1, "unit_price_tyiyn": 200_000 },
                      { "kind": "service", "service_id": kind_id, "qty": 1, "unit_price_tyiyn": 0 }],
            "payments": [{ "method": "card", "amount_tyiyn": 200_000 }]
        })
    };
    // Интервал вне пределов — чек не проводится.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(
            json!([{ "slot": "oil", "interval_km": 100, "interval_days": 30 }]),
        )),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, sold) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(
            json!([{ "slot": "oil", "interval_km": 4828, "interval_days": 90 }]),
        )),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sold}");
    let (_, _, book) = call(
        &app,
        "GET",
        &format!("/api/v1/vehicles/{vid}/oil-book"),
        Some(&admin),
        None,
    )
    .await;
    let rec = &book["records"][0];
    assert_eq!(
        (
            rec["mileage"].clone(),
            rec["mileage_unit"].clone(),
            rec["mileage_km"].clone()
        ),
        (json!(50_000), json!("mi"), json!(80_467))
    );
    let oil_slot = book["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "oil")
        .unwrap();
    assert_eq!(
        (
            oil_slot["interval_km"].clone(),
            oil_slot["interval_days"].clone(),
            oil_slot["own_interval"].clone(),
            oil_slot["next_km"].clone(),
            oil_slot["next_mileage"].clone(),
            oil_slot["unit"].clone()
        ),
        (
            json!(4828),
            json!(90),
            json!(true),
            json!(85_295),
            json!(53_000),
            json!("mi")
        )
    );
    assert_eq!(book["mileage_unit"], "mi");
    assert_eq!(book["next_mileage"], 53_000);
    assert_eq!(
        actions(&pool, "vehicle.oil_interval").await[0]["via"],
        "sale"
    );
    // Машину перевели в км — прежняя запись остаётся в милях, следующая — в км.
    let (s, _, c) = call(
        &app,
        "PATCH",
        &format!("/api/v1/parties/{pid}/vehicles/{vid}"),
        Some(&admin),
        Some(json!({ "plate": "01KG777AAA", "brand": "Toyota", "model": "Camry", "mileage_unit": "km" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    let (_, _, book) = call(
        &app,
        "GET",
        &format!("/api/v1/vehicles/{vid}/oil-book"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(book["records"][0]["mileage_unit"], "mi");
    assert_eq!(book["next_mileage"], 85_295);
    let (s, _, bad) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!((s, bad), (StatusCode::OK, json!([])));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stock_in_and_supplier_return_lists_by_period_and_pages(pool: PgPool) {
    // Экраны «Оприходование» и «Возврат поставщику» из меню (SPEC-29 §25): общий список за период
    // со страницами, число строк — в `X-Total-Count`; администратору — без себестоимости возврата.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let a = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let b = product(&app, &owner, "Свеча", 30_000, 0).await;
    for (item, qty) in [(&a, 1), (&b, 2), (&a, 3)] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/stock-ins",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "product_id": item, "qty": qty,
                         "price_tyiyn": 10_000, "comment": "нашли на складе" }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    let today = (chrono::Utc::now() + chrono::Duration::hours(6)).date_naive();
    let (s, total, page) = call_total(
        &app,
        &format!("/api/v1/stock-ins?from={today}&to={today}&limit=2&offset=0"),
        &admin,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(total, Some(3));
    let page = page.as_array().unwrap();
    assert_eq!(page.len(), 2);
    // Новые сверху: № 3, потом № 2; цена закупки видна администратору (ADR-008).
    assert_eq!(
        (
            page[0]["number"].as_i64(),
            page[0]["lines"][0]["qty"].as_i64()
        ),
        (Some(3), Some(3))
    );
    assert_eq!(page[0]["cost_tyiyn"], 30_000);
    let (_, total, rest) = call_total(&app, "/api/v1/stock-ins?limit=2&offset=2", &owner).await;
    assert_eq!((total, rest.as_array().unwrap().len()), (Some(3), 1));
    let (_, total, _) =
        call_total(&app, &format!("/api/v1/stock-ins?product_id={b}"), &owner).await;
    assert_eq!(total, Some(1));
    let yesterday = today.pred_opt().unwrap();
    let (_, total, _) = call_total(
        &app,
        &format!("/api/v1/stock-ins?from={yesterday}&to={yesterday}"),
        &owner,
    )
    .await;
    assert_eq!(total, Some(0));

    // Возвраты двум поставщикам — в одном списке; у поставщика — только свои.
    let s1 = new_supplier(&app, &owner, "Автоснаб").await;
    let s2 = new_supplier(&app, &owner, "Другой").await;
    for sup in [&s1, &s2] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/receipts",
            Some(&owner),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": sup, "payment": "debt",
                         "lines": [{ "product_id": a, "qty": 5, "cost_tyiyn": 10_000 }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/supplier-returns",
            Some(&admin),
            Some(
                json!({ "op_id": Uuid::now_v7(), "supplier_id": sup, "settle": "debt",
                         "lines": [{ "product_id": a, "qty": 1 }] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    let (s, total, list) = call_total(
        &app,
        &format!("/api/v1/supplier-returns?from={today}&to={today}&limit=25&offset=0"),
        &admin,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(total, Some(2));
    let list = list.as_array().unwrap();
    assert_eq!(list[0]["supplier_name"], "Другой");
    assert_eq!(list[0]["settle"], "debt");
    assert!(
        list.iter().all(|r| r.get("cost_tyiyn").is_none()),
        "{list:?}"
    );
    let (_, _, own) = call_total(&app, "/api/v1/supplier-returns?limit=25", &owner).await;
    assert!(
        own.as_array()
            .unwrap()
            .iter()
            .all(|r| r["cost_tyiyn"].is_i64())
    );
    let (_, total, _) = call_total(
        &app,
        &format!("/api/v1/supplier-returns?supplier_id={s1}"),
        &owner,
    )
    .await;
    assert_eq!(total, Some(1));
    let (_, _, verify) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!(verify, json!([]));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn new_account_takes_default_marks_on_create(pool: PgPool) {
    // Отметки «по умолчанию для карты / перевода» при заведении счёта ставятся сразу, прежний
    // счёт их теряет в той же транзакции (SPEC-29 §25); у наличного — 422, у «только владелец» — 422.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let (_, old_bank, _) = accounts_of(&app, &owner).await;
    let create = |body: Value| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            call(
                &app,
                "POST",
                "/api/v1/cash/accounts",
                Some(&owner),
                Some(body),
            )
            .await
        }
    };
    let (s, _, qr) = create(
        json!({ "op_id": Uuid::now_v7(), "name": "Личный счёт владельца",
                                    "kind": "bank", "default_transfer": true }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{qr}");
    assert_eq!(
        (
            qr["default_transfer"].as_bool(),
            qr["default_card"].as_bool()
        ),
        (Some(true), Some(false))
    );
    let (s, _, term) = create(json!({ "op_id": Uuid::now_v7(), "name": "Терминал", "kind": "bank",
                                      "fee_rate_bp": 50, "default_card": true, "default_transfer": false })).await;
    assert_eq!(s, StatusCode::OK, "{term}");
    assert_eq!(term["default_card"], true);
    let (_, _, accounts) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let marks: Vec<(String, bool, bool)> = accounts
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["id"].as_str().unwrap().to_string(),
                a["default_card"].as_bool().unwrap(),
                a["default_transfer"].as_bool().unwrap(),
            )
        })
        .collect();
    let of = |id: &str| {
        marks
            .iter()
            .find(|m| m.0 == id)
            .map(|m| (m.1, m.2))
            .unwrap()
    };
    assert_eq!(of(&old_bank), (false, false));
    assert_eq!(of(qr["id"].as_str().unwrap()), (false, true));
    assert_eq!(of(term["id"].as_str().unwrap()), (true, false));
    assert_eq!(marks.iter().filter(|m| m.1).count(), 1);
    assert_eq!(marks.iter().filter(|m| m.2).count(), 1);
    // Наличному отметка не ставится; «только владелец» с отметкой — 422, прежний счёт с отметкой.
    let (s, _, _) = create(
        json!({ "op_id": Uuid::now_v7(), "name": "Сейф 2", "kind": "safe", "default_card": true }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = create(
        json!({ "op_id": Uuid::now_v7(), "name": "Скрытый", "kind": "bank",
                                   "owner_only": true, "default_card": true }),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let still: bool =
        sqlx::query_scalar("select default_card from cash_accounts where id = $1::uuid")
            .bind(term["id"].as_str().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stock_in_is_a_receipt_variant_with_lines(pool: PgPool) {
    // Оприходование — вариант прихода (ADR-086): несколько строк одним документом, одной операцией;
    // в списке «Приход» вместе с накладными; сторно — всего документа.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let a = product(&app, &owner, "Хомут", 30_000, 0).await;
    let b = product(&app, &owner, "Шайба", 5_000, 0).await;
    // Прошлая закупка хомута — приход по 150 с: после сторно оприходования она снова последняя.
    let safe = owner_safe(&app, &owner).await;
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                     "lines": [{ "product_id": a, "qty": 1, "cost_tyiyn": 15_000 }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "comment": "привёз владелец со склада", "lines": [
        { "product_id": a, "qty": 3, "price_tyiyn": 20_000, "sale_price_tyiyn": 32_000 },
        { "product_id": b, "qty": 2, "price_tyiyn": 1_000 },
    ] });
    // Без комментария или без строк — 422; две цены продажи у одного товара — 422.
    let mut bad = body.clone();
    bad["comment"] = json!(" ");
    let (s, _, _) = call(&app, "POST", "/api/v1/stock-ins", Some(&admin), Some(bad)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let mut bad = body.clone();
    bad["lines"] = json!([]);
    let (s, _, _) = call(&app, "POST", "/api/v1/stock-ins", Some(&admin), Some(bad)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let mut bad = body.clone();
    bad["lines"] = json!([
        { "product_id": a, "qty": 1, "price_tyiyn": 20_000, "sale_price_tyiyn": 32_000 },
        { "product_id": a, "qty": 1, "price_tyiyn": 20_000, "sale_price_tyiyn": 33_000 },
    ]);
    let (s, _, _) = call(&app, "POST", "/api/v1/stock-ins", Some(&admin), Some(bad)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, si) = call(
        &app,
        "POST",
        "/api/v1/stock-ins",
        Some(&admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{si}");
    // Повтор с тем же op_id — тот же документ, склад не меняется второй раз.
    let (s, _, again) = call(&app, "POST", "/api/v1/stock-ins", Some(&admin), Some(body)).await;
    assert_eq!((s, &again["id"]), (StatusCode::OK, &si["id"]));
    assert_eq!(si["cost_tyiyn"], 62_000);
    assert_eq!(si["lines"].as_array().unwrap().len(), 2);
    assert_eq!(si["lines"][0]["sale_price_tyiyn"], 32_000);
    assert_eq!(stock_of(&pool, &a).await, (4, 75_000));
    assert_eq!(stock_of(&pool, &b).await, (2, 2_000));
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{a}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(
        (&p["last_purchase_price_tyiyn"], &p["sale_price_tyiyn"]),
        (&json!(20_000), &json!(32_000))
    );
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    let items = list["items"].as_array().unwrap();
    assert!(
        items.iter().any(|i| i["action"] == "stock_in.post"
            && i["title"]
                .as_str()
                .unwrap()
                .contains("Хомут × 3 шт, Шайба × 2 шт")),
        "{list}"
    );
    assert!(
        items.iter().any(|i| i["action"] == "product.prices"
            && i["details"].as_str().unwrap().contains("оприходование № 1")),
        "{list}"
    );

    // «Приход»: вместе по дате, с видом; отбор по виду; без отбора — только накладные, как раньше.
    let (s, total, all) =
        call_total(&app, "/api/v1/receipts?kind=all&limit=25&offset=0", &admin).await;
    assert_eq!((s, total), (StatusCode::OK, Some(2)));
    assert_eq!(all[0]["kind"], "stock_in");
    assert_eq!(all[0]["goods"], "Хомут, Шайба");
    assert_eq!(all[0]["comment"], "привёз владелец со склада");
    assert_eq!(all[1]["kind"], "receipt");
    let (_, total, only) = call_total(&app, "/api/v1/receipts?kind=stock_in", &admin).await;
    assert_eq!((total, &only[0]["id"]), (Some(1), &si["id"]));
    let (_, total, _) = call_total(&app, "/api/v1/receipts?kind=receipt", &admin).await;
    assert_eq!(total, Some(1));
    let (_, total, _) = call_total(&app, "/api/v1/receipts", &admin).await;
    assert_eq!(total, Some(1));
    let (s, _, _) = call(&app, "GET", "/api/v1/receipts?kind=x", Some(&admin), None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Шайбу частью списали — сторно уведёт её в минус: администратору 403, владельцу можно.
    sqlx::query(
        "insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
         select $1, branch_id, product_id, -1, -1000, 'revision', $1 from branch_products where product_id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&b)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update branch_products set stock_qty = stock_qty - 1, stock_value_tyiyn = stock_value_tyiyn - 1000 where product_id = $1::uuid")
        .bind(&b)
        .execute(&pool)
        .await
        .unwrap();
    let id = si["id"].as_str().unwrap();
    let uri = format!("/api/v1/stock-ins/{id}/reverse");
    let (s, _, _) = call(
        &app,
        "POST",
        &uri,
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, back) = call(
        &app,
        "POST",
        &uri,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["cost_tyiyn"], -62_000);
    assert_eq!(
        (
            back["lines"][0]["qty"].as_i64(),
            back["lines"][1]["qty"].as_i64()
        ),
        (Some(-3), Some(-2))
    );
    assert_eq!(stock_of(&pool, &a).await, (1, 15_000));
    assert_eq!(stock_of(&pool, &b).await.0, -1);
    let (_, _, p) = call(
        &app,
        "GET",
        &format!("/api/v1/products/{a}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(p["last_purchase_price_tyiyn"], 15_000);
    let (_, _, list) = call(&app, "GET", "/api/v1/notifications", Some(&owner), None).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["action"] == "stock_in.reverse"
                && i["details"]
                    .as_str()
                    .unwrap()
                    .contains("остаток ушёл в минус: Шайба")),
        "{list}"
    );
    let (s, _, _) = call(
        &app,
        "POST",
        &uri,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ещё" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (_, _, all) = call(
        &app,
        "GET",
        "/api/v1/receipts?kind=stock_in",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[0]["reversal_of"], si["id"]);
    assert_eq!(all[1]["reversed"], true);
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrations = false)]
async fn old_single_line_stock_ins_survive_migration(pool: PgPool) {
    // Миграция 0049: прежнее оприходование одного товара — документ с одной строкой; читается,
    // показывается в «Приходе», исправляется и сторнируется, как новое (ADR-086).
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 49) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    // Филиал — как в прежней базе (до SPEC-33 у филиала только название).
    sqlx::query("insert into branches (id, name) values ($1, 'Основной')")
        .bind(Uuid::now_v7())
        .execute(&pool)
        .await
        .unwrap();
    let app = setup(pool.clone()).await;
    // Карточка — как её заводила прежняя версия: запросы программы идут уже к новой схеме
    // (филиал запроса, SPEC-33), поэтому вход — после миграций.
    let item = Uuid::now_v7();
    sqlx::raw_sql(&format!(
        "insert into categories (id, name, kind) values (gen_random_uuid(), 'Кат Фильтр', 'filter');
         insert into products (id, category_id, name, unit)
           select '{item}', id, 'Фильтр', 'piece' from categories where name = 'Кат Фильтр';
         insert into branch_products (branch_id, product_id, sale_price_tyiyn)
           select id, '{item}', 50000 from branches;"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let item = item.to_string();
    let (old, rev_old) = (Uuid::now_v7(), Uuid::now_v7());
    // Как проводила прежняя версия: документ, движение, остаток и последняя закупочная; второе —
    // сторнированное, со своим сторно.
    sqlx::raw_sql(&format!(
        "insert into stock_ins (id, branch_id, number, product_id, qty, price_tyiyn, cost_tyiyn, comment, user_id, device_id)
           select '{old}', branch_id, 1, '{item}', 3, 20000, 60000, 'нашли', id, gen_random_uuid() from users where login = 'admin';
         insert into stock_ins (id, branch_id, number, product_id, qty, price_tyiyn, cost_tyiyn, comment, user_id, device_id)
           select '{rev_old}', branch_id, 2, '{item}', 1, 20000, 20000, 'лишнее', id, gen_random_uuid() from users where login = 'admin';
         insert into stock_ins (id, branch_id, number, product_id, qty, price_tyiyn, cost_tyiyn, comment, reversal_of, user_id, device_id)
           select gen_random_uuid(), branch_id, 3, '{item}', -1, 20000, -20000, 'ошибка', '{rev_old}', id, gen_random_uuid() from users where login = 'admin';
         insert into branch_products (branch_id, product_id) select branch_id, '{item}' from users where login = 'admin'
           on conflict do nothing;
         insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
           select gen_random_uuid(), branch_id, '{item}', 3, 60000, 'stock_in', '{old}' from users where login = 'admin';
         update branch_products set stock_qty = 3, stock_value_tyiyn = 60000, last_cost_qty = 3, last_cost_tyiyn = 60000
           where product_id = '{item}';
         insert into counters (branch_id, name, value) select branch_id, 'stock_in', 3 from users where login = 'admin';"
    ))
    .execute(&pool)
    .await
    .unwrap();
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version >= 49) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, doc) = call(
        &app,
        "GET",
        &format!("/api/v1/stock-ins/{old}"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{doc}");
    assert_eq!(doc["cost_tyiyn"], 60_000);
    assert_eq!(doc["lines"].as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &doc["lines"][0]["product_id"],
            &doc["lines"][0]["qty"],
            &doc["lines"][0]["price_tyiyn"]
        ),
        (&json!(item), &json!(3), &json!(20_000))
    );
    let (_, total, list) = call_total(&app, "/api/v1/receipts?kind=all", &admin).await;
    assert_eq!(total, Some(3));
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .all(|r| r["goods"] == "Фильтр")
    );
    // Сторнированное прежнее — уже сторнировано.
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/stock-ins/{rev_old}/reverse"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // Форма исправления — строками; сторно прежнего документа — администратором, как раньше.
    let (s, _, draft) = call(
        &app,
        "GET",
        &format!("/api/v1/corrections/draft/stock_in/{old}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{draft}");
    assert_eq!(draft["doc"]["lines"][0]["qty"], 3);
    let (s, _, back) = call(
        &app,
        "POST",
        &format!("/api/v1/stock-ins/{old}/reverse"),
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{back}");
    assert_eq!(back["lines"][0]["qty"], -3);
    assert_eq!(stock_of(&pool, &item).await, (0, 0));
    assert_stock_matches_movements(&app, &owner).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn purchase_reversals_keep_stock_value_sane(pool: PgPool) {
    // Сторно закупки после продаж (ADR-044, ADR-086, ADR-087): последней закупочной становится
    // живая закупка любого вида, и остаток оценивается по ней, а не по цене отменённого документа.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let safe = owner_safe(&app, &owner).await;
    let receipt = |p: &str, qty: i64, cost: i64| {
        json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": p, "qty": qty, "cost_tyiyn": cost }] })
    };
    let sell = |p: &str, qty: i64, price: i64| {
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": p, "qty": qty, "unit_price_tyiyn": price }],
                "payments": [{ "method": "cash", "amount_tyiyn": qty * price }] })
    };
    let post = |path: &'static str, body: Value| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            let (s, _, v) = call(&app, "POST", path, Some(&owner), Some(body)).await;
            assert_eq!(s, StatusCode::OK, "{path}: {v}");
            v
        }
    };

    // 1. Оприходование с лишним нулём в цене, продажа, сторно оприходования: 5 шт по цене прихода.
    let a = product(&app, &owner, "Хомут", 500_000, 0).await;
    post("/api/v1/receipts", receipt(&a, 10, 100_000)).await;
    let si = post(
        "/api/v1/stock-ins",
        json!({ "op_id": Uuid::now_v7(), "comment": "со склада",
                "lines": [{ "product_id": a, "qty": 1, "price_tyiyn": 1_000_000 }] }),
    )
    .await;
    post("/api/v1/sales", sell(&a, 5, 500_000)).await;
    assert_eq!(stock_of(&pool, &a).await, (6, 600_000));
    let path = format!("/api/v1/stock-ins/{}/reverse", si["id"].as_str().unwrap());
    let (s, _, v) = call(
        &app,
        "POST",
        &path,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "лишний ноль" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(stock_of(&pool, &a).await, (5, 50_000));

    // 2. Сторно прихода, когда после него было оприходование: последняя закупочная — оприходование.
    let b = product(&app, &owner, "Шайба", 50_000, 0).await;
    let r = post("/api/v1/receipts", receipt(&b, 10, 100_000)).await;
    post(
        "/api/v1/stock-ins",
        json!({ "op_id": Uuid::now_v7(), "comment": "нашли",
                "lines": [{ "product_id": b, "qty": 5, "price_tyiyn": 20_000 }] }),
    )
    .await;
    let path = format!("/api/v1/receipts/{}/reverse", r["id"].as_str().unwrap());
    let (s, _, v) = call(
        &app,
        "POST",
        &path,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "не тот поставщик" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let last: (i64, i64) = sqlx::query_as(
        "select last_cost_qty, last_cost_tyiyn from branch_products where product_id = $1::uuid",
    )
    .bind(&b)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(last, (5, 100_000));

    // 3. Сторно дешёвого прихода после продаж: остаток не дороже самой дорогой живой закупки,
    //    лишнее — переоценкой; товар продаётся по обычной цене.
    let c = product(&app, &owner, "Фильтр", 600_000, 0).await;
    let cheap = post("/api/v1/receipts", receipt(&c, 2, 30_000)).await;
    post("/api/v1/receipts", receipt(&c, 6, 3_000_000)).await;
    post("/api/v1/sales", sell(&c, 5, 600_000)).await;
    let path = format!("/api/v1/receipts/{}/reverse", cheap["id"].as_str().unwrap());
    let (s, _, v) = call(
        &app,
        "POST",
        &path,
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(stock_of(&pool, &c).await, (1, 500_000));
    post("/api/v1/sales", sell(&c, 1, 600_000)).await;
    let (_, _, verify) = call(&app, "GET", "/api/v1/stock/verify", Some(&owner), None).await;
    assert_eq!(verify.as_array().map(Vec::len), Some(0), "{verify}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn refunds_fees_and_forgiveness_reverse_exactly(pool: PgPool) {
    // Проверка 20.10.2026 (ADR-088): комиссия возвращается по своему счёту; «Исправить»
    // погашения картой не берёт комиссию дважды; отмена прощения долга возвращает ожидание 2 %.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let safe = owner_safe(&app, &owner).await;
    let post = |path: String, body: Value| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            let (s, _, v) = call(&app, "POST", &path, Some(&owner), Some(body)).await;
            assert_eq!(s, StatusCode::OK, "{path}: {v}");
            v
        }
    };
    let balance = |id: String| {
        let pool = pool.clone();
        async move { balance_of(&pool, &id).await }
    };
    let (_, bank, _) = accounts_of(&app, &owner).await;
    let personal = post(
        "/api/v1/cash/accounts".into(),
        json!({ "op_id": Uuid::now_v7(), "name": "Карта владельца", "kind": "bank" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let p = product(&app, &owner, "Масло", 100_000, 0).await;
    post(
        "/api/v1/receipts".into(),
        json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": p, "qty": 10, "cost_tyiyn": 600_000 }] }),
    )
    .await;

    // 1. Чек: карта 1 000 на «Счёт» (0,5 %) + перевод 1 000 на личный (0 %). Возврат одной
    //    штуки переводом: личный счёт — ровно 0, комиссия «Счёта» не трогается.
    let sale = post(
        "/api/v1/sales".into(),
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                "lines": [{ "kind": "piece", "product_id": p, "qty": 2, "unit_price_tyiyn": 100_000 }],
                "payments": [
                    { "method": "card", "amount_tyiyn": 100_000, "account_id": bank },
                    { "method": "transfer", "amount_tyiyn": 100_000, "account_id": personal }
                ] }),
    )
    .await;
    let bank_after_sale = balance(bank.clone()).await;
    assert_eq!(balance(personal.clone()).await, 100_000);
    post(
        format!("/api/v1/sales/{}/return", sale["id"].as_str().unwrap()),
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "transfer", "amount_tyiyn": 100_000 }] }),
    )
    .await;
    assert_eq!(balance(personal.clone()).await, 0);
    assert_eq!(balance(bank.clone()).await, bank_after_sale);

    // 2. Погашение 500 картой (комиссия 2,50), «Исправить» на 400: как будто сразу 400 —
    //    на «Счёт» пришло 398, комиссии 2,00.
    let client = post(
        "/api/v1/parties".into(),
        json!({ "name": "ОсОО Должник", "kind": "company", "inn": "01234567890123" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    post(
        "/api/v1/sales".into(),
        json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier, "party_id": client,
                "lines": [{ "kind": "piece", "product_id": p, "qty": 1, "unit_price_tyiyn": 100_000 }],
                "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }] }),
    )
    .await;
    let before = balance(bank.clone()).await;
    let paid = post(
        "/api/v1/debts/repayments".into(),
        json!({ "op_id": Uuid::now_v7(), "party_id": client, "amount_tyiyn": 50_000, "method": "card" }),
    )
    .await;
    assert_eq!(balance(bank.clone()).await - before, 49_750);
    let ledger = paid["ledger_id"].as_str().unwrap().to_string();
    let (s, _, mut draft) = call(
        &app,
        "GET",
        &format!("/api/v1/corrections/draft/ledger/{ledger}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{draft}");
    draft["doc"]["amount_tyiyn"] = json!(40_000);
    post(
        "/api/v1/corrections".into(),
        json!({ "op_id": Uuid::now_v7(), "kind": "ledger", "original_id": ledger,
                "comment": "не та сумма", "doc": draft["doc"] }),
    )
    .await;
    assert_eq!(balance(bank.clone()).await - before, 39_800);

    // 3. Прощение долга −600 отменили и остаток оплатили — 2 % кассиру с этой части начислены.
    let percent = || {
        let pool = pool.clone();
        let cashier = cashier.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals
                 where employee_id = $1::uuid and kind = 'revenue_percent'",
            )
            .bind(cashier)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let start = percent().await;
    let forgiven = post(
        "/api/v1/debts/adjust".into(),
        json!({ "op_id": Uuid::now_v7(), "party_id": client, "amount_tyiyn": -60_000, "comment": "прощаем" }),
    )
    .await;
    post(
        format!(
            "/api/v1/debts/ledger/{}/reverse",
            forgiven["ledger_id"].as_str().unwrap()
        ),
        json!({ "op_id": Uuid::now_v7(), "comment": "передумали" }),
    )
    .await;
    post(
        "/api/v1/debts/repayments".into(),
        json!({ "op_id": Uuid::now_v7(), "party_id": client, "amount_tyiyn": 60_000, "method": "card" }),
    )
    .await;
    // Ожидание: валовая 400 с на долг 1 000 с; оплата 600 с после отмены прощения даёт 2 % от
    // 240 с = 4,80 с (без отмены — 0).
    assert_eq!(percent().await - start, 480);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_catalog_extra_codes_and_zeros(pool: PgPool) {
    // Настоящая выгрузка (проверка 20.10.2026): «Доп. код» через «;», нули впереди потеряны;
    // карточка из позиции получает и заводской код; заведённая позиция не открывается как новая.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let rows = json!([
        { "name": "TOTACHI 5W30 4Л", "barcode": "2110000043070", "extra_code": "4589904921551; 4589904921552", "unit": "литр" },
        { "name": "Фильтр C-110", "barcode": "123456789012", "unit": "шт" }
    ]);
    let (s, _, _) = call(&app, "PUT", "/api/v1/old-catalog", Some(&owner), Some(rows)).await;
    assert_eq!(s, StatusCode::OK);
    let search = |code: &'static str| {
        let app = app.clone();
        let owner = owner.clone();
        async move {
            let (_, _, hits) = call(
                &app,
                "GET",
                &format!("/api/v1/old-catalog/search?barcode={code}"),
                Some(&owner),
                None,
            )
            .await;
            hits
        }
    };
    let hits = search("4589904921552").await;
    assert_eq!(
        (hits[0]["exact"].clone(), hits[0]["name"].clone()),
        (json!(true), json!("TOTACHI 5W30 4Л"))
    );
    // Код с упаковки с нулём впереди находит позицию, у которой Excel ноль потерял.
    let hits = search("0123456789012").await;
    assert_eq!(hits[0]["exact"], json!(true));
    // Карточка по основному коду позиции получает и её «доп. коды».
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Масла", "kind": "oil" })),
    )
    .await;
    let (s, _, p) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Totachi 5W30",
                     "container_ml": 4000, "sale_price_tyiyn": 200_000, "barcodes": ["2110000043070"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    let (s, _, found) = call(
        &app,
        "GET",
        "/api/v1/products/by-barcode/4589904921551",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(found["id"], p["id"]);
    // Уже заведена — скан любого её кода показывает связь, а не «новый товар».
    let hits = search("4589904921552").await;
    assert_eq!(hits[0]["linked_product_id"], p["id"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn list_summaries_cover_the_whole_selection(pool: PgPool) {
    // Итоги «Расходов» и «Клиентов» — по всему отбору, а не по первой странице (проверка 20.10.2026).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let safe = owner_safe(&app, &owner).await;
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&owner), None).await;
    let article = articles[0]["id"].clone();
    for _ in 0..3 {
        let (s, _, e) = call(
            &app,
            "POST",
            "/api/v1/expenses",
            Some(&owner),
            Some(
                json!({ "op_id": Uuid::now_v7(), "article_id": article, "amount_tyiyn": 10_000,
                         "source": "account", "account_id": safe, "comment": "тест" }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{e}");
    }
    let (s, _, page) = call(&app, "GET", "/api/v1/expenses?limit=1", Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(page.as_array().map(Vec::len), Some(1));
    let (_, _, sum) = call(&app, "GET", "/api/v1/expenses/summary", Some(&owner), None).await;
    assert_eq!(
        (sum["count"].clone(), sum["total_tyiyn"].clone()),
        (json!(3), json!(30_000))
    );

    for (name, balance) in [("ОсОО А", 50_000), ("ОсОО Б", -20_000)] {
        let (_, _, p) = call(
            &app,
            "POST",
            "/api/v1/parties",
            Some(&owner),
            Some(json!({ "name": name, "kind": "company", "inn": format!("0123456789012{}", name.len() % 10) })),
        )
        .await;
        let (s, _, _) = call(
            &app,
            "POST",
            "/api/v1/debts/adjust",
            Some(&owner),
            Some(json!({ "op_id": Uuid::now_v7(), "party_id": p["id"], "amount_tyiyn": balance, "comment": "из тетради" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (_, _, sum) = call(
        &app,
        "GET",
        "/api/v1/parties/summary?role=customer",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(
        (sum["owe_tyiyn"].clone(), sum["advance_tyiyn"].clone()),
        (json!(50_000), json!(20_000))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_queue_rules_adr_093(pool: PgPool) {
    // Чек из очереди устройства (ADR-093): признак без сети действует и через секунды после
    // продажи; огромный пробег не роняет чек ошибкой формата; часы устройства, сбитые на дни, —
    // причина владельцу; чужой чек владелец отправляет от себя; отказ сообщается один раз.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр ADR-093", 50_000, 0).await;
    let (_, _, master) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Мастер Бакыт", "is_master": true })),
    )
    .await;
    let (_, _, p) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "kind": "person", "first_name": "Улан", "last_name": "Исаков", "phone": "0555 20-30-40" })),
    )
    .await;
    let pid = p["id"].as_str().unwrap().to_string();
    let (s, _, car) = call(
        &app,
        "POST",
        &format!("/api/v1/parties/{pid}/vehicles"),
        Some(&admin),
        Some(json!({ "plate": "01KG093AAA" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{car}");
    // Вид замены (ADR-096); 40 с при прайсе 50 с онлайн — скидка: право у администратора есть (ADR-097).
    let kind_id = oil_change_kind(&app, &owner).await;
    grant_admin(&pool, "discount").await;
    let sale = |offline: bool, secs_ago: i64| {
        let mut body = json!({
            "op_id": Uuid::now_v7(), "sale_type": "service", "cashier_id": cashier,
            "master_id": master["id"], "party_id": pid, "vehicle_id": car["id"],
            "mileage": 12_345_678_901_i64,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 40_000,
                        "seen_list_price_tyiyn": 40_000 },
                      { "kind": "service", "service_id": kind_id, "qty": 1, "unit_price_tyiyn": 0 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 40_000 }]
        });
        if offline {
            body["offline"] = json!(true);
            body["client_time"] =
                json!((chrono::Utc::now() - chrono::Duration::seconds(secs_ago)).to_rfc3339());
        }
        body
    };
    // Онлайн — понятная ошибка пробега, а не «неверный формат запроса».
    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(false, 0)),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(e.to_string().contains("пробег"), "{e}");
    // Из очереди через 5 с после продажи — чек без сети: проводится без пробега, цена из
    // снимка (40 с при прайсе 50 с) — старая цена, а не правка кассира.
    let (s, _, off) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(true, 5)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{off}");
    let id = off["id"].as_str().unwrap();
    let reasons: Vec<Value> = sqlx::query_scalar(
        "select data from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        reasons
            .iter()
            .any(|r| r.to_string().contains("пробег не записан")),
        "{reasons:?}"
    );
    let overrides: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'sale.price_override' and entity_id = $1::uuid",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(overrides, 0);
    let stale: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'sale.stale_price' and entity_id = $1::uuid",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stale, 1);
    // Дробный пробег и строка — тоже не ошибка формата.
    let mut odd = sale(true, 60);
    odd["mileage"] = json!(1.5);
    assert_eq!(
        call(&app, "POST", "/api/v1/sales", Some(&admin), Some(odd))
            .await
            .0,
        StatusCode::OK
    );
    // Часы устройства отстали на 3 дня — чек днём проведения, владельцу причина.
    let mut late = sale(true, 3 * 24 * 3600);
    late["mileage"] = json!(120_000);
    let (s, _, late) = call(&app, "POST", "/api/v1/sales", Some(&admin), Some(late)).await;
    assert_eq!(s, StatusCode::OK, "{late}");
    let reasons: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
    )
    .bind(late["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        reasons.to_string().contains("проверьте часы кассы"),
        "{reasons}"
    );

    // Чек другого пользователя кассы: администратор от себя отправить не может, владелец — может,
    // в журнале — чей он был.
    let admin_id: Uuid = sqlx::query_scalar("select id from users where login = 'admin'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut foreign = sale(true, 600);
    foreign["mileage"] = json!(121_000);
    foreign["claimed_user_id"] = json!(owner_id);
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/sales",
            Some(&admin),
            Some(foreign.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    foreign["claimed_user_id"] = json!(admin_id);
    let (s, _, claimed) = call(&app, "POST", "/api/v1/sales", Some(&owner), Some(foreign)).await;
    assert_eq!(s, StatusCode::OK, "{claimed}");
    let post: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'sale.post' and entity_id = $1::uuid",
    )
    .bind(claimed["id"].as_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(post["claimed_user_id"], json!(admin_id), "{post}");
    assert_eq!(post["claimed_user_name"], "Админ", "{post}");

    // Отказ по чеку сообщают один раз; повтор от другого пользователя той же кассы — не 409.
    let op = Uuid::now_v7();
    let report = json!({ "op_id": op, "error": "товар не найден", "body": { "lines": [] } });
    for who in [&admin, &owner] {
        let (s, _, r) = call(
            &app,
            "POST",
            "/api/v1/sales/offline-rejected",
            Some(who),
            Some(report.clone()),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}");
    }
    assert_eq!(
        actions(&pool, "sale.offline_rejected")
            .await
            .iter()
            .filter(|a| a["op_id"] == json!(op))
            .count(),
        1
    );

    // Снимок кассы без сети: вид категории у товара, услуги, клиент с машиной.
    let (_, _, snap) = call(&app, "GET", "/api/v1/offline/snapshot", Some(&admin), None).await;
    let prod = snap["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(item))
        .unwrap();
    assert_eq!(prod["category_kind"], "filter");
    assert!(snap["services"].is_array(), "{snap}");
    assert!(
        snap["categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["kind"] == "filter")
    );
    let client = snap["clients"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == json!(pid))
        .unwrap();
    assert_eq!(client["vehicles"][0]["plate"], "01KG093AAA");
    assert!(client.get("cost").is_none());
}

/// id тестового администратора «admin».
async fn admin_id(app: &Router, owner: &str) -> String {
    let (_, _, users) = call(app, "GET", "/api/v1/users", Some(owner), None).await;
    users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["login"] == "admin")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Права администратора целиком — «Что можно» (ADR-097).
async fn set_admin_permissions(app: &Router, owner: &str, permissions: &[&str]) -> Value {
    let id = admin_id(app, owner).await;
    let (s, _, u) = call(
        app,
        "PUT",
        &format!("/api/v1/users/{id}/permissions"),
        Some(owner),
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": permissions })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{u}");
    u
}

#[sqlx::test(migrations = false)]
async fn migration_gives_existing_admins_default_permissions(pool: PgPool) {
    // Миграция 0056: существующим администраторам — все права «да» по умолчанию, перелив
    // остаётся у тех, кому его выдали, скидки нет ни у кого; владельцу права не пишутся (SPEC-30 §5).
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 56) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let (branch, owner, plain, oil) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "insert into branches (id, name) values ('{branch}', 'Тест');
         insert into users (id, branch_id, login, password_hash, role, full_name) values
           ('{owner}', '{branch}', 'o', 'x', 'owner', 'Владелец'),
           ('{plain}', '{branch}', 'a1', 'x', 'admin', 'Админ'),
           ('{oil}', '{branch}', 'a2', 'x', 'admin', 'Эрбол');
         insert into user_permissions (user_id, branch_id, permission, granted_by)
           values ('{oil}', '{branch}', 'oil_transfer', '{owner}');"
    ))
    .execute(&pool)
    .await
    .unwrap();
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version >= 56) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let of = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "select permission from user_permissions where user_id = $1 order by permission",
            )
            .bind(id)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    let mut defaults: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    defaults.sort();
    assert_eq!(defaults.len(), 14);
    assert!(
        !defaults
            .iter()
            .any(|p| p == "discount" || p == "oil_transfer")
    );
    assert_eq!(of(plain).await, defaults);
    let mut with_oil = defaults.clone();
    with_oil.push("oil_transfer".into());
    with_oil.sort();
    assert_eq!(of(oil).await, with_oil);
    assert!(of(owner).await.is_empty());
    // Выданное владельцем раньше — как было, с «кто выдал».
    let by: Option<Uuid> = sqlx::query_scalar(
        "select granted_by from user_permissions where user_id = $1 and permission = 'oil_transfer'",
    )
    .bind(oil)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(by, Some(owner));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_sets_admin_permissions_with_journal(pool: PgPool) {
    // «Что можно»: права — только владелец, списком целиком, повтор `op_id` — тот же ответ без
    // второй записи; журнал `user.permissions` — кому, что включено и выключено; новый
    // администратор получает права по умолчанию (SPEC-30 §5, ADR-097).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let id = admin_id(&app, &owner).await;
    let uri = format!("/api/v1/users/{id}/permissions");
    let mut defaults: Vec<&str> = avtodom_server::auth::default_permissions();
    defaults.sort_unstable();
    let (_, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
    assert_eq!(me["permissions"], json!(defaults));

    // Администратор сам себе права не выдаёт; неизвестное право — 422.
    let body = json!({ "op_id": Uuid::now_v7(), "permissions": ["discount"] });
    let (s, _, _) = call(&app, "PUT", &uri, Some(&admin), Some(body)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let body = json!({ "op_id": Uuid::now_v7(), "permissions": ["price_edit"] });
    let (s, _, err) = call(&app, "PUT", &uri, Some(&owner), Some(body)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{err}");

    // Скидку включили, расходы и деньги выключили.
    let mut next: Vec<&str> = defaults
        .iter()
        .copied()
        .filter(|p| *p != "expenses" && *p != "cash_moves")
        .collect();
    next.push("discount");
    let body = json!({ "op_id": Uuid::now_v7(), "permissions": next });
    let (s, _, u) = call(&app, "PUT", &uri, Some(&owner), Some(body.clone())).await;
    assert_eq!(s, StatusCode::OK, "{u}");
    let got: Vec<String> = serde_json::from_value(u["permissions"].clone()).unwrap();
    assert!(got.contains(&"discount".to_string()) && !got.contains(&"expenses".to_string()));
    assert_eq!(got.len(), 13);
    // Повтор того же запроса — тот же ответ, вторая запись в журнал не пишется.
    let (s, _, again) = call(&app, "PUT", &uri, Some(&owner), Some(body)).await;
    assert_eq!((s, &again), (StatusCode::OK, &u));
    let log = actions(&pool, "user.permissions").await;
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0]["login"], "admin");
    assert_eq!(log[0]["granted"], json!(["discount"]));
    assert_eq!(log[0]["revoked"], json!(["cash_moves", "expenses"]));
    let who: Uuid = sqlx::query_scalar(
        "select user_id from audit_log where action = 'user.permissions' and entity_id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(who, owner_id);
    let note = notice_of(&app, &owner, "user.permissions").await;
    assert_eq!(note[0]["title"], "Права администратора: Админ");
    assert_eq!(
        note[0]["details"],
        "включено: Скидка в чеке; выключено: Внести, изъять, перевести деньги, Расходы"
    );
    // Тот же набор новой операцией — ничего не поменялось, записи нет.
    let body = json!({ "op_id": Uuid::now_v7(), "permissions": next });
    let (s, _, _) = call(&app, "PUT", &uri, Some(&owner), Some(body)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(actions(&pool, "user.permissions").await.len(), 1);
    let (_, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&admin), None).await;
    assert!(
        !me["permissions"]
            .as_array()
            .unwrap()
            .contains(&json!("expenses"))
    );

    // Владельцу права не нужны.
    let body = json!({ "op_id": Uuid::now_v7(), "permissions": ["expenses"] });
    let (s, _, err) = call(
        &app,
        "PUT",
        &format!("/api/v1/users/{owner_id}/permissions"),
        Some(&owner),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{err}");

    // Новый администратор — с правами по умолчанию; запись о создании их называет.
    let (s, _, created) = call(
        &app,
        "POST",
        "/api/v1/users",
        Some(&owner),
        Some(json!({ "login": "erbol", "password": "erbol-pass-1", "full_name": "Эрбол", "role": "admin" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{created}");
    assert_eq!(created["permissions"], json!(defaults));
    let erbol = login(&app, "erbol", "erbol-pass-1").await;
    let (_, _, me) = call(&app, "GET", "/api/v1/auth/me", Some(&erbol), None).await;
    assert_eq!(me["permissions"], json!(defaults));
    assert_eq!(
        actions(&pool, "user.create").await[0]["permissions"],
        json!(defaults)
    );
    // Правка имени старым экраном с полем `permissions` права не трогает.
    let cid = created["id"].as_str().unwrap();
    let (s, _, u) = call(
        &app,
        "PATCH",
        &format!("/api/v1/users/{cid}"),
        Some(&owner),
        Some(json!({ "full_name": "Эрбол Асанов", "permissions": [] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{u}");
    assert_eq!(u["permissions"], json!(defaults));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn every_admin_permission_closes_its_actions(pool: PgPool) {
    // Каждое действие администратора сверяется со своим правом (SPEC-30 §5): без права — 403
    // «Нет права: …», с правом — проводится. Владельцу права не нужны.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let item = product(&app, &owner, "Фильтр", 50_000, 0).await;
    let (_, _, cats) = call(&app, "GET", "/api/v1/categories", Some(&owner), None).await;
    let cat = cats[0]["id"].as_str().unwrap().to_string();
    let sup = new_supplier(&app, &owner, "Автоснаб").await;
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "supplier_id": sup, "payment": "debt",
                     "lines": [{ "product_id": item, "qty": 50, "cost_tyiyn": 500_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let cashier = named_cashier(&app, &owner, "Айбек").await;
    let (_, _, master) = call(
        &app,
        "POST",
        "/api/v1/employees",
        Some(&owner),
        Some(json!({ "full_name": "Мастер Нурлан", "is_master": true })),
    )
    .await;
    let master = master["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/payroll/accruals",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": master, "kind": "bonus",
                     "amount_tyiyn": 100_000, "comment": "за неделю" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/cash/movements",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 1_000_000, "comment": "размен" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, party) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&owner),
        Some(json!({ "name": "Эрлан", "phone": "+996 555 12-34-56", "inn": "21201199000123" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{party}");
    let client = party["id"].as_str().unwrap().to_string();
    let (s, _, sale) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                     "lines": [{ "kind": "piece", "product_id": item, "qty": 5, "unit_price_tyiyn": 50_000 }],
                     "payments": [{ "method": "cash", "amount_tyiyn": 250_000 }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let sale = sale["id"].as_str().unwrap().to_string();
    let (_, _, articles) = call(&app, "GET", "/api/v1/expense-articles", Some(&admin), None).await;
    let household = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Хозтовары")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (till, _, _) = accounts_of(&app, &owner).await;
    let (_, _, accs) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let shop_safe = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "safe" && a["owner_only"] == false)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Право, метод, адрес и тело запроса (каждый раз со своим `op_id`).
    type Case = (&'static str, &'static str, String, Box<dyn Fn() -> Value>);
    let op = Uuid::now_v7;
    let cases: Vec<Case> = vec![
        (
            "sale_return",
            "POST",
            format!("/api/v1/sales/{sale}/return"),
            Box::new(move || {
                json!({ "op_id": op(), "lines": [{ "line_no": 1, "qty": 1 }],
                        "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }] })
            }),
        ),
        ("sale_debt", "POST", "/api/v1/sales".into(), {
            let (item, client, cashier) = (item.clone(), client.clone(), cashier.clone());
            Box::new(move || {
                json!({ "op_id": op(), "sale_type": "takeaway", "cashier_id": cashier, "party_id": client,
                        "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
                        "payments": [{ "method": "debt", "amount_tyiyn": 50_000 }] })
            })
        }),
        ("debt_payment", "POST", "/api/v1/debts/repayments".into(), {
            let client = client.clone();
            Box::new(
                move || json!({ "op_id": op(), "party_id": client, "amount_tyiyn": 10_000, "method": "cash" }),
            )
        }),
        (
            "cash_moves",
            "POST",
            "/api/v1/cash/movements".into(),
            Box::new(
                move || json!({ "op_id": op(), "kind": "cash_out", "amount_tyiyn": 1_000, "comment": "в банк" }),
            ),
        ),
        ("cash_moves", "POST", "/api/v1/cash/transfers".into(), {
            let (till, safe) = (till.clone(), shop_safe.clone());
            Box::new(
                move || json!({ "op_id": op(), "from_account_id": till, "to_account_id": safe, "amount_tyiyn": 1_000 }),
            )
        }),
        ("expenses", "POST", "/api/v1/expenses".into(), {
            let article = household.clone();
            Box::new(
                move || json!({ "op_id": op(), "article_id": article, "amount_tyiyn": 1_000, "comment": "мыло" }),
            )
        }),
        ("payroll_pay", "POST", "/api/v1/payouts".into(), {
            let master = master.clone();
            Box::new(move || json!({ "op_id": op(), "employee_id": master, "amount_tyiyn": 1_000 }))
        }),
        ("receipts", "POST", "/api/v1/receipts".into(), {
            let (item, sup) = (item.clone(), sup.clone());
            Box::new(move || {
                json!({ "op_id": op(), "supplier_id": sup, "payment": "debt",
                        "lines": [{ "product_id": item, "qty": 1, "cost_tyiyn": 10_000 }] })
            })
        }),
        ("receipts", "POST", "/api/v1/debts/repayments".into(), {
            let sup = sup.clone();
            Box::new(
                move || json!({ "op_id": op(), "party_id": sup, "amount_tyiyn": 1_000, "method": "cash" }),
            )
        }),
        ("stock_in", "POST", "/api/v1/stock-ins".into(), {
            let item = item.clone();
            Box::new(move || {
                json!({ "op_id": op(), "comment": "нашли на полке",
                        "lines": [{ "product_id": item, "qty": 1, "price_tyiyn": 10_000 }] })
            })
        }),
        (
            "supplier_returns",
            "POST",
            "/api/v1/supplier-returns".into(),
            {
                let (item, sup) = (item.clone(), sup.clone());
                Box::new(move || {
                    json!({ "op_id": op(), "supplier_id": sup, "settle": "debt",
                        "lines": [{ "product_id": item, "qty": 1 }] })
                })
            },
        ),
        ("defects", "POST", "/api/v1/defects".into(), {
            let item = item.clone();
            Box::new(
                move || json!({ "op_id": op(), "product_id": item, "qty": 1, "reason": "трещина", "no_supplier": true }),
            )
        }),
        (
            "revision",
            "POST",
            "/api/v1/revisions".into(),
            Box::new(|| json!({ "category_id": null })),
        ),
        (
            "battery_intake",
            "POST",
            "/api/v1/batteries/intake".into(),
            Box::new(
                move || json!({ "op_id": op(), "grams": 1_000, "price_per_kg_tyiyn": 10_000 }),
            ),
        ),
        ("products", "POST", "/api/v1/products".into(), {
            let cat = cat.clone();
            Box::new(move || {
                json!({ "op_id": op(), "category_id": cat, "name": format!("Фильтр {}", op()),
                        "sale_price_tyiyn": 10_000 })
            })
        }),
        (
            "products",
            "POST",
            format!("/api/v1/products/{item}/barcodes"),
            Box::new(|| json!({})),
        ),
        (
            "products",
            "POST",
            "/api/v1/categories".into(),
            Box::new(move || json!({ "name": format!("Кат {}", op()), "kind": "filter" })),
        ),
        (
            "clients",
            "POST",
            "/api/v1/parties".into(),
            Box::new(move || json!({ "kind": "company", "name": format!("ОсОО {}", op()) })),
        ),
        (
            "clients",
            "PATCH",
            format!("/api/v1/parties/{client}"),
            Box::new(|| json!({ "comment": "постоянный" })),
        ),
        (
            "clients",
            "POST",
            format!("/api/v1/parties/{client}/vehicles"),
            Box::new(
                move || json!({ "plate": format!("01KG{}", &op().simple().to_string()[26..]) }),
            ),
        ),
    ];
    let all = avtodom_server::auth::default_permissions();
    for (perm, method, uri, body) in &cases {
        let label = avtodom_server::auth::permission_def(perm).unwrap().label;
        let without: Vec<&str> = all.iter().copied().filter(|p| p != perm).collect();
        set_admin_permissions(&app, &owner, &without).await;
        let (s, _, err) = call(&app, method, uri, Some(&admin), Some(body())).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{perm} {uri}: {err}");
        assert_eq!(
            err["error"]["message"],
            format!("Нет права: {label}"),
            "{perm} {uri}"
        );
        set_admin_permissions(&app, &owner, &all).await;
        let (s, _, out) = call(&app, method, uri, Some(&admin), Some(body())).await;
        assert_eq!(s, StatusCode::OK, "{perm} {uri}: {out}");
    }
    // Каждое право, кроме скидки (её проверяет чек) и перелива (своя проверка), закрывает
    // хотя бы одно действие из списка.
    for p in avtodom_server::auth::PERMISSIONS {
        if p.key != "discount" && p.key != "oil_transfer" {
            assert!(cases.iter().any(|c| c.0 == p.key), "{} без проверки", p.key);
        }
    }

    // Чек в долг без сети без права проводится, владелец получает причину (инвариант 5).
    let without: Vec<&str> = all.iter().copied().filter(|p| *p != "sale_debt").collect();
    set_admin_permissions(&app, &owner, &without).await;
    let (s, _, out) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                     "party_id": client, "offline": true, "client_time": chrono::Utc::now(),
                     "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": 50_000 }],
                     "payments": [{ "method": "debt", "amount_tyiyn": 50_000 }] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let bypass = actions(&pool, "sale.offline_bypass").await;
    assert!(
        bypass.iter().any(|b| b["reasons"]
            .to_string()
            .contains("продажа в долг без права")),
        "{bypass:?}"
    );
    // Владельцу права не нужны.
    set_admin_permissions(&app, &owner, &[]).await;
    let (s, _, out) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1_000,
                     "account_id": till, "comment": "мыло" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_pays_only_masters(pool: PgPool) {
    // Администратор с правом выплат выдаёт деньги только мастерам; кассиру и остальным —
    // владелец (SPEC-30 §6). Расчёт дня администратору — только мастера.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = named_cashier(&app, &owner, "Айбек").await;
    let mut ids = Vec::new();
    for body in [
        json!({ "full_name": "Мастер Нурлан", "is_master": true }),
        json!({ "full_name": "Уборщица Айгуль" }),
        json!({ "full_name": "Мастер-кассир Бакыт", "is_master": true, "is_cashier": true }),
    ] {
        let (s, _, e) = call(&app, "POST", "/api/v1/employees", Some(&owner), Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{e}");
        ids.push(e["id"].as_str().unwrap().to_string());
    }
    let (master, other, both) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
    for emp in [&cashier, &master, &other, &both] {
        let (s, _, _) = call(
            &app,
            "POST",
            "/api/v1/payroll/accruals",
            Some(&owner),
            Some(
                json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "kind": "bonus",
                         "amount_tyiyn": 50_000, "comment": "за неделю" }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
    }
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
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
    let pay = |who: &str, emp: &str| {
        let (app, who, emp) = (app.clone(), who.to_string(), emp.to_string());
        async move {
            call(
                &app,
                "POST",
                "/api/v1/payouts",
                Some(&who),
                Some(
                    json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "amount_tyiyn": 10_000 }),
                ),
            )
            .await
        }
    };
    let (s, _, err) = pay(&admin, &cashier).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(err["error"]["message"], "Кассиру выплачивает владелец");
    let (s, _, _) = pay(&admin, &both).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, err) = pay(&admin, &other).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{err}");
    let (s, _, out) = pay(&admin, &master).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    for emp in [&cashier, &other, &both] {
        let (s, _, out) = pay(&owner, emp).await;
        assert_eq!(s, StatusCode::OK, "{out}");
    }
    let names = |day: &Value| -> Vec<String> {
        day.as_array()
            .unwrap()
            .iter()
            .map(|r| r["full_name"].as_str().unwrap().to_string())
            .collect()
    };
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&admin), None).await;
    assert_eq!(names(&day), vec!["Мастер Нурлан".to_string()]);
    let (_, _, day) = call(&app, "GET", "/api/v1/payroll/day", Some(&owner), None).await;
    assert_eq!(names(&day).len(), 4);
    // Без права выплат — и мастеру нельзя.
    let all = avtodom_server::auth::default_permissions();
    let without: Vec<&str> = all
        .iter()
        .copied()
        .filter(|p| *p != "payroll_pay")
        .collect();
    set_admin_permissions(&app, &owner, &without).await;
    let (s, _, err) = pay(&admin, &master).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(err["error"]["message"], "Нет права: Выплаты мастерам");
}

// ---------- SPEC-30 §1: оплата «По счёту» ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn invoice_payment_lands_on_its_account(pool: PgPool) {
    // «По счёту» (ADR-095): без клиента — 422; деньги — на счёт по умолчанию для «По счёту», без
    // него — на счёт карты; комиссии нет; возврат — с того же счёта; видно в смене, чеках дня,
    // «Поступлениях» и кабинете юрлица.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (_, card_bank, _) = accounts_of(&app, &owner).await;
    let cashier = cashier_employee(&app, &owner).await;
    let item = product(&app, &owner, "Фильтр по счёту", 50_000, 0).await;
    let inn = "01204201910777";
    let (_, _, firm) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "name": "ОсОО Транзит", "kind": "company", "inn": inn })),
    )
    .await;
    let firm = firm["id"].as_str().unwrap().to_string();
    let (s, _, shift) = call(
        &app,
        "POST",
        "/api/v1/shifts/open",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{shift}");
    let sale = |party: Option<&str>, qty: i64| {
        let mut body = json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": qty, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "invoice", "amount_tyiyn": 50_000 * qty }]
        });
        if let Some(p) = party {
            body["party_id"] = json!(p);
        }
        body
    };
    // Без клиента — 422.
    let (s, _, e) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(None, 1)),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        e["error"]["message"],
        "По счёту платит клиент — выберите его"
    );
    // Счёта для «По счёту» нет — деньги на счёте карты, комиссии нет (у счёта карты она 0,5 %).
    let before = balance_of(&pool, &card_bank).await;
    let (s, _, first) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(Some(&firm), 2)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert_eq!(first["payments"][0]["method"], "invoice");
    assert_eq!(first["payments"][0]["account_id"], json!(card_bank));
    assert_eq!(balance_of(&pool, &card_bank).await, before + 100_000);
    let fees: i64 = sqlx::query_scalar(
        "select coalesce(sum(fee_tyiyn), 0)::bigint from sale_payments where method = 'invoice'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fees, 0);

    // Свой счёт для «По счёту» отмечает владелец; «только владелец» для него нельзя.
    let (s, _, settle) = call(
        &app,
        "POST",
        "/api/v1/cash/accounts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "name": "Расчётный счёт", "kind": "bank",
                     "fee_rate_bp": 70, "default_invoice": true }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{settle}");
    assert_eq!(settle["default_invoice"], true);
    let settle = settle["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/cash/accounts/{settle}"),
        Some(&owner),
        Some(json!({ "owner_only": true })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, second) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(Some(&firm), 1)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{second}");
    assert_eq!(second["payments"][0]["account_id"], json!(settle));
    assert_eq!(balance_of(&pool, &settle).await, 50_000);

    // Возврат — с того же счёта, без комиссии.
    let sid = first["id"].as_str().unwrap();
    let (s, _, ret) = call(
        &app,
        "POST",
        &format!("/api/v1/sales/{sid}/return"),
        Some(&admin),
        Some(
            json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                     "payments": [{ "method": "invoice", "amount_tyiyn": 50_000 }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{ret}");
    assert_eq!(ret["payments"][0]["account_id"], json!(card_bank));
    assert_eq!(balance_of(&pool, &card_bank).await, before + 50_000);
    assert_eq!(balance_of(&pool, &settle).await, 50_000);

    // Смена, чеки дня, «Поступления» — «По счёту» отдельной строкой.
    let (_, _, cur) = call(&app, "GET", "/api/v1/shifts/current", Some(&admin), None).await;
    assert_eq!(cur["invoice_tyiyn"], 100_000);
    let (_, _, day) = call(&app, "GET", "/api/v1/sales", Some(&admin), None).await;
    assert_eq!(day["totals"]["invoice_tyiyn"], 100_000);
    let (_, _, income) = call(&app, "GET", "/api/v1/reports/income", Some(&admin), None).await;
    let row = income["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["method"] == "invoice")
        .unwrap()
        .clone();
    assert_eq!(
        (
            row["sales_tyiyn"].clone(),
            row["returns_tyiyn"].clone(),
            row["fee_tyiyn"].clone()
        ),
        (json!(150_000), json!(50_000), json!(0))
    );
    let (_, _, bank) = call(
        &app,
        "GET",
        &format!("/api/v1/cash/bank?account_id={settle}"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(bank["invoice"]["total_tyiyn"], 50_000);
    assert_eq!(bank["transfer"]["total_tyiyn"], 0);

    // Кабинет юрлица: у чека — сколько «По счёту».
    let (s, cookie, _) = call(
        &app,
        "POST",
        "/api/v1/client/login",
        None,
        Some(json!({ "inn": inn, "password": "avtodom2026" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let c = cookie.unwrap();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/client/password",
        Some(&c),
        Some(json!({ "old_password": "avtodom2026", "new_password": "transit-pass-7" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, sales) = call(&app, "GET", "/api/v1/client/sales", Some(&c), None).await;
    let mine = sales
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == second["id"])
        .unwrap()
        .clone();
    assert_eq!(mine["invoice_tyiyn"], 50_000);

    // Отметку можно снять — «По счёту» снова идёт на счёт карты.
    let (s, _, a) = call(
        &app,
        "PATCH",
        &format!("/api/v1/cash/accounts/{settle}"),
        Some(&owner),
        Some(json!({ "default_invoice": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(a["default_invoice"], false);
    let (_, _, third) = call(
        &app,
        "POST",
        "/api/v1/sales",
        Some(&admin),
        Some(sale(Some(&firm), 1)),
    )
    .await;
    assert_eq!(third["payments"][0]["account_id"], json!(card_bank));

    // Долг юрлица гасится «По счёту»: на тот же безналичный счёт, без комиссии.
    let mut debt = sale(Some(&firm), 1);
    debt["payments"] = json!([{ "method": "debt", "amount_tyiyn": 50_000 }]);
    let (s, _, d) = call(&app, "POST", "/api/v1/sales", Some(&admin), Some(debt)).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let before = balance_of(&pool, &card_bank).await;
    let (s, _, r) = call(
        &app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": firm, "amount_tyiyn": 50_000, "method": "invoice" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(balance_of(&pool, &card_bank).await, before + 50_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_catalog_found_by_typed_code(pool: PgPool) {
    // Замечание заказчика 26.10.2026: «в приходе по коду чтобы тоже искал» — набранный код
    // (штрихкод, «доп. код», артикул; целиком, по началу, без потерянных нулей) находит строку Умага.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let rows = json!([
        { "name": "AZMOL DISEL 10W40 200L", "barcode": "2760458", "extra_code": "1459", "unit": "шт" },
        { "name": "LED ЛАМПА M3 H11", "barcode": "2110000050825", "extra_code": "2011303430805", "unit": "шт" },
        { "name": "Фильтр C-110", "barcode": "123456789012", "article": "C-110", "unit": "шт" },
        { "name": "ГАЗПРОМ ЛИТОЛ 24", "barcode": "4670026845129", "unit": "шт" }
    ]);
    let (s, _, _) = call(&app, "PUT", "/api/v1/old-catalog", Some(&owner), Some(rows)).await;
    assert_eq!(s, StatusCode::OK);
    let first = |q: &str| {
        let app = app.clone();
        let owner = owner.clone();
        let uri = format!("/api/v1/old-catalog/search?q={q}");
        async move {
            let (s, _, hits) = call(&app, "GET", &uri, Some(&owner), None).await;
            assert_eq!(s, StatusCode::OK, "{hits}");
            hits
        }
    };
    // «Доп. код» целиком — точное совпадение.
    let hits = first("1459").await;
    assert_eq!(hits[0]["name"], "AZMOL DISEL 10W40 200L", "{hits}");
    assert_eq!(hits[0]["exact"], json!(true));
    // Штрихкод по началу, пока набирают.
    let hits = first("21100000508").await;
    assert_eq!(hits[0]["name"], "LED ЛАМПА M3 H11", "{hits}");
    assert_eq!(hits[0]["exact"], json!(false));
    // «Доп. код» второй строки целиком и заводской код с нулём впереди, потерянным Excel.
    assert_eq!(first("2011303430805").await[0]["name"], "LED ЛАМПА M3 H11");
    assert_eq!(first("0123456789012").await[0]["exact"], json!(true));
    // «Доп. код» по началу — тоже (SPEC-35 §2), но это не точное совпадение; меньше четырёх
    // знаков — не начало кода.
    let hits = first("20113034").await;
    assert_eq!(hits[0]["name"], "LED ЛАМПА M3 H11", "{hits}");
    assert_eq!(hits[0]["exact"], json!(false));
    let hits = first("201").await;
    assert_eq!(hits.as_array().map(Vec::len), Some(0), "{hits}");
    // Артикул латиницей с дефисом, без учёта регистра.
    let hits = first("c-110").await;
    assert_eq!(hits[0]["name"], "Фильтр C-110", "{hits}");
    assert_eq!(hits[0]["exact"], json!(true));
    // Поиск по названию как раньше.
    assert_eq!(
        first("%D0%BB%D0%B8%D1%82%D0%BE%D0%BB").await[0]["name"],
        "ГАЗПРОМ ЛИТОЛ 24"
    );
    // Чужие цифры ничего не находят.
    let hits = first("99999999").await;
    assert_eq!(hits.as_array().map(Vec::len), Some(0), "{hits}");
}
