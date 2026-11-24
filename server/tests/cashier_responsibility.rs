//! Чек юрлица под ответственность кассира (задача 046, ADR-213): «ждём подтверждения фирмы»,
//! неподтверждённые водитель и машина проводятся с пояснением кассира; владельцу — событие и блок
//! в отчёте смены; отказ фирмы и другие отказы не обходятся; повтор `op_id` — тот же чек.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, bootstrap, build_state};

const SIG: &str = "iVBORw0KGgpzaWctb25lIQ==";
const CHAT: i64 = 46_046;

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

fn with_note(mut draft: Value, note: &str) -> Value {
    draft["responsibility"] = json!({ "note": note });
    draft
}

impl W {
    async fn audits(&self) -> Vec<Value> {
        sqlx::query_scalar(
            "select data from audit_log where action = 'sale.cashier_responsibility' order by created_at",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    /// Водитель и машина фирмы в нужном статусе (как из кабинета или бота).
    async fn driver_car(&self, firm: &str, contact: &str, car: &str) -> (String, String) {
        let c = self
            .ok(
                "POST",
                &format!("/api/v1/parties/{firm}/contacts"),
                json!({ "full_name": "Бакыт Водитель" }),
            )
            .await;
        let v = self
            .ok(
                "POST",
                &format!("/api/v1/parties/{firm}/vehicles"),
                json!({ "plate": "01KG046ABC" }),
            )
            .await;
        let (c, v) = (
            c["id"].as_str().unwrap().to_string(),
            v["id"].as_str().unwrap().to_string(),
        );
        sqlx::query("update party_contacts set status = $2 where id = $1::uuid")
            .bind(&c)
            .bind(contact)
            .execute(&self.pool)
            .await
            .unwrap();
        sqlx::query("update party_vehicles set status = $2 where id = $1::uuid")
            .bind(&v)
            .bind(car)
            .execute(&self.pool)
            .await
            .unwrap();
        (c, v)
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn waiting_firm_posts_under_responsibility(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("wait", 10).await;
    let firm = w
        .firm("ОсОО Айкос", "0700 46-46-46", "04604604604604")
        .await;
    w.link_tg(&firm, "0700464646").await;
    let cab = w.cabinet(&firm, "04604604604604").await;

    // Без пояснения и с пустым — прежний отказ; короткое — ошибка поля.
    let draft = w.sale(&firm);
    let (s, v) = w.req("POST", "/api/v1/sales", Some(draft.clone())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "approval_required");
    let (s, v) = w
        .req(
            "POST",
            "/api/v1/sales",
            Some(with_note(draft.clone(), "   ")),
        )
        .await;
    assert_eq!(code(&v), "approval_required", "{s} {v}");
    let (s, _) = w
        .req(
            "POST",
            "/api/v1/sales",
            Some(with_note(draft.clone(), "нет")),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(w.sales_count().await, 0);

    // С пояснением — чек проведён тем же op_id, запрос фирме появляется уже с чеком.
    let body = with_note(draft, "бухгалтер фирмы не отвечает");
    let sale = w.ok("POST", "/api/v1/sales", body.clone()).await;
    assert_eq!(sale["approval"]["status"], "pending");
    assert_eq!(sale["approval"]["forced"], true);
    assert_eq!(
        sale["approval"]["forced_note"],
        "бухгалтер фирмы не отвечает"
    );
    assert_eq!(sale["approval"]["forced_by"], "Владелец");
    // Повтор op_id — тот же чек, одно событие (I7, I8).
    let again = w.ok("POST", "/api/v1/sales", body).await;
    assert_eq!(again["id"], sale["id"]);
    assert_eq!(w.sales_count().await, 1);
    let a = w.audits().await;
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["number"], sale["number"]);
    assert_eq!(a[0]["note"], "бухгалтер фирмы не отвечает");
    assert_eq!(a[0]["cashier"], "Айбек");
    assert_eq!(a[0]["debt_tyiyn"], 100_000);
    assert_eq!(
        a[0]["reasons"],
        json!(["фирма ещё не подтвердила обслуживание"])
    );
    // Владельцу — сразу в ленте.
    let n = w.get("/api/v1/notifications").await.to_string();
    assert!(n.contains("под ответственность кассира Айбек"), "{n}");
    assert!(n.contains("бухгалтер фирмы не отвечает"), "{n}");
    // Фирме — запрос в чат, как в режиме «после».
    let mut conn = w.pool.acquire().await.unwrap();
    let notices = avtodom_server::api::sale_approvals::notices(&mut conn)
        .await
        .unwrap();
    assert_eq!(notices.len(), 1);
    drop(conn);

    // Ответ фирмы после чека — отметка на чеке.
    let (_, _, list) = call(&w.app, "GET", "/api/v1/client/approvals", Some(&cab), None).await;
    let id = list[0]["id"].as_str().unwrap().to_string();
    assert_eq!(list[0]["sale_number"], sale["number"]);
    let (s, _, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/client/approvals/{id}/approve"),
        Some(&cab),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let got = w
        .get(&format!("/api/v1/sales/{}", sale["id"].as_str().unwrap()))
        .await;
    assert_eq!(got["approval"]["status"], "approved");
    assert_eq!(
        got["approval"]["forced_note"],
        "бухгалтер фирмы не отвечает"
    );

    // Запрос уже ждал ответа — тот же запрос получает `forced_*`.
    let draft = w.sale(&firm);
    let asked = w.ask(&draft).await;
    let sale = w
        .ok(
            "POST",
            "/api/v1/sales",
            with_note(draft, "клиент торопится, звонили"),
        )
        .await;
    let (note, by): (Option<String>, Option<Uuid>) =
        sqlx::query_as("select forced_note, forced_by from sale_approvals where id = $1::uuid")
            .bind(asked["id"].as_str().unwrap())
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(note.as_deref(), Some("клиент торопится, звонили"));
    assert!(by.is_some());
    assert_eq!(sale["approval"]["forced"], true);

    // Фирма отклонила — не обходится.
    let draft = w.sale(&firm);
    let id = w.ask(&draft).await["id"].as_str().unwrap().to_string();
    let (s, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/client/approvals/{id}/reject"),
        Some(&cab),
        Some(json!({ "reason": "не наша машина" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, v) = w
        .req(
            "POST",
            "/api/v1/sales",
            Some(with_note(draft, "очень надо провести")),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(code(&v), "approval_rejected");
    assert_eq!(w.audits().await.len(), 2);

    // Отчёт смены — блок «Под ответственность кассира».
    let shift: Uuid = sqlx::query_scalar("select id from shifts limit 1")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let r = w.get(&format!("/api/v1/shifts/{shift}/report")).await;
    let rows = r["responsibility"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["party_name"], "ОсОО Айкос");
    assert_eq!(rows[0]["note"], "бухгалтер фирмы не отвечает");
    assert_eq!(rows[0]["debt_tyiyn"], 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn pending_driver_and_car_under_responsibility(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w
        .firm("ОсОО Автодом", "0700 64-64-64", "06406406406406")
        .await;

    // Водитель ждёт подтверждения фирмы.
    let (driver, car) = w.driver_car(&firm, "pending", "active").await;
    let mut draft = w.sale(&firm);
    draft["contact_id"] = json!(driver);
    draft["vehicle_id"] = json!(car);
    let (_, v) = w.req("POST", "/api/v1/sales", Some(draft.clone())).await;
    assert_eq!(code(&v), "contact_pending", "{v}");
    let sale = w
        .ok(
            "POST",
            "/api/v1/sales",
            with_note(draft, "водитель новый, фирма в курсе"),
        )
        .await;
    // Метка — в чеке, в истории дня и в карточке фирмы.
    let mark = "Под ответственность: Айбек — «водитель новый, фирма в курсе»";
    assert_eq!(sale["responsibility"], mark);
    let day = w.get("/api/v1/sales").await;
    assert_eq!(day["sales"][0]["responsibility"], mark, "{day}");
    let card = w.get(&format!("/api/v1/parties/{firm}/card")).await;
    assert!(card["timeline"].to_string().contains(mark), "{card}");
    let a = w.audits().await;
    assert_eq!(a.len(), 1);
    assert_eq!(a[0]["number"], sale["number"]);
    assert!(
        a[0]["reasons"][0]
            .as_str()
            .unwrap()
            .contains("Бакыт Водитель"),
        "{a:?}"
    );

    // Машина ждёт подтверждения фирмы.
    sqlx::query("update party_contacts set status = 'active'")
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("update party_vehicles set status = 'pending'")
        .execute(&w.pool)
        .await
        .unwrap();
    let mut draft = w.sale(&firm);
    draft["contact_id"] = json!(driver);
    draft["vehicle_id"] = json!(car);
    let (_, v) = w.req("POST", "/api/v1/sales", Some(draft.clone())).await;
    assert_eq!(code(&v), "vehicle_pending", "{v}");

    // Другой отказ вместе с обходимым — чек не проводится (цена ниже закупки).
    let mut cheap = with_note(draft.clone(), "машина фирмы, номер сменили");
    cheap["lines"][0]["unit_price_tyiyn"] = json!(10_000);
    cheap["payments"][0]["amount_tyiyn"] = json!(10_000);
    let before = w.sales_count().await;
    let (s, v) = w.req("POST", "/api/v1/sales", Some(cheap)).await;
    assert_ne!(s, StatusCode::OK, "{v}");
    assert_eq!(w.sales_count().await, before);

    w.ok(
        "POST",
        "/api/v1/sales",
        with_note(draft, "машина фирмы, номер сменили"),
    )
    .await;
    let a = w.audits().await;
    assert_eq!(a.len(), 2);
    assert!(
        a[1]["reasons"][0].as_str().unwrap().contains("01KG046ABC"),
        "{a:?}"
    );
    let n = w.get("/api/v1/notifications").await.to_string();
    assert!(n.contains("машина фирмы, номер сменили"), "{n}");

    // Отклонённую фирмой машину — не провести.
    sqlx::query("update party_vehicles set status = 'rejected'")
        .execute(&w.pool)
        .await
        .unwrap();
    let mut draft = with_note(w.sale(&firm), "очень надо провести");
    draft["contact_id"] = json!(driver);
    draft["vehicle_id"] = json!(car);
    let (_, v) = w.req("POST", "/api/v1/sales", Some(draft)).await;
    assert_eq!(code(&v), "vehicle_pending", "{v}");
    assert_eq!(w.audits().await.len(), 2);
}
