//! Долг юрлица по видам: машины фирмы и личные машины работников (задача 051, ADR-215).
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

impl W {
    fn sale_car(&self, party: &str, car: &str) -> Value {
        let mut s = self.sale(party);
        s["vehicle_id"] = json!(car);
        s
    }

    async fn car(&self, firm: &str, plate: &str, owner: Option<&str>) -> String {
        let mut b = json!({ "plate": plate });
        if let Some(o) = owner {
            b["owner_name"] = json!(o);
            b["owner_phone"] = json!("0555 11 22 33");
        }
        let v = self
            .ok("POST", &format!("/api/v1/parties/{firm}/vehicles"), b)
            .await;
        v["id"].as_str().unwrap().to_string()
    }

    async fn repay(&self, firm: &str, amount: i64) {
        self.ok(
            "POST",
            "/api/v1/debts/repayments",
            json!({ "op_id": Uuid::now_v7(), "party_id": firm, "amount_tyiyn": amount, "method": "cash" }),
        )
        .await;
    }

    async fn party(&self, firm: &str) -> Value {
        self.get(&format!("/api/v1/parties/{firm}")).await
    }
}

/// Долг 3 000 с: старый чек за личную машину, затем два за машину фирмы.
async fn setup(w: &W) -> (String, String, String) {
    w.mode("off", 10).await;
    let firm = w.firm("ОсОО Виды", "0700 44-55-66", "05050505050505").await;
    let own = w.car(&firm, "01KG100AAA", None).await;
    let personal = w.car(&firm, "01KG200BBB", Some("Бакыт")).await;
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &personal))
        .await;
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &own)).await;
    w.ok("POST", "/api/v1/sales", w.sale(&firm)).await;
    (firm, own, personal)
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn two_kinds_sum_to_balance(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, _) = setup(&w).await;
    let p = w.party(&firm).await;
    assert_eq!(p["balance_tyiyn"], 300_000);
    assert_eq!(p["personal_debt_tyiyn"], 100_000);
    // Карточка и список «Долги» — то же.
    let card = w.get(&format!("/api/v1/parties/{firm}/card")).await;
    assert_eq!(card["party"]["personal_debt_tyiyn"], 100_000);
    let list = w.get("/api/v1/debts/list").await;
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(firm))
        .unwrap();
    assert_eq!(row["personal_debt_tyiyn"], 100_000);
    // Акт: пометка у чека на личной машине и подытог.
    let act = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await;
    let marks: Vec<&Value> = act["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| &r["personal"])
        .collect();
    assert_eq!(
        marks,
        vec![&json!("Бакыт, 01KG200BBB"), &Value::Null, &Value::Null]
    );
    assert_eq!(act["closing_tyiyn"], 300_000);
    assert_eq!(act["closing_personal_tyiyn"], 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn repayment_closes_oldest_first(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, _) = setup(&w).await;
    // Самый старый — личный: 600 с гасят его частично.
    w.repay(&firm, 60_000).await;
    let p = w.party(&firm).await;
    assert_eq!(p["balance_tyiyn"], 240_000);
    assert_eq!(p["personal_debt_tyiyn"], 40_000);
    // Ещё 400 с — личный погашен, остаток весь за машины фирмы.
    w.repay(&firm, 40_000).await;
    let p = w.party(&firm).await;
    assert_eq!(p["balance_tyiyn"], 200_000);
    assert_eq!(p["personal_debt_tyiyn"], 0);
    // Акт на сегодня — без подытогов.
    let act = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await;
    assert_eq!(act["closing_personal_tyiyn"], 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn kind_fixed_in_sale(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, personal) = setup(&w).await;
    // Машину перевели в фирменную: вид старого чека прежний.
    sqlx::query(
        "update party_vehicles set party_id = $1::uuid, paid_by_party_id = null where id = $2::uuid",
    )
    .bind(&firm)
    .bind(&personal)
    .execute(&w.pool)
    .await
    .unwrap();
    assert_eq!(w.party(&firm).await["personal_debt_tyiyn"], 100_000);
    // Новый чек на ней — уже за машины фирмы.
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &personal))
        .await;
    let p = w.party(&firm).await;
    assert_eq!(p["balance_tyiyn"], 400_000);
    assert_eq!(p["personal_debt_tyiyn"], 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn firm_without_personal_as_before(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w.firm("ОсОО Своё", "0700 44-55-77", "05050505050506").await;
    let own = w.car(&firm, "01KG100AAA", None).await;
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &own)).await;
    let p = w.party(&firm).await;
    assert_eq!(p["balance_tyiyn"], 100_000);
    assert_eq!(p["personal_debt_tyiyn"], 0);
    let act = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await;
    assert!(act["rows"][0].get("personal").is_none(), "{act}");
    assert_eq!(act["closing_personal_tyiyn"], 0);
    let text = avtodom_server::api::bot_debt::customer_debt(
        &mut w.pool.acquire().await.unwrap(),
        firm.parse().unwrap(),
    )
    .await
    .unwrap();
    assert!(!text.contains("личные машины"), "{text}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cabinet_and_bot_two_lines(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, _) = setup(&w).await;
    let cab = w.cabinet(&firm, "05050505050505").await;
    let (s, _, me) = call(&w.app, "GET", "/api/v1/client/me", Some(&cab), None).await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(me["balance_tyiyn"], 300_000);
    assert_eq!(me["personal_debt_tyiyn"], 100_000);
    let (s, _, act) = call(
        &w.app,
        "GET",
        "/api/v1/client/reconciliation",
        Some(&cab),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{act}");
    assert_eq!(act["closing_personal_tyiyn"], 100_000);
    let text = avtodom_server::api::bot_debt::customer_debt(
        &mut w.pool.acquire().await.unwrap(),
        firm.parse().unwrap(),
    )
    .await
    .unwrap();
    assert!(text.contains("машины фирмы и прочее: 2"), "{text}");
    assert!(text.contains("личные машины работников: 1"), "{text}");
}

// Задача 055, ADR-218: два отдельных акта фирмы — машины фирмы и личные машины работников.

async fn act(w: &W, firm: &str, kind: &str) -> Value {
    w.get(&format!(
        "/api/v1/parties/{firm}/reconciliation?kind={kind}"
    ))
    .await
}

fn sums(a: &Value) -> (i64, i64, i64, i64) {
    (
        a["opening_tyiyn"].as_i64().unwrap(),
        a["debit_total_tyiyn"].as_i64().unwrap(),
        a["credit_total_tyiyn"].as_i64().unwrap(),
        a["closing_tyiyn"].as_i64().unwrap(),
    )
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn separate_acts_by_kind(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, _) = setup(&w).await;
    // 600 с гасят самый старый — личный — частично.
    w.repay(&firm, 60_000).await;
    let all = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await;
    assert_eq!(all["has_personal"], true);
    let p = act(&w, &firm, "personal").await;
    let c = act(&w, &firm, "company").await;
    assert_eq!(p["act_kind"], "personal");
    assert_eq!(c["act_kind"], "company");
    // Личный: чек 1000 с и 600 с оплаты; остаток 400 с — как в карточке.
    assert_eq!(sums(&p), (0, 100_000, 60_000, 40_000));
    assert_eq!(p["rows"].as_array().unwrap().len(), 2, "{p}");
    assert_eq!(p["rows"][0]["personal"], "Бакыт, 01KG200BBB");
    // Фирма: два чека, оплаты на неё не ушло.
    assert_eq!(sums(&c), (0, 200_000, 0, 200_000));
    assert_eq!(c["rows"].as_array().unwrap().len(), 2, "{c}");
    // Подытогов в отдельных актах нет; вместе — общий акт.
    assert_eq!(p["closing_personal_tyiyn"], 0);
    assert_eq!(
        p["closing_tyiyn"].as_i64().unwrap() + c["closing_tyiyn"].as_i64().unwrap(),
        all["closing_tyiyn"].as_i64().unwrap()
    );
    // Ещё 600 с: 400 с — на личные (погашены), 200 с — на машины фирмы, с пометкой.
    w.repay(&firm, 60_000).await;
    let p = act(&w, &firm, "personal").await;
    let c = act(&w, &firm, "company").await;
    assert_eq!(sums(&p).3, 0);
    assert_eq!(sums(&c).3, 180_000);
    let split = c["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["credit_tyiyn"] == 20_000)
        .expect("часть оплаты на машины фирмы");
    assert!(
        split["comment"].as_str().unwrap().contains("из 600"),
        "{split}"
    );
    // Кабинет фирмы — те же акты.
    let cab = w.cabinet(&firm, "05050505050505").await;
    let (s, _, cp) = call(
        &w.app,
        "GET",
        "/api/v1/client/reconciliation?kind=personal",
        Some(&cab),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{cp}");
    assert_eq!(sums(&cp), sums(&p));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn separate_acts_opening_split(pool: PgPool) {
    let w = W::new(pool).await;
    let (firm, _, _) = setup(&w).await;
    w.repay(&firm, 60_000).await;
    // Движения прошлых дней — в сальдо на начало, по видам.
    for q in [
        "alter table party_ledger disable trigger party_ledger_immutable",
        "update party_ledger set business_date = business_date - 10",
        "alter table party_ledger enable trigger party_ledger_immutable",
    ] {
        sqlx::query(q).execute(&w.pool).await.unwrap();
    }
    let today = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await["to"]
        .as_str()
        .unwrap()
        .to_string();
    let p = w
        .get(&format!(
            "/api/v1/parties/{firm}/reconciliation?kind=personal&from={today}"
        ))
        .await;
    let c = w
        .get(&format!(
            "/api/v1/parties/{firm}/reconciliation?kind=company&from={today}"
        ))
        .await;
    assert_eq!(sums(&p), (40_000, 0, 0, 40_000));
    assert_eq!(sums(&c), (200_000, 0, 0, 200_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn separate_acts_only_for_firm(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w.firm("ОсОО Своё", "0700 44-55-77", "05050505050506").await;
    let own = w.car(&firm, "01KG100AAA", None).await;
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &own)).await;
    let all = w
        .get(&format!("/api/v1/parties/{firm}/reconciliation"))
        .await;
    assert_eq!(all["has_personal"], false);
    let (s, _) = w
        .req(
            "GET",
            &format!("/api/v1/parties/{firm}/reconciliation?kind=bad"),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn separate_acts_return_personal(pool: PgPool) {
    let w = W::new(pool).await;
    w.mode("off", 10).await;
    let firm = w
        .firm("ОсОО Возврат", "0700 44-55-88", "05050505050507")
        .await;
    let own = w.car(&firm, "01KG100AAA", None).await;
    let personal = w.car(&firm, "01KG200BBB", Some("Бакыт")).await;
    let s = w
        .ok("POST", "/api/v1/sales", w.sale_car(&firm, &personal))
        .await;
    w.ok("POST", "/api/v1/sales", w.sale_car(&firm, &own)).await;
    let sid = s["id"].as_str().unwrap();
    let (st, v) = w
        .req(
            "POST",
            &format!("/api/v1/sales/{sid}/return"),
            Some(
                json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                         "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }] }),
            ),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let p = act(&w, &firm, "personal").await;
    let c = act(&w, &firm, "company").await;
    assert_eq!(sums(&p), (0, 100_000, 100_000, 0), "{p}");
    assert_eq!(sums(&c), (0, 100_000, 0, 100_000), "{c}");
}
