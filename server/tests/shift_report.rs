//! Отчёт смены, как в Умаге, и отменённые товары (SPEC-30 §7, §8, ADR-099).
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
    // Администратор с правами по умолчанию, как после миграции 0056 (ADR-097).
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

/// POST, который обязан пройти.
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

fn line(product: &str, qty: i64, price: i64) -> Value {
    json!({ "kind": "piece", "product_id": product, "qty": qty, "unit_price_tyiyn": price })
}

fn pay(method: &str, amount: i64) -> Value {
    json!({ "method": method, "amount_tyiyn": amount })
}

async fn sale(
    app: &Router,
    who: &str,
    cashier: &str,
    lines: Value,
    payments: Value,
    extra: Value,
) -> Value {
    let mut body = json!({
        "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
        "lines": lines, "payments": payments
    });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    ok(app, who, "/api/v1/sales", body).await
}

fn amount_of(lines: &Value, key: &str) -> i64 {
    lines
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["key"] == json!(key))
        .map_or(0, |l| l["amount_tyiyn"].as_i64().unwrap())
}

fn total(lines: &Value) -> i64 {
    lines
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["amount_tyiyn"].as_i64().unwrap())
        .sum()
}

fn method(r: &Value, m: &str) -> i64 {
    r["revenue"]["methods"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["method"] == json!(m))
        .map_or(0, |x| x["amount_tyiyn"].as_i64().unwrap())
}

/// Движение денег сходится, выручка по способам равна итогу, а суммы — те же, что на
/// экране «Смена».
fn check_balances(r: &Value) {
    let c = &r["cash"];
    let opening = c["opening_tyiyn"].as_i64().unwrap();
    let income = c["income_tyiyn"].as_i64().unwrap();
    let outgo = c["outgo_tyiyn"].as_i64().unwrap();
    assert_eq!(income, total(&c["income"]), "{c}");
    assert_eq!(outgo, total(&c["outgo"]), "{c}");
    assert_eq!(
        opening + income - outgo,
        c["expected_tyiyn"].as_i64().unwrap(),
        "{c}"
    );
    // Разницы, которую пришлось бы показывать «прочим», нет.
    assert_eq!(amount_of(&c["income"], "other_in"), 0, "{c}");
    assert_eq!(amount_of(&c["outgo"], "other_out"), 0, "{c}");
    let s = &r["shift"];
    assert_eq!(c["expected_tyiyn"], s["expected_tyiyn"]);
    let v = &r["revenue"];
    let net = v["net_tyiyn"].as_i64().unwrap();
    assert_eq!(
        net,
        v["sales_tyiyn"].as_i64().unwrap() - v["returns_tyiyn"].as_i64().unwrap()
    );
    assert_eq!(v["rest_tyiyn"], 0, "{v}");
    let by_method: i64 = v["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["amount_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(by_method, net);
    // Те же суммы, что у смены на экране «Смена».
    assert_eq!(v["sales_tyiyn"], s["sales_total_tyiyn"]);
    assert_eq!(v["returns_tyiyn"], s["returns_tyiyn"]);
    assert_eq!(v["sales_count"], s["sales_count"]);
    for (m, field) in [
        ("cash", "cash_sales_tyiyn"),
        ("card", "card_tyiyn"),
        ("transfer", "transfer_tyiyn"),
        ("debt", "debt_tyiyn"),
        ("bonus", "bonus_tyiyn"),
        ("battery", "battery_tyiyn"),
    ] {
        assert_eq!(method(r, m), s[field].as_i64().unwrap(), "{m}");
    }
    // Столбики по часам в сумме — продажи смены.
    let hours: i64 = r["hours"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["sales_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(hours, v["sales_tyiyn"].as_i64().unwrap());
    // Наличные движения кассы складываются из тех же видов, что разбивка экрана «Смена».
    let breakdown: i64 = s["breakdown"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["sum_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(
        s["opening_expected_tyiyn"].as_i64().unwrap() + breakdown,
        c["expected_tyiyn"].as_i64().unwrap()
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_report_adds_up_and_hides_cost_from_admin(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;

    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let item = ok(
        &app,
        &owner,
        "/api/v1/products",
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 50_000 }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cashier = ok(
        &app,
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Иман", "is_cashier": true, "pin": "1234" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let master = ok(
        &app,
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Мастер Бек", "is_master": true }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let client = ok(
        &app,
        &admin,
        "/api/v1/parties",
        json!({ "name": "ОсОО Клиент", "kind": "company", "inn": "01234567890123" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let accounts = get(&app, &owner, "/api/v1/cash/accounts").await;
    let safe = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Сейф магазина")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Открытие: в кассе 0, пересчитали 100 с — это остаток на начало.
    let shift = ok(
        &app,
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234", "counted_tyiyn": 10_000 }),
    )
    .await;
    let sid = shift["id"].as_str().unwrap().to_string();
    ok(
        &app,
        &admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" }),
    )
    .await;
    // Приход из кассы смены: 10 фильтров по 200 с.
    let receipt = ok(
        &app,
        &admin,
        "/api/v1/receipts",
        json!({
            "op_id": Uuid::now_v7(), "payment": "paid",
            "lines": [{ "product_id": item, "qty": 10, "cost_tyiyn": 200_000 }]
        }),
    )
    .await;

    // Продажи разными способами; в первой — строка, удалённая крестиком до проведения.
    let removed_at = chrono::Utc::now().to_rfc3339();
    let cash_sale = sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 2, 50_000)]),
        json!([pay("cash", 100_000)]),
        json!({ "removed_lines": [
            { "reason": "remove", "kind": "piece", "product_id": item, "name": "Фильтр", "qty": 3, "unit_price_tyiyn": 50_000, "removed_at": removed_at },
            { "kind": "piece", "qty": "много" }
        ] }),
    )
    .await;
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("card", 50_000)]),
        json!({}),
    )
    .await;
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("transfer", 50_000)]),
        json!({}),
    )
    .await;
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("debt", 50_000)]),
        json!({ "party_id": client }),
    )
    .await;
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("cash", 20_000), pay("card", 30_000)]),
        json!({}),
    )
    .await;
    // Возврат одного фильтра из первого чека наличными.
    let cash_sale_id = cash_sale["id"].as_str().unwrap();
    ok(
        &app,
        &admin,
        &format!("/api/v1/sales/{cash_sale_id}/return"),
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }], "payments": [pay("cash", 50_000)] }),
    )
    .await;
    // Оплата долга, расход, изъятие, перевод в сейф, выплата мастеру.
    ok(
        &app,
        &admin,
        "/api/v1/debts/repayments",
        json!({ "op_id": Uuid::now_v7(), "party_id": client, "amount_tyiyn": 20_000 }),
    )
    .await;
    let articles = get(&app, &admin, "/api/v1/expense-articles").await;
    let household = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Хозтовары")
        .unwrap()["id"]
        .clone();
    ok(
        &app,
        &admin,
        "/api/v1/expenses",
        json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 30_000, "comment": "швабра" }),
    )
    .await;
    ok(
        &app,
        &admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_out", "amount_tyiyn": 10_000, "comment": "на хлеб" }),
    )
    .await;
    let till = shift["account_id"].as_str().unwrap().to_string();
    ok(
        &app,
        &admin,
        "/api/v1/cash/transfers",
        json!({ "op_id": Uuid::now_v7(), "from_account_id": till, "to_account_id": safe, "amount_tyiyn": 100_000 }),
    )
    .await;
    ok(
        &app,
        &owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": master, "amount_tyiyn": 15_000, "source": "account", "account_id": till, "advance": true, "comment": "аванс" }),
    )
    .await;
    // «Очистить»: две строки отдельной операцией; повтор с тем же op_id не дублирует.
    let cleared = json!({
        "op_id": Uuid::now_v7(), "shift_id": sid, "client_time": chrono::Utc::now().to_rfc3339(),
        "lines": [
            { "reason": "clear", "kind": "piece", "product_id": item, "name": "Фильтр", "qty": 1, "unit_price_tyiyn": 45_000 },
            { "reason": "clear", "kind": "pour", "name": "Масло на розлив", "qty": 1_500, "unit_price_tyiyn": 60_000 }
        ]
    });
    let first = ok(
        &app,
        &admin,
        "/api/v1/sales/cancelled-lines",
        cleared.clone(),
    )
    .await;
    assert_eq!(
        first,
        json!({ "count": 2, "amount_tyiyn": 45_000 + 90_000 })
    );
    let again = ok(&app, &admin, "/api/v1/sales/cancelled-lines", cleared).await;
    assert_eq!(again, first);
    let stored: i64 = sqlx::query_scalar("select count(*) from cancelled_lines")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 3);

    // Отчёт владельца.
    let r = get(&app, &owner, &format!("/api/v1/shifts/{sid}/report")).await;
    check_balances(&r);
    let current = get(&app, &admin, "/api/v1/shifts/current").await;
    assert_eq!(r["cash"]["expected_tyiyn"], current["expected_tyiyn"]);
    let c = &r["cash"];
    assert_eq!(c["opening_expected_tyiyn"], 0);
    assert_eq!(c["opening_tyiyn"], 10_000);
    assert_eq!(amount_of(&c["income"], "cash_sales"), 120_000);
    assert_eq!(amount_of(&c["income"], "debt_repayments"), 20_000);
    assert_eq!(amount_of(&c["income"], "cash_in"), 500_000);
    assert_eq!(amount_of(&c["outgo"], "supplier_payments"), 200_000);
    assert_eq!(amount_of(&c["outgo"], "sale_returns"), 50_000);
    assert_eq!(amount_of(&c["outgo"], "expenses"), 30_000);
    assert_eq!(amount_of(&c["outgo"], "cash_out"), 10_000);
    assert_eq!(amount_of(&c["outgo"], "transfers_out"), 100_000);
    assert_eq!(amount_of(&c["outgo"], "payouts"), 15_000);
    assert_eq!(c["expected_tyiyn"], 245_000);
    assert!(c["counted_tyiyn"].is_null());

    let v = &r["revenue"];
    assert_eq!(v["sales_count"], 5);
    assert_eq!(v["sales_tyiyn"], 300_000);
    assert_eq!(v["returns_tyiyn"], 50_000);
    assert_eq!(method(&r, "cash"), 70_000);
    assert_eq!(method(&r, "card"), 80_000);
    assert_eq!(method(&r, "transfer"), 50_000);
    assert_eq!(method(&r, "debt"), 50_000);
    // Себестоимость: продано 6 фильтров по 200 с, вернули 1.
    assert_eq!(v["cost_sold_tyiyn"], 120_000);
    assert_eq!(v["cost_returned_tyiyn"], 20_000);
    assert_eq!(v["cost_tyiyn"], 100_000);
    assert_eq!(v["gross_tyiyn"], 250_000 - 100_000);
    assert_eq!(r["owner_view"], true);

    // Вкладки: продажи и возвраты, закупки, денежные операции, долги, выплаты, отменённые.
    let sales = r["sales"].as_array().unwrap();
    assert_eq!(sales.len(), 6);
    assert_eq!(sales.iter().filter(|s| s["kind"] == "return").count(), 1);
    let ret = sales.iter().find(|s| s["kind"] == "return").unwrap();
    assert_eq!(ret["reversal_of_number"], cash_sale["number"]);
    let mixed = sales
        .iter()
        .find(|s| s["payments"].as_array().unwrap().len() == 2)
        .unwrap();
    assert_eq!(mixed["payments"][0]["method"], "cash");
    assert_eq!(mixed["payments"][0]["account_name"], "Касса");
    assert_eq!(r["sales_totals"]["sales_tyiyn"], 300_000);
    assert_eq!(r["sales_totals"]["debt_tyiyn"], 50_000);
    assert_eq!(r["sales_totals"]["paid_tyiyn"], 250_000);
    assert_eq!(r["returns_totals"]["count"], 1);
    assert_eq!(r["returns_totals"]["total_tyiyn"], 50_000);

    let receipts = r["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["id"], receipt["id"]);
    assert_eq!(receipts[0]["paid_till_tyiyn"], 200_000);

    let mt = &r["money_totals"];
    assert_eq!(amount_of(mt, "cash_in"), 500_000);
    assert_eq!(amount_of(mt, "cash_out"), 10_000);
    assert_eq!(amount_of(mt, "expenses"), 30_000);
    assert_eq!(amount_of(mt, "payouts"), 15_000);
    assert_eq!(amount_of(mt, "transfers_out"), 100_000);
    assert_eq!(amount_of(mt, "debt_repayments"), 20_000);
    assert_eq!(amount_of(mt, "supplier_payments"), 200_000);
    let money = r["money"].as_array().unwrap();
    let expense = money.iter().find(|m| m["kind"] == "expenses").unwrap();
    assert_eq!(expense["counterparty"], "Хозтовары");
    assert_eq!(expense["comment"], "швабра");
    let transfer = money.iter().find(|m| m["kind"] == "transfers_out").unwrap();
    assert_eq!(transfer["to_account_name"], "Сейф магазина");
    let repay = money
        .iter()
        .find(|m| m["kind"] == "debt_repayments")
        .unwrap();
    assert_eq!(repay["counterparty"], "ОсОО Клиент");
    let payout = money.iter().find(|m| m["kind"] == "payouts").unwrap();
    assert_eq!(payout["counterparty"], "Мастер Бек");

    let debts = r["debts"].as_array().unwrap();
    assert_eq!(debts.len(), 2);
    assert!(
        debts
            .iter()
            .any(|d| d["kind"] == "sale" && d["amount_tyiyn"] == 50_000)
    );
    assert!(
        debts
            .iter()
            .any(|d| d["kind"] == "repayment" && d["amount_tyiyn"] == -20_000)
    );

    let payouts = r["payouts"].as_array().unwrap();
    assert_eq!(payouts.len(), 1);
    assert_eq!(payouts[0]["employee_name"], "Мастер Бек");
    assert_eq!(payouts[0]["account_name"], "Касса");
    assert_eq!(r["payouts_tyiyn"], 15_000);

    let cancelled = r["cancelled"].as_array().unwrap();
    assert_eq!(cancelled.len(), 3);
    let crossed = cancelled.iter().find(|c| c["reason"] == "remove").unwrap();
    assert_eq!(crossed["sale_number"], cash_sale["number"]);
    assert_eq!(crossed["amount_tyiyn"], 150_000);
    assert_eq!(crossed["cashier_name"], "Иман");
    assert_eq!(r["cancelled_tyiyn"], 150_000 + 135_000);

    // Администратор: те же суммы, но без себестоимости и валовой (инвариант 6).
    let a = get(&app, &admin, &format!("/api/v1/shifts/{sid}/report")).await;
    check_balances(&a);
    assert_eq!(a["owner_view"], false);
    for key in [
        "cost_sold_tyiyn",
        "cost_returned_tyiyn",
        "cost_tyiyn",
        "gross_tyiyn",
    ] {
        assert!(a["revenue"].get(key).is_none(), "{key}: {}", a["revenue"]);
    }
    assert_eq!(a["cash"]["expected_tyiyn"], r["cash"]["expected_tyiyn"]);
    assert_eq!(a["cancelled"].as_array().unwrap().len(), 3);

    // Закрытие с излишком 10 с: пересчёт виден, «должно быть» прежнее, всё сходится.
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{sid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 246_000, "comment": "нашли" }),
    )
    .await;
    let closed = get(&app, &owner, &format!("/api/v1/shifts/{sid}/report")).await;
    check_balances(&closed);
    assert_eq!(closed["cash"]["expected_tyiyn"], 245_000);
    assert_eq!(closed["cash"]["counted_tyiyn"], 246_000);
    assert_eq!(closed["cash"]["diff_tyiyn"], 1_000);
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{sid}/handover"),
        json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 200_000, "to_account_id": safe }),
    )
    .await;
    let handed = get(&app, &owner, &format!("/api/v1/shifts/{sid}/report")).await;
    check_balances(&handed);
    assert_eq!(handed["cash"]["handover_tyiyn"], 200_000);
    assert_eq!(handed["cash"]["handover_to"], "Сейф магазина");
    assert_eq!(handed["cash"]["left_tyiyn"], 46_000);

    // Бот: отчёт смены коротко; валовая — только владельцу.
    use avtodom_server::api::shift_report::{load, report_text};
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let shift_id: Uuid = sid.parse().unwrap();
    let text = report_text(&load(&mut conn, branch, true, shift_id).await.unwrap(), &[]);
    for part in [
        "на начало: 100 с",
        "должно быть: 2 450 с",
        "пересчитали: 2 460 с — излишек 10 с",
        "сдали в «Сейф магазина»: 2 000 с, в кассе осталось 460 с",
        "наличные 700 с · карта / QR банка 800 с · QR владельцу 500 с · в долг 500 с",
        "валовая прибыль: 1 500 с",
        "Мастер Бек 150 с",
        "Оплаты долгов: ОсОО Клиент 200 с",
        "Расходы: Хозтовары 300 с",
        "в «Сейф магазина» 1 000 с",
        "Отменённые товары: 3 на 2 850 с",
    ] {
        assert!(text.contains(part), "{part}\n{text}");
    }
    let text = report_text(
        &load(&mut conn, branch, false, shift_id).await.unwrap(),
        &[],
    );
    assert!(!text.contains("валовая"), "{text}");
    drop(conn);

    // Следующая смена: только свои документы и сравнение с прошлой.
    let next = ok(
        &app,
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    let nid = next["id"].as_str().unwrap();
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("cash", 50_000)]),
        json!({}),
    )
    .await;
    let n = get(&app, &owner, &format!("/api/v1/shifts/{nid}/report")).await;
    check_balances(&n);
    assert_eq!(n["sales"].as_array().unwrap().len(), 1);
    assert_eq!(n["cash"]["opening_tyiyn"], 46_000);
    assert!(n["cancelled"].as_array().unwrap().is_empty());
    assert!(n["money"].as_array().unwrap().is_empty());
    assert!(n["payouts"].as_array().unwrap().is_empty());
    // Смена идёт — прошлая берётся за то же время от открытия, не целиком.
    let cmp = &n["compare"];
    assert_eq!(cmp["number"], shift["number"]);
    assert_eq!(cmp["same_time"], true);
    assert!(cmp["net_tyiyn"].as_i64().unwrap() <= 250_000);
    // Закрыли — сравнение со всей прошлой сменой.
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{nid}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": 96_000, "comment": "" }),
    )
    .await;
    let n = get(&app, &owner, &format!("/api/v1/shifts/{nid}/report")).await;
    let cmp = &n["compare"];
    assert_eq!(cmp["same_time"], false);
    assert_eq!(cmp["net_tyiyn"], 250_000);
    assert_eq!(cmp["diff_tyiyn"], 50_000 - 250_000);
    assert_eq!(cmp["change_bp"], -8_000);

    // Чужой и несуществующей смены нет.
    let (s, _, _) = call(
        &app,
        "GET",
        &format!("/api/v1/shifts/{}/report", Uuid::now_v7()),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cancelled_lines_without_shift_and_bad_lines(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    // Смены нет: строки записываются без смены; неверные строки пропускаются, а не отклоняют.
    let out = ok(
        &app,
        &admin,
        "/api/v1/sales/cancelled-lines",
        json!({
            "op_id": Uuid::now_v7(),
            "lines": [
                { "reason": "held", "kind": "service", "name": "Замена масла", "qty": 1, "unit_price_tyiyn": 30_000 },
                { "reason": "held", "kind": "piece", "name": "", "qty": 1, "unit_price_tyiyn": 100 },
                { "reason": "held", "kind": "piece", "name": "Минус", "qty": -1, "unit_price_tyiyn": 100 },
                { "reason": "held", "kind": "box", "name": "Ящик", "qty": 1, "unit_price_tyiyn": 100 }
            ]
        }),
    )
    .await;
    assert_eq!(out, json!({ "count": 1, "amount_tyiyn": 30_000 }));
    let row: (Option<Uuid>, String, String) =
        sqlx::query_as("select shift_id, reason, name from cancelled_lines")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, (None, "held".into(), "Замена масла".into()));
    // Слишком много строк за раз — 422.
    let many: Vec<Value> = (0..301)
        .map(|_| json!({ "kind": "piece", "name": "x", "qty": 1, "unit_price_tyiyn": 1 }))
        .collect();
    let (s, _, _) = call(
        &app,
        "POST",
        "/api/v1/sales/cancelled-lines",
        Some(&admin),
        Some(json!({ "op_id": Uuid::now_v7(), "lines": many })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

/// Сумма строк со знаком.
fn signed_sum(lines: &Value) -> i64 {
    lines
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["amount_tyiyn"].as_i64().unwrap())
        .sum()
}

/// Чистая прибыль за смену (SPEC-36 §9, ADR-120): формула прибыли дня по времени смены, без
/// оклада, только владельцу; две смены за день без чеков вне смен в сумме дают прибыль дня.
/// Безнал по счетам — по движениям счетов: виден и при одном счёте, с оплатой долга картой,
/// возвратом на карту и «упало на счёт» = итог − комиссия.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_net_profit_and_noncash_by_account(pool: PgPool) {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;

    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let item = ok(
        &app,
        &owner,
        "/api/v1/products",
        json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 50_000 }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let cashier = ok(
        &app,
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Иман", "is_cashier": true, "pin": "1234" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let client = ok(
        &app,
        &admin,
        "/api/v1/parties",
        json!({ "name": "ОсОО Клиент", "kind": "company", "inn": "01234567890123" }),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    let accounts = get(&app, &owner, "/api/v1/cash/accounts").await;
    let by_name = |name: &str| {
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == name)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let safe = by_name("Сейф магазина");
    let bank = by_name("Счёт");
    let banks = accounts
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["kind"] == "bank" && a["active"] == true)
        .count();
    assert_eq!(banks, 1, "по умолчанию один безналичный счёт");
    // Пустой счёт владельца в безнал смены не попадает: администратор его не видит (ADR-037).
    ok(
        &app,
        &owner,
        "/api/v1/cash/accounts",
        json!({ "op_id": Uuid::now_v7(), "name": "Личный", "kind": "bank", "owner_only": true }),
    )
    .await;
    // Товар приходит до смен: накладная в смены не входит.
    ok(
        &app,
        &owner,
        "/api/v1/receipts",
        json!({
            "op_id": Uuid::now_v7(), "payment": "paid", "paid_from": safe,
            "lines": [{ "product_id": item, "qty": 20, "cost_tyiyn": 20_000 }]
        }),
    )
    .await;
    let household = get(&app, &admin, "/api/v1/expense-articles")
        .await
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Хозтовары")
        .unwrap()["id"]
        .clone();

    // Смена 1: наличные и карта, расход, недостача при закрытии.
    let s1 = ok(
        &app,
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    let s1 = s1["id"].as_str().unwrap().to_string();
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 2, 50_000)]),
        json!([pay("cash", 100_000)]),
        json!({}),
    )
    .await;
    let card_sale = sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("card", 50_000)]),
        json!({}),
    )
    .await;
    ok(
        &app,
        &admin,
        "/api/v1/expenses",
        json!({ "op_id": Uuid::now_v7(), "article_id": household, "amount_tyiyn": 3_000, "comment": "швабра" }),
    )
    .await;
    let cur = get(&app, &admin, "/api/v1/shifts/current").await;
    let expected = cur["expected_tyiyn"].as_i64().unwrap();
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{s1}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected - 1_000, "comment": "не хватает" }),
    )
    .await;
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{s1}/handover"),
        json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 0 }),
    )
    .await;

    // Смена 2: продажа в долг, оплата долга картой, возврат чека первой смены на карту.
    let s2 = ok(
        &app,
        &admin,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    let s2 = s2["id"].as_str().unwrap().to_string();
    sale(
        &app,
        &admin,
        &cashier,
        json!([line(&item, 1, 50_000)]),
        json!([pay("debt", 50_000)]),
        json!({ "party_id": client }),
    )
    .await;
    ok(
        &app,
        &admin,
        "/api/v1/debts/repayments",
        json!({ "op_id": Uuid::now_v7(), "party_id": client, "amount_tyiyn": 30_000, "method": "card" }),
    )
    .await;
    let card_id = card_sale["id"].as_str().unwrap();
    ok(
        &app,
        &owner,
        &format!("/api/v1/sales/{card_id}/return"),
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }], "payments": [pay("card", 50_000)] }),
    )
    .await;

    // Открытая смена: чистая прибыль — сумма слагаемых, администратору её нет.
    let r2 = get(&app, &owner, &format!("/api/v1/shifts/{s2}/report")).await;
    let v2 = &r2["revenue"];
    let net2 = v2["net_profit_tyiyn"].as_i64().unwrap();
    assert_eq!(signed_sum(&v2["profit"]), net2, "{v2}");
    assert_eq!(
        amount_of(&v2["profit"], "gross"),
        v2["gross_tyiyn"].as_i64().unwrap()
    );
    let a2 = get(&app, &admin, &format!("/api/v1/shifts/{s2}/report")).await;
    assert!(
        a2["revenue"].get("net_profit_tyiyn").is_none(),
        "{}",
        a2["revenue"]
    );
    assert!(a2["revenue"].get("profit").is_none());
    assert!(a2["revenue"].get("gross_tyiyn").is_none());

    // Безнал второй смены: один счёт, оплата долга картой и возврат на карту — в нём.
    let split = r2["shift"]["bank_split"].as_array().unwrap();
    assert_eq!(split.len(), 1, "{split:?}");
    let b = &split[0];
    assert_eq!(b["account_id"], json!(bank));
    assert_eq!(b["card_tyiyn"], 0);
    assert_eq!(b["debt_tyiyn"], 30_000);
    assert_eq!(b["returns_tyiyn"], 50_000);
    assert_eq!(b["total_tyiyn"], 30_000 - 50_000);
    // Комиссия: с оплаты долга взята, с возвращённого чека — вернулась.
    let fee2 = b["fee_tyiyn"].as_i64().unwrap();
    assert_eq!(fee2, 150 - 250);
    assert_eq!(b["landed_tyiyn"].as_i64().unwrap(), -20_000 - fee2);
    // Тот же блок у администратора — безнал не секрет.
    assert_eq!(a2["shift"]["bank_split"], r2["shift"]["bank_split"]);

    let cur = get(&app, &admin, "/api/v1/shifts/current").await;
    let expected = cur["expected_tyiyn"].as_i64().unwrap();
    ok(
        &app,
        &admin,
        &format!("/api/v1/shifts/{s2}/close"),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "" }),
    )
    .await;

    // Первая смена: продажа картой — в счёте, комиссия вычтена из «упало»; недостача входит.
    let r1 = get(&app, &owner, &format!("/api/v1/shifts/{s1}/report")).await;
    // Для чека смены: кто закрыл.
    assert_eq!(r1["shift"]["closed_by"], "Админ");
    let b1 = &r1["shift"]["bank_split"][0];
    assert_eq!(b1["card_tyiyn"], 50_000);
    assert_eq!(b1["total_tyiyn"], 50_000);
    let fee1 = b1["fee_tyiyn"].as_i64().unwrap();
    assert_eq!(fee1, 250, "комиссия счёта по умолчанию — 0,5 %");
    assert_eq!(b1["landed_tyiyn"].as_i64().unwrap(), 50_000 - fee1);
    assert_eq!(fee1, r1["revenue"]["bank_fee_tyiyn"].as_i64().unwrap());
    let v1 = &r1["revenue"];
    assert_eq!(amount_of(&v1["profit"], "cash_diff"), -1_000);
    assert_eq!(amount_of(&v1["profit"], "expenses"), -3_000);
    assert_eq!(amount_of(&v1["profit"], "bank_fee"), -fee1);
    // Недостачу удержали с кассира: начисление −1 000 с компенсирует её в прибыли (ADR-065).
    let payroll1 = amount_of(&v1["profit"], "payroll");
    let gross1 = v1["gross_tyiyn"].as_i64().unwrap();
    assert_eq!(gross1, 150_000 - 3_000);
    assert_eq!(
        v1["net_profit_tyiyn"].as_i64().unwrap(),
        gross1 + payroll1 - fee1 - 3_000 - 1_000
    );
    assert_eq!(
        signed_sum(&v1["profit"]),
        v1["net_profit_tyiyn"].as_i64().unwrap()
    );

    // Две смены за день, чеков вне смен нет, оклада нет: сумма чистых — чистая прибыль дня.
    let r2 = get(&app, &owner, &format!("/api/v1/shifts/{s2}/report")).await;
    let day = get(&app, &owner, "/api/v1/reports/profit").await;
    assert_eq!(day["from"], day["to"]);
    assert_eq!(
        v1["net_profit_tyiyn"].as_i64().unwrap()
            + r2["revenue"]["net_profit_tyiyn"].as_i64().unwrap(),
        day["totals"]["net_tyiyn"].as_i64().unwrap(),
        "смена 1: {}\nсмена 2: {}\nдень: {}",
        v1["profit"],
        r2["revenue"]["profit"],
        day["totals"]
    );
}

/// Настройка «печатать чек смены после сдачи кассы» (SPEC-36 §10): по умолчанию включена, меняет
/// только владелец, читают все.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_print_setting(pool: PgPool) {
    let app = setup(pool).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    assert_eq!(
        get(&app, &admin, "/api/v1/settings/shift").await,
        json!({ "print_on_close": true })
    );
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/shift",
        Some(&admin),
        Some(json!({ "print_on_close": false })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, out) = call(
        &app,
        "PUT",
        "/api/v1/settings/shift",
        Some(&owner),
        Some(json!({ "print_on_close": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        get(&app, &admin, "/api/v1/settings/shift").await,
        json!({ "print_on_close": false })
    );
}
