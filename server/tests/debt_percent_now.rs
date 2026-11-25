//! «Процент кассира с долга — сразу» (задача 050, ADR-214): включённая настройка филиала даёт
//! процент со всей валовой прибыли долгового чека при проведении, без ожидания погашения;
//! возврат снимает его как у оплаченного чека; выключенная — как раньше (ADR-035).
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
            .body(Body::from(with_debt_signature(uri, b).to_string())),
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
    branch: Uuid,
    owner: String,
    admin: String,
    cashier: String,
    firm: String,
    filter: Value,
}

impl World {
    /// Владелец, администратор с правами по умолчанию, кассир и открытая смена.
    async fn new(pool: PgPool) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let branch: Uuid = sqlx::query_scalar("select id from branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        let hash = hash_password("admin-pass-1".into()).await.unwrap();
        let admin_id = Uuid::now_v7();
        sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Эрбол')")
            .bind(admin_id)
            .bind(branch)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
        let keys: Vec<String> = avtodom_server::auth::default_permissions()
            .into_iter()
            .map(String::from)
            .collect();
        sqlx::query(
            "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
        )
        .bind(admin_id)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
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
        let safe = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["owner_only"] == json!(true))
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        ok(
            &app,
            "POST",
            "/api/v1/shifts/open",
            &admin,
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
        )
        .await;
        let today: chrono::NaiveDate =
            sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
                .fetch_one(&pool)
                .await
                .unwrap();
        let _ = admin_id;
        let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
            .fetch_one(&pool)
            .await
            .unwrap();
        // Кассиру 2 % с валовой прибыли.
        sqlx::query(
            "insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base, rate_bp, user_id)
             values ($1, $2, $3::uuid, 'revenue_percent', 'cashier', 'gross', 200, $4)",
        )
        .bind(Uuid::now_v7())
        .bind(branch)
        .bind(&cashier)
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();
        let firm = ok(
            &app,
            "POST",
            "/api/v1/parties",
            &owner,
            json!({ "kind": "company", "name": "ОсОО Долг", "inn": "01204201910123", "phone": "0555 10 10 10" }),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let cat = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
        let filter = ok(
            &app,
            "POST",
            "/api/v1/products",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 30_000 }),
        )
        .await["id"]
            .clone();
        ok(
            &app,
            "POST",
            "/api/v1/receipts",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                    "lines": [{ "product_id": filter, "qty": 5, "cost_tyiyn": 50_000 }] }),
        )
        .await;
        let _ = today;
        World {
            app,
            pool,
            branch,
            owner,
            admin,
            cashier,
            firm,
            filter,
        }
    }

    /// Настройка кассы этого филиала: «Процент кассира с долга — сразу».
    async fn debt_percent_now(&self, cookie: &str, on: bool) -> (StatusCode, Value) {
        call(
            &self.app,
            "PUT",
            "/api/v1/settings/cashier",
            Some(cookie),
            Some(json!({ "op_id": Uuid::now_v7(), "debt_percent_now": on })),
        )
        .await
    }

    /// Чек в долг: фильтр 300 с, валовая 200 с; оплачено `paid`, остальное — долг.
    async fn debt_sale(&self, paid: i64) -> String {
        let mut payments = vec![json!({ "method": "debt", "amount_tyiyn": 30_000 - paid })];
        if paid > 0 {
            payments.push(json!({ "method": "cash", "amount_tyiyn": paid }));
        }
        ok(
            &self.app,
            "POST",
            "/api/v1/sales",
            &self.admin,
            json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
                "party_id": self.firm,
                "lines": [{ "kind": "piece", "product_id": self.filter, "qty": 1, "unit_price_tyiyn": 30_000 }],
                "payments": payments
            }),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn percent(&self) -> i64 {
        sqlx::query_scalar(
            "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'revenue_percent'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    async fn pending(&self) -> i64 {
        sqlx::query_scalar("select count(*)::bigint from payroll_pending")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn repay(&self, amount: i64) {
        ok(
            &self.app,
            "POST",
            "/api/v1/debts/repayments",
            &self.admin,
            json!({ "op_id": Uuid::now_v7(), "party_id": self.firm, "amount_tyiyn": amount, "method": "cash" }),
        )
        .await;
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn only_owner_switches_it_and_it_is_journaled(pool: PgPool) {
    let w = World::new(pool).await;
    let (s, _) = w.debt_percent_now(&w.admin.clone(), true).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = w.debt_percent_now(&w.owner.clone(), true).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["debt_percent_now"], json!(true));
    let logged: i64 = sqlx::query_scalar(
        "select count(*)::bigint from audit_log where action = 'settings.cashier'
           and (data->'after'->>'debt_percent_now')::boolean
           and not (data->'before'->>'debt_percent_now')::boolean",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(logged, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn on_gives_full_percent_now_and_repayment_adds_nothing(pool: PgPool) {
    let w = World::new(pool).await;
    // Ждущий чек до включения (ответ 1б) — остаётся ждать погашения.
    w.debt_sale(0).await;
    assert_eq!((w.percent().await, w.pending().await), (0, 1));
    let (s, v) = w.debt_percent_now(&w.owner.clone(), true).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Чек 300 с: 100 с наличными, 200 с в долг. Валовая 200 с × 2 % = 4 с — сразу вся.
    w.debt_sale(10_000).await;
    assert_eq!(w.percent().await, 400);
    assert_eq!(w.pending().await, 1, "новый чек ожидания не заводит");
    // Погашение 500 с: первый (старый) чек — его 4 с приходят как раньше; второй — без процента.
    w.repay(50_000).await;
    assert_eq!(w.percent().await, 800);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn return_takes_percent_back_like_paid_sale(pool: PgPool) {
    let w = World::new(pool).await;
    w.debt_percent_now(&w.owner.clone(), true).await;
    let sale = w.debt_sale(0).await;
    assert_eq!((w.percent().await, w.pending().await), (400, 0));
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/sales/{sale}/return"),
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "debt", "amount_tyiyn": 30_000 }] }),
    )
    .await;
    assert_eq!(w.percent().await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn off_and_other_branch_on_keep_old_way(pool: PgPool) {
    let w = World::new(pool).await;
    // Настройка включена в другом филиале — этот филиал не затронут.
    let other = ok(
        &w.app,
        "POST",
        "/api/v1/branches",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
    )
    .await;
    let other = Uuid::parse_str(other["id"].as_str().unwrap()).unwrap();
    assert_ne!(other, w.branch);
    sqlx::query(
        "insert into settings (branch_id, key, value) values ($1, 'cashier', '{\"debt_percent_now\": true}')",
    )
    .bind(other)
    .execute(&w.pool)
    .await
    .unwrap();
    w.debt_sale(10_000).await;
    // С оплаченной трети — сразу 4 с × 1/3 ≈ 1,33 с; остальное ждёт погашения.
    assert_eq!((w.percent().await, w.pending().await), (133, 1));
    w.repay(20_000).await;
    assert_eq!(w.percent().await, 400);
}

fn with_debt_signature(uri: &str, mut b: Value) -> Value {
    if uri.ends_with("/sales")
        && let Some(o) = b.as_object_mut()
        && !o.contains_key("debt_signature_png")
    {
        o.insert(
            "debt_signature_png".into(),
            Value::from("iVBORw0KGgpzaWctb25lIQ=="),
        );
    }
    b
}
