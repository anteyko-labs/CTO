//! Выплаты по видам и аванс; кассиру выплачивает владелец (SPEC-36 §11, ADR-122).
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
    // Администратор с правами по умолчанию (ADR-097): «Аванс мастерам» среди них нет.
    let keys: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    assert!(!keys.iter().any(|k| k == "payroll_advance"));
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

async fn ok(app: &Router, who: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, "POST", uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn get(app: &Router, who: &str, uri: &str) -> Value {
    let (s, _, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

/// Мир теста: владелец, администратор, кассир с окладом, мастер, открытая смена с деньгами.
struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    master: String,
}

async fn world(pool: PgPool) -> W {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = ok(
        &app,
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Кассир Айбек", "is_cashier": true, "pin": "1234" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let master = ok(
        &app,
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Мастер Нурлан", "is_master": true }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    ok(
        &app,
        &admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 10_000_000, "comment": "размен" }),
    )
    .await;
    W {
        app,
        pool,
        owner,
        admin,
        cashier,
        master,
    }
}

impl W {
    /// Начисление из чека или смены — прямо в таблицу, как его пишет проведение.
    async fn accrue(&self, emp: &str, kind: &str, amount: i64) {
        self.accrue_on(
            emp,
            kind,
            amount,
            "(now() at time zone 'Asia/Bishkek')::date",
        )
        .await;
    }

    async fn accrue_on(&self, emp: &str, kind: &str, amount: i64, date_sql: &str) {
        sqlx::query(&format!(
            "insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn, user_id, device_id)
             select $1, e.branch_id, e.id, {date_sql}, $3, $4, (select id from users where login = 'owner'), $1
             from employees e where e.id = $2::uuid"
        ))
        .bind(Uuid::now_v7())
        .bind(emp)
        .bind(kind)
        .bind(amount)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn pay(&self, who: &str, emp: &str, lines: Value) -> (StatusCode, Value) {
        let (s, _, out) = call(
            &self.app,
            "POST",
            "/api/v1/payouts",
            Some(who),
            Some(json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "lines": lines })),
        )
        .await;
        (s, out)
    }

    async fn remains(&self, emp: &str) -> Value {
        get(
            &self.app,
            &self.owner,
            &format!("/api/v1/payroll/remains?employee_id={emp}"),
        )
        .await
    }

    async fn purposes(&self, emp: &str) -> Vec<(String, i64)> {
        sqlx::query_as(
            "select purpose, amount_tyiyn from payouts where employee_id = $1::uuid order by number",
        )
        .bind(emp)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn day_row(&self, who: &str, emp: &str) -> Value {
        get(&self.app, who, "/api/v1/payroll/day")
            .await
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["employee_id"] == json!(emp))
            .cloned()
            .unwrap_or(Value::Null)
    }
}

fn r(v: &Value, key: &str) -> i64 {
    v["remains"][key].as_i64().unwrap()
}

#[sqlx::test(migrations = false)]
async fn migration_marks_old_payouts_general(pool: PgPool) {
    // Миграция 0069: прежние выплаты получают вид `general`, запрет правки выплат не мешает.
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 69) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    sqlx::raw_sql(
        "insert into employees (id, branch_id, full_name, is_master)
           select gen_random_uuid(), id, 'Мастер', true from branches;
         insert into payouts (id, branch_id, number, employee_id, amount_tyiyn, source, user_id, device_id)
           select gen_random_uuid(), e.branch_id, 1, e.id, 5000, 'account', u.id, gen_random_uuid()
           from employees e, users u;",
    )
    .execute(&pool)
    .await
    .unwrap();
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version == 69) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let rows: Vec<(String, Option<Uuid>)> = sqlx::query_as("select purpose, op_id from payouts")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows, vec![("general".to_string(), None)]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn percent_and_bonus_in_one_operation(pool: PgPool) {
    // Проценты и бонус одной операцией — две записи с одним op_id; повтор не дублирует.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    w.accrue(&w.cashier, "bonus", 100_000).await;
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "employee_id": w.cashier, "comment": "за неделю",
                       "lines": [{ "purpose": "bonus", "amount_tyiyn": 100_000 },
                                 { "purpose": "percent", "amount_tyiyn": 50_000 }] });
    let first = ok(&w.app, &w.owner, "/api/v1/payouts", body.clone()).await;
    assert_eq!(first["lines"].as_array().unwrap().len(), 2, "{first}");
    assert_eq!(first["balance_tyiyn"], 0);
    let again = ok(&w.app, &w.owner, "/api/v1/payouts", body).await;
    assert_eq!(again, first);
    let rows: Vec<(String, i64, Option<Uuid>)> = sqlx::query_as(
        "select purpose, amount_tyiyn, op_id from payouts where employee_id = $1::uuid order by number",
    )
    .bind(&w.cashier)
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("percent".to_string(), 50_000, Some(op)),
            ("bonus".to_string(), 100_000, Some(op)),
        ]
    );
    let till: i64 =
        sqlx::query_scalar("select balance_tyiyn from cash_accounts where name = 'Касса'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(till, 10_000_000 - 150_000);
    let rem = w.remains(&w.cashier).await;
    assert_eq!(rem["balance_tyiyn"], 0, "{rem}");
    // В «Начислениях и выплатах» видно, за что.
    let date = chrono::Utc::now().date_naive();
    let entries = get(
        &w.app,
        &w.owner,
        &format!(
            "/api/v1/payroll/entries?employee_id={}&from={}&to={}",
            w.cashier,
            date.pred_opt().unwrap(),
            date.succ_opt().unwrap()
        ),
    )
    .await;
    let mut purposes: Vec<&str> = entries["payouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["purpose"].as_str().unwrap())
        .collect();
    purposes.sort_unstable();
    assert_eq!(purposes, vec!["bonus", "percent"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn line_over_its_purpose_is_rejected(pool: PgPool) {
    // Строка больше остатка своего вида — 422, даже если всего должны больше.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    w.accrue(&w.cashier, "bonus", 100_000).await;
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "percent", "amount_tyiyn": 60_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert!(
        out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("проценты: к выдаче 500 с"),
        "{out}"
    );
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "bonus", "amount_tyiyn": 1_000 }, { "purpose": "bonus", "amount_tyiyn": 1_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "general", "amount_tyiyn": 1_000 }, { "purpose": "bonus", "amount_tyiyn": 1_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert!(w.purposes(&w.cashier).await.is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn advance_is_taken_from_salary_not_percent(pool: PgPool) {
    // После аванса 10 000 проценты дня всё равно выдаются; при окладе 30 000 к выплате оклада 20 000.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "advance", "amount_tyiyn": 1_000_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let rem = w.remains(&w.cashier).await;
    assert_eq!(rem["advance_to"], "salary");
    assert_eq!(r(&rem, "salary_tyiyn"), -1_000_000);
    assert_eq!(r(&rem, "percent_tyiyn"), 50_000);
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "percent", "amount_tyiyn": 50_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let month = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-01")
        .to_string();
    ok(
        &w.app,
        &w.owner,
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": month, "amount_tyiyn": 3_000_000 }),
    )
    .await;
    let rem = w.remains(&w.cashier).await;
    assert_eq!(r(&rem, "salary_tyiyn"), 2_000_000, "{rem}");
    assert_eq!(rem["balance_tyiyn"], 2_000_000);
    // Оклада больше остатка не выдать; ровно остаток — можно.
    let (s, _) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "salary", "amount_tyiyn": 2_000_001 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // «Расчёт за день»: оклад, аванс отдельными колонками.
    let row = w.day_row(&w.owner, &w.cashier).await;
    assert_eq!(row["salary_tyiyn"], 3_000_000, "{row}");
    assert_eq!(row["advance_tyiyn"], 1_000_000, "{row}");
    assert_eq!(row["paid_tyiyn"], 1_050_000, "{row}");
    // «Расчёт за месяц» — колонка «Аванс».
    let m = get(&w.app, &w.owner, "/api/v1/payroll/month").await;
    let mrow = m["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["employee_id"] == json!(w.cashier))
        .unwrap();
    assert_eq!(mrow["advance_tyiyn"], 1_000_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_payout_is_split_by_purposes(pool: PgPool) {
    // Выплата одной суммой — `general`; при чтении она гасит проценты, потом бонусы.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    w.accrue(&w.cashier, "bonus", 100_000).await;
    ok(
        &w.app,
        &w.owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "amount_tyiyn": 70_000 }),
    )
    .await;
    assert_eq!(
        w.purposes(&w.cashier).await,
        vec![("general".to_string(), 70_000)]
    );
    let rem = w.remains(&w.cashier).await;
    assert_eq!(r(&rem, "percent_tyiyn"), 0, "{rem}");
    assert_eq!(r(&rem, "bonus_tyiyn"), 80_000, "{rem}");
    let row = w.day_row(&w.owner, &w.cashier).await;
    assert_eq!(rem["balance_tyiyn"], row["balance_tyiyn"]);
    assert_eq!(row["bonus_tyiyn"], 100_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn withholding_comes_off_the_nearest_payout(pool: PgPool) {
    // Удержание вычитается из остатков по порядку: сначала из процентов.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    ok(
        &w.app,
        &w.owner,
        "/api/v1/payroll/accruals",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "kind": "penalty",
                "amount_tyiyn": 20_000, "comment": "опоздание" }),
    )
    .await;
    let rem = w.remains(&w.cashier).await;
    assert_eq!(r(&rem, "percent_tyiyn"), 30_000, "{rem}");
    assert_eq!(r(&rem, "withheld_tyiyn"), 0, "{rem}");
    let (s, _) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "percent", "amount_tyiyn": 50_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let row = w.day_row(&w.owner, &w.cashier).await;
    assert_eq!(row["withheld_tyiyn"], -20_000, "{row}");
    // Мастеру без процентов удержать не из чего — остаётся «удержано».
    w.accrue(&w.master, "shortage", -5_000).await;
    let rem = w.remains(&w.master).await;
    assert_eq!(r(&rem, "withheld_tyiyn"), -5_000, "{rem}");
    assert_eq!(rem["advance_to"], "service_fee");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_pays_masters_for_services_and_advance_only_with_right(pool: PgPool) {
    // Администратор: кассиру — 403; мастеру за услуги — проведено; бонус — владелец;
    // аванс без права — 403, с правом — не больше заработанного за услуги.
    let w = world(pool).await;
    w.accrue(&w.cashier, "revenue_percent", 50_000).await;
    w.accrue(&w.master, "service_fee", 3_000).await;
    w.accrue(&w.master, "bonus", 10_000).await;
    let (s, out) = w
        .pay(
            &w.admin,
            &w.cashier,
            json!([{ "purpose": "percent", "amount_tyiyn": 50_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["error"]["message"], "Кассиру выплачивает владелец");
    let (s, _, out) = call(
        &w.app,
        "GET",
        &format!("/api/v1/payroll/remains?employee_id={}", w.cashier),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    let (s, out) = w
        .pay(
            &w.admin,
            &w.master,
            json!([{ "purpose": "bonus", "amount_tyiyn": 10_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    let (s, out) = w
        .pay(
            &w.admin,
            &w.master,
            json!([{ "purpose": "service_fee", "amount_tyiyn": 3_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let (s, out) = w
        .pay(
            &w.admin,
            &w.master,
            json!([{ "purpose": "advance", "amount_tyiyn": 1_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(out["error"]["message"], "Нет права: Аванс мастерам");
    // Владелец выдаёт право.
    let admin_id: Uuid = sqlx::query_scalar("select id from users where login = 'admin'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let mut perms = avtodom_server::auth::default_permissions();
    perms.push("payroll_advance");
    let (s, _, out) = call(
        &w.app,
        "PUT",
        &format!("/api/v1/users/{admin_id}/permissions"),
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": perms })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let (s, out) = w
        .pay(
            &w.admin,
            &w.master,
            json!([{ "purpose": "advance", "amount_tyiyn": 1_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    w.accrue(&w.master, "service_fee", 5_000).await;
    let (s, out) = w
        .pay(
            &w.admin,
            &w.master,
            json!([{ "purpose": "advance", "amount_tyiyn": 2_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let rem = w.remains(&w.master).await;
    assert_eq!(r(&rem, "service_fee_tyiyn"), 3_000, "{rem}");
    assert_eq!(r(&rem, "bonus_tyiyn"), 10_000, "{rem}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn reversal_and_fix_keep_the_purpose(pool: PgPool) {
    // Сторно выплаты — того же вида; «Исправить» проводит новую выплату того же вида.
    let w = world(pool).await;
    w.accrue(&w.cashier, "bonus", 100_000).await;
    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "bonus", "amount_tyiyn": 100_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let pid = out["id"].as_str().unwrap().to_string();
    ok(
        &w.app,
        &w.owner,
        &format!("/api/v1/payouts/{pid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    assert_eq!(
        w.purposes(&w.cashier).await,
        vec![
            ("bonus".to_string(), 100_000),
            ("bonus".to_string(), -100_000)
        ]
    );
    assert_eq!(r(&w.remains(&w.cashier).await, "bonus_tyiyn"), 100_000);

    let (s, out) = w
        .pay(
            &w.owner,
            &w.cashier,
            json!([{ "purpose": "bonus", "amount_tyiyn": 60_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let pid = out["id"].as_str().unwrap().to_string();
    let mut draft = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/corrections/draft/payout/{pid}"),
    )
    .await;
    assert_eq!(draft["doc"]["purpose"], "bonus", "{draft}");
    draft["doc"]["amount_tyiyn"] = json!(80_000);
    let fixed = ok(
        &w.app,
        &w.owner,
        "/api/v1/corrections",
        json!({ "op_id": Uuid::now_v7(), "kind": "payout", "original_id": pid,
                "comment": "ошибка ввода", "doc": draft["doc"] }),
    )
    .await;
    assert!(fixed.is_object(), "{fixed}");
    let rows = w.purposes(&w.cashier).await;
    assert_eq!(
        rows.last().unwrap(),
        &("bonus".to_string(), 80_000),
        "{rows:?}"
    );
    assert_eq!(r(&w.remains(&w.cashier).await, "bonus_tyiyn"), 20_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn month_keeps_disabled_employee_while_money_is_open(pool: PgPool) {
    // Отключённый сотрудник виден в «Расчёте за месяц», если в месяце были выплаты или есть
    // остаток на начало или на конец; без расчёта — нет.
    let w = world(pool).await;
    let prev =
        "(date_trunc('month', (now() at time zone 'Asia/Bishkek')::date) - interval '1 day')::date";
    w.accrue_on(&w.master, "service_fee", 3_000, prev).await;
    let gone = ok(
        &w.app,
        &w.owner,
        "/api/v1/employees",
        json!({ "full_name": "Мастер Старый", "is_master": true }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    w.accrue_on(&gone, "service_fee", 4_000, prev).await;
    let (s, out) = w
        .pay(
            &w.owner,
            &gone,
            json!([{ "purpose": "service_fee", "amount_tyiyn": 4_000 }]),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let quiet = ok(
        &w.app,
        &w.owner,
        "/api/v1/employees",
        json!({ "full_name": "Мастер Тихий", "is_master": true }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    sqlx::query("update employees set active = false where id = any($1::uuid[])")
        .bind(vec![
            Uuid::parse_str(&w.master).unwrap(),
            Uuid::parse_str(&gone).unwrap(),
            Uuid::parse_str(&quiet).unwrap(),
        ])
        .execute(&w.pool)
        .await
        .unwrap();
    let m = get(&w.app, &w.owner, "/api/v1/payroll/month").await;
    let ids: Vec<&str> = m["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["employee_id"].as_str().unwrap())
        .collect();
    // Мастеру должны с прошлого месяца; «Старому» выплатили в этом месяце; «Тихому» — ничего.
    assert!(ids.contains(&w.master.as_str()), "{m}");
    assert!(ids.contains(&gone.as_str()), "{m}");
    assert!(!ids.contains(&quiet.as_str()), "{m}");
}
