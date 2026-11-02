//! Подпись клиента за долг и подпись руководителя для документов о долге (задача 008): онлайн-чек
//! с долгом без подписи — 422; с подписью — хранится, отдаётся PNG; подпись неизменна; чек без
//! долга подписи не просит; чек без сети без подписи проводится, владельцу — причина; подпись
//! руководителя ставит и снимает только владелец.
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

const SIG: &str = "iVBORw0KGgpzaWctb25lIQ==";
const SIG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nsig-one!";
const OWNER_SIG: &str = "data:image/png;base64,iVBORw0KGgpvd25lci0wMQ==";
const OWNER_SIG_BYTES: &[u8] = b"\x89PNG\r\n\x1a\nowner-01";

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

async fn raw(app: &Router, uri: &str, cookie: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let req = Request::builder()
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
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, ct, bytes)
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    item: String,
    party: String,
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
            party: String::new(),
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
        let party = w
            .ok(
                "POST",
                "/api/v1/parties",
                json!({ "kind": "person", "first_name": "Бакыт", "last_name": "Должник", "phone": "0700 11-22-33", "inn": "21201199000123" }),
            )
            .await;
        w.party = party["id"].as_str().unwrap().to_string();
        w.ok(
            "POST",
            "/api/v1/shifts/open",
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": w.cashier, "cashier_pin": "1234" }),
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

    /// Чек на 1000 сом: `debt` — сколько в долг, остальное картой.
    fn sale(&self, debt: i64, sig: Option<&str>) -> Value {
        let mut payments = Vec::new();
        if debt < 100_000 {
            payments.push(json!({ "method": "card", "amount_tyiyn": 100_000 - debt }));
        }
        if debt > 0 {
            payments.push(json!({ "method": "debt", "amount_tyiyn": debt }));
        }
        let mut b = json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
            "party_id": self.party,
            "lines": [{ "kind": "piece", "product_id": self.item, "qty": 1, "unit_price_tyiyn": 100_000 }],
            "payments": payments
        });
        if let Some(s) = sig {
            b["debt_signature_png"] = json!(s);
        }
        b
    }

    async fn sell(&self, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = call(
            &self.app,
            "POST",
            "/api/v1/sales",
            Some(&self.owner),
            Some(body),
        )
        .await;
        (s, v)
    }

    async fn signatures(&self) -> i64 {
        sqlx::query_scalar("select count(*) from sale_signatures")
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
async fn debt_sale_needs_client_signature(pool: PgPool) {
    let w = W::new(pool).await;
    // Без подписи и с пустой — 422 с понятным текстом, чек не проведён.
    for sig in [None, Some(""), Some("  ")] {
        let (s, v) = w.sell(w.sale(100_000, sig)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(message(&v), "Клиент не расписался за долг");
    }
    // Не PNG — 422.
    let (s, v) = w.sell(w.sale(100_000, Some("/9j/4AAQSkZJRgABAQ=="))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (s, v) = w.sell(w.sale(100_000, Some("не base64"))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Смешанная оплата с долгом — тоже нужна.
    let (s, v) = w.sell(w.sale(40_000, None)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(w.signatures().await, 0);

    // С подписью — проводится, подпись отдаётся тем же PNG.
    let (s, sale) = w.sell(w.sale(40_000, Some(SIG))).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(sale["has_debt_signature"], true);
    let id = sale["id"].as_str().unwrap();
    let got = w.get(&format!("/api/v1/sales/{id}")).await;
    assert_eq!(got["has_debt_signature"], true);
    for who in [&w.owner, &w.admin] {
        let (s, ct, bytes) = raw(&w.app, &format!("/api/v1/sales/{id}/signature"), who).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(ct.as_deref(), Some("image/png"));
        assert_eq!(bytes, SIG_BYTES);
    }

    // Подпись не меняется и не удаляется.
    let upd = sqlx::query("update sale_signatures set signature_png = signature_png")
        .execute(&w.pool)
        .await;
    assert!(upd.is_err());
    let del = sqlx::query("delete from sale_signatures")
        .execute(&w.pool)
        .await;
    assert!(del.is_err());
    assert_eq!(w.signatures().await, 1);

    // Чек без долга — подписи не просит, присланная не хранится.
    let (s, plain) = w.sell(w.sale(0, Some(SIG))).await;
    assert_eq!(s, StatusCode::OK, "{plain}");
    assert_eq!(plain["has_debt_signature"], false);
    let (s, plain) = w.sell(w.sale(0, None)).await;
    assert_eq!(s, StatusCode::OK, "{plain}");
    let pid = plain["id"].as_str().unwrap();
    let (s, _, _) = raw(&w.app, &format!("/api/v1/sales/{pid}/signature"), &w.owner).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = raw(
        &w.app,
        &format!("/api/v1/sales/{}/signature", Uuid::now_v7()),
        &w.owner,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(w.signatures().await, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_debt_without_signature_is_posted_and_reported(pool: PgPool) {
    let w = W::new(pool).await;
    let mut b = w.sale(100_000, None);
    b["offline"] = json!(true);
    b["client_time"] = json!((chrono::Utc::now() - chrono::Duration::minutes(3)).to_rfc3339());
    let (s, sale) = w.sell(b).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(sale["has_debt_signature"], false);
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
            .any(|r| r == "чек в долг без подписи клиента"),
        "{reasons:?}"
    );
    // Неверная подпись без сети — всё равно ошибка формата, а не молчаливый пропуск.
    let mut b = w.sale(100_000, Some("не PNG"));
    b["offline"] = json!(true);
    let (s, v) = w.sell(b).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_signature_for_debt_docs(pool: PgPool) {
    let w = W::new(pool).await;
    let uri = "/api/v1/settings/debt-docs/signature";
    let docs = w.get("/api/v1/settings/debt-docs").await;
    assert_eq!(docs["has_owner_signature"], false);
    let (s, _, _) = raw(&w.app, uri, &w.owner).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Администратор не ставит и не снимает.
    let (s, _, v) = call(
        &w.app,
        "PUT",
        uri,
        Some(&w.admin),
        Some(json!({ "signature_png": OWNER_SIG })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    // Не PNG — 422.
    let (s, _, v) = call(
        &w.app,
        "PUT",
        uri,
        Some(&w.owner),
        Some(json!({ "signature_png": "/9j/4AAQSkZJRgABAQ==" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");

    w.ok("PUT", uri, json!({ "signature_png": OWNER_SIG }))
        .await;
    let docs = w.get("/api/v1/settings/debt-docs").await;
    assert_eq!(docs["has_owner_signature"], true);
    // Видна любому вошедшему точки — печатается в документах.
    for who in [&w.owner, &w.admin] {
        let (s, ct, bytes) = raw(&w.app, uri, who).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(ct.as_deref(), Some("image/png"));
        assert_eq!(bytes, OWNER_SIG_BYTES);
    }
    // Замена — та же строка.
    w.ok("PUT", uri, json!({ "signature_png": SIG })).await;
    let (_, _, bytes) = raw(&w.app, uri, &w.owner).await;
    assert_eq!(bytes, SIG_BYTES);
    let audits: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'settings.debt_docs_signature'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(audits, 2);

    let (s, _, _) = call(&w.app, "DELETE", uri, Some(&w.admin), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&w.app, "DELETE", uri, Some(&w.owner), None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = raw(&w.app, uri, &w.owner).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let docs = w.get("/api/v1/settings/debt-docs").await;
    assert_eq!(docs["has_owner_signature"], false);
}
