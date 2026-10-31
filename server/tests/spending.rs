//! «Расходы» — все траты, кроме оплаты поставщикам (задача 003 Н12б): расходы по статьям,
//! выплаты работникам по дате выплаты и начисленные бонусы одним списком; сторно скрыто;
//! администратору выплаты — только с «Расчётом»; оклад «ожидается» в «Прибыли и убытках».
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{Datelike, Days, NaiveDate};
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

/// Ответ: код, cookie, `x-total-count`, тело.
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Option<i64>, Value) {
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
    let total = res
        .headers()
        .get("x-total-count")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok());
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, cookie, total, json)
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (status, cookie, _, _) = call(
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

async fn ok(app: &Router, who: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (s, _, _, out) = call(app, method, uri, Some(who), body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {out}");
    out
}

/// Список и `x-total-count`.
async fn list(app: &Router, who: &str, uri: &str) -> (Vec<Value>, i64) {
    let (s, _, total, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    (out.as_array().unwrap().clone(), total.unwrap())
}

fn kinds(rows: &[Value]) -> Vec<(String, String, i64)> {
    rows.iter()
        .map(|r| {
            (
                r["kind"].as_str().unwrap().to_string(),
                r["payee"].as_str().unwrap().to_string(),
                r["amount_tyiyn"].as_i64().unwrap(),
            )
        })
        .collect()
}

async fn set_permission(pool: &PgPool, key: &str, on: bool) {
    if on {
        sqlx::query(
            "insert into user_permissions (user_id, branch_id, permission)
             select id, branch_id, $1 from users where login = 'admin' on conflict do nothing",
        )
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    } else {
        sqlx::query(
            "delete from user_permissions where permission = $1
               and user_id = (select id from users where login = 'admin')",
        )
        .bind(key)
        .execute(pool)
        .await
        .unwrap();
    }
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    master: String,
    safe: String,
    today: NaiveDate,
    yesterday: NaiveDate,
}

/// Владелец и администратор, кассир (с окладом по умолчанию) и мастер, открытая смена с разменом.
async fn world(pool: PgPool) -> W {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let id = |v: Value| v["id"].as_str().unwrap().to_string();
    let cashier = id(ok(
        &app,
        &owner,
        "POST",
        "/api/v1/employees",
        Some(json!({ "full_name": "Кассир Айбек", "is_cashier": true, "pin": "1234" })),
    )
    .await);
    let master = id(ok(
        &app,
        &owner,
        "POST",
        "/api/v1/employees",
        Some(json!({ "full_name": "Мастер Нурлан", "is_master": true })),
    )
    .await);
    ok(
        &app,
        &admin,
        "POST",
        "/api/v1/shifts/open",
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    ok(
        &app,
        &admin,
        "POST",
        "/api/v1/cash/movements",
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 1_000_000, "comment": "размен" })),
    )
    .await;
    let accounts = ok(&app, &owner, "GET", "/api/v1/cash/accounts", None).await;
    let safe = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф владельца")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let today: NaiveDate = sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
        .fetch_one(&pool)
        .await
        .unwrap();
    W {
        app,
        pool,
        owner,
        admin,
        cashier,
        master,
        safe,
        today,
        yesterday: today.checked_sub_days(Days::new(1)).unwrap(),
    }
}

impl W {
    async fn article(&self, name: &str) -> String {
        let list = ok(
            &self.app,
            &self.owner,
            "GET",
            "/api/v1/expense-articles",
            None,
        )
        .await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expenses_list_has_payouts_and_bonuses_but_not_supplier_payments(pool: PgPool) {
    let w = world(pool.clone()).await;
    let (app, owner, admin) = (&w.app, w.owner.as_str(), w.admin.as_str());
    let household = w.article("Хозтовары").await;

    // Расход из кассы с «Кому» и ошибочный расход, который сторнировали.
    ok(
        app,
        admin,
        "POST",
        "/api/v1/expenses",
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 10_000,
                     "payee": "Хозмаг", "comment": "ведро" }),
        ),
    )
    .await;
    let wrong = ok(
        app,
        admin,
        "POST",
        "/api/v1/expenses",
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 7_000,
                     "payee": "Ошибка" }),
        ),
    )
    .await;
    ok(
        app,
        admin,
        "POST",
        &format!("/api/v1/expenses/{}/reverse", wrong["id"].as_str().unwrap()),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;

    // Бонусы мастеру и кассиру; бонус мастеру выдан частично «вчерашней» датой из сейфа,
    // кассиру — выплата без вида.
    for (emp, amount) in [(&w.master, 5_000), (&w.cashier, 3_000)] {
        ok(
            app,
            owner,
            "POST",
            "/api/v1/payroll/accruals",
            Some(
                json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "kind": "bonus",
                         "amount_tyiyn": amount, "comment": "за план" }),
            ),
        )
        .await;
    }
    ok(
        app,
        owner,
        "POST",
        "/api/v1/payouts",
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": w.master, "account_id": w.safe,
                     "pay_date": w.yesterday, "comment": "премия",
                     "lines": [{ "purpose": "bonus", "amount_tyiyn": 2_000 }] }),
        ),
    )
    .await;
    ok(
        app,
        owner,
        "POST",
        "/api/v1/payouts",
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "account_id": w.safe,
                     "amount_tyiyn": 1_000, "purpose": "general" }),
        ),
    )
    .await;

    // Оплата поставщику за товар — не трата для «Расходов».
    let cat = ok(
        app,
        owner,
        "POST",
        "/api/v1/categories",
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let product = ok(
        app,
        owner,
        "POST",
        "/api/v1/products",
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр",
                     "sale_price_tyiyn": 50_000 }),
        ),
    )
    .await;
    ok(
        app,
        owner,
        "POST",
        "/api/v1/receipts",
        Some(json!({ "op_id": Uuid::now_v7(), "paid_from": w.safe,
                     "lines": [{ "product_id": product["id"], "qty": 2, "cost_tyiyn": 30_000 }] })),
    )
    .await;
    let paid: i64 =
        sqlx::query_scalar("select count(*) from cash_movements where kind = 'supplier_payment'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(paid, 1);

    let period = format!("from={}&to={}", w.yesterday, w.today);
    let (rows, total) = list(app, owner, &format!("/api/v1/expenses/all?{period}")).await;
    assert_eq!(total, 5, "{rows:?}");
    let got = kinds(&rows);
    assert!(
        got.contains(&("expense".into(), "Хозмаг".into(), 10_000)),
        "{got:?}"
    );
    assert!(
        got.contains(&("payout".into(), "Мастер Нурлан".into(), 2_000)),
        "{got:?}"
    );
    assert!(
        got.contains(&("payout".into(), "Кассир Айбек".into(), 1_000)),
        "{got:?}"
    );
    assert!(
        got.contains(&("bonus_accrual".into(), "Мастер Нурлан".into(), 5_000)),
        "{got:?}"
    );
    assert!(
        got.contains(&("bonus_accrual".into(), "Кассир Айбек".into(), 3_000)),
        "{got:?}"
    );
    // Пара «расход + сторно» скрыта; поставщика в списке нет.
    assert!(!got.iter().any(|g| g.1 == "Ошибка"), "{got:?}");
    assert!(rows.iter().all(|r| r["amount_tyiyn"] != 60_000));
    // Выплата — датой выплаты в расчёте, с видом, кассой и кто провёл.
    let bonus_pay = rows
        .iter()
        .find(|r| r["kind"] == "payout" && r["purpose"] == "bonus")
        .unwrap();
    assert_eq!(bonus_pay["date"], w.yesterday.to_string());
    assert_eq!(bonus_pay["account_name"], "Сейф владельца");
    assert_eq!(bonus_pay["comment"], "премия");
    assert_eq!(bonus_pay["article"], "Зарплата работникам");
    assert_eq!(bonus_pay["user_name"], "Владелец");
    // Новые сначала: вчерашняя выплата — последней.
    assert_eq!(rows.last().unwrap()["date"], w.yesterday.to_string());

    // «Показать сторно» — пара на месте; отборы «Расходы» и «Выплаты работникам».
    let (all, total) = list(
        app,
        owner,
        &format!("/api/v1/expenses/all?{period}&reversals=1"),
    )
    .await;
    assert_eq!(total, 7);
    assert_eq!(all.iter().filter(|r| r["payee"] == "Ошибка").count(), 2);
    let (only, _) = list(
        app,
        owner,
        &format!("/api/v1/expenses/all?{period}&kind=expenses"),
    )
    .await;
    assert!(only.iter().all(|r| r["kind"] == "expense") && only.len() == 1);
    let (staff, _) = list(
        app,
        owner,
        &format!("/api/v1/expenses/all?{period}&kind=payouts"),
    )
    .await;
    assert_eq!(staff.len(), 4);
    assert!(staff.iter().all(|r| r["kind"] != "expense"));
    let (s, _, _, _) = call(
        app,
        "GET",
        "/api/v1/expenses/all?kind=bank",
        Some(owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Страницы: одна выборка, число строк — в заголовке; за концом — пусто с тем же числом.
    let (page, total) = list(
        app,
        owner,
        &format!("/api/v1/expenses/all?{period}&limit=2&offset=2"),
    )
    .await;
    assert_eq!((page.len(), total), (2, 5));
    let (page, total) = list(
        app,
        owner,
        &format!("/api/v1/expenses/all?{period}&limit=2&offset=40"),
    )
    .await;
    assert_eq!((page.len(), total), (0, 5));

    // Итог: расходы по статьям, «Зарплата работникам» — выданное, в т. ч. бонусы; начислено и не
    // выдано — отдельно, в итог не входит.
    let sum = ok(
        app,
        owner,
        "GET",
        &format!("/api/v1/expenses/all/summary?{period}"),
        None,
    )
    .await;
    assert_eq!(
        sum["articles"],
        json!([{ "article": "Хозтовары", "amount_tyiyn": 10_000 }])
    );
    assert_eq!(sum["expenses_tyiyn"], 10_000);
    assert_eq!(sum["payouts_tyiyn"], 3_000);
    assert_eq!(sum["bonus_paid_tyiyn"], 2_000);
    assert_eq!(sum["bonus_accrued_tyiyn"], 8_000);
    assert_eq!(sum["bonus_unpaid_tyiyn"], 6_000);
    assert_eq!(sum["total_tyiyn"], 13_000);

    // Настройка «Показывать начисленные бонусы» выключена — ни строк, ни «начислено, не выдано».
    let mut st = ok(app, owner, "GET", "/api/v1/settings/profit-report", None).await;
    assert_eq!(st["expenses_bonus_accruals"], true);
    st["expenses_bonus_accruals"] = json!(false);
    ok(
        app,
        owner,
        "PUT",
        "/api/v1/settings/profit-report",
        Some(st),
    )
    .await;
    let (rows, total) = list(app, owner, &format!("/api/v1/expenses/all?{period}")).await;
    assert_eq!(total, 3);
    assert!(rows.iter().all(|r| r["kind"] != "bonus_accrual"));
    let sum = ok(
        app,
        owner,
        "GET",
        &format!("/api/v1/expenses/all/summary?{period}"),
        None,
    )
    .await;
    assert_eq!(sum["bonus_unpaid_tyiyn"], Value::Null);
    assert_eq!(sum["total_tyiyn"], 13_000);
    let mut st = ok(app, owner, "GET", "/api/v1/settings/profit-report", None).await;
    st["expenses_bonus_accruals"] = json!(true);
    ok(
        app,
        owner,
        "PUT",
        "/api/v1/settings/profit-report",
        Some(st),
    )
    .await;

    // Администратор с «Расчётом», без «Заработка кассиров»: мастер виден, кассир — нет; счёт
    // владельца не называется.
    let (rows, _) = list(app, admin, &format!("/api/v1/expenses/all?{period}")).await;
    let got = kinds(&rows);
    assert!(
        got.contains(&("expense".into(), "Хозмаг".into(), 10_000)),
        "{got:?}"
    );
    assert!(
        got.contains(&("payout".into(), "Мастер Нурлан".into(), 2_000)),
        "{got:?}"
    );
    assert!(!got.iter().any(|g| g.1 == "Кассир Айбек"), "{got:?}");
    let pay = rows.iter().find(|r| r["kind"] == "payout").unwrap();
    assert_eq!(pay["account_name"], Value::Null);
    let sum = ok(
        app,
        admin,
        "GET",
        &format!("/api/v1/expenses/all/summary?{period}"),
        None,
    )
    .await;
    assert_eq!(sum["payouts_tyiyn"], 2_000);
    assert_eq!(sum["bonus_unpaid_tyiyn"], 3_000);
    // С «Заработком кассиров» — и кассир.
    set_permission(&pool, "payroll_view_all", true).await;
    let (rows, _) = list(app, admin, &format!("/api/v1/expenses/all?{period}")).await;
    assert!(rows.iter().any(|r| r["payee"] == "Кассир Айбек"));
    set_permission(&pool, "payroll_view_all", false).await;

    // Без «Расчёта» — только расходы; отбор «Выплаты работникам» — 403.
    set_permission(&pool, "view_payroll", false).await;
    let (rows, total) = list(app, admin, &format!("/api/v1/expenses/all?{period}")).await;
    assert_eq!(total, 1);
    assert!(rows.iter().all(|r| r["kind"] == "expense"));
    let sum = ok(
        app,
        admin,
        "GET",
        &format!("/api/v1/expenses/all/summary?{period}"),
        None,
    )
    .await;
    assert_eq!(
        (sum["payouts"].clone(), sum["payouts_tyiyn"].clone()),
        (json!(false), json!(0))
    );
    let (s, _, _, err) = call(
        app,
        "GET",
        "/api/v1/expenses/all?kind=payouts",
        Some(admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(err["error"]["message"], "Нет права: Расчёт сотрудников");
    // Без «Расходов» и без «Расчёта» — 403 и списку, и итогу.
    set_permission(&pool, "expenses", false).await;
    for uri in ["/api/v1/expenses/all", "/api/v1/expenses/all/summary"] {
        let (s, _, _, err) = call(app, "GET", uri, Some(admin), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(err["error"]["message"], "Нет права: Расходы");
    }
    // Только «Расчёт» — выплаты без расходов.
    set_permission(&pool, "view_payroll", true).await;
    let (rows, _) = list(app, admin, &format!("/api/v1/expenses/all?{period}")).await;
    assert!(!rows.is_empty() && rows.iter().all(|r| r["kind"] != "expense"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn profit_shows_expected_salary_outside_net(pool: PgPool) {
    let w = world(pool).await;
    let (app, owner) = (&w.app, w.owner.as_str());
    let first = w.today.with_day(1).unwrap();
    let uri = format!(
        "/api/v1/reports/profit?detail=1&from={first}&to={}",
        w.today
    );
    // Кассир с окладом по правилу, период (по дню расчёта 1 — месяц) не закончился, не начислен.
    let before = ok(app, owner, "GET", &uri, None).await;
    let expected = before["detail"]["salary_expected"].as_array().unwrap();
    assert_eq!(expected.len(), 1, "{expected:?}");
    let row = &expected[0];
    assert_eq!(row["employee_name"], "Кассир Айбек");
    assert_eq!(row["period_from"], first.to_string());
    assert!(row["expected_tyiyn"].as_i64().unwrap() > 0);
    let pay: NaiveDate = row["pay_date"].as_str().unwrap().parse().unwrap();
    assert!(pay > w.today);
    // В чистую не входит: зарплаты за период ещё нет.
    assert_eq!(before["totals"]["payroll_tyiyn"], 0);

    // Начислили оклад — строки «ожидается» больше нет, оклад — в зарплате.
    ok(
        app,
        owner,
        "POST",
        "/api/v1/payroll/salary",
        Some(json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": first })),
    )
    .await;
    let after = ok(app, owner, "GET", &uri, None).await;
    assert_eq!(after["detail"]["salary_expected"], json!([]));
    assert_eq!(after["totals"]["payroll_tyiyn"], row["expected_tyiyn"]);
    let _ = (&w.pool, &w.admin, &w.master, &w.safe);
}
