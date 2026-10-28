//! Старый долг из тетради (SPEC-38, ADR-133): запись долга с датой долга — срок и просрочка от
//! неё, акт сверки, права, повтор, сторно и «Исправить», отчёт смены, бот клиента. Склад,
//! деньги, выручка, прибыль и процент кассира не меняются.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

/// Дата долга из примера заказчика: фирма со сроком 14 дней — последний день оплаты 28.03.
const MAR15: &str = "2026-03-15";

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

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

fn message(v: &Value) -> String {
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

struct World {
    app: Router,
    pool: PgPool,
    branch: Uuid,
    owner: String,
    admin: String,
    admin_id: Uuid,
    cashier: String,
    safe: String,
    today: NaiveDate,
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
        // Старые долги из тетради — не по умолчанию (SPEC-38 §4).
        assert!(!keys.iter().any(|k| k == "old_debts"));
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
        let today: NaiveDate =
            sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
                .fetch_one(&pool)
                .await
                .unwrap();
        World {
            app,
            pool,
            branch,
            owner,
            admin,
            admin_id,
            cashier,
            safe,
            today,
        }
    }

    /// Клиент владельцем: фирма — по названию (срок оплаты 14 дней по умолчанию), человек —
    /// «Фамилия Имя».
    async fn client(&self, kind: &str, name: &str, due_days: Option<i32>) -> String {
        let mut body = if kind == "company" {
            json!({ "kind": "company", "name": name, "inn": "01204201910123" })
        } else {
            let (last, first) = name.split_once(' ').unwrap();
            json!({ "kind": "person", "first_name": first, "last_name": last })
        };
        if let Some(d) = due_days {
            body["due_days"] = json!(d);
        }
        let p = ok(&self.app, "POST", "/api/v1/parties", &self.owner, body).await;
        p["id"].as_str().unwrap().to_string()
    }

    async fn old_debt(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        let mut body = body;
        if body.get("op_id").is_none() {
            body["op_id"] = json!(Uuid::now_v7());
        }
        call(
            &self.app,
            "POST",
            "/api/v1/debts/old",
            Some(cookie),
            Some(body),
        )
        .await
    }

    /// Старый долг владельцем: ответ сервера.
    async fn owner_old_debt(&self, party: &str, amount: i64, date: &str, comment: &str) -> Value {
        let (s, v) = self
            .old_debt(
                &self.owner,
                json!({ "party_id": party, "amount_tyiyn": amount, "date": date, "comment": comment }),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }

    async fn balance(&self, party: &str) -> i64 {
        sqlx::query_scalar("select balance_tyiyn from parties where id = $1::uuid")
            .bind(party)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// Строка экрана «Долги» клиента.
    async fn debt_row(&self, party: &str) -> Value {
        let list = get(&self.app, "/api/v1/debts/list", &self.owner).await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(party))
            .cloned()
            .unwrap_or_else(|| panic!("нет строки клиента в «Долгах»: {list}"))
    }

    /// Владелец выдаёт администратору права: по умолчанию и `extra`.
    async fn grant(&self, extra: &[&str], defaults: bool) {
        let mut list: Vec<String> = if defaults {
            avtodom_server::auth::default_permissions()
                .into_iter()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        list.extend(extra.iter().map(|s| (*s).to_string()));
        ok(
            &self.app,
            "PUT",
            &format!("/api/v1/users/{}/permissions", self.admin_id),
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "permissions": list }),
        )
        .await;
    }

    /// Всё, что меняет продажа или деньги: движения, остатки счетов, начисления, прибыль.
    async fn untouched(&self) -> Vec<String> {
        let mut out = Vec::new();
        for sql in [
            "select count(*)::bigint from cash_movements",
            "select count(*)::bigint from stock_movements",
            "select count(*)::bigint from sales",
            "select count(*)::bigint from payroll_accruals",
            "select count(*)::bigint from payroll_pending",
            "select coalesce(sum(balance_tyiyn), 0)::bigint from cash_accounts",
        ] {
            let v: i64 = sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap();
            out.push(format!("{sql} = {v}"));
        }
        for (from, to) in [
            (MAR15.to_string(), MAR15.to_string()),
            (self.today.to_string(), self.today.to_string()),
        ] {
            let p = get(
                &self.app,
                &format!("/api/v1/reports/profit?from={from}&to={to}"),
                &self.owner,
            )
            .await;
            out.push(format!("{from}: {}", p["totals"]));
        }
        out
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_debt_counts_from_its_date_and_moves_no_money(pool: PgPool) {
    let w = World::new(pool).await;
    let firm = w.client("company", "ОсОО Тетрадь", None).await;
    let person = w.client("person", "Иванов Пётр", None).await;
    let before = w.untouched().await;

    // Фирма со сроком 14 дней и долгом от 15.03.2026: последний день оплаты 28.03, просрочка с 29.03.
    let out = w.owner_old_debt(&firm, 500_000, MAR15, "стр. 12").await;
    let due = day("2026-03-28");
    let overdue = (w.today - due).num_days();
    assert_eq!(out["balance_tyiyn"], json!(500_000));
    assert_eq!(out["due_date"], json!("2026-03-28"));
    assert_eq!(out["overdue_days"], json!(overdue));
    let (kind, doc, date, comment, branch): (String, String, NaiveDate, String, Uuid) =
        sqlx::query_as(
            "select kind, doc_type, business_date, comment, branch_id from party_ledger where id = $1::uuid",
        )
        .bind(out["ledger_id"].as_str().unwrap())
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(
        (kind.as_str(), doc.as_str(), date, comment.as_str(), branch),
        (
            "debt",
            "old_debt",
            day(MAR15),
            "Старый долг (из тетради): стр. 12",
            w.branch
        )
    );
    // Не продажа: склад, касса, выручка, прибыль и 2 % кассира — без изменений.
    assert_eq!(w.untouched().await, before);

    let row = w.debt_row(&firm).await;
    assert_eq!(row["balance_tyiyn"], json!(500_000));
    assert_eq!(row["overdue_tyiyn"], json!(500_000));
    assert_eq!(row["overdue_days"], json!(overdue));
    assert_eq!(row["due_date"], json!("2026-03-28"));

    // У физлица без срока оплаты просрочки нет.
    let out = w.owner_old_debt(&person, 300_000, MAR15, "").await;
    assert_eq!(out["due_date"], Value::Null);
    let row = w.debt_row(&person).await;
    assert_eq!(
        (
            &row["overdue_tyiyn"],
            &row["overdue_days"],
            &row["due_date"]
        ),
        (&json!(0), &json!(0), &Value::Null)
    );

    // Карточка: «Старый долг (из тетради)» с датой долга, текстом и кто занёс; владельцу — сторно.
    let card = get(&w.app, &format!("/api/v1/parties/{firm}/card"), &w.owner).await;
    let item = card["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["kind"] == json!("debt"))
        .unwrap()
        .clone();
    assert_eq!(item["title"], json!("Старый долг (из тетради)"));
    assert_eq!(item["debt_date"], json!(MAR15));
    assert_eq!(item["user_name"], json!("Владелец"));
    assert_eq!(item["comment"], json!("стр. 12"));
    assert_eq!(item["reversible"], json!(true));
    assert_eq!(card["debt_taken_tyiyn"], json!(500_000));

    // Окно приёма оплаты: долг просрочен, подпись — из записи.
    let open = get(
        &w.app,
        &format!("/api/v1/parties/{firm}/open-debts"),
        &w.owner,
    )
    .await;
    let items = open["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["overdue"], json!(true));
    assert_eq!(items[0]["business_date"], json!(MAR15));
    assert!(
        items[0]["comment"]
            .as_str()
            .unwrap()
            .starts_with("Старый долг (из тетради)")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn new_client_and_old_debt_are_one_transaction(pool: PgPool) {
    let w = World::new(pool).await;
    let parties = |name: &'static str| {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from parties where name = $1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // Фирма без ИНН: ПИН/ИНН не обязателен, срок оплаты 14 дней — как у новой фирмы.
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "new_party": { "kind": "company", "name": "ОсОО Новая", "phone": "0555 11 22 33" },
                    "amount_tyiyn": 200_000, "date": MAR15, "comment": "масло" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["party_id"].as_str().unwrap();
    let p = get(&w.app, &format!("/api/v1/parties/{id}"), &w.owner).await;
    assert_eq!(
        (&p["kind"], &p["due_days"], &p["inn"], &p["balance_tyiyn"]),
        (&json!("company"), &json!(14), &json!(""), &json!(200_000))
    );
    assert_eq!(v["due_date"], json!("2026-03-28"));
    let created: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'party.create' and entity_id = $1::uuid",
    )
    .bind(id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(created, 1);

    // Ошибка карточки или долга — нет ни карточки, ни долга.
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "new_party": { "kind": "person", "first_name": "Пётр", "last_name": "" },
                    "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(parties("Пётр").await, 0);
    let tomorrow = (w.today + chrono::Duration::days(1)).to_string();
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "new_party": { "kind": "person", "first_name": "Пётр", "last_name": "Сидоров" },
                    "amount_tyiyn": 100_000, "date": tomorrow }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(parties("Сидоров Пётр").await, 0);
    let (s, _) = w
        .old_debt(
            &w.owner,
            json!({ "new_party": { "kind": "person", "first_name": "Пётр", "last_name": "Сидоров" },
                    "amount_tyiyn": 0, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(parties("Сидоров Пётр").await, 0);

    // Клиент — ровно один: из списка или новый; поставщику — «Правкой баланса».
    let (s, _) = w
        .old_debt(&w.owner, json!({ "amount_tyiyn": 100_000, "date": MAR15 }))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = w
        .old_debt(
            &w.owner,
            json!({ "party_id": id, "new_party": { "kind": "company", "name": "ОсОО Ещё" },
                    "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let supplier = ok(
        &w.app,
        "POST",
        "/api/v1/suppliers",
        &w.owner,
        json!({ "name": "Автоснаб" }),
    )
    .await;
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "party_id": supplier["id"], "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(message(&v).contains("Правкой баланса"), "{v}");

    // Администратор со «Старыми долгами», но без «Клиентов и машин»: нового клиента не заводит.
    w.grant(&["old_debts"], false).await;
    let (s, v) = w
        .old_debt(
            &w.admin,
            json!({ "new_party": { "kind": "person", "first_name": "Пётр", "last_name": "Сидоров" },
                    "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(message(&v), "Нет права: Клиенты и машины");
    assert_eq!(parties("Сидоров Пётр").await, 0);
    let (s, v) = w
        .old_debt(
            &w.admin,
            json!({ "party_id": id, "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.balance(id).await, 300_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn dates_repeats_and_op_id(pool: PgPool) {
    let w = World::new(pool).await;
    let p = w.client("person", "Асанов Бакыт", None).await;

    // Дата — не позже сегодня и не раньше чем 10 лет назад.
    let tomorrow = w.today + chrono::Duration::days(1);
    let ten_years = w
        .today
        .checked_sub_months(chrono::Months::new(120))
        .unwrap();
    for (date, status) in [
        (tomorrow, StatusCode::UNPROCESSABLE_ENTITY),
        (
            ten_years - chrono::Duration::days(1),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (ten_years, StatusCode::OK),
        (w.today, StatusCode::OK),
    ] {
        let (s, v) = w
            .old_debt(
                &w.owner,
                json!({ "party_id": p, "amount_tyiyn": 1_000, "date": date }),
            )
            .await;
        assert_eq!(s, status, "{date}: {v}");
    }
    assert_eq!(w.balance(&p).await, 2_000);

    // Тот же клиент, сумма и дата — 409 «уже занесён … Занести ещё раз?», с подтверждением —
    // вторая запись; повтор `op_id` — та же запись.
    let first = w.owner_old_debt(&p, 100_000, MAR15, "").await;
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "party_id": p, "amount_tyiyn": 100_000, "date": MAR15 }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let today_short = w.today.format("%d.%m").to_string();
    assert_eq!(
        message(&v),
        format!("Такой долг уже занесён {today_short} (Владелец). Занести ещё раз?")
    );
    assert_eq!(w.balance(&p).await, 102_000);
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "party_id": p, "amount_tyiyn": 100_000, "date": MAR15,
                       "confirm_duplicate": true });
    let (s, second) = w.old_debt(&w.owner, body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{second}");
    assert_ne!(second["ledger_id"], first["ledger_id"]);
    let (s, again) = w.old_debt(&w.owner, body).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again, second);
    assert_eq!(w.balance(&p).await, 202_000);
    // Другая сумма или дата — не повтор.
    w.owner_old_debt(&p, 100_000, "2026-03-16", "").await;

    // Сторнированный долг повтором не считается.
    for l in [&first, &second] {
        ok(
            &w.app,
            "POST",
            &format!(
                "/api/v1/debts/ledger/{}/reverse",
                l["ledger_id"].as_str().unwrap()
            ),
            &w.owner,
            json!({ "op_id": Uuid::now_v7(), "comment": "задвоили" }),
        )
        .await;
    }
    w.owner_old_debt(&p, 100_000, MAR15, "").await;
    assert_eq!(w.balance(&p).await, 202_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_needs_the_right_and_owner_hears_about_it(pool: PgPool) {
    let w = World::new(pool).await;
    let p = w.client("person", "Иванов Пётр", None).await;
    w.owner_old_debt(&p, 200_000, "2026-02-01", "").await;

    let body =
        json!({ "party_id": p, "amount_tyiyn": 500_000, "date": MAR15, "comment": "стр. 12" });
    let (s, v) = w.old_debt(&w.admin, body.clone()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(message(&v), "Нет права: Старые долги из тетради");
    assert_eq!(w.balance(&p).await, 200_000);

    // Владелец включает право на экране «Что можно» — администратор заносит.
    w.grant(&["old_debts"], true).await;
    let (s, v) = w.old_debt(&w.admin, body).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let ledger = v["ledger_id"].as_str().unwrap().to_string();
    assert_eq!(w.balance(&p).await, 700_000);

    // Владельцу — уведомление `debt.old`: кто, кому, сколько, от какой даты и долг клиента.
    let list = get(&w.app, "/api/v1/notifications", &w.owner).await;
    let n = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["action"] == json!("debt.old") && n["user_name"] == json!("Эрбол"))
        .unwrap_or_else(|| panic!("нет уведомления: {list}"))
        .clone();
    assert_eq!(
        n["title"],
        json!("Старый долг: Иванов Пётр — 5 000 с от 15.03.2026 (из тетради)")
    );
    assert_eq!(n["details"], json!("Долг клиента — 7 000 с — стр. 12"));

    // Сторно и «Исправить» — только владелец.
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/debts/ledger/{ledger}/reverse"),
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/corrections/draft/ledger/{ledger}"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/corrections",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "ledger", "original_id": ledger, "comment": "ошибка",
                     "doc": { "type": "old_debt", "party_id": p, "amount_tyiyn": 1_000, "date": MAR15, "comment": "" } })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(w.balance(&p).await, 700_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn reconciliation_shows_old_debt_on_its_date(pool: PgPool) {
    let w = World::new(pool).await;
    let p = w.client("company", "ОсОО Сверка", None).await;
    let out = w.owner_old_debt(&p, 500_000, MAR15, "стр. 12").await;

    let act = get(
        &w.app,
        &format!("/api/v1/parties/{p}/reconciliation"),
        &w.owner,
    )
    .await;
    assert_eq!(act["from"], json!(MAR15));
    assert_eq!(act["opening_tyiyn"], json!(0));
    let rows = act["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["date"], json!(MAR15));
    assert_eq!(rows[0]["document"], json!("Старый долг (из тетради)"));
    assert_eq!(rows[0]["comment"], json!("стр. 12"));
    assert_eq!(rows[0]["debit_tyiyn"], json!(500_000));
    assert_eq!(act["closing_tyiyn"], json!(500_000));
    // Сальдо на начало периода после даты долга его включает.
    let later = get(
        &w.app,
        &format!("/api/v1/parties/{p}/reconciliation?from=2026-04-01"),
        &w.owner,
    )
    .await;
    assert_eq!(later["opening_tyiyn"], json!(500_000));
    assert_eq!(later["rows"], json!([]));

    // Сторно — тем же днём долга: прошлые периоды не искажаются.
    ok(
        &w.app,
        "POST",
        &format!(
            "/api/v1/debts/ledger/{}/reverse",
            out["ledger_id"].as_str().unwrap()
        ),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "не тот клиент" }),
    )
    .await;
    let act = get(
        &w.app,
        &format!("/api/v1/parties/{p}/reconciliation"),
        &w.owner,
    )
    .await;
    let docs: Vec<(&str, &str)> = act["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["date"].as_str().unwrap(), r["document"].as_str().unwrap()))
        .collect();
    assert_eq!(
        docs,
        [
            (MAR15, "Старый долг (из тетради)"),
            (MAR15, "Сторно старого долга (из тетради)")
        ]
    );
    assert_eq!(act["closing_tyiyn"], json!(0));
    let later = get(
        &w.app,
        &format!("/api/v1/parties/{p}/reconciliation?from=2026-04-01"),
        &w.owner,
    )
    .await;
    assert_eq!(later["opening_tyiyn"], json!(0));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn repayment_pays_old_debt_first_and_cashier_percent_as_before(pool: PgPool) {
    let w = World::new(pool).await;
    let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base, rate_bp, user_id)
         values ($1, $2, $3::uuid, 'revenue_percent', 'cashier', 'gross', 200, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(w.branch)
    .bind(&w.cashier)
    .bind(owner_id)
    .execute(&w.pool)
    .await
    .unwrap();
    let firm = w.client("company", "ОсОО Очередь", None).await;
    let cat = ok(
        &w.app,
        "POST",
        "/api/v1/categories",
        &w.owner,
        json!({ "name": "Фильтры", "kind": "filter" }),
    )
    .await;
    let filter = ok(
        &w.app,
        "POST",
        "/api/v1/products",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 30_000 }),
    )
    .await["id"]
        .clone();
    ok(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "paid_from": w.safe,
                "lines": [{ "product_id": filter, "qty": 5, "cost_tyiyn": 50_000 }] }),
    )
    .await;
    // Чек в долг сегодня: 300 с, валовая 200 с, 2 % ждут оплаты.
    ok(
        &w.app,
        "POST",
        "/api/v1/sales",
        &w.admin,
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier, "party_id": firm,
            "lines": [{ "kind": "piece", "product_id": filter, "qty": 1, "unit_price_tyiyn": 30_000 }],
            "payments": [{ "method": "debt", "amount_tyiyn": 30_000 }]
        }),
    )
    .await;
    let percent = || {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'revenue_percent'",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(percent().await, 0);

    // Старый долг 500 с от 15.03: он самый старый — срок и просрочка от него.
    w.owner_old_debt(&firm, 50_000, MAR15, "").await;
    let row = w.debt_row(&firm).await;
    assert_eq!(row["balance_tyiyn"], json!(80_000));
    assert_eq!(row["due_date"], json!("2026-03-28"));
    assert_eq!(row["overdue_tyiyn"], json!(50_000));

    // Погашение 500 с гасит старый долг первым: срок — от сегодняшнего чека, просрочки нет.
    ok(
        &w.app,
        "POST",
        "/api/v1/debts/repayments",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "party_id": firm, "amount_tyiyn": 50_000, "method": "cash" }),
    )
    .await;
    let row = w.debt_row(&firm).await;
    assert_eq!(row["balance_tyiyn"], json!(30_000));
    assert_eq!(
        row["due_date"],
        json!((w.today + chrono::Duration::days(13)).to_string())
    );
    assert_eq!(
        (&row["overdue_days"], &row["overdue_tyiyn"]),
        (&json!(0), &json!(0))
    );
    let open = get(
        &w.app,
        &format!("/api/v1/parties/{firm}/open-debts"),
        &w.owner,
    )
    .await;
    let items = open["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0]["sale_number"].is_number(), "{open}");
    // 2 % по долговому чеку — при погашении, по очереди чеков, как раньше: старый долг процент
    // не задерживает. Валовая 200 с × 2 % = 4 с.
    assert_eq!(percent().await, 400);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_report_leaves_old_debt_out(pool: PgPool) {
    let w = World::new(pool).await;
    let p = w.client("person", "Иванов Пётр", None).await;
    let out = w.owner_old_debt(&p, 500_000, MAR15, "").await;
    ok(
        &w.app,
        "POST",
        &format!(
            "/api/v1/debts/ledger/{}/reverse",
            out["ledger_id"].as_str().unwrap()
        ),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    w.owner_old_debt(&p, 400_000, MAR15, "").await;
    // Оплата долга в смену — в отчёте есть, старый долг и его сторно — нет.
    ok(
        &w.app,
        "POST",
        "/api/v1/debts/repayments",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "party_id": p, "amount_tyiyn": 10_000, "method": "cash" }),
    )
    .await;
    let shift = get(&w.app, "/api/v1/shifts/current", &w.owner).await;
    let report = get(
        &w.app,
        &format!("/api/v1/shifts/{}/report", shift["id"].as_str().unwrap()),
        &w.owner,
    )
    .await;
    let debts = report["debts"].as_array().unwrap();
    let kinds: Vec<(&str, i64)> = debts
        .iter()
        .map(|d| {
            (
                d["kind"].as_str().unwrap(),
                d["amount_tyiyn"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(kinds, [("repayment", -10_000)], "{report}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_reverses_and_corrects_old_debt(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.client("person", "Иванов Пётр", None).await;
    let b = w.client("company", "ОсОО Верная", None).await;
    let out = w.owner_old_debt(&a, 500_000, MAR15, "стр. 12").await;
    let id = out["ledger_id"].as_str().unwrap().to_string();

    // «Исправить»: черновик — клиент, сумма, дата и текст; верный — другому клиенту, другой
    // суммой и датой.
    let draft = get(
        &w.app,
        &format!("/api/v1/corrections/draft/ledger/{id}"),
        &w.owner,
    )
    .await;
    assert_eq!(
        draft["doc"],
        json!({ "type": "old_debt", "party_id": a, "amount_tyiyn": 500_000, "date": MAR15, "comment": "стр. 12" })
    );
    let fixed = ok(
        &w.app,
        "POST",
        "/api/v1/corrections",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "kind": "ledger", "original_id": id, "comment": "не тот клиент",
                "doc": { "type": "old_debt", "party_id": b, "amount_tyiyn": 400_000, "date": "2026-04-10", "comment": "стр. 13" } }),
    )
    .await;
    let what: Vec<&str> = fixed["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["what"].as_str().unwrap())
        .collect();
    assert_eq!(what, ["Клиент", "Сумма", "Дата долга", "Комментарий"]);
    assert_eq!(w.balance(&a).await, 0);
    assert_eq!(w.balance(&b).await, 400_000);
    // Сторно исходного — в день его долга, верный — в свой день.
    let mut dates: Vec<(String, String, NaiveDate)> = sqlx::query_as(
        "select p.name, l.doc_type, l.business_date from party_ledger l join parties p on p.id = l.party_id",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    dates.sort();
    assert_eq!(
        dates,
        [
            (
                "Иванов Пётр".to_string(),
                "old_debt".to_string(),
                day(MAR15)
            ),
            (
                "Иванов Пётр".to_string(),
                "reversal".to_string(),
                day(MAR15)
            ),
            (
                "ОсОО Верная".to_string(),
                "old_debt".to_string(),
                day("2026-04-10")
            ),
        ]
    );
    // Исправленный долг — снова «Исправить» и «Сторно»; исходный — нет.
    let new_id = fixed["new_id"].as_str().unwrap().to_string();
    let card = get(&w.app, &format!("/api/v1/parties/{a}/card"), &w.owner).await;
    let first = card["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["ledger_id"] == json!(id))
        .unwrap()
        .clone();
    assert_eq!(first["reversible"], json!(false));

    // Сторно владельцем: долг снят, повторное сторно — 409.
    let rev = format!("/api/v1/debts/ledger/{new_id}/reverse");
    ok(
        &w.app,
        "POST",
        &rev,
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" }),
    )
    .await;
    assert_eq!(w.balance(&b).await, 0);
    let (s, _) = call(
        &w.app,
        "POST",
        &rev,
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let notes: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'debt.reverse'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(notes, 2);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn customer_bot_shows_old_debt_by_its_date_and_reminds_only_ahead(pool: PgPool) {
    use avtodom_server::api::bot_debt::{customer_debt, due_debt_reminders};
    let w = World::new(pool).await;
    // Срок оплаты 10 дней: долг 8 дней назад — срок завтра, напомнит «за 3 дня».
    let soon = w.client("person", "Скорый Срок", Some(10)).await;
    let past = w.client("person", "Прошлый Срок", Some(10)).await;
    let soon_day = w.today - chrono::Duration::days(8);
    w.owner_old_debt(&soon, 100_000, &soon_day.to_string(), "")
        .await;
    w.owner_old_debt(&past, 100_000, MAR15, "").await;
    for (chat, party) in [(91_001i64, &soon), (91_002i64, &past)] {
        sqlx::query(
            "insert into telegram_customers (chat_id, party_id, phone) values ($1, $2::uuid, '')",
        )
        .bind(chat)
        .bind(party)
        .execute(&w.pool)
        .await
        .unwrap();
    }
    let mut conn = w.pool.acquire().await.unwrap();
    let text = customer_debt(&mut conn, soon.parse().unwrap())
        .await
        .unwrap();
    assert!(
        text.contains(&format!(
            "{} — старый долг (из тетради) 1 000 с",
            soon_day.format("%d.%m.%Y")
        )),
        "{text}"
    );
    assert!(text.contains("Оплатить до"), "{text}");
    let text = customer_debt(&mut conn, past.parse().unwrap())
        .await
        .unwrap();
    assert!(
        text.contains("15.03.2026 — старый долг (из тетради) 1 000 с"),
        "{text}"
    );
    assert!(text.contains("Срок оплаты прошёл 24.03.2026"), "{text}");
    // Прошедший срок не напоминается; срок через 1–3 дня — напомнит.
    let reminders = due_debt_reminders(&mut conn).await.unwrap();
    let chats: Vec<(i64, i32)> = reminders
        .iter()
        .map(|r| (r.chat_id, r.days_before))
        .collect();
    assert_eq!(chats, [(91_001, 3)]);
    assert!(
        reminders[0].text.contains("завтра"),
        "{}",
        reminders[0].text
    );
}
