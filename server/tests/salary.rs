//! Оклад (задача 003 Н11): дата выплаты в расчёте, день расчёта оклада у работника, вкладка
//! «Оклад», выплата без открытой смены, «Сегодня расчёт оклада» (ADR-154, ADR-155).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{Datelike, Days, Months, NaiveDate};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::domain::salary;
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

async fn ok(app: &Router, who: &str, method: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, method, uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn get(app: &Router, who: &str, uri: &str) -> Value {
    let (s, _, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

fn msg(v: &Value) -> String {
    v["error"]["message"].as_str().unwrap_or("").to_string()
}

/// Владелец, администратор, кассир с окладом 30 000 с (правило по умолчанию), мастер.
/// `shift` — открыть смену с разменом.
struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    master: String,
    today: NaiveDate,
}

async fn world(pool: PgPool, shift: bool) -> W {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let id = |v: Value| v["id"].as_str().unwrap().to_string();
    let cashier = id(ok(
        &app,
        &owner,
        "POST",
        "/api/v1/employees",
        json!({ "full_name": "Кассир Айбек", "is_cashier": true, "pin": "1234" }),
    )
    .await);
    let master = id(ok(
        &app,
        &owner,
        "POST",
        "/api/v1/employees",
        json!({ "full_name": "Мастер Нурлан", "is_master": true }),
    )
    .await);
    // Ставка оклада действует с начала прошлого месяца: тесты начисляют и прошлый месяц.
    sqlx::query("update employee_pay_rules set active_from = active_from - interval '3 months'")
        .execute(&pool)
        .await
        .unwrap();
    if shift {
        ok(
            &app,
            &admin,
            "POST",
            "/api/v1/shifts/open",
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
        )
        .await;
        ok(
            &app,
            &admin,
            "POST",
            "/api/v1/cash/movements",
            json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 10_000_000, "comment": "размен" }),
        )
        .await;
    }
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
        today,
    }
}

impl W {
    async fn pay(&self, who: &str, body: Value) -> (StatusCode, Value) {
        let mut body = body;
        body["op_id"] = json!(body.get("op_id").cloned().unwrap_or(json!(Uuid::now_v7())));
        let (s, _, out) = call(&self.app, "POST", "/api/v1/payouts", Some(who), Some(body)).await;
        (s, out)
    }

    async fn salary(&self, who: &str, query: &str) -> Vec<Value> {
        get(&self.app, who, &format!("/api/v1/payroll/salary{query}")).await["rows"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn salary_of(&self, emp: &str, date: Option<NaiveDate>) -> Value {
        let q = match date {
            Some(d) => format!("?employee_id={emp}&date={d}"),
            None => format!("?employee_id={emp}"),
        };
        let rows = self.salary(&self.owner, &q).await;
        assert_eq!(rows.len(), 1, "{rows:?}");
        rows[0].clone()
    }

    async fn account(&self, kind: &str) -> String {
        let list = get(&self.app, &self.owner, "/api/v1/cash/accounts").await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == kind && a["active"] == true)
            .map(|a| a["id"].as_str().unwrap().to_string())
            .unwrap_or_else(|| panic!("нет счёта {kind}: {list}"))
    }

    async fn settings(&self, patch: Value) {
        let mut s = get(&self.app, &self.owner, "/api/v1/settings/payroll").await;
        for (k, v) in patch.as_object().unwrap() {
            s[k] = v.clone();
        }
        ok(&self.app, &self.owner, "PUT", "/api/v1/settings/payroll", s).await;
    }
}

fn first_of(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap()
}

fn last_of(d: NaiveDate) -> NaiveDate {
    first_of(d)
        .checked_add_months(Months::new(1))
        .unwrap()
        .pred_opt()
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_payout_with_past_date_lands_in_that_day_and_month(pool: PgPool) {
    let w = world(pool, true).await;
    let prev = first_of(w.today)
        .checked_sub_months(Months::new(1))
        .unwrap();
    let prev_last = last_of(prev);
    // Оклад за прошлый месяц и выплата «за последний день того месяца» — деньги из кассы сегодня.
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": prev }),
    )
    .await;
    let till = w.account("register").await;
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.cashier, "account_id": till, "pay_date": prev_last,
                    "lines": [{ "purpose": "salary", "amount_tyiyn": 1_000_000 }] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let pid = out["id"].as_str().unwrap().to_string();
    let month = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/month?month={prev}"),
    )
    .await;
    let row = month["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == w.cashier.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["paid_tyiyn"], 1_000_000, "{row}");
    assert_eq!(row["closing_tyiyn"], 2_000_000, "{row}");
    // В «За день» той даты — выплата есть, в сегодняшнем — нет.
    let day = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/day?date={prev_last}"),
    )
    .await;
    let d = day
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == w.cashier.as_str())
        .unwrap();
    assert_eq!(d["paid_tyiyn"], 1_000_000, "{d}");
    // Деньги — движением кассы сегодня, в открытой смене.
    let (pay_date, money_day): (NaiveDate, NaiveDate) = sqlx::query_as(
        "select p.pay_date, (m.created_at at time zone 'Asia/Bishkek')::date
         from payouts p join cash_movements m on m.doc_id = p.id where p.id = $1::uuid",
    )
    .bind(&pid)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!((pay_date, money_day), (prev_last, w.today));
    let entries = get(
        &w.app,
        &w.owner,
        &format!(
            "/api/v1/payroll/entries?employee_id={}&from={prev}&to={prev_last}",
            w.cashier
        ),
    )
    .await;
    assert_eq!(entries["payouts"][0]["pay_date"], prev_last.to_string());
    // Журнал: дата в расчёте и день денег.
    let data: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'payout.post' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(data["pay_date"], prev_last.to_string(), "{data}");
    assert_eq!(data["money_date"], w.today.to_string(), "{data}");

    // Сторно — той же датой: месяц исходной выплаты сходится в ноль.
    ok(
        &w.app,
        &w.owner,
        "POST",
        &format!("/api/v1/payouts/{pid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    let rev: NaiveDate =
        sqlx::query_scalar("select pay_date from payouts where reversal_of = $1::uuid")
            .bind(&pid)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(rev, prev_last);
    let month = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/month?month={prev}"),
    )
    .await;
    let row = month["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == w.cashier.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["paid_tyiyn"], 0, "{row}");

    // «Исправить» несёт дату исходной выплаты.
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.cashier, "account_id": till, "pay_date": prev_last,
                    "lines": [{ "purpose": "salary", "amount_tyiyn": 500_000 }] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let pid = out["id"].as_str().unwrap().to_string();
    let draft = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/corrections/draft/payout/{pid}"),
    )
    .await;
    assert_eq!(draft["doc"]["pay_date"], prev_last.to_string(), "{draft}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn payout_date_rules(pool: PgPool) {
    let w = world(pool, true).await;
    sqlx::query(
        "insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn, user_id, device_id)
         select $1, e.branch_id, e.id, (now() at time zone 'Asia/Bishkek')::date, 'service_fee', 500000,
                (select id from users where login = 'owner'), $1
         from employees e where e.id = $2::uuid",
    )
    .bind(Uuid::now_v7())
    .bind(&w.master)
    .execute(&w.pool)
    .await
    .unwrap();
    let line = json!([{ "purpose": "service_fee", "amount_tyiyn": 10_000 }]);
    let tomorrow = w.today.succ_opt().unwrap();
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "pay_date": tomorrow, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    let old = w.today.checked_sub_days(Days::new(36)).unwrap();
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "pay_date": old, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    assert!(msg(&out).contains("не раньше"), "{out}");
    // Администратор — только сегодня; сегодняшняя дата явно — можно.
    let yesterday = w.today.pred_opt().unwrap();
    let (s, out) = w
        .pay(
            &w.admin,
            json!({ "employee_id": w.master, "pay_date": yesterday, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(msg(&out), "Дату выплаты меняет владелец");
    let (s, out) = w
        .pay(
            &w.admin,
            json!({ "employee_id": w.master, "pay_date": w.today, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Повтор op_id с другой датой — та же выплата.
    let op = Uuid::now_v7();
    let (s, first) = w
        .pay(
            &w.owner,
            json!({ "op_id": op, "employee_id": w.master, "pay_date": yesterday, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{first}");
    let (s, again) = w
        .pay(
            &w.owner,
            json!({ "op_id": op, "employee_id": w.master, "pay_date": w.today, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(first["id"], again["id"]);
    // 0 дней — только сегодня.
    w.settings(json!({ "payout_backdate_days": 0 })).await;
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "pay_date": yesterday, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    // Настройки «Зарплата» — только владелец, изменение — в журнал.
    let (s, _, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/payroll",
        Some(&w.admin),
        Some(json!({ "payout_backdate_days": 10 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/payroll",
        Some(&w.owner),
        Some(json!({ "payout_backdate_days": 400 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let logged: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'settings.payroll'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(logged, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn salary_now_shows_rate_before_accrual(pool: PgPool) {
    let w = world(pool, true).await;
    let till = w.account("register").await;
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.cashier, "account_id": till,
                    "lines": [{ "purpose": "advance", "amount_tyiyn": 500_000 }] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let p = salary::period_of(1, w.today).unwrap();
    let row = w.salary_of(&w.cashier, None).await;
    assert_eq!(row["period_from"], p.from.to_string(), "{row}");
    assert_eq!(row["pay_date"], p.pay.to_string(), "{row}");
    assert_eq!(row["rule_tyiyn"], 3_000_000, "{row}");
    assert_eq!(row["accrued_tyiyn"], Value::Null, "{row}");
    assert_eq!(row["advance_tyiyn"], 500_000, "{row}");
    assert_eq!(row["expected_tyiyn"], 2_500_000, "{row}");
    assert!(row.get("to_date_tyiyn").is_none(), "{row}");
    // Начислили — то же «к выплате»; окно «Выплата» показывает тот же блок.
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "period_from": p.from }),
    )
    .await;
    let row = w.salary_of(&w.cashier, None).await;
    assert_eq!(row["accrued_tyiyn"], 3_000_000, "{row}");
    assert_eq!(row["expected_tyiyn"], 2_500_000, "{row}");
    let rem = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/remains?employee_id={}", w.cashier),
    )
    .await;
    assert_eq!(rem["salary"]["expected_tyiyn"], 2_500_000, "{rem}");
    assert_eq!(rem["shift_open"], true, "{rem}");
    // Следующий период: недоплата этого — «с прошлых периодов».
    let next = salary::next(1, p).unwrap();
    let row = w.salary_of(&w.cashier, Some(next.from)).await;
    assert_eq!(row["carry_tyiyn"], 2_500_000, "{row}");
    assert_eq!(row["expected_tyiyn"], 5_500_000, "{row}");
    // Администратор кассира не видит: ему — только мастера.
    assert!(w.salary(&w.admin, "").await.is_empty());
    // «Заработано на сегодня» — только с настройкой, одно округление.
    w.settings(json!({ "salary_show_to_date": true })).await;
    let row = w.salary_of(&w.cashier, None).await;
    assert_eq!(
        row["to_date_tyiyn"],
        salary::earned_to_date(3_000_000, p.from, p.to, w.today),
        "{row}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn salary_period_by_employee_day(pool: PgPool) {
    let w = world(pool, false).await;
    let uri = format!("/api/v1/employees/{}/salary-day", w.cashier);
    // День расчёта — только владелец; 1–28.
    let (s, _, _) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.admin),
        Some(json!({ "salary_day": 20 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(&w.app, "GET", &uri, Some(&w.admin), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.owner),
        Some(json!({ "salary_day": 29 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(get(&w.app, &w.owner, &uri).await["salary_day"], 1);

    // Прошлый месяц начислен по-старому (календарный), потом день расчёта стал 20-м.
    let prev = first_of(w.today)
        .checked_sub_months(Months::new(1))
        .unwrap();
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": prev }),
    )
    .await;
    ok(&w.app, &w.owner, "PUT", &uri, json!({ "salary_day": 20 })).await;
    let journal: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'employee.update' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(journal["before"]["salary_day"], 1, "{journal}");
    assert_eq!(journal["after"]["salary_day"], 20, "{journal}");
    // Период «20.(M−1)–19.M» пересекается с начисленным месяцем — 409 с датами начисленного.
    let (s, _, out) = call(
        &w.app,
        "POST",
        "/api/v1/payroll/salary",
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": prev })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{out}");
    assert_eq!(
        msg(&out),
        format!(
            "оклад за {} уже начислен",
            salary::label(prev, last_of(prev))
        )
    );
    // Следующий период начинается после начисленного месяца: «01.M–19.M», к выплате 20.M.
    let cur = first_of(w.today);
    let row = w.salary_of(&w.cashier, Some(last_of(prev))).await;
    assert_eq!(row["period_from"], cur.to_string(), "{row}");
    assert_eq!(
        row["period_to"],
        cur.with_day(19).unwrap().to_string(),
        "{row}"
    );
    assert_eq!(
        row["pay_date"],
        cur.with_day(20).unwrap().to_string(),
        "{row}"
    );
    // Начисление за переходный период — своими датами, ложится концом периода (не позже сегодня).
    let out = ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier,
                "period_from": cur, "period_to": cur.with_day(19).unwrap(), "amount_tyiyn": 1_800_000 }),
    )
    .await;
    let (date, from, to): (NaiveDate, NaiveDate, NaiveDate) = sqlx::query_as(
        "select business_date, period_from, period_to from payroll_accruals where id = $1::uuid",
    )
    .bind(out["id"].as_str().unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        (date, from, to),
        (
            cur.with_day(19).unwrap().min(w.today),
            cur,
            cur.with_day(19).unwrap()
        )
    );
    // Повтор за тот же период — 409.
    let (s, _, out) = call(
        &w.app,
        "POST",
        "/api/v1/payroll/salary",
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "period_from": cur })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{out}");
    // Будущий период — 422.
    let (s, _, out) = call(
        &w.app,
        "POST",
        "/api/v1/payroll/salary",
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier,
                     "period_from": cur.checked_add_months(Months::new(1)).unwrap().with_day(20).unwrap() })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{out}");
    // Сторно оклада — за тот же период; после сторно начисляется заново.
    let aid = out_id(&w, cur).await;
    ok(
        &w.app,
        &w.owner,
        "POST",
        &format!("/api/v1/payroll/accruals/{aid}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    let rev: Option<NaiveDate> =
        sqlx::query_scalar("select period_from from payroll_accruals where reversal_of = $1::uuid")
            .bind(&aid)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(rev, Some(cur));
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier,
                "period_from": cur, "period_to": cur.with_day(19).unwrap(), "amount_tyiyn": 1_900_000 }),
    )
    .await;
}

async fn out_id(w: &W, from: NaiveDate) -> String {
    let id: Uuid = sqlx::query_scalar(
        "select id from payroll_accruals where kind = 'monthly_salary' and period_from = $1 and reversal_of is null",
    )
    .bind(from)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    id.to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn payout_without_shift_goes_from_safe(pool: PgPool) {
    let w = world(pool, false).await;
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/accruals",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.master, "kind": "bonus",
                "amount_tyiyn": 300_000, "comment": "премия" }),
    )
    .await;
    let till = w.account("register").await;
    let safe = w.account("safe").await;
    let line = json!([{ "purpose": "bonus", "amount_tyiyn": 100_000 }]);
    // Касса смены без смены — 409 с подсказкой; из сейфа — проходит.
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "account_id": till, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{out}");
    assert!(msg(&out).contains("выберите сейф или счёт"), "{out}");
    let rem = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/remains?employee_id={}", w.master),
    )
    .await;
    assert_eq!(rem["shift_open"], false, "{rem}");
    assert_eq!(rem["requires_shift"], false, "{rem}");
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "account_id": safe, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Администратор без смены не платит: только из кассы смены.
    let svc = json!([{ "purpose": "general", "amount_tyiyn": 1_000 }]);
    let (s, out) = w
        .pay(&w.admin, json!({ "employee_id": w.master, "lines": svc }))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{out}");
    assert!(msg(&out).contains("Откройте смену"), "{out}");
    // «Выплаты только при открытой смене» — и сейф закрыт.
    w.settings(json!({ "payout_requires_shift": true })).await;
    let (s, out) = w
        .pay(
            &w.owner,
            json!({ "employee_id": w.master, "account_id": safe, "lines": line }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{out}");
    assert!(msg(&out).contains("только при открытой смене"), "{out}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn salary_due_event_once_on_pay_day(pool: PgPool) {
    let w = world(pool, false).await;
    // Ближайший будущий день расчёта: завтра, а если завтра 29–31 — 1-е следующего месяца.
    let mut day = w.today.succ_opt().unwrap();
    if day.day() > 28 {
        day = first_of(day).checked_add_months(Months::new(1)).unwrap();
    }
    ok(
        &w.app,
        &w.owner,
        "PUT",
        &format!("/api/v1/employees/{}/salary-day", w.cashier),
        json!({ "salary_day": day.day() }),
    )
    .await;
    let mut conn = w.pool.acquire().await.unwrap();
    let sent = avtodom_server::api::payroll::salary_due_on(&mut conn, day)
        .await
        .unwrap();
    assert_eq!(sent, 1);
    let again = avtodom_server::api::payroll::salary_due_on(&mut conn, day)
        .await
        .unwrap();
    assert_eq!(again, 0, "второй раз за тот же период не приходит");
    let screen = get(&w.app, &w.owner, "/api/v1/notifications").await;
    let item = screen["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["action"] == "payroll.salary_due")
        .unwrap_or_else(|| panic!("{screen}"))
        .clone();
    assert_eq!(
        item["title"], "Сегодня расчёт оклада: Кассир Айбек",
        "{item}"
    );
    assert!(
        item["details"]
            .as_str()
            .unwrap()
            .contains("к выплате 30 000"),
        "{item}"
    );
    // Выключено в настройках «Зарплата» — новому работнику с тем же днём не пишется.
    let second = ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/employees",
        json!({ "full_name": "Кассир Бакыт", "is_cashier": true }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &w.app,
        &w.owner,
        "PUT",
        &format!("/api/v1/employees/{second}/salary-day"),
        json!({ "salary_day": day.day() }),
    )
    .await;
    w.settings(json!({ "salary_due_notify": false })).await;
    let sent = avtodom_server::api::payroll::salary_due_on(&mut conn, day)
        .await
        .unwrap();
    assert_eq!(sent, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn month_marks_salary_by_employee_period(pool: PgPool) {
    // «За месяц»: оклад «начислен» — за период работника, который заканчивается в этом месяце.
    let w = world(pool, false).await;
    let cur = first_of(w.today);
    ok(
        &w.app,
        &w.owner,
        "PUT",
        &format!("/api/v1/employees/{}/salary-day", w.cashier),
        json!({ "salary_day": 20 }),
    )
    .await;
    let prev20 = cur
        .checked_sub_months(Months::new(1))
        .unwrap()
        .with_day(20)
        .unwrap();
    ok(
        &w.app,
        &w.owner,
        "POST",
        "/api/v1/payroll/salary",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": cur }),
    )
    .await;
    let month = get(
        &w.app,
        &w.owner,
        &format!("/api/v1/payroll/month?month={cur}"),
    )
    .await;
    let row = month["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == w.cashier.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["salary_done"], true, "{row}");
    assert_eq!(row["salary_day"], 20, "{row}");
    assert_eq!(row["salary_from"], prev20.to_string(), "{row}");
    assert_eq!(
        row["salary_to"],
        cur.with_day(19).unwrap().to_string(),
        "{row}"
    );
}
