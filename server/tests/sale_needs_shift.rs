//! Продажа только при открытой смене (задача 005 Н4, ADR-{n4}): онлайн-чек без смены — 409 для
//! всех, включая владельца; после открытия смены — проводится; повтор `op_id` проведённого чека —
//! прежний ответ (I8); чек без сети без смены проводится, владельцу — причина в
//! `sale.offline_bypass` (I9, I10); «Исправить» и «Изменить оплату» не ограничиваются; настройка
//! в «Настройки → Касса» — только владелец, журнал, повтор `op_id`.
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

    async fn close_shift(&self, id: &str) {
        let cur = self.get("/api/v1/shifts/current").await;
        let expected = cur["expected_tyiyn"].as_i64().unwrap();
        self.ok(
            "POST",
            &format!("/api/v1/shifts/{id}/close"),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "" }),
        )
        .await;
    }

    async fn sales(&self) -> i64 {
        sqlx::query_scalar("select count(*) from sales")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn bypass_count(&self, sale: &Value) -> i64 {
        sqlx::query_scalar(
            "select count(*) from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
        )
        .bind(sale["id"].as_str().unwrap())
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn online_sale_needs_open_shift_for_everyone(pool: PgPool) {
    let w = W::new(pool).await;
    // По умолчанию настройка включена (прямая просьба заказчика, I21).
    let cfg = w.get("/api/v1/settings/cashier").await;
    assert_eq!(cfg["sale_requires_shift"], true);
    // Без смены онлайн-чек не проводится — ни у администратора, ни у владельца.
    for who in [&w.admin, &w.owner] {
        let (s, v) = w.sell(who, w.sale_body(Uuid::now_v7(), false)).await;
        assert_eq!(s, StatusCode::CONFLICT, "{v}");
        assert_eq!(
            message(&v),
            "Смена не открыта: откройте её на экране «Смена»"
        );
    }
    assert_eq!(w.sales().await, 0);
    // Смену открыли — чек проходит.
    let shift = w.open_shift().await;
    let op = Uuid::now_v7();
    let (s, first) = w.sell(&w.admin, w.sale_body(op, false)).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    // Смену закрыли: повтор того же op_id — прежний ответ, а не 409 (I8).
    w.close_shift(&shift).await;
    let (s, again) = w.sell(&w.admin, w.sale_body(op, false)).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["id"], first["id"]);
    assert_eq!(w.sales().await, 1);
    // Новый чек после закрытия — снова 409.
    let (s, v) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), false)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_without_shift_is_posted_and_reported(pool: PgPool) {
    let w = W::new(pool).await;
    // Чек без сети уже состоялся (I9, I10): проводится, владелец получает причину.
    let (s, sale) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), true)).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
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
            .any(|r| r == "чек без открытой смены"),
        "{reasons:?}"
    );
    // Чек без сети, пробитый в открытой смене, — без причины.
    w.open_shift().await;
    let (s, during) = w.sell(&w.admin, w.sale_at(Uuid::now_v7(), true, 0)).await;
    assert_eq!(s, StatusCode::OK, "{during}");
    assert_eq!(w.bypass_count(&during).await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn correction_and_pay_change_are_not_limited(pool: PgPool) {
    let w = W::new(pool).await;
    let shift = w.open_shift().await;
    let (s, sale) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), false)).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let (s, other) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), false)).await;
    assert_eq!(s, StatusCode::OK, "{other}");
    w.close_shift(&shift).await;
    // «Исправить» — правка проведённого чека владельцем: смена не нужна.
    let sid = sale["id"].as_str().unwrap();
    let draft = w
        .get(&format!("/api/v1/corrections/draft/sale/{sid}"))
        .await;
    let mut doc = draft["doc"].clone();
    doc["lines"][0]["unit_price_tyiyn"] = json!(90_000);
    doc["payments"] = json!([{ "method": "card", "amount_tyiyn": 90_000 }]);
    let fixed = w
        .ok(
            "POST",
            "/api/v1/corrections",
            json!({ "op_id": Uuid::now_v7(), "kind": "sale", "original_id": sid, "comment": "не та цена", "doc": doc }),
        )
        .await;
    assert!(fixed["new_id"].is_string(), "{fixed}");
    // «Изменить оплату» — тоже: карта → перевод.
    let oid = other["id"].as_str().unwrap();
    let changed = w
        .ok(
            "POST",
            &format!("/api/v1/sales/{oid}/pay-change"),
            json!({ "op_id": Uuid::now_v7(), "comment": "ошиблись способом оплаты",
                    "payments": [{ "method": "transfer", "amount_tyiyn": 100_000 }] }),
        )
        .await;
    assert!(changed["new_sale_id"].is_string(), "{changed}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn setting_off_allows_sale_and_only_owner_changes_it(pool: PgPool) {
    let w = W::new(pool).await;
    // Администратор настройку не меняет.
    let (s, _, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/cashier",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "sale_requires_shift": false })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Владелец выключает; повтор op_id — тот же ответ, одна запись журнала.
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "sale_requires_shift": false });
    let off = w.ok("PUT", "/api/v1/settings/cashier", body.clone()).await;
    assert_eq!(off["sale_requires_shift"], false);
    let again = w.ok("PUT", "/api/v1/settings/cashier", body).await;
    assert_eq!(again, off);
    let logged: Vec<Value> = sqlx::query_scalar(
        "select data from audit_log where action = 'settings.cashier' order by created_at",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["before"]["sale_requires_shift"], true);
    assert_eq!(logged[0]["after"]["sale_requires_shift"], false);
    // Выключено — онлайн-чек без смены проходит, как раньше.
    let (s, v) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), false)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Чек без сети без смены — без причины «без открытой смены».
    let (s, off_sale) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), true)).await;
    assert_eq!(s, StatusCode::OK, "{off_sale}");
    assert_eq!(w.bypass_count(&off_sale).await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn no_default_register_is_a_clear_refusal(pool: PgPool) {
    let w = W::new(pool).await;
    sqlx::query("update cash_accounts set is_default = false where kind = 'register'")
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, v) = w.sell(&w.admin, w.sale_body(Uuid::now_v7(), false)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(message(&v).contains("нет кассы по умолчанию"), "{v}");
}
