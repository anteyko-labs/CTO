//! Находки проверки 27.10.2026 по серверу (ADR-128): склад, деньги, права, вход.
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
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let mut json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(c) = cookie
        && uri.ends_with("/auth/login")
    {
        json = json!(c);
    }
    (status, json)
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, Some(cookie), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn get(app: &Router, uri: &str, cookie: &str) -> Value {
    let (s, v) = call(app, "GET", uri, Some(cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    c.as_str().unwrap().to_string()
}

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    safe: String,
}

impl World {
    async fn new(pool: PgPool) -> Self {
        let app = setup(pool.clone()).await;
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        let e = ok(
            &app,
            "POST",
            "/api/v1/employees",
            &owner,
            json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" }),
        )
        .await;
        let cashier = e["id"].as_str().unwrap().to_string();
        let accounts = get(&app, "/api/v1/cash/accounts", &owner).await;
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
        let safe = find(&|a| a["owner_only"] == json!(true));
        ok(
            &app,
            "POST",
            "/api/v1/shifts/open",
            &admin,
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
        )
        .await;
        World {
            app,
            pool,
            owner,
            admin,
            cashier,
            safe,
        }
    }

    async fn product(&self, name: &str, price: i64) -> String {
        let cat = ok(
            &self.app,
            "POST",
            "/api/v1/categories",
            &self.owner,
            json!({ "name": format!("Кат {name}"), "kind": "filter" }),
        )
        .await;
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/products",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name, "sale_price_tyiyn": price }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    /// Приход за деньги из сейфа: товар на склад по цене `cost` за штуку.
    async fn stock(&self, product: &str, qty: i64, cost: i64) {
        ok(
            &self.app,
            "POST",
            "/api/v1/receipts",
            &self.owner,
            json!({
                "op_id": Uuid::now_v7(), "paid_from": self.safe,
                "lines": [{ "product_id": product, "qty": qty, "cost_tyiyn": qty * cost }]
            }),
        )
        .await;
    }

    async fn supplier(&self, name: &str) -> String {
        let s = ok(
            &self.app,
            "POST",
            "/api/v1/suppliers",
            &self.owner,
            json!({ "name": name }),
        )
        .await;
        s["id"].as_str().unwrap().to_string()
    }

    async fn oil(&self, cat: &Value, name: &str, price: i64) -> String {
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/products",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
                    "container_ml": 4000, "sale_price_tyiyn": price }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    async fn stock_value(&self, product: &str) -> (i64, i64) {
        sqlx::query_as(
            "select stock_qty, stock_value_tyiyn from branch_products where product_id = $1::uuid",
        )
        .bind(product)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn sale(&self, lines: Value, payments: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/sales",
            Some(&self.admin),
            Some(json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
                "lines": lines, "payments": payments
            })),
        )
        .await
    }

    async fn draft(&self, kind: &str, id: &str) -> Value {
        get(
            &self.app,
            &format!("/api/v1/corrections/draft/{kind}/{id}"),
            &self.owner,
        )
        .await
    }

    async fn correct(&self, op_id: Uuid, kind: &str, id: &str, doc: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/corrections",
            Some(&self.owner),
            Some(json!({ "op_id": op_id, "kind": kind, "original_id": id, "comment": "ошибка ввода", "doc": doc })),
        )
        .await
    }

    /// Сумма начислений 2 % кассиру.
    async fn percent(&self) -> i64 {
        sqlx::query_scalar(
            "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'revenue_percent'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn verify_stock(&self) {
        let v = get(&self.app, "/api/v1/stock/verify", &self.owner).await;
        assert_eq!(v, json!([]), "остаток = сумма движений");
    }
}

// ---------- Склад ----------

/// Отмена перелива после продаж из получателя: остаток не дороже самой дорогой живой
/// закупки получателя и не по нулю (ADR-087 и для перелива).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_transfer_reversal_keeps_cost_within_purchases(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = ok(
        &w.app,
        "POST",
        "/api/v1/categories",
        &w.owner,
        json!({ "name": "Масла", "kind": "oil" }),
    )
    .await;
    // (дорогое своё, дешёвое перелитое, своя цена за литр после отмены)
    for (own, poured, left) in [(6_000, 3_000, 6_000), (3_000, 5_000, 3_000)] {
        let to = w.oil(&cat, &format!("Получатель {own}"), 400_000).await;
        let from = w.oil(&cat, &format!("Источник {own}"), 400_000).await;
        w.stock(&to, 4000, own / 1000).await;
        w.stock(&from, 4000, poured / 1000).await;
        let t = ok(
            &w.app,
            "POST",
            "/api/v1/oil/transfers",
            &w.owner,
            json!({ "op_id": Uuid::now_v7(), "from_product_id": from, "to_product_id": to, "qty_ml": 4000 }),
        )
        .await;
        let (s, v) = w
            .sale(
                json!([{ "kind": "pour", "product_id": to, "qty": 3000, "unit_price_tyiyn": 100_000 }]),
                json!([{ "method": "cash", "amount_tyiyn": 300_000 }]),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        ok(
            &w.app,
            "POST",
            &format!(
                "/api/v1/oil/transfers/{}/reverse",
                t["id"].as_str().unwrap()
            ),
            &w.owner,
            json!({ "op_id": Uuid::now_v7(), "comment": "не туда" }),
        )
        .await;
        assert_eq!(w.stock_value(&to).await, (1000, left), "своё {own}");
    }
    w.verify_stock().await;
}

/// Излишек ревизии у товара без закупки стоит 0: продажи из него — «без цены закупки»
/// (2 % ждут цены владельца, ADR-062), «Цена закупки» переоценивает остаток, «Проверено»
/// без цены не снимается.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_surplus_without_price_waits_for_owner_price(pool: PgPool) {
    let w = World::new(pool).await;
    let item = w.product("Свеча", 15_000).await;
    let rev = ok(&w.app, "POST", "/api/v1/revisions", &w.owner, json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    ok(
        &w.app,
        "PUT",
        &format!("/api/v1/revisions/{rid}/lines"),
        &w.owner,
        json!({ "product_id": item, "counted_qty": 10, "op_id": Uuid::now_v7() }),
    )
    .await;
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/revisions/{rid}/post"),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "пересчитал Айбек" }),
    )
    .await;
    assert_eq!(w.stock_value(&item).await, (10, 0));
    let (s, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/stock/{item}/review-done"),
        Some(&w.owner),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let (s, v) = w
        .sale(
            json!([{ "kind": "piece", "product_id": item, "qty": 2, "unit_price_tyiyn": 15_000 }]),
            json!([{ "method": "cash", "amount_tyiyn": 30_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let unknown: bool = sqlx::query_scalar(
        "select cost_unknown from sale_lines where sale_id = $1::uuid and line_no = 1",
    )
    .bind(v["id"].as_str().unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert!(unknown, "строка без цены закупки");
    assert_eq!(w.percent().await, 0, "2 % ждут цены владельца");
    // Владелец указал цену закупки 100 с: 2 % с маржи 2 × 50 с, остаток 8 шт по 100 с.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/stock/{item}/purchase-price"),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "price_tyiyn": 10_000 }),
    )
    .await;
    assert_eq!(w.percent().await, 200);
    assert_eq!(w.stock_value(&item).await, (8, 80_000));
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/stock/{item}/review-done"),
        &w.owner,
        json!({}),
    )
    .await;
    w.verify_stock().await;
}

// ---------- Деньги ----------

/// Погашение долга «По счёту» — не карта: в сверке с банком его нет среди операций по карте,
/// в разбивке безнала оно в «По счёту».
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn invoice_repayment_is_not_card(pool: PgPool) {
    let w = World::new(pool).await;
    let item = w.product("Фильтр", 50_000).await;
    w.stock(&item, 10, 10_000).await;
    let firm = ok(
        &w.app,
        "POST",
        "/api/v1/parties",
        &w.admin,
        json!({ "name": "ОсОО Транзит", "kind": "company", "inn": "01204201910777" }),
    )
    .await;
    let firm = firm["id"].as_str().unwrap().to_string();
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/sales",
        Some(&w.admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
            "party_id": firm,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 2, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    for (method, amount) in [("invoice", 50_000), ("card", 10_000)] {
        ok(
            &w.app,
            "POST",
            "/api/v1/debts/repayments",
            &w.admin,
            json!({ "op_id": Uuid::now_v7(), "party_id": firm, "amount_tyiyn": amount, "method": method }),
        )
        .await;
    }
    let bank = get(&w.app, "/api/v1/cash/bank", &w.owner).await;
    assert_eq!(bank["card"]["repayments_tyiyn"], 10_000, "{bank}");
    assert_eq!(bank["invoice"]["repayments_tyiyn"], 50_000, "{bank}");
    assert_eq!(bank["invoice"]["total_tyiyn"], 50_000, "{bank}");
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let r = get(
        &w.app,
        &format!("/api/v1/bank/reconcile?from={today}&to={today}"),
        &w.owner,
    )
    .await;
    assert_eq!(r["days"][0]["ours_tyiyn"], 10_000, "{r}");
    assert_eq!(r["days"][0]["ours_count"], 1, "{r}");
}

/// Чек в долг, долг частично погашен — «Исправить» проходит, и итог как у сразу верного чека:
/// тот же долг клиента и тот же процент кассира.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_sale_after_partial_repayment_can_be_corrected(pool: PgPool) {
    let w = World::new(pool).await;
    let item = w.product("Фильтр", 20_000).await;
    w.stock(&item, 10, 5_000).await;
    let mut out = Vec::new();
    for (name, inn, qty, debt) in [
        ("ОсОО Исправленный", "01204201910111", 2, 30_000),
        ("ОсОО Верный", "01204201910222", 1, 10_000),
    ] {
        let firm = ok(
            &w.app,
            "POST",
            "/api/v1/parties",
            &w.admin,
            json!({ "name": name, "kind": "company", "inn": inn }),
        )
        .await;
        let firm = firm["id"].as_str().unwrap().to_string();
        let before = w.percent().await;
        let (s, sale) = call(
            &w.app,
            "POST",
            "/api/v1/sales",
            Some(&w.admin),
            Some(json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
                "party_id": firm,
                "lines": [{ "kind": "piece", "product_id": item, "qty": qty, "unit_price_tyiyn": 20_000 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 10_000 },
                             { "method": "debt", "amount_tyiyn": debt }]
            })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{sale}");
        ok(
            &w.app,
            "POST",
            "/api/v1/debts/repayments",
            &w.admin,
            json!({ "op_id": Uuid::now_v7(), "party_id": firm, "amount_tyiyn": 10_000, "method": "cash" }),
        )
        .await;
        if qty == 2 {
            let sid = sale["id"].as_str().unwrap().to_string();
            let mut draft = w.draft("sale", &sid).await;
            draft["doc"]["lines"][0]["qty"] = json!(1);
            draft["doc"]["payments"] = json!([{ "method": "cash", "amount_tyiyn": 10_000 },
                                              { "method": "debt", "amount_tyiyn": 10_000 }]);
            let (s, v) = w
                .correct(Uuid::now_v7(), "sale", &sid, draft["doc"].clone())
                .await;
            assert_eq!(s, StatusCode::OK, "{v}");
        }
        let balance: i64 =
            sqlx::query_scalar("select balance_tyiyn from parties where id = $1::uuid")
                .bind(&firm)
                .fetch_one(&w.pool)
                .await
                .unwrap();
        out.push((balance, w.percent().await - before));
    }
    assert_eq!(out[0], out[1]);
    w.verify_stock().await;
}

/// Возврат масла с удержанием за бесплатную замену: удержанное — выручка услуги, и 2 % кассира
/// с неё остаются (ADR-104): снимается процент только с того, что ушло из валовой.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_return_withholding_keeps_percent_on_withheld(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = ok(
        &w.app,
        "POST",
        "/api/v1/categories",
        &w.owner,
        json!({ "name": "Масла", "kind": "oil" }),
    )
    .await;
    let oil = w.oil(&cat, "Mobil 5W-30", 400_000).await;
    // Канистра 4 л: закупка 3 000 с, продажа 4 000 с.
    w.stock(&oil, 16_000, 75).await;
    let master = ok(
        &w.app,
        "POST",
        "/api/v1/employees",
        &w.owner,
        json!({ "full_name": "Мастер", "is_master": true }),
    )
    .await;
    let service = ok(
        &w.app,
        "POST",
        "/api/v1/services",
        &w.owner,
        json!({ "name": "Замена масла", "price_tyiyn": 50_000, "master_fee_tyiyn": 3_000,
                "free_with_oil": true, "oil_category_id": cat["id"] }),
    )
    .await;
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name, first_name, last_name, phone) select $1, id, 'customer', 'person', 'Асанов Бакыт', 'Бакыт', 'Асанов', '0700 11-22-33' from branches")
        .bind(party)
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) select $1, id, $2, '01KG900XYZ', '01KG900XYZ' from branches")
        .bind(vehicle)
        .bind(party)
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, sale) = call(
        &w.app,
        "POST",
        "/api/v1/sales",
        Some(&w.admin),
        Some(json!({
            "op_id": Uuid::now_v7(), "sale_type": "service", "cashier_id": w.cashier,
            "master_id": master["id"], "party_id": party, "vehicle_id": vehicle, "mileage_km": 85_000,
            "lines": [{ "kind": "container", "product_id": oil, "qty": 1, "unit_price_tyiyn": 400_000 },
                      { "kind": "service", "service_id": service["id"], "qty": 1, "unit_price_tyiyn": 0 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 400_000 }]
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(w.percent().await, 2_000);
    // Канистру вернули: 500 с удержано за замену, 3 500 с наличными.
    let r = ok(
        &w.app,
        "POST",
        &format!("/api/v1/sales/{}/return", sale["id"].as_str().unwrap()),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 350_000 }] }),
    )
    .await;
    assert_eq!(r["total_tyiyn"], -350_000, "{r}");
    // Валовая чека и возврата — 500 с удержания: 2 % = 10 с.
    assert_eq!(w.percent().await, 1_000);
}

// ---------- Права ----------

/// Администратор без права «Приход товара» поставщика не правит и не отключает, без права
/// «Товары и штрихкоды» — настройку этикеток.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_and_labels_need_permission(pool: PgPool) {
    let w = World::new(pool).await;
    let supplier = w.supplier("Поставщик А").await;
    let labels = get(&w.app, "/api/v1/settings/labels", &w.admin).await;
    let tries = |admin: String| {
        let (app, supplier, labels) = (w.app.clone(), supplier.clone(), labels.clone());
        async move {
            let mut out = Vec::new();
            for (method, uri, body) in [
                (
                    "PATCH",
                    format!("/api/v1/suppliers/{supplier}"),
                    json!({ "name": "Переименован", "active": false }),
                ),
                (
                    "PATCH",
                    format!("/api/v1/parties/{supplier}"),
                    json!({ "name": "Переименован 2" }),
                ),
                ("PUT", "/api/v1/settings/labels".to_string(), labels.clone()),
            ] {
                let (s, _) = call(&app, method, &uri, Some(&admin), Some(body)).await;
                out.push(s);
            }
            out
        }
    };
    sqlx::query("delete from user_permissions where permission in ('receipts', 'products')")
        .execute(&w.pool)
        .await
        .unwrap();
    assert_eq!(tries(w.admin.clone()).await, vec![StatusCode::FORBIDDEN; 3]);
    let name: String = sqlx::query_scalar("select name from suppliers where id = $1::uuid")
        .bind(&supplier)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(name, "Поставщик А");
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select id, branch_id, unnest(array['receipts', 'products']) from users where login = 'admin'")
        .execute(&w.pool)
        .await
        .unwrap();
    assert_eq!(tries(w.admin.clone()).await, vec![StatusCode::OK; 3]);
}

/// Черновик бота в закрытом филиале не проводится: закрытый — только для чтения, как на экране.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_does_not_post_in_closed_branch(pool: PgPool) {
    use avtodom_server::api::bot_drafts::{DraftOp, execute};
    use avtodom_server::auth::{Ctx, CurrentUser, Role};
    let w = World::new(pool).await;
    let b = ok(
        &w.app,
        "POST",
        "/api/v1/branches",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б" }),
    )
    .await;
    let b: Uuid = b["id"].as_str().unwrap().parse().unwrap();
    ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/branches/{b}"),
        &w.owner,
        json!({ "archived": true }),
    )
    .await;
    let (owner_id, login, full_name): (Uuid, String, String) =
        sqlx::query_as("select id, login, full_name from users where login = 'owner'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let article: Uuid = sqlx::query_scalar("select id from expense_articles limit 1")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let ctx = Ctx::bot(
        CurrentUser {
            id: owner_id,
            branch_id: b,
            login,
            full_name,
            role: Role::Owner,
        },
        970_001,
    );
    let op: DraftOp = serde_json::from_value(json!({
        "op": "expense", "op_id": Uuid::now_v7(), "article_id": article,
        "amount_tyiyn": 10_000, "source": "account", "comment": "из бота"
    }))
    .unwrap();
    let mut tx = w.pool.begin().await.unwrap();
    match execute(&mut tx, &ctx, op).await {
        Err(avtodom_server::error::AppError::Conflict(m)) => {
            assert!(m.contains("филиал закрыт"), "{m}")
        }
        Err(e) => panic!("ждали «филиал закрыт», получили {e:?}"),
        Ok(p) => panic!("проведено в закрытом филиале: {}", p.text),
    }
}

// ---------- Вход ----------

/// Поток неверных входов со случайными логинами: проверок argon2 одновременно не больше
/// нескольких, лишние сразу получают 429; адрес после 50 неудач ждёт окно.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn login_flood_is_bounded(pool: PgPool) {
    let w = World::new(pool).await;
    let tries: Vec<_> = (0..70)
        .map(|i| {
            let app = w.app.clone();
            tokio::spawn(async move {
                call(
                    &app,
                    "POST",
                    "/api/v1/auth/login",
                    None,
                    Some(json!({ "login": format!("нет-такого-{i}"), "password": "x" })),
                )
                .await
            })
        })
        .collect();
    let mut out = Vec::new();
    for t in tries {
        out.push(t.await.unwrap());
    }
    let busy = out
        .iter()
        .filter(|(s, v)| {
            *s == StatusCode::TOO_MANY_REQUESTS
                && v["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("сервер занят"))
        })
        .count();
    let limited = out
        .iter()
        .filter(|(s, v)| {
            *s == StatusCode::TOO_MANY_REQUESTS
                && v["error"]["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("слишком много неудачных"))
        })
        .count();
    let denied = out
        .iter()
        .filter(|(s, _)| *s == StatusCode::UNAUTHORIZED)
        .count();
    assert!(busy > 0, "очередь проверок не ограничена: {out:?}");
    assert!(limited > 0, "адрес не ограничен");
    assert_eq!(busy + limited + denied, 70, "{out:?}");
    // Строк попыток не больше, чем пропущено к проверке, плюс адрес.
    let rows: i64 = sqlx::query_scalar("select count(*) from login_attempts")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert!(rows <= 2 * 50 + 1, "{rows}");
}

/// Старые строки попыток входа удаляются; блокировка и свежие — остаются.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_login_attempts_are_cleaned(pool: PgPool) {
    sqlx::query(
        "insert into login_attempts (login, failures, last_failed_at, locked_until) values
         ('старый|1.1.1.1', 1, now() - interval '2 days', null),
         ('свежий|1.1.1.1', 1, now(), null),
         ('pin|закрыт', 16, now() - interval '3 days', now() + interval '100 years')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n = avtodom_server::api::auth::clean_login_attempts(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let mut left: Vec<String> = sqlx::query_scalar("select login from login_attempts")
        .fetch_all(&pool)
        .await
        .unwrap();
    left.sort();
    assert_eq!(
        left,
        vec!["pin|закрыт".to_string(), "свежий|1.1.1.1".into()]
    );
}

/// PIN кассира: блокировка не обнуляет счёт — после трёх блокировок PIN закрыт до нового;
/// неверные PIN — в журнале, блокировка — владельцу в ленте.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cashier_pin_locks_add_up(pool: PgPool) {
    let w = World::new(pool).await;
    let e = ok(
        &w.app,
        "POST",
        "/api/v1/employees",
        &w.owner,
        json!({ "full_name": "Бекжан", "is_cashier": true, "pin": "4321" }),
    )
    .await;
    let emp = e["id"].as_str().unwrap().to_string();
    let open = |pin: &'static str| {
        let (app, admin, emp) = (w.app.clone(), w.admin.clone(), emp.clone());
        async move {
            call(
                &app,
                "POST",
                "/api/v1/shifts/open",
                Some(&admin),
                Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": emp, "cashier_pin": pin })),
            )
            .await
        }
    };
    let expire = || async {
        sqlx::query("update login_attempts set locked_until = now() - interval '1 second' where login like 'pin|%' and locked_until < now() + interval '1 day'")
            .execute(&w.pool)
            .await
            .unwrap();
    };
    for round in 0..3 {
        let wrong = if round == 0 { 5 } else { 4 };
        for _ in 0..wrong {
            let (s, v) = open("0000").await;
            assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        }
        let (s, v) = open("0000").await;
        assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
        let m = v["error"]["message"].as_str().unwrap().to_string();
        if round < 2 {
            assert!(m.contains("заблокирован"), "{m}");
        } else {
            assert!(m.contains("закрыт"), "{m}");
        }
        expire().await;
    }
    // Закрыт — и верный PIN не открывает.
    let (s, _) = open("4321").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    let count = |action: &'static str| {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from audit_log where action = $1")
                .bind(action)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count("shift.pin_failed").await, 13);
    assert_eq!(count("shift.pin_locked").await, 3);
    let n = get(&w.app, "/api/v1/notifications", &w.owner).await;
    assert!(
        n["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["title"] == "PIN кассира закрыт: Бекжан"),
        "{n}"
    );
    // Новый PIN от владельца снимает закрытие.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/employees/{emp}/pin"),
        &w.owner,
        json!({ "pin": "5678" }),
    )
    .await;
    let (s, v) = open("5678").await;
    assert_ne!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert_ne!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
}
