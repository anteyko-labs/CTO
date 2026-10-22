//! Доработки по проверке на точке, часть 4 (SPEC-30 §11–13): порядок найденного в поиске товара,
//! «Товары» с отборами бывших «Остатков», баллы у всех клиентов.
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
    // Администратор с правами по умолчанию, как после миграции 0056 (ADR-097).
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
) -> (StatusCode, Value) {
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

async fn category(app: &Router, owner: &str, name: &str, kind: &str) -> Value {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/categories",
        Some(owner),
        Some(json!({ "name": name, "kind": kind })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    c
}

async fn product(app: &Router, owner: &str, body: Value) -> Value {
    let mut body = body;
    body["op_id"] = json!(Uuid::now_v7());
    let (s, p) = call(app, "POST", "/api/v1/products", Some(owner), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{p}");
    p
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn search_puts_exact_code_and_name_start_first(pool: PgPool) {
    // 21.10.2026: кассир набрал моторное масло, а первым в найденном стоял воздушный фильтр.
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let oil = category(&app, &owner, "Масла", "oil").await;
    let filters = category(&app, &owner, "Фильтры", "filter").await;
    // По алфавиту фильтр «Воздушный…» стоит раньше масла — раньше он и выходил первым.
    product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "Воздушный фильтр для масла", "sale_price_tyiyn": 30_000 }),
    )
    .await;
    product(
        &app,
        &owner,
        json!({ "category_id": oil["id"], "name": "Масло моторное 5W-30", "container_ml": 4000, "sale_price_tyiyn": 150_000 }),
    )
    .await;
    product(
        &app,
        &owner,
        json!({ "category_id": oil["id"], "name": "Масло трансмиссионное", "container_ml": 1000, "sale_price_tyiyn": 120_000 }),
    )
    .await;
    let (s, found) = call(&app, "GET", "/api/v1/products?q=масл", Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK, "{found}");
    let mut found = names(&found);
    // Внутри одной ступени — по названию (порядок букв зависит от настроек базы).
    found[..2].sort();
    assert_eq!(
        found,
        [
            "Масло моторное 5W-30",
            "Масло трансмиссионное",
            "Воздушный фильтр для масла"
        ]
    );

    // Точный артикул и штрихкод — раньше названия с начала; часть артикула — в остальном.
    product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "Фильтр OC905", "article": "OC905", "sale_price_tyiyn": 40_000 }),
    )
    .await;
    product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "OC90 переходник", "sale_price_tyiyn": 10_000 }),
    )
    .await;
    product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "Фильтр масляный Knecht", "article": "oc90", "sale_price_tyiyn": 45_000 }),
    )
    .await;
    let (_, found) = call(&app, "GET", "/api/v1/products?q=OC90", Some(&owner), None).await;
    assert_eq!(
        names(&found),
        ["Фильтр масляный Knecht", "OC90 переходник", "Фильтр OC905"]
    );
    let coded = product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "Ячейка 4607", "barcodes": ["4607001234567"], "sale_price_tyiyn": 1_000 }),
    )
    .await;
    product(
        &app,
        &owner,
        json!({ "category_id": filters["id"], "name": "4607001234567 набор", "sale_price_tyiyn": 1_000 }),
    )
    .await;
    let (_, found) = call(
        &app,
        "GET",
        "/api/v1/products?q=4607001234567",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(found[0]["id"], coded["id"], "{found}");

    // Отбор по виду категории работает, как раньше.
    let (_, only_oil) = call(
        &app,
        "GET",
        "/api/v1/products?q=масл&kind=oil",
        Some(&owner),
        None,
    )
    .await;
    let mut only_oil = names(&only_oil);
    only_oil.sort();
    assert_eq!(only_oil, ["Масло моторное 5W-30", "Масло трансмиссионное"]);
    // Список товаров по-прежнему сортирует сам: по названию, без ступеней поиска.
    let (_, list) = call(
        &app,
        "GET",
        "/api/v1/products/list?q=масл",
        Some(&owner),
        None,
    )
    .await;
    let listed: Vec<String> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    let mut by_name = listed.clone();
    by_name.sort_by_key(|n| n.to_lowercase());
    assert_eq!(listed, by_name);
    assert_eq!(list["totals"]["count"], 4);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn products_list_takes_over_stock_filters(pool: PgPool) {
    // SPEC-30 §11: «Остатки» вошли в «Товары» — отборы по складу, залежалые, итоги по отбору.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cat = category(&app, &owner, "Лампы", "other").await;
    let mk = |name: &'static str, min: i64| json!({ "category_id": cat["id"], "name": name, "sale_price_tyiyn": 10_000, "min_stock": min });
    let plenty = product(&app, &owner, mk("Лампа H4", 2)).await;
    let low = product(&app, &owner, mk("Лампа H7", 5)).await;
    let none = product(&app, &owner, mk("Лампа H1", 1)).await;
    let (_, safe) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let safe = safe
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф владельца")
        .unwrap()["id"]
        .clone();
    let (s, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": safe, "lines": [
            { "product_id": plenty["id"], "qty": 10, "cost_tyiyn": 60_000 },
            { "product_id": low["id"], "qty": 3, "cost_tyiyn": 18_000 },
        ] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    // «Лампа H4» будто пришла 90 дней назад и с тех пор не продавалась — залежалась.
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id, created_at)
         values ($1, $2, $3, 0, 0, 'receipt', $1, now() - interval '90 days')",
    )
    .bind(Uuid::now_v7())
    .bind(branch)
    .bind(Uuid::parse_str(plenty["id"].as_str().unwrap()).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let (s, all) = call(
        &app,
        "GET",
        "/api/v1/products/list?q=Лампа",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{all}");
    let counts = &all["totals"]["counts"];
    assert_eq!(
        (
            &counts["all"],
            &counts["low"],
            &counts["out"],
            &counts["stale"],
            &counts["review"]
        ),
        (&json!(3), &json!(2), &json!(1), &json!(1), &json!(0))
    );
    assert_eq!(all["totals"]["stale_days"], 60);
    let h4 = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == plenty["id"])
        .unwrap();
    assert_eq!(h4["stale"]["sold_ever"], false);
    assert!(h4["stale"]["days"].as_i64().unwrap() >= 89);

    // «Ниже минимума»: H7 (3 из 5) и H1 (0 из 1); итоги — по отбору.
    let (_, below) = call(
        &app,
        "GET",
        "/api/v1/products/list?q=Лампа&stock=low",
        Some(&owner),
        None,
    )
    .await;
    let ids: Vec<&Value> = below["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["id"])
        .collect();
    assert_eq!(ids, [&none["id"], &low["id"]]);
    assert_eq!(below["totals"]["count"], 2);
    assert_eq!(below["totals"]["purchase_tyiyn"], 18_000);
    assert_eq!(below["totals"]["sale_tyiyn"], 30_000);
    // Числа отборов — при том же поиске, а не только по выбранному отбору.
    assert_eq!(below["totals"]["counts"]["all"], 3);

    let (_, out) = call(
        &app,
        "GET",
        "/api/v1/products/list?q=Лампа&stock=out",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(out["items"][0]["id"], none["id"]);
    assert_eq!(out["totals"]["count"], 1);
    // Администратору — без закупки и в отборе.
    assert!(out["totals"].get("purchase_tyiyn").is_none());
    let (_, stale) = call(
        &app,
        "GET",
        "/api/v1/products/list?q=Лампа&stock=stale&sort=stock_purchase&dir=desc",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(stale["items"][0]["id"], plenty["id"]);
    assert_eq!(stale["totals"]["sale_tyiyn"], 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn points_of_all_clients_on_clients_summary_and_dashboard(pool: PgPool) {
    // SPEC-30 §12: сколько баллов у всех клиентов вместе (1 балл = 1 сом).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut parties = Vec::new();
    for name in ["Баллов Первый", "Баллов Второй", "Баллов Третий"]
    {
        let (s, p) = call(
            &app,
            "POST",
            "/api/v1/parties",
            Some(&admin),
            Some(json!({ "role": "customer", "kind": "company", "name": name })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{p}");
        parties.push(Uuid::parse_str(p["id"].as_str().unwrap()).unwrap());
    }
    // Первый накопил 12,50 и 7,50; второй накопил 3 и всё списал; у третьего баллов не было.
    for (party, kind, amount) in [
        (parties[0], "accrual", 1_250i64),
        (parties[0], "accrual", 750),
        (parties[1], "accrual", 300),
        (parties[1], "redeem", -300),
    ] {
        sqlx::query("insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, doc_type, doc_id) values ($1, $2, $3, $4, $5, 'sale', $6)")
            .bind(Uuid::now_v7())
            .bind(branch)
            .bind(party)
            .bind(kind)
            .bind(amount)
            .bind(Uuid::now_v7())
            .execute(&pool)
            .await
            .unwrap();
    }
    // Итог — по всем клиентам, отбор экрана его не меняет.
    let (s, sum) = call(
        &app,
        "GET",
        "/api/v1/parties/summary?role=customer&q=Второй",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sum}");
    assert_eq!(sum["points_tyiyn"], 2_000);
    assert_eq!(sum["points_clients"], 1);

    let (s, d) = call(&app, "GET", "/api/v1/owner/dashboard", Some(&owner), None).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["points"]["points_tyiyn"], 2_000);
    assert_eq!(d["points"]["clients"], 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cashier_debtors_are_only_those_who_owe(pool: PgPool) {
    // Окно «Оплата долга» на кассе: только должники — авансы не вытесняют их из списка.
    let app = setup(pool.clone()).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let mut ids = Vec::new();
    for (name, balance) in [
        ("Должник Касса", 425_000i64),
        ("Аванс Касса", -10_000),
        ("Ровно Касса", 0),
    ] {
        let (_, p) = call(
            &app,
            "POST",
            "/api/v1/parties",
            Some(&admin),
            Some(json!({ "role": "customer", "kind": "company", "name": name })),
        )
        .await;
        let id = Uuid::parse_str(p["id"].as_str().unwrap()).unwrap();
        sqlx::query("update parties set balance_tyiyn = $2 where id = $1")
            .bind(id)
            .bind(balance)
            .execute(&pool)
            .await
            .unwrap();
        ids.push(id);
    }
    let (s, owe) = call(
        &app,
        "GET",
        "/api/v1/parties?role=customer&owes=true",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{owe}");
    assert_eq!(names(&owe), ["Должник Касса"]);
    let (_, both) = call(
        &app,
        "GET",
        "/api/v1/parties?role=customer&only_debtors=true",
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(both.as_array().unwrap().len(), 2);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_creation_needs_receipts_and_owner_accounts_hidden(pool: PgPool) {
    // Нового поставщика заводят с правом «Приход товара» — и `POST /suppliers`, и `POST /parties`
    // с ролью «поставщик»; счета «только владелец» администратору не видны ни в списке, ни в
    // снимке кассы (SPEC-30, проверка вживую).
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let supplier = |name: &str| json!({ "name": name, "phone": "", "comment": "" });
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(supplier("Смазка")),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    sqlx::query("delete from user_permissions where permission = 'receipts'")
        .execute(&pool)
        .await
        .unwrap();
    let (s, e) = call(
        &app,
        "POST",
        "/api/v1/suppliers",
        Some(&admin),
        Some(supplier("Фильтры")),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "role": "supplier", "kind": "company", "name": "Шины" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Покупателя — по-прежнему с правом «Клиенты и машины».
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/parties",
        Some(&admin),
        Some(json!({ "role": "customer", "kind": "company", "name": "Такси" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, acc) = call(
        &app,
        "POST",
        "/api/v1/cash/accounts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "name": "Личная карта", "kind": "bank",
                     "allow_negative": false, "owner_only": true }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{acc}");
    let hidden = acc["id"].clone();
    let (_, list) = call(&app, "GET", "/api/v1/cash/accounts", Some(&admin), None).await;
    assert!(list.as_array().unwrap().iter().all(|a| a["id"] != hidden));
    let (_, list) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    assert!(list.as_array().unwrap().iter().any(|a| a["id"] == hidden));
    let (s, snap) = call(&app, "GET", "/api/v1/offline/snapshot", Some(&admin), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        snap["bank_accounts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["id"] != hidden)
    );
    let (s, _) = call(
        &app,
        "GET",
        &format!("/api/v1/cash/accounts/{}", hidden.as_str().unwrap()),
        Some(&admin),
        None,
    )
    .await;
    assert_ne!(s, StatusCode::OK);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stock_sort_keeps_units_apart(pool: PgPool) {
    // Сортировка «Остаток»: литры и штуки не смешиваются — сначала штуки, потом литры,
    // внутри — по количеству в выбранном направлении.
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let other = category(&app, &owner, "Разное Юнит", "other").await;
    let oils = category(&app, &owner, "Масла Юнит", "oil").await;
    let few = product(
        &app,
        &owner,
        json!({ "category_id": other["id"], "name": "Юнит штука 2", "sale_price_tyiyn": 1_000 }),
    )
    .await;
    let many = product(
        &app,
        &owner,
        json!({ "category_id": other["id"], "name": "Юнит штука 9", "sale_price_tyiyn": 1_000 }),
    )
    .await;
    let oil = product(
        &app,
        &owner,
        json!({ "category_id": oils["id"], "name": "Юнит масло", "sale_price_tyiyn": 100_000, "container_ml": 4_000 }),
    )
    .await;
    let (_, accs) = call(&app, "GET", "/api/v1/cash/accounts", Some(&owner), None).await;
    let safe = accs
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф владельца")
        .unwrap()["id"]
        .clone();
    let (s, r) = call(
        &app,
        "POST",
        "/api/v1/receipts",
        Some(&owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "paid_from": safe, "lines": [
            { "product_id": few["id"], "qty": 2, "cost_tyiyn": 1_000 },
            { "product_id": many["id"], "qty": 9, "cost_tyiyn": 4_500 },
            { "product_id": oil["id"], "qty": 8_000, "cost_tyiyn": 100_000 },
        ] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    for (dir, want) in [
        ("asc", ["Юнит штука 2", "Юнит штука 9", "Юнит масло"]),
        ("desc", ["Юнит штука 9", "Юнит штука 2", "Юнит масло"]),
    ] {
        let (s, list) = call(
            &app,
            "GET",
            &format!("/api/v1/products/list?q=Юнит&sort=stock&dir={dir}"),
            Some(&owner),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{list}");
        assert_eq!(names(&list["items"]), want, "{dir}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn service_oil_kind_by_name_and_no_receipt_return_owner_only(pool: PgPool) {
    // Категория масла у услуги (ADR-104): без выбора — по названию; выбрать — только владельцу.
    // Возврат без чека (ADR-105): администратору — 403.
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let motor = category(&app, &owner, "Моторные масла", "oil").await;
    let gear = category(&app, &owner, "Трансмиссионные масла", "oil").await;
    let create = |name: &str| json!({ "name": name, "price_tyiyn": 20000, "master_fee_tyiyn": 3000, "free_with_oil": true });
    let (s, m) = call(
        &app,
        "POST",
        "/api/v1/services",
        Some(&owner),
        Some(create("Замена в моторе")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["oil_category_id"], motor["id"]);
    let (_, g) = call(
        &app,
        "POST",
        "/api/v1/services",
        Some(&owner),
        Some(create("Замена масла АКПП")),
    )
    .await;
    assert_eq!(g["oil_category_id"], gear["id"]);
    let uri = format!("/api/v1/services/{}", m["id"].as_str().unwrap());
    let (s, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&admin),
        Some(json!({ "oil_category_id": gear["id"] })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, m2) = call(
        &app,
        "PATCH",
        &uri,
        Some(&owner),
        Some(json!({ "oil_category_id": gear["id"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m2}");
    assert_eq!(m2["oil_category_id"], gear["id"]);
    // Не масло — не категория масла.
    let other = category(&app, &owner, "Автохимия", "other").await;
    let (s, _) = call(
        &app,
        "PATCH",
        &uri,
        Some(&owner),
        Some(json!({ "oil_category_id": other["id"] })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let body = json!({
        "op_id": Uuid::now_v7(), "method": "card", "comment": "без чека",
        "lines": [{ "kind": "piece", "product_id": Uuid::now_v7(), "qty": 1, "amount_tyiyn": 100 }],
    });
    let (s, _) = call(
        &app,
        "POST",
        "/api/v1/sales/return-without-receipt",
        Some(&admin),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
