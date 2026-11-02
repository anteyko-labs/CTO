//! Касса без продажи в минус (задача 015, ADR-184): онлайн-чек сверх остатка — 409 для всех,
//! включая владельца, остаток не меняется; ровно в остаток — проходит; строки одного товара
//! складываются; разлив из бочки с нулём — тоже 409; чек без сети проводится, владельцу —
//! причина в `sale.offline_bypass` (I10); «Продажа в минус на кассе» включена — прежнее поведение.
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

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    item: String,
}

impl W {
    async fn new(pool: PgPool) -> Self {
        let app = setup(pool.clone()).await;
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        let mut w = W {
            app,
            pool,
            owner,
            admin,
            cashier: String::new(),
            item: String::new(),
        };
        let e = w
            .ok(
                "POST",
                "/api/v1/employees",
                json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" }),
            )
            .await;
        w.cashier = e["id"].as_str().unwrap().to_string();
        let cat = w
            .ok(
                "POST",
                "/api/v1/categories",
                json!({ "name": "Фильтры", "kind": "filter" }),
            )
            .await;
        let p = w
            .ok(
                "POST",
                "/api/v1/products",
                json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 100_000 }),
            )
            .await;
        w.item = p["id"].as_str().unwrap().to_string();
        let accounts = w.get("/api/v1/cash/accounts").await;
        let safe = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["owner_only"] == json!(true))
            .unwrap()["id"]
            .clone();
        w.ok(
            "POST",
            "/api/v1/receipts",
            json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                    "lines": [{ "product_id": w.item, "qty": 20, "cost_tyiyn": 20 * 60_000 }] }),
        )
        .await;
        w
    }

    async fn ok(&self, method: &str, uri: &str, body: Value) -> Value {
        let (s, _, v) = call(&self.app, method, uri, Some(&self.owner), Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn get(&self, uri: &str) -> Value {
        let (s, _, v) = call(&self.app, "GET", uri, Some(&self.owner), None).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    /// Чек; `offline` — без сети, пробит `ago` минут назад.
    fn sale_body(&self, op: Uuid, offline: bool) -> Value {
        self.sale_at(op, offline, 5)
    }

    fn sale_at(&self, op: Uuid, offline: bool, ago: i64) -> Value {
        let mut b = json!({
            "op_id": op, "sale_type": "takeaway", "cashier_id": self.cashier,
            "lines": [{ "kind": "piece", "product_id": self.item, "qty": 1, "unit_price_tyiyn": 100_000 }],
            "payments": [{ "method": "card", "amount_tyiyn": 100_000 }]
        });
        if offline {
            b["offline"] = json!(true);
            b["client_time"] =
                json!((chrono::Utc::now() - chrono::Duration::minutes(ago)).to_rfc3339());
        }
        b
    }

    async fn sell(&self, who: &str, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, "POST", "/api/v1/sales", Some(who), Some(body)).await;
        (s, v)
    }

    async fn open_shift(&self) -> String {
        let s = self
            .ok(
                "POST",
                "/api/v1/shifts/open",
                json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": self.cashier, "cashier_pin": "1234" }),
            )
            .await;
        s["id"].as_str().unwrap().to_string()
    }

    async fn sales(&self) -> i64 {
        sqlx::query_scalar("select count(*) from sales")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn message(v: &Value) -> String {
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

impl W {
    fn sell_qty(&self, lines: Value, total: i64, offline: bool) -> Value {
        let mut b = self.sale_body(Uuid::now_v7(), offline);
        b["lines"] = lines;
        b["payments"] = json!([{ "method": "card", "amount_tyiyn": total }]);
        b
    }

    fn piece(&self, qty: i64) -> Value {
        json!({ "kind": "piece", "product_id": self.item, "qty": qty, "unit_price_tyiyn": 100_000 })
    }

    async fn stock(&self, product: &str) -> i64 {
        sqlx::query_scalar(
            "select coalesce(sum(stock_qty), 0)::bigint from branch_products where product_id = $1::uuid",
        )
        .bind(product)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_over_stock_is_refused_for_everyone(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    assert_eq!(
        w.get("/api/v1/settings/cashier").await["sale_allow_negative"],
        false
    );
    // Остаток 20: 21 не продать ни администратору, ни владельцу; остаток не изменился.
    for who in [&w.admin, &w.owner] {
        let (s, v) = w
            .sell(who, w.sell_qty(json!([w.piece(21)]), 2_100_000, false))
            .await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(message(&v), "не хватает: Фильтр, остаток 20 шт");
    }
    // Две строки одного товара в сумме сверх остатка — тоже отказ.
    let (s, v) = w
        .sell(
            &w.admin,
            w.sell_qty(json!([w.piece(15), w.piece(6)]), 2_100_000, false),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(w.stock(&w.item).await, 20);
    assert_eq!(w.sales().await, 0);
    // Ровно в остаток — проходит, остаток 0; следующая штука — отказ.
    let (s, v) = w
        .sell(&w.admin, w.sell_qty(json!([w.piece(20)]), 2_000_000, false))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.stock(&w.item).await, 0);
    let (s, v) = w
        .sell(&w.owner, w.sell_qty(json!([w.piece(1)]), 100_000, false))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(message(&v), "не хватает: Фильтр, остаток 0 шт");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn barrel_pour_with_zero_stock_is_refused(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    let cat = w
        .ok(
            "POST",
            "/api/v1/categories",
            json!({ "name": "Масла", "kind": "oil" }),
        )
        .await;
    let barrel = w
        .ok(
            "POST",
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Масло в бочке",
                    "unit": "ml", "container_ml": 200_000, "sale_price_tyiyn": 50_000 }),
        )
        .await;
    let line = json!([{ "kind": "pour", "product_id": barrel["id"], "qty": 1_000, "unit_price_tyiyn": 50_000 }]);
    let (s, v) = w.sell(&w.owner, w.sell_qty(line, 50_000, false)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(message(&v).starts_with("не хватает: Масло в бочке"), "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_over_stock_is_posted_and_reported(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    // Без сети товар уже выдан (I10): чек проводится, остаток в минусе, владельцу — причина.
    let (s, sale) = w
        .sell(&w.admin, w.sell_qty(json!([w.piece(22)]), 2_200_000, true))
        .await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(w.stock(&w.item).await, -2);
    let reasons: Vec<Value> = sqlx::query_scalar(
        "select data->'reasons' from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
    )
    .bind(sale["id"].as_str().unwrap())
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(reasons.len(), 1);
    assert!(
        reasons[0]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "не хватает: Фильтр, остаток 20 шт"),
        "{reasons:?}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn allow_negative_setting_restores_old_behavior(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    // Администратор настройку не меняет.
    let (s, _, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/cashier",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "sale_allow_negative": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let on = w
        .ok(
            "PUT",
            "/api/v1/settings/cashier",
            json!({ "op_id": Uuid::now_v7(), "sale_allow_negative": true }),
        )
        .await;
    assert_eq!(on["sale_allow_negative"], true);
    let logged: Vec<Value> = sqlx::query_scalar(
        "select data from audit_log where action = 'settings.cashier' order by created_at",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["after"]["sale_allow_negative"], true);
    // Включено — продажа в минус проходит, как раньше.
    let (s, v) = w
        .sell(&w.admin, w.sell_qty(json!([w.piece(25)]), 2_500_000, false))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.stock(&w.item).await, -5);
}
