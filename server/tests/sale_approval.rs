//! Подтверждение заправки юрлицом (задача 039 C, ADR-202): режимы «ждать», «после», «с таймаутом»
//! и «не спрашивать»; подтверждение из кабинета и из бота; отклонение; таймаут → «Провести без
//! подтверждения»; фирма без Телеграма и кабинета — «не спрошено».
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::telegram::handle_callback;
use avtodom_server::{app, bootstrap, build_state};

const SIG: &str = "iVBORw0KGgpzaWctb25lIQ==";
const CHAT: i64 = 39_039;

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

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    cashier: String,
    item: String,
}

impl W {
    async fn new(pool: PgPool) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let (s, c, _) = call(
            &app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "login": "owner", "password": "owner-pass-1" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let mut w = W {
            app,
            pool,
            owner: c.unwrap(),
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
                json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр MANN", "sale_price_tyiyn": 100_000 }),
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
                    "lines": [{ "product_id": w.item, "qty": 50, "cost_tyiyn": 50 * 60_000 }] }),
        )
        .await;
        w.ok(
            "POST",
            "/api/v1/shifts/open",
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": w.cashier, "cashier_pin": "1234" }),
        )
        .await;
        w
    }

    async fn req(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, method, uri, Some(&self.owner), body).await;
        (s, v)
    }

    async fn ok(&self, method: &str, uri: &str, body: Value) -> Value {
        let (s, v) = self.req(method, uri, Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn get(&self, uri: &str) -> Value {
        let (s, v) = self.req("GET", uri, None).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn mode(&self, mode: &str, timeout: i64) {
        self.ok(
            "PUT",
            "/api/v1/settings/legal",
            json!({ "op_id": Uuid::now_v7(), "approval_mode": mode, "approval_timeout_min": timeout }),
        )
        .await;
    }

    async fn firm(&self, name: &str, phone: &str, inn: &str) -> String {
        let p = self
            .ok(
                "POST",
                "/api/v1/parties",
                json!({ "kind": "company", "name": name, "phone": phone, "inn": inn }),
            )
            .await;
        p["id"].as_str().unwrap().to_string()
    }

    async fn link_tg(&self, party: &str, phone: &str) {
        sqlx::query(
            "insert into telegram_customers (chat_id, party_id, phone) values ($1, $2::uuid, $3)",
        )
        .bind(CHAT)
        .bind(party)
        .bind(phone)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// Кабинет фирмы: сброс пароля владельцем, вход по ИНН, смена пароля.
    async fn cabinet(&self, party: &str, inn: &str) -> String {
        self.ok(
            "POST",
            &format!("/api/v1/parties/{party}/cabinet/reset"),
            json!({}),
        )
        .await;
        let (s, c, v) = call(
            &self.app,
            "POST",
            "/api/v1/client/login",
            None,
            Some(json!({ "inn": inn, "password": "avtodom2026" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let c = c.unwrap();
        let (s, _, v) = call(
            &self.app,
            "POST",
            "/api/v1/client/password",
            Some(&c),
            Some(json!({ "old_password": "avtodom2026", "new_password": "свой-пароль-1" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        c
    }

    /// Чек на 1000 сом целиком в долг фирме.
    fn sale(&self, party: &str) -> Value {
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
            "party_id": party,
            "lines": [{ "kind": "piece", "product_id": self.item, "qty": 1, "unit_price_tyiyn": 100_000 }],
            "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }],
            "debt_signature_png": SIG,
        })
    }

    async fn ask(&self, draft: &Value) -> Value {
        self.ok("POST", "/api/v1/sales/approvals", json!({ "draft": draft }))
            .await
    }

    async fn events(&self) -> Vec<Value> {
        sqlx::query_scalar(
            "select data from audit_log where action = 'party.sale_approval' order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn sales_count(&self) -> i64 {
        sqlx::query_scalar("select count(*) from sales")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn code(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn off_mode_and_no_contacts(pool: PgPool) {
    let w = W::new(pool).await;
    // По умолчанию — «с таймаутом», 10 минут.
    let s = w.get("/api/v1/settings/legal").await;
    assert_eq!(s["approval_mode"], "timeout");
    assert_eq!(s["approval_timeout_min"], 10);
    let firm = w
        .firm("ОсОО Тихая", "0700 10-10-10", "01010101010101")
        .await;

    // Фирма без Телеграма и кабинета — запрос не создаётся, чек как раньше, «не спрошено».
    let draft = w.sale(&firm);
    let a = w.ask(&draft).await;
    assert_eq!(a["status"], "skipped");
    assert_eq!(a["can_post"], true);
    let sale = w.ok("POST", "/api/v1/sales", draft).await;
    assert_eq!(sale["approval"]["status"], "skipped");

    // Не спрашивать — как сейчас, даже если Телеграм есть.
    w.mode("off", 10).await;
    w.link_tg(&firm, "0700101010").await;
    let draft = w.sale(&firm);
    assert_eq!(w.ask(&draft).await["status"], "not_needed");
    let sale = w.ok("POST", "/api/v1/sales", draft).await;
    assert!(sale["approval"].is_null(), "{sale}");
    // Физлицо не спрашивают.
    let n: i64 = sqlx::query_scalar("select count(*) from sale_approvals")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn wait_mode_bot_approves_cabinet_rejects(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("wait", 10).await;
    let firm = w
        .firm("ОсОО Ташкент", "0700 36-36-36", "02020202020202")
        .await;
    w.link_tg(&firm, "0700363636").await;

    // Без запроса чек не проводится.
    let (s, v) = w.req("POST", "/api/v1/sales", Some(w.sale(&firm))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "approval_required");

    let draft = w.sale(&firm);
    let a = w.ask(&draft).await;
    assert_eq!(a["status"], "pending");
    assert_eq!(a["can_post"], false);
    assert_eq!(a["summary"]["lines"], json!(["Фильтр MANN"]));
    let id = a["id"].as_str().unwrap().to_string();
    // Повтор с тем же op_id — тот же запрос.
    assert_eq!(w.ask(&draft).await["id"], a["id"]);
    let before = w.sales_count().await;
    let (s, v) = w.req("POST", "/api/v1/sales", Some(draft.clone())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(w.sales_count().await, before);

    // В чат фирмы — запрос с кнопками.
    let mut conn = w.pool.acquire().await.unwrap();
    let notices = avtodom_server::api::sale_approvals::notices(&mut conn)
        .await
        .unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].chat_id, CHAT);
    assert!(
        notices[0].text.contains("Подтвердите обслуживание"),
        "{}",
        notices[0].text
    );

    // Чужой чат кнопку не нажмёт.
    let r = handle_callback(&mut conn, 777, &format!("ap:ok:{id}"))
        .await
        .unwrap();
    assert!(!r.text.contains("подтверждено"), "{}", r.text);
    let r = handle_callback(&mut conn, CHAT, &format!("ap:ok:{id}"))
        .await
        .unwrap();
    assert!(r.text.contains("подтверждено"), "{}", r.text);
    let r = handle_callback(&mut conn, CHAT, &format!("ap:no:{id}"))
        .await
        .unwrap();
    assert!(r.text.contains("уже ответили"), "{}", r.text);
    drop(conn);

    let a = w.get(&format!("/api/v1/sales/approvals/{id}")).await;
    assert_eq!(a["status"], "approved");
    assert_eq!(a["can_post"], true);
    let sale = w.ok("POST", "/api/v1/sales", draft.clone()).await;
    assert_eq!(sale["approval"]["status"], "approved");
    // Повтор чека — тот же чек (I7).
    let again = w.ok("POST", "/api/v1/sales", draft).await;
    assert_eq!(again["id"], sale["id"]);
    let ev = w.events().await;
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["decision"], "approved");

    // Отклонение из кабинета: без причины — нельзя; с причиной — чек не проводится.
    let cab = w.cabinet(&firm, "02020202020202").await;
    let draft = w.sale(&firm);
    let id = w.ask(&draft).await["id"].as_str().unwrap().to_string();
    let (s, _, list) = call(&w.app, "GET", "/api/v1/client/approvals", Some(&cab), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list[0]["id"].as_str(), Some(id.as_str()));
    assert_eq!(list[0]["status"], "pending");
    let uri = format!("/api/v1/client/approvals/{id}/reject");
    let (s, _, _) = call(
        &w.app,
        "POST",
        &uri,
        Some(&cab),
        Some(json!({ "reason": " " })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, v) = call(
        &w.app,
        "POST",
        &uri,
        Some(&cab),
        Some(json!({ "reason": "машина не наша" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let a = w.get(&format!("/api/v1/sales/approvals/{id}")).await;
    assert_eq!(a["status"], "rejected");
    assert_eq!(a["reason"], "машина не наша");
    let (s, v) = w.req("POST", "/api/v1/sales", Some(draft)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(code(&v), "approval_rejected");
    assert_eq!(w.events().await.len(), 2);

    // Подтверждение из кабинета.
    let draft = w.sale(&firm);
    let id = w.ask(&draft).await["id"].as_str().unwrap().to_string();
    let (s, _, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/client/approvals/{id}/approve"),
        Some(&cab),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["decided_by"], "Кабинет");
    w.ok("POST", "/api/v1/sales", draft).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn timeout_mode_forces_after_expiry(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("timeout", 5).await;
    let firm = w
        .firm("ОсОО Медленная", "0700 55-55-55", "03030303030303")
        .await;
    w.link_tg(&firm, "0700555555").await;
    let draft = w.sale(&firm);
    let a = w.ask(&draft).await;
    assert_eq!(a["status"], "pending");
    assert!(a["expires_at"].is_string());
    let id = a["id"].as_str().unwrap().to_string();
    let force = format!("/api/v1/sales/approvals/{id}/force");
    // До таймаута — нельзя.
    let (s, _) = w.req("POST", &force, Some(json!({}))).await;
    assert_eq!(s, StatusCode::CONFLICT);
    sqlx::query(
        "update sale_approvals set expires_at = now() - interval '1 minute' where id = $1::uuid",
    )
    .bind(&id)
    .execute(&w.pool)
    .await
    .unwrap();
    let a = w.get(&format!("/api/v1/sales/approvals/{id}")).await;
    assert_eq!(a["status"], "expired");
    assert_eq!(a["can_force"], true);
    assert_eq!(a["can_post"], false);
    let (s, v) = w.req("POST", "/api/v1/sales", Some(draft.clone())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let a = w.ok("POST", &force, json!({})).await;
    assert_eq!(a["forced"], true);
    assert_eq!(a["can_post"], true);
    let sale = w.ok("POST", "/api/v1/sales", draft).await;
    assert_eq!(sale["approval"]["forced"], true);
    let ev = w.events().await;
    assert_eq!(ev.last().unwrap()["decision"], "forced");
    // Фирма поздно — уже проведено без подтверждения.
    let mut conn = w.pool.acquire().await.unwrap();
    let r = handle_callback(&mut conn, CHAT, &format!("ap:ok:{id}"))
        .await
        .unwrap();
    assert!(r.text.contains("уже ответили"), "{}", r.text);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn after_mode_posts_then_marks(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("after", 10).await;
    let firm = w
        .firm("ОсОО Быстрая", "0700 77-77-77", "04040404040404")
        .await;
    let cab = w.cabinet(&firm, "04040404040404").await;
    // Чек сразу, запрос — после.
    let draft = w.sale(&firm);
    assert_eq!(w.ask(&draft).await["status"], "not_needed");
    let sale = w.ok("POST", "/api/v1/sales", draft).await;
    assert_eq!(sale["approval"]["status"], "pending");
    let (_, _, list) = call(&w.app, "GET", "/api/v1/client/approvals", Some(&cab), None).await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert_eq!(list[0]["sale_number"], sale["number"]);
    let (s, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/client/approvals/{id}/reject"),
        Some(&cab),
        Some(json!({ "reason": "не заказывали" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let got = w
        .get(&format!("/api/v1/sales/{}", sale["id"].as_str().unwrap()))
        .await;
    assert_eq!(got["approval"]["status"], "rejected");
    assert_eq!(got["approval"]["reason"], "не заказывали");
    let ev = w.events().await;
    assert_eq!(ev[0]["decision"], "rejected");
    assert_eq!(ev[0]["number"], sale["number"]);
    // Событие — в уведомлениях владельца.
    let n = w.get("/api/v1/notifications").await;
    assert!(n.to_string().contains("отклонила обслуживание"), "{n}");
}
