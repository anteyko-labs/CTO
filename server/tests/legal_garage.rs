//! Гаражи юрлица, подтверждение машин и пересчёт долга (задача 037 п. 1–4, ADR-199).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::legal_garage::{mark_notice, notices};
use avtodom_server::api::telegram::{handle_callback, handle_message};
use avtodom_server::{app, bootstrap, build_state};

const SIG: &str = "iVBORw0KGgpzaWctb25lIQ==";
const CHAT: i64 = 37_037;

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
}

fn code(v: &Value) -> String {
    v["error"]["code"].as_str().unwrap_or_default().to_string()
}

impl W {
    fn sale_car(&self, party: &str, car: &str) -> Value {
        let mut s = self.sale(party);
        s["vehicle_id"] = json!(car);
        s
    }

    async fn balance(&self, party: &str) -> i64 {
        sqlx::query_scalar("select balance_tyiyn from parties where id = $1::uuid")
            .bind(party)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn status(&self, car: &str) -> String {
        sqlx::query_scalar("select status from party_vehicles where id = $1::uuid")
            .bind(car)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn post_as(&self, who: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, "POST", uri, Some(who), Some(body)).await;
        (s, v)
    }

    /// Администратор с правами `perms`; возвращает (вход, id).
    async fn admin(&self, perms: &[&str]) -> (String, String) {
        let u = self
            .ok(
                "POST",
                "/api/v1/users",
                json!({ "login": "adm", "password": "adm-pass-1", "full_name": "Админ", "role": "admin" }),
            )
            .await;
        let id = u["id"].as_str().unwrap().to_string();
        self.perms(&id, perms).await;
        let (s, c, v) = call(
            &self.app,
            "POST",
            "/api/v1/auth/login",
            None,
            Some(json!({ "login": "adm", "password": "adm-pass-1" })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        (c.unwrap(), id)
    }

    async fn perms(&self, id: &str, perms: &[&str]) {
        self.ok(
            "PUT",
            &format!("/api/v1/users/{id}/permissions"),
            json!({ "op_id": Uuid::now_v7(), "permissions": perms }),
        )
        .await;
    }

    /// Работник фирмы: его номер записан у работника фирмы, чат бота — на его карточке.
    async fn worker(&self, firm: &str, phone: &str) -> String {
        self.ok(
            "POST",
            &format!("/api/v1/parties/{firm}/contacts"),
            json!({ "full_name": "Нурлан", "phone": phone }),
        )
        .await;
        let p = self
            .ok(
                "POST",
                "/api/v1/parties",
                json!({ "kind": "person", "name": "Нурлан", "phone": phone, "inn": "21212121212121" }),
            )
            .await;
        let id = p["id"].as_str().unwrap().to_string();
        self.link_tg(&id, phone).await;
        id
    }

    /// Работник добавляет в боте машину «Оплачивает фирма».
    async fn bot_car(&self, firm: &str, plate: &str) -> String {
        let mut conn = self.pool.acquire().await.unwrap();
        handle_callback(&mut conn, CHAT, "add").await.unwrap();
        handle_message(&mut conn, CHAT, CHAT, Some(plate), None)
            .await
            .unwrap();
        let r = handle_message(&mut conn, CHAT, CHAT, Some("Toyota Camry"), None)
            .await
            .unwrap();
        let data: Vec<String> = r.inline.iter().flatten().map(|b| b.data.clone()).collect();
        assert!(data.contains(&format!("payf:{firm}")), "{data:?}");
        assert!(data.contains(&"pays".to_string()));
        let r = handle_callback(&mut conn, CHAT, &format!("payf:{firm}"))
            .await
            .unwrap();
        assert!(r.text.contains("ждёт подтверждения"), "{}", r.text);
        let (car, paid_by): (Uuid, Option<Uuid>) =
            sqlx::query_as("select id, paid_by_party_id from party_vehicles where plate_norm = $1")
                .bind(plate)
                .fetch_one(&self.pool)
                .await
                .unwrap();
        assert_eq!(paid_by.unwrap().to_string(), firm);
        car.to_string()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cabinet_car_waits_for_approval(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w
        .firm("ОсОО Гараж", "0700 37-37-37", "03030303030303")
        .await;
    let cab = w.cabinet(&firm, "03030303030303").await;
    let add =
        json!({ "op_id": Uuid::now_v7(), "plate": "01KG037AAA", "brand": "Isuzu", "model": "NQR" });
    let (s, r) = w
        .post_as(&cab, "/api/v1/client/vehicles", add.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "pending");
    // Повтор с тем же op_id — тот же запрос.
    let (_, again) = w.post_as(&cab, "/api/v1/client/vehicles", add).await;
    assert_eq!(again["id"], r["id"]);
    let car = r["vehicle_id"].as_str().unwrap().to_string();
    assert_eq!(w.status(&car).await, "pending");
    // В долг фирмы — нельзя, пока не подтвердят.
    let (s, v) = w
        .req("POST", "/api/v1/sales", Some(w.sale_car(&firm, &car)))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "vehicle_pending");
    // Владельцу — событие.
    let n: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'party.vehicle_request'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Руководитель подтверждает в кабинете.
    let rid = r["id"].as_str().unwrap();
    let (s, v) = w
        .post_as(
            &cab,
            &format!("/api/v1/client/requests/{rid}/approve"),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.status(&car).await, "active");
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &car)).await;
    assert_eq!(w.balance(&firm).await, 100_000);

    // Вторая машина: отклонение — только с причиной.
    let (_, r2) = w
        .post_as(
            &cab,
            "/api/v1/client/vehicles",
            json!({ "op_id": Uuid::now_v7(), "plate": "01KG037BBB" }),
        )
        .await;
    let rid2 = r2["id"].as_str().unwrap();
    let uri = format!("/api/v1/client/requests/{rid2}/reject");
    let (s, _) = w.post_as(&cab, &uri, json!({ "reason": " " })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = w.post_as(&cab, &uri, json!({ "reason": "не наша" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "rejected");
    let (_, _, list) = call(&w.app, "GET", "/api/v1/client/requests", Some(&cab), None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn personal_car_from_bot_debt_on_firm(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w
        .firm("ОсОО Работа", "0700 11-22-33", "04040404040404")
        .await;
    let person = w.worker(&firm, "0555123456").await;
    let car = w.bot_car(&firm, "01KG555NUR").await;
    assert_eq!(w.status(&car).await, "pending");

    // Касса выбирает работника и его машину — в долг нельзя, пока фирма не подтвердит.
    let (s, v) = w
        .req("POST", "/api/v1/sales", Some(w.sale_car(&person, &car)))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");

    // Администратор без права «Клиенты и машины» — 403; с правом — подтверждает.
    let reqs = w.get(&format!("/api/v1/parties/{firm}/requests")).await;
    let rid = reqs[0]["id"].as_str().unwrap().to_string();
    assert_eq!(reqs[0]["personal_of"], "Нурлан");
    let (adm, adm_id) = w.admin(&[]).await;
    let uri = format!("/api/v1/parties/{firm}/requests/{rid}/approve");
    let (s, _) = w
        .post_as(&adm, &uri, json!({ "op_id": Uuid::now_v7() }))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    w.perms(&adm_id, &["clients"]).await;
    let (s, v) = w
        .post_as(&adm, &uri, json!({ "op_id": Uuid::now_v7() }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.status(&car).await, "active");

    // Долг за личную машину — на фирму, у работника долга нет.
    w.ok("POST", "/api/v1/sales", w.sale_car(&person, &car))
        .await;
    assert_eq!(w.balance(&firm).await, 100_000);
    assert_eq!(w.balance(&person).await, 0);

    // Автору — сообщение в бот о решении.
    let mut conn = w.pool.acquire().await.unwrap();
    let notes = notices(&mut conn).await.unwrap();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].chat_id, CHAT);
    assert!(notes[0].text.contains("подтвердила"), "{}", notes[0].text);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn rejected_request_tells_author(pool: PgPool) {
    let w = W::new(pool).await;
    let firm = w
        .firm("ОсОО Отказ", "0700 44-44-44", "05050505050505")
        .await;
    w.worker(&firm, "0555998877").await;
    w.bot_car(&firm, "01KG444REJ").await;
    let reqs = w.get(&format!("/api/v1/parties/{firm}/requests")).await;
    let rid = reqs[0]["id"].as_str().unwrap();
    let (s, v) = w
        .req(
            "POST",
            &format!("/api/v1/parties/{firm}/requests/{rid}/reject"),
            Some(json!({ "op_id": Uuid::now_v7(), "reason": "не работает у нас" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let mut conn = w.pool.acquire().await.unwrap();
    let notes = notices(&mut conn).await.unwrap();
    assert!(
        notes[0].text.contains("не работает у нас"),
        "{}",
        notes[0].text
    );
    mark_notice(&mut conn, &notes[0]).await.unwrap();
    assert!(notices(&mut conn).await.unwrap().is_empty());
    // Решённое заново не решается.
    let (s, _) = w
        .req(
            "POST",
            &format!("/api/v1/parties/{firm}/requests/{rid}/approve"),
            Some(json!({ "op_id": Uuid::now_v7() })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn recount_fixes_balance_by_ledger(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w.firm("ОсОО Счёт", "0700 55-55-55", "06060606060606").await;
    w.ok("POST", "/api/v1/sales", w.sale(&firm)).await;
    let uri = format!("/api/v1/parties/{firm}/recount");
    let count = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from audit_log where action = 'party.recount'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    // Совпадает — без записи в журнал.
    let r = w.ok("POST", &uri, json!({ "op_id": Uuid::now_v7() })).await;
    assert_eq!(r["matched"], true);
    assert_eq!(count(w.pool.clone()).await, 0);
    // Расхождение: кэш испорчен — исправляется по журналу долга, запись в журнал.
    sqlx::query("update parties set balance_tyiyn = 777 where id = $1::uuid")
        .bind(&firm)
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("update party_balances set balance_tyiyn = 5 where party_id = $1::uuid")
        .bind(&firm)
        .execute(&w.pool)
        .await
        .unwrap();
    let r = w.ok("POST", &uri, json!({ "op_id": Uuid::now_v7() })).await;
    assert_eq!(r["matched"], false);
    assert_eq!(r["before_tyiyn"], 777);
    assert_eq!(r["after_tyiyn"], 100_000);
    assert_eq!(w.balance(&firm).await, 100_000);
    let part: i64 =
        sqlx::query_scalar("select balance_tyiyn from party_balances where party_id = $1::uuid")
            .bind(&firm)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(part, 100_000);
    assert_eq!(count(w.pool.clone()).await, 1);
    // Не владелец — 403.
    let (adm, _) = w.admin(&["clients"]).await;
    let (s, _) = w
        .post_as(&adm, &uri, json!({ "op_id": Uuid::now_v7() }))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

/// Водитель из кабинета ждёт подтверждения, как машина: в долг по нему нельзя; подтверждает
/// администратор с правом «Клиенты и машины» (без права — 403) или руководитель в кабинете.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cabinet_driver_waits_for_approval(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w
        .firm("ОсОО Водители", "0700 38-38-38", "08080808080808")
        .await;
    let cab = w.cabinet(&firm, "08080808080808").await;
    let add = json!({ "op_id": Uuid::now_v7(), "full_name": "Талант", "phone": "0700 99-99-99" });
    let (s, r) = w
        .post_as(&cab, "/api/v1/client/contacts", add.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "pending");
    let (_, again) = w.post_as(&cab, "/api/v1/client/contacts", add).await;
    assert_eq!(again["id"], r["id"]);
    let contact = r["contact_id"].as_str().unwrap().to_string();
    let mut sale = w.sale(&firm);
    sale["contact_id"] = json!(contact);
    let (s, v) = w.req("POST", "/api/v1/sales", Some(sale.clone())).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "contact_pending");
    let n: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'party.contact_request'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Администратор без права — 403, с правом — подтверждает.
    let (adm, adm_id) = w.admin(&[]).await;
    let rid = r["id"].as_str().unwrap();
    let uri = format!("/api/v1/parties/{firm}/contact-requests/{rid}/approve");
    let (s, _) = w.post_as(&adm, &uri, json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    w.perms(&adm_id, &["clients"]).await;
    let (s, v) = w.post_as(&adm, &uri, json!({})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "approved");
    w.ok("POST", "/api/v1/sales", sale).await;

    // Второй водитель: руководитель отклоняет в кабинете — только с причиной.
    let (_, r2) = w
        .post_as(
            &cab,
            "/api/v1/client/contacts",
            json!({ "op_id": Uuid::now_v7(), "full_name": "Чужой" }),
        )
        .await;
    let rid2 = r2["id"].as_str().unwrap();
    let uri = format!("/api/v1/client/contact-requests/{rid2}/reject");
    let (s, _) = w.post_as(&cab, &uri, json!({ "reason": "" })).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = w
        .post_as(&cab, &uri, json!({ "reason": "не работает у нас" }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "rejected");
    let (_, _, list) = call(
        &w.app,
        "GET",
        "/api/v1/client/contact-requests",
        Some(&cab),
        None,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    let n = w.get("/api/v1/notifications").await;
    assert!(n.to_string().contains("водитель отклонён"), "{n}");
}
