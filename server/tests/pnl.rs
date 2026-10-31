//! «Прибыль и убытки» (задача 003 Н12, SPEC-08): раскрытие до документов сходится с итогами,
//! «Кому» у расхода, настройки отчёта — только владелец.
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
    let keys: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
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

/// Запрос, который должен пройти; ответ — тело.
async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
    let (s, _, v) = call(app, method, uri, Some(cookie), body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
    v
}

fn sum(rows: &Value, key: &str) -> i64 {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| r[key].as_i64().unwrap())
        .sum()
}

async fn account(app: &Router, owner: &str, pred: impl Fn(&Value) -> bool) -> String {
    let list = ok(app, "GET", "/api/v1/cash/accounts", owner, None).await;
    list.as_array().unwrap().iter().find(|a| pred(a)).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn product(app: &Router, owner: &str, name: &str, price: i64) -> (String, String) {
    let cat = ok(
        app,
        "POST",
        "/api/v1/categories",
        owner,
        Some(json!({ "name": format!("Кат {name}"), "kind": "filter" })),
    )
    .await;
    let p = ok(
        app,
        "POST",
        "/api/v1/products",
        owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name,
                     "sale_price_tyiyn": price }),
        ),
    )
    .await;
    (
        p["id"].as_str().unwrap().to_string(),
        cat["id"].as_str().unwrap().to_string(),
    )
}

async fn article(app: &Router, who: &str, name: &str) -> String {
    let list = ok(app, "GET", "/api/v1/expense-articles", who, None).await;
    list.as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn profit_detail_ties_out_to_totals(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let safe = account(&app, &owner, |a| a["name"] == "Сейф владельца").await;
    let till = account(&app, &owner, |a| a["kind"] == "register").await;

    // Без продаж — пустое раскрытие, без `detail` — прежний ответ.
    let plain = ok(&app, "GET", "/api/v1/reports/profit", &owner, None).await;
    assert!(plain.get("detail").is_none());

    // Товары двух категорий, работа, кассир и мастер.
    let (filter, filter_cat) = product(&app, &owner, "Фильтр", 50_000).await;
    let (oil, _) = product(&app, &owner, "Масло", 100_000).await;
    ok(
        &app,
        "POST",
        "/api/v1/receipts",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                     "lines": [{ "product_id": filter, "qty": 10, "cost_tyiyn": 30_000 },
                               { "product_id": oil, "qty": 10, "cost_tyiyn": 60_000 }] })),
    )
    .await;
    let cashier = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        Some(json!({ "full_name": "Рома", "is_cashier": true, "pin": "1234" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let master = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        Some(json!({ "full_name": "Иман", "is_master": true })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let service = ok(
        &app,
        "POST",
        "/api/v1/services",
        &owner,
        Some(json!({ "name": "Замена масла", "price_tyiyn": 30_000, "master_fee_tyiyn": 10_000 })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let shift = ok(
        &app,
        "POST",
        "/api/v1/shifts/open",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "POST",
        "/api/v1/cash/movements",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" })),
    )
    .await;

    // Чек фильтров (2 шт.) и чек масла с работой; один фильтр вернули.
    let sale = ok(
        &app,
        "POST",
        "/api/v1/sales",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
                     "lines": [{ "kind": "piece", "product_id": filter, "qty": 2, "unit_price_tyiyn": 50_000 }],
                     "payments": [{ "method": "cash", "amount_tyiyn": 100_000 }] })),
    )
    .await;
    let party = ok(
        &app,
        "POST",
        "/api/v1/parties",
        &admin,
        Some(json!({ "kind": "person", "first_name": "Бакыт", "last_name": "Асанов", "phone": "0700 11-22-33" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let car = ok(
        &app,
        "POST",
        &format!("/api/v1/parties/{party}/vehicles"),
        &admin,
        Some(json!({ "plate": "01KG777AAA" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "POST",
        "/api/v1/sales",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "sale_type": "service", "cashier_id": cashier,
                     "master_id": master, "party_id": party, "vehicle_id": car, "mileage": 100_000,
                     "lines": [{ "kind": "piece", "product_id": oil, "qty": 1, "unit_price_tyiyn": 100_000 },
                               { "kind": "service", "service_id": service, "qty": 1, "unit_price_tyiyn": 30_000 }],
                     "payments": [{ "method": "cash", "amount_tyiyn": 130_000 }] })),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/sales/{}/return", sale["id"].as_str().unwrap()),
        &admin,
        Some(
            json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                     "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }] }),
        ),
    )
    .await;

    // Два расхода в разных статьях с «Кому», один сторнирован.
    let household = article(&app, &owner, "Хозтовары").await;
    let taxes = article(&app, &owner, "Налоги").await;
    let e1 = ok(
        &app,
        "POST",
        "/api/v1/expenses",
        &admin,
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 12_000,
                     "payee": "Хозмаг", "comment": "ведро" }),
        ),
    )
    .await;
    let e2 = ok(
        &app,
        "POST",
        "/api/v1/expenses",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": taxes, "amount_tyiyn": 40_000,
                     "account_id": safe, "payee": "Налоговая" }),
        ),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/expenses/{}/reverse", e2["id"].as_str().unwrap()),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;

    // Бонус («доп. доход») и удержание кассиру, выплата из сейфа.
    for (kind, amount) in [("bonus", 21_650), ("penalty", 5_000)] {
        ok(
            &app,
            "POST",
            "/api/v1/payroll/accruals",
            &owner,
            Some(
                json!({ "op_id": Uuid::now_v7(), "employee_id": cashier, "kind": kind,
                         "amount_tyiyn": amount, "comment": "за месяц" }),
            ),
        )
        .await;
    }
    ok(
        &app,
        "POST",
        "/api/v1/payouts",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": cashier, "amount_tyiyn": 15_000,
                     "source": "account", "account_id": safe }),
        ),
    )
    .await;

    // Ревизия фильтров: одного не хватает.
    let rev = ok(
        &app,
        "POST",
        "/api/v1/revisions",
        &owner,
        Some(json!({ "category_id": filter_cat })),
    )
    .await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    ok(
        &app,
        "PUT",
        &format!("/api/v1/revisions/{rid}/lines"),
        &owner,
        Some(json!({ "product_id": filter, "counted_qty": 8 })),
    )
    .await;
    ok(
        &app,
        "POST",
        &format!("/api/v1/revisions/{rid}/post"),
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт" })),
    )
    .await;

    // Деньги, которые прибыль не меняют: изъятие и перевод в сейф.
    ok(
        &app,
        "POST",
        "/api/v1/cash/movements",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_out", "amount_tyiyn": 10_000, "comment": "инкассация" })),
    )
    .await;
    ok(
        &app,
        "POST",
        "/api/v1/cash/transfers",
        &owner,
        Some(json!({ "op_id": Uuid::now_v7(), "from_account_id": till, "to_account_id": safe, "amount_tyiyn": 20_000 })),
    )
    .await;
    // Недостача 1 000 с при закрытии смены.
    let balance: i64 = sqlx::query_scalar("select balance_tyiyn from cash_accounts where id = $1")
        .bind(Uuid::parse_str(&till).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    ok(
        &app,
        "POST",
        &format!("/api/v1/shifts/{shift}/close"),
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": balance - 100_000, "comment": "не хватает" })),
    )
    .await;

    let plain = ok(&app, "GET", "/api/v1/reports/profit", &owner, None).await;
    let full = ok(&app, "GET", "/api/v1/reports/profit?detail=1", &owner, None).await;
    assert!(plain.get("detail").is_none());
    // Итоги не меняются: раскрытие только раскладывает те же числа.
    assert_eq!(plain["totals"], full["totals"]);
    let t = &full["totals"];
    let d = &full["detail"];
    let num = |v: &Value| v.as_i64().unwrap();

    // Выручка: товары по товарам (с категорией), работы по услугам.
    assert_eq!(sum(&d["products"], "revenue_tyiyn"), num(&t["goods_tyiyn"]));
    assert_eq!(
        sum(&d["services"], "revenue_tyiyn"),
        num(&t["services_tyiyn"])
    );
    assert_eq!(num(&t["services_tyiyn"]), 30_000);
    let filter_row = d["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Фильтр")
        .unwrap();
    assert_eq!(filter_row["units"], 1, "{filter_row}");
    assert_eq!(filter_row["category"], "Кат Фильтр");
    assert_eq!(d["services"][0]["master_fee_tyiyn"], 10_000);
    assert_eq!(d["returns_tyiyn"], 50_000);
    // Себестоимость: проданное + ревизия + переоценка.
    assert_eq!(
        num(&d["revision_tyiyn"]) + num(&d["revaluation_only_tyiyn"]),
        num(&t["revaluation_tyiyn"])
    );
    assert_eq!(num(&d["revision_tyiyn"]), 3_000, "{d}");
    assert_eq!(sum(&d["revisions"], "amount_tyiyn"), 3_000);
    assert_eq!(d["revisions"][0]["user_name"], "Владелец");
    assert_eq!(
        sum(&d["products"], "cost_tyiyn") + num(&t["revaluation_tyiyn"]),
        num(&t["cost_tyiyn"])
    );
    // Расходы: все строки, сторно — парой в той же статье, «Кому» сохраняется в сторно.
    let expenses = d["expenses"].as_array().unwrap();
    assert_eq!(expenses.len(), 3);
    assert_eq!(
        sum(&d["expenses"], "amount_tyiyn"),
        num(&t["expenses_tyiyn"])
    );
    assert_eq!(num(&t["expenses_tyiyn"]), 12_000);
    let reversal = expenses
        .iter()
        .find(|e| !e["reversal_of"].is_null())
        .unwrap();
    assert_eq!(reversal["payee"], "Налоговая");
    assert_eq!(reversal["article"], "Налоги");
    assert_eq!(reversal["amount_tyiyn"], -40_000);
    let first = expenses.iter().find(|e| e["id"] == e1["id"]).unwrap();
    assert_eq!(first["payee"], "Хозмаг");
    assert_eq!(first["user_name"], "Админ");
    assert_eq!(first["account_name"], "Касса");
    // Зарплата работникам: начислено по видам = оплата труда, выдано — списком.
    assert_eq!(sum(&full["staff"], "total_tyiyn"), num(&t["payroll_tyiyn"]));
    let roma = full["staff"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["employee_id"] == cashier.as_str())
        .unwrap();
    assert!(
        roma["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["kind"] == "bonus" && i["amount_tyiyn"] == 21_650)
    );
    let payouts = d["payouts"].as_array().unwrap();
    assert_eq!(payouts.len(), 1);
    assert_eq!(payouts[0]["employee_name"], "Рома");
    assert_eq!(payouts[0]["amount_tyiyn"], 15_000);
    assert_eq!(payouts[0]["account_name"], "Сейф владельца");
    // Комиссия, касса, брак, возвраты поставщику — сходятся со своими строками.
    assert_eq!(sum(&d["fees"], "amount_tyiyn"), num(&t["bank_fee_tyiyn"]));
    assert_eq!(
        sum(&d["cash_diffs"], "amount_tyiyn"),
        num(&t["cash_diff_tyiyn"])
    );
    assert_eq!(num(&t["cash_diff_tyiyn"]), -100_000);
    assert_eq!(d["cash_diffs"][0]["cashier"], "Рома");
    assert_eq!(
        sum(&d["defects"], "writeoff_tyiyn"),
        num(&t["defect_writeoff_tyiyn"])
    );
    assert_eq!(
        sum(&d["defects"], "diff_tyiyn"),
        num(&t["defect_diff_tyiyn"])
    );
    assert_eq!(
        sum(&d["supplier_returns"], "diff_tyiyn"),
        num(&t["return_diff_tyiyn"])
    );
    // Не входит в прибыль: оплата поставщику за приход, изъятие, перевод.
    let kinds: Vec<&str> = d["not_in_profit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["kind"].as_str().unwrap())
        .collect();
    for k in ["supplier_payment", "cash_out", "transfer_out"] {
        assert!(kinds.contains(&k), "{k}: {kinds:?}");
    }

    // Выплата, изъятие и перевод прибыль не меняют — она по начислению (ADR-023).
    let net = num(&t["net_tyiyn"]);
    ok(
        &app,
        "POST",
        "/api/v1/payouts",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 5_000,
                     "source": "account", "account_id": safe }),
        ),
    )
    .await;
    let again = ok(&app, "GET", "/api/v1/reports/profit?detail=1", &owner, None).await;
    assert_eq!(num(&again["totals"]["net_tyiyn"]), net);
    assert_eq!(again["detail"]["payouts"].as_array().unwrap().len(), 2);

    // Раскрытие и настройки — только владелец (инвариант 13), и с любыми правами.
    sqlx::query(
        "alter table user_permissions drop constraint if exists user_permissions_permission_check",
    )
    .execute(&pool)
    .await
    .unwrap();
    for p in avtodom_server::auth::PERMISSIONS {
        sqlx::query(
            "insert into user_permissions (user_id, branch_id, permission)
             select id, branch_id, $1 from users where login = 'admin' on conflict do nothing",
        )
        .bind(p.key)
        .execute(&pool)
        .await
        .unwrap();
    }
    for uri in [
        "/api/v1/reports/profit?detail=1",
        "/api/v1/reports/profit",
        "/api/v1/settings/profit-report",
    ] {
        let (s, _, _) = call(&app, "GET", uri, Some(&admin), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}");
    }
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/profit-report",
        Some(&admin),
        Some(json!({ "default_period": "today", "expense_payee": "off", "show_reversals": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expense_payee_is_stored_reversed_and_set_up_by_owner(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = ok(
        &app,
        "POST",
        "/api/v1/employees",
        &owner,
        Some(json!({ "full_name": "Алексей", "is_cashier": true, "pin": "1234" })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &app,
        "POST",
        "/api/v1/shifts/open",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" })),
    )
    .await;
    ok(
        &app,
        "POST",
        "/api/v1/cash/movements",
        &admin,
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" })),
    )
    .await;
    let household = article(&app, &admin, "Хозтовары").await;

    // По умолчанию «Кому» необязательно.
    let settings = ok(&app, "GET", "/api/v1/settings/profit-report", &owner, None).await;
    assert_eq!(
        settings,
        json!({ "default_period": "month", "expense_payee": "optional", "show_reversals": false,
                "expenses_bonus_accruals": true })
    );
    let form = ok(&app, "GET", "/api/v1/expenses/payees", &admin, None).await;
    assert_eq!(form["mode"], "optional");
    assert!(
        form["names"]
            .as_array()
            .unwrap()
            .contains(&json!("Алексей"))
    );
    let (s, _, none) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{none}");
    assert_eq!(none["payee"], "");

    // С «Кому»: список отдаёт, повтор op_id — та же запись, сторно повторяет «Кому».
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "article_id": household, "amount_tyiyn": 28_500,
                       "payee": "  Иман  ", "comment": "зарплата" });
    let first = ok(&app, "POST", "/api/v1/expenses", &admin, Some(body.clone())).await;
    assert_eq!(first["payee"], "Иман");
    let again = ok(&app, "POST", "/api/v1/expenses", &admin, Some(body)).await;
    assert_eq!(again["id"], first["id"]);
    let list = ok(&app, "GET", "/api/v1/expenses", &owner, None).await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["payee"], "Иман");
    let id = first["id"].as_str().unwrap();
    let back = ok(
        &app,
        "POST",
        &format!("/api/v1/expenses/{id}/reverse"),
        &admin,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(back["payee"], "Иман");
    assert_eq!(back["amount_tyiyn"], -28_500);
    let form = ok(&app, "GET", "/api/v1/expenses/payees", &admin, None).await;
    assert_eq!(form["names"][0], "Иман");

    // «Исправить» переносит «Кому» в черновик.
    let draft = ok(
        &app,
        "GET",
        &format!(
            "/api/v1/corrections/draft/expense/{}",
            none["id"].as_str().unwrap()
        ),
        &owner,
        None,
    )
    .await;
    assert_eq!(draft["doc"]["payee"], "", "{draft}");

    // Длиннее 100 знаков — 422.
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&admin),
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1_000,
                     "payee": "я".repeat(101) }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Владелец делает «Кому» обязательным: без него — 422, с ним — проходит; всё — в журнал.
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/profit-report",
        Some(&owner),
        Some(json!({ "default_period": "year", "expense_payee": "required", "show_reversals": false })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/profit-report",
        Some(&owner),
        Some(json!({ "default_period": "month", "expense_payee": "always", "show_reversals": false })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    ok(
        &app,
        "PUT",
        "/api/v1/settings/profit-report",
        &owner,
        Some(json!({ "default_period": "today", "expense_payee": "required", "show_reversals": true })),
    )
    .await;
    let log: Vec<Value> = sqlx::query_scalar(
        "select data from audit_log where action = 'settings.profit_report' order by created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(
        log[0]["changes"]["expense_payee"],
        json!({ "old": "optional", "new": "required" })
    );
    let form = ok(&app, "GET", "/api/v1/expenses/payees", &admin, None).await;
    assert_eq!(form["mode"], "required");
    let (s, _, err) = call(
        &app,
        "POST",
        "/api/v1/expenses",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1_000, "payee": " " })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        err["error"]["message"].as_str().unwrap().contains("кому"),
        "{err}"
    );
    ok(
        &app,
        "POST",
        "/api/v1/expenses",
        &admin,
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 1_000,
                     "payee": "Электросети" }),
        ),
    )
    .await;
}

/// Подсказки «Кому» (перекрёстная проверка задачи 003): по праву «Расходы»; администратору —
/// только из того, что ему показывает список расходов (касса, неделя, без статей владельца).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn expense_payees_follow_expense_rights(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let household = article(&app, &owner, "Хозтовары").await;
    let safe = account(&app, &owner, |a| a["owner_only"] == json!(true)).await;
    ok(
        &app,
        "POST",
        "/api/v1/expenses",
        &owner,
        Some(
            json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 100_000,
                     "source": "account", "account_id": safe, "payee": "ИП Асанов" }),
        ),
    )
    .await;
    let names = |v: &Value| v["names"].as_array().unwrap().clone();
    let all = ok(&app, "GET", "/api/v1/expenses/payees", &owner, None).await;
    assert!(names(&all).contains(&json!("ИП Асанов")), "{all}");
    let form = ok(&app, "GET", "/api/v1/expenses/payees", &admin, None).await;
    assert!(!names(&form).contains(&json!("ИП Асанов")), "{form}");
    // Без права «Расходы» — 403.
    sqlx::query(
        "delete from user_permissions where permission = 'expenses'
           and user_id = (select id from users where login = 'admin')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (s, _, _) = call(&app, "GET", "/api/v1/expenses/payees", Some(&admin), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
