//! «Изменить оплату» чека и «из наличных в QR» за смену (SPEC-37, ADR-132). Главное правило:
//! пересчитанные наличные не трогаем — у закрытой смены меняется расхождение и удержание, а не
//! остаток «Кассы».
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

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    till: String,
    bank: String,
    item: String,
}

impl W {
    async fn new(pool: PgPool) -> Self {
        let app = setup(pool.clone()).await;
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        let mut w = W {
            app,
            pool,
            owner,
            admin,
            cashier: String::new(),
            till: String::new(),
            bank: String::new(),
            item: String::new(),
        };
        let e = w
            .ok(
                "/api/v1/employees",
                json!({ "full_name": "Айбек", "is_cashier": true, "pin": "1234" }),
            )
            .await;
        w.cashier = e["id"].as_str().unwrap().to_string();
        let accounts = w.get("/api/v1/cash/accounts").await;
        let find = |kind: &str| {
            accounts
                .as_array()
                .unwrap()
                .iter()
                .find(|a| a["kind"] == json!(kind))
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        w.till = find("register");
        w.bank = find("bank");
        let cat = w
            .ok(
                "/api/v1/categories",
                json!({ "name": "Фильтры", "kind": "filter" }),
            )
            .await;
        let p = w
            .ok(
                "/api/v1/products",
                json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 100_000 }),
            )
            .await;
        w.item = p["id"].as_str().unwrap().to_string();
        let safe = accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["owner_only"] == json!(true))
            .unwrap()["id"]
            .clone();
        w.ok(
            "/api/v1/receipts",
            json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                    "lines": [{ "product_id": w.item, "qty": 20, "cost_tyiyn": 20 * 60_000 }] }),
        )
        .await;
        w
    }

    async fn post(&self, who: &str, uri: &str, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, "POST", uri, Some(who), Some(body)).await;
        (s, v)
    }

    async fn ok(&self, uri: &str, body: Value) -> Value {
        let (s, v) = self.post(&self.owner, uri, body).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn get(&self, uri: &str) -> Value {
        let (s, _, v) = call(&self.app, "GET", uri, Some(&self.owner), None).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn open_shift(&self) -> String {
        let s = self
            .ok(
                "/api/v1/shifts/open",
                json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": self.cashier, "cashier_pin": "1234" }),
            )
            .await;
        s["id"].as_str().unwrap().to_string()
    }

    /// Закрывает смену: пересчитано на `diff` больше «должно быть» (< 0 — недостача).
    async fn close_shift(&self, id: &str, diff: i64) {
        let cur = self.get("/api/v1/shifts/current").await;
        let expected = cur["expected_tyiyn"].as_i64().unwrap();
        self.ok(
            &format!("/api/v1/shifts/{id}/close"),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected + diff, "comment": "пересчёт" }),
        )
        .await;
        self.ok(
            &format!("/api/v1/shifts/{id}/handover"),
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": expected + diff }),
        )
        .await;
    }

    async fn sale(&self, payments: Value) -> (String, i64) {
        let s = self
            .ok(
                "/api/v1/sales",
                json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
                        "lines": [{ "kind": "piece", "product_id": self.item, "qty": 1, "unit_price_tyiyn": 100_000 }],
                        "payments": payments }),
            )
            .await;
        (
            s["id"].as_str().unwrap().to_string(),
            s["number"].as_i64().unwrap(),
        )
    }

    async fn balance(&self, account: &str) -> i64 {
        sqlx::query_scalar("select balance_tyiyn from cash_accounts where id = $1::uuid")
            .bind(account)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn scalar(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap()
    }

    async fn withheld(&self) -> i64 {
        self.scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'shortage'")
            .await
    }

    async fn profit_net(&self) -> i64 {
        let p = self.get("/api/v1/reports/profit").await;
        p["totals"]["net_tyiyn"].as_i64().unwrap()
    }

    async fn pay_change(
        &self,
        who: &str,
        sale: &str,
        op: Uuid,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut b = body;
        b["op_id"] = json!(op);
        if b.get("comment").is_none() {
            b["comment"] = json!("ошиблись способом оплаты");
        }
        self.post(who, &format!("/api/v1/sales/{sale}/pay-change"), b)
            .await
    }
}

/// Уведомления «счёт ушёл в минус» (SPEC-13).
const NEGATIVE: &str = "select count(*) from audit_log where action = 'cash.negative'";

fn card(amount: i64, account: &str) -> Value {
    json!({ "method": "card", "amount_tyiyn": amount, "account_id": account })
}

fn cash(amount: i64) -> Value {
    json!({ "method": "cash", "amount_tyiyn": amount })
}

#[sqlx::test(migrations = "./migrations")]
async fn open_shift_cash_to_qr_changes_till_and_bank(pool: PgPool) {
    let w = W::new(pool).await;
    let shift = w.open_shift().await;
    let (sid, number) = w.sale(json!([cash(100_000)])).await;
    let till = w.balance(&w.till).await;
    let bank = w.balance(&w.bank).await;
    let accruals = w
        .scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals")
        .await;
    let gross_before = w.get("/api/v1/reports/profit").await["totals"]["gross_tyiyn"].clone();
    let expected = w.get("/api/v1/shifts/current").await["expected_tyiyn"]
        .as_i64()
        .unwrap();

    // Окно: оплата сейчас и что распределить.
    let info = w.get(&format!("/api/v1/sales/{sid}/pay-change")).await;
    assert_eq!(info["to_split_tyiyn"], 100_000);
    assert!(info["counted"].is_null(), "{info}");

    // Администратору — нет.
    let (s, _) = w
        .pay_change(
            &w.admin,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Сумма не сходится — 422; причина обязательна — 422.
    let (s, _) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [card(90_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)], "comment": " " }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Пробное проведение ничего не меняет.
    let (s, dry) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)], "dry_run": true }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{dry}");
    assert_eq!(dry["effect"]["cash_delta_tyiyn"], -100_000);
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.scalar("select count(*) from pay_fixes").await, 0);

    let op = Uuid::now_v7();
    let (s, out) = w
        .pay_change(
            &w.owner,
            &sid,
            op,
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let new_id = out["new_sale_id"].as_str().unwrap().to_string();
    assert_eq!(w.balance(&w.till).await, till - 100_000);
    // Комиссия 0,5 %: на счёт 1 000, комиссия 5.
    assert_eq!(w.balance(&w.bank).await, bank + 100_000 - 500);
    let cur = w.get("/api/v1/shifts/current").await;
    assert_eq!(cur["expected_tyiyn"].as_i64().unwrap(), expected - 100_000);
    // Выручка, валовая и начисления — те же.
    assert_eq!(
        w.get("/api/v1/reports/profit").await["totals"]["gross_tyiyn"],
        gross_before
    );
    assert_eq!(
        w.scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals")
            .await,
        accruals
    );
    let reason: String =
        sqlx::query_scalar("select comment from doc_corrections where original_id = $1::uuid")
            .bind(&sid)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(reason, "ошиблись способом оплаты");
    // Повтор с тем же op_id — то же изменение.
    let (s, again) = w
        .pay_change(
            &w.owner,
            &sid,
            op,
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["new_sale_id"], json!(new_id));
    assert_eq!(w.scalar("select count(*) from pay_fixes").await, 1);
    // Исправленный чек второй раз не меняется.
    let (s, _) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [cash(100_000)] }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Отчёт открытой смены: строка «Изменение оплаты чеков», «должно быть» сходится.
    let r = w.get(&format!("/api/v1/shifts/{shift}/report")).await;
    let outgo = r["cash"]["outgo"].as_array().unwrap();
    let line = outgo.iter().find(|l| l["key"] == json!("pay_fix_out"));
    assert_eq!(line.unwrap()["amount_tyiyn"], 100_000, "{outgo:?}");
    assert!(r["after_close"].is_null());
    // Уведомление владельцу — с пометкой «оплата».
    let n = w.get("/api/v1/notifications").await;
    let title = n["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["title"].as_str().unwrap().to_string())
        .find(|t| t.starts_with("Изменена оплата"));
    assert_eq!(
        title.as_deref(),
        Some(format!("Изменена оплата чека № {number} → № {}", out["new_number"]).as_str())
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn closed_shift_shortage_returns_to_cashier(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    let (sid, _) = w.sale(json!([cash(100_000)])).await;
    // Недостача 1 000 — удержана с кассира при закрытии.
    w.close_shift(&s1, -100_000).await;
    assert_eq!(w.withheld().await, -100_000);
    let s2 = w.open_shift().await;
    let till = w.balance(&w.till).await;
    let r2_before = w.get(&format!("/api/v1/shifts/{s2}/report")).await;
    let profit_before = w.profit_net().await;

    let info = w.get(&format!("/api/v1/sales/{sid}/pay-change")).await;
    assert_eq!(info["counted"]["shift_id"], json!(s1));
    assert_eq!(info["counted"]["at"], "close");
    assert_eq!(info["diff_tyiyn"], -100_000);
    assert_eq!(info["withheld_tyiyn"], 100_000);

    // «Исправить» только оплаты у закрытой смены — 409.
    let draft = w
        .get(&format!("/api/v1/corrections/draft/sale/{sid}"))
        .await;
    let mut doc = draft["doc"].clone();
    doc["payments"] = json!([card(100_000, &w.bank)]);
    let (s, e) = w
        .post(
            &w.owner,
            "/api/v1/corrections",
            json!({ "op_id": Uuid::now_v7(), "kind": "sale", "original_id": sid, "comment": "x", "doc": doc }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");

    let negative = w.scalar(NEGATIVE).await;
    let (s, out) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let e = &out["effect"];
    assert_eq!(e["diff_before_tyiyn"], -100_000);
    assert_eq!(e["diff_after_tyiyn"], 0);
    assert_eq!(e["withhold_tyiyn"], 100_000);
    // Остаток «Кассы» не меняется (всё сдано в сейф — ноль) и в минус не уходил даже на миг.
    assert_eq!(till, 0);
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.scalar(NEGATIVE).await, negative);
    assert_eq!(w.withheld().await, 0);
    // Прибыль дня: −5 (комиссия), недостача и удержание взаимно.
    assert_eq!(w.profit_net().await, profit_before - 500);

    // Открытая сейчас смена не меняется.
    let r2 = w.get(&format!("/api/v1/shifts/{s2}/report")).await;
    assert_eq!(r2["cash"], r2_before["cash"]);
    assert_eq!(r2["revenue"], r2_before["revenue"]);
    assert_eq!(r2["sales"], r2_before["sales"]);
    assert!(r2["after_close"].is_null());

    // Отчёт закрытой смены: основная часть как при закрытии, блок «После закрытия».
    let r1 = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    assert_eq!(r1["cash"]["diff_tyiyn"], -100_000);
    assert_eq!(r1["shift"]["diff_fix_tyiyn"], 100_000);
    let a = &r1["after_close"];
    assert_eq!(a["at"], "close");
    assert_eq!(a["items"].as_array().unwrap().len(), 1);
    assert_eq!(a["before"]["diff_tyiyn"], -100_000);
    assert_eq!(a["after"]["diff_tyiyn"], 0);
    assert_eq!(a["before"]["withheld_tyiyn"], 100_000);
    assert_eq!(a["after"]["withheld_tyiyn"], 0);
    assert_eq!(a["before"]["cash_sales_tyiyn"], 100_000);
    assert_eq!(a["after"]["cash_sales_tyiyn"], 0);
    let net_before = a["before"]["net_profit_tyiyn"].as_i64().unwrap();
    assert_eq!(
        a["after"]["net_profit_tyiyn"].as_i64().unwrap(),
        net_before - 500
    );
    // Администратору — без прибыли.
    let (_, _, ra) = call(
        &w.app,
        "GET",
        &format!("/api/v1/shifts/{s1}/report"),
        Some(&w.admin),
        None,
    )
    .await;
    assert!(ra["after_close"]["after"].get("net_profit_tyiyn").is_none());
}

#[sqlx::test(migrations = "./migrations")]
async fn qr_to_cash_after_close_withholds_unless_unchecked(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    let (a, _) = w.sale(json!([card(100_000, &w.bank)])).await;
    let (b, _) = w.sale(json!([card(100_000, &w.bank)])).await;
    w.close_shift(&s1, 0).await;
    w.open_shift().await;
    // Без расхождения, QR → наличные: недостача 1 000 — удержать.
    let (s, out) = w
        .pay_change(
            &w.owner,
            &a,
            Uuid::now_v7(),
            json!({ "payments": [cash(100_000)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["effect"]["diff_after_tyiyn"], -100_000);
    assert_eq!(w.withheld().await, -100_000);
    // Без галочки — расхождение меняется, удержание нет.
    let (s, out) = w
        .pay_change(
            &w.owner,
            &b,
            Uuid::now_v7(),
            json!({ "payments": [cash(100_000)], "fix_withholding": false }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["effect"]["diff_after_tyiyn"], -200_000);
    assert_eq!(out["effect"]["withhold_tyiyn"], 0);
    assert_eq!(w.withheld().await, -100_000);
}

#[sqlx::test(migrations = "./migrations")]
async fn debt_needs_pin_and_keeps_purchase_day(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    let (sid, _) = w.sale(json!([cash(100_000)])).await;
    let party = w
        .ok(
            "/api/v1/parties",
            json!({ "role": "customer", "kind": "person", "first_name": "Бакыт", "last_name": "Асанов", "phone": "+996555000111" }),
        )
        .await;
    let pid = party["id"].as_str().unwrap().to_string();
    let debt = json!({ "method": "debt", "amount_tyiyn": 100_000 });
    let (s, e) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [debt], "party_id": pid }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let (s, out) = w
        .pay_change(
            &w.owner,
            &sid,
            Uuid::now_v7(),
            json!({ "payments": [debt], "party_id": pid, "party_inn": "12345678901234" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let balance: i64 = sqlx::query_scalar("select balance_tyiyn from parties where id = $1::uuid")
        .bind(&pid)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(balance.abs(), 100_000);
    let party_of_new: Option<Uuid> = sqlx::query_scalar("select party_id from sales where id = $1")
        .bind(Uuid::parse_str(out["new_sale_id"].as_str().unwrap()).unwrap())
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(party_of_new.map(|p| p.to_string()), Some(pid));
}

#[sqlx::test(migrations = "./migrations")]
async fn cash_to_bank_in_open_and_closed_shift(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    w.sale(json!([cash(100_000)])).await;
    w.sale(json!([cash(100_000)])).await;
    w.sale(json!([cash(100_000)])).await;
    w.sale(json!([cash(100_000)])).await;
    let info = w.get(&format!("/api/v1/shifts/{s1}/cash-to-bank")).await;
    assert_eq!(info["limit_tyiyn"], 400_000);
    assert_eq!(info["open"], true);
    let till = w.balance(&w.till).await;
    let bank = w.balance(&w.bank).await;
    let uri = format!("/api/v1/shifts/{s1}/cash-to-bank");

    // Больше предела — 422; администратору — 403.
    let (s, _) = w
        .post(
            &w.owner,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 400_001, "account_id": w.bank, "comment": "QR" }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = w
        .post(
            &w.admin,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 300_000, "account_id": w.bank, "comment": "QR" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "amount_tyiyn": 300_000, "account_id": w.bank, "comment": "клиенты платили QR" });
    let out = w.ok(&uri, body.clone()).await;
    assert_eq!(w.balance(&w.till).await, till - 300_000);
    assert_eq!(w.balance(&w.bank).await, bank + 300_000 - 1_500);
    // Повтор — тот же перенос.
    let again = w.ok(&uri, body).await;
    assert_eq!(again, out);
    assert_eq!(w.scalar("select count(*) from pay_fixes").await, 1);
    // Отчёт: строка «Из наличных в QR», перенос в списке и в безнале.
    let r = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    let outgo = r["cash"]["outgo"].as_array().unwrap();
    let line = outgo.iter().find(|l| l["key"] == json!("cash_to_bank"));
    assert_eq!(line.unwrap()["amount_tyiyn"], 300_000);
    assert_eq!(r["transfers"].as_array().unwrap().len(), 1);
    assert_eq!(r["shift"]["bank_split"][0]["cash_to_bank_tyiyn"], 300_000);
    // Отменить перенос — всё обратно.
    let fix = out["id"].as_str().unwrap().to_string();
    w.ok(
        &format!("/api/v1/cash-to-bank/{fix}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибся" }),
    )
    .await;
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.balance(&w.bank).await, bank);
    let (s, _) = w
        .post(
            &w.owner,
            &format!("/api/v1/cash-to-bank/{fix}/reverse"),
            json!({ "op_id": Uuid::now_v7(), "comment": "ещё раз" }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Закрытая смена с недостачей 3 000: касса не меняется, поправка +3 000, возврат удержания.
    w.close_shift(&s1, -300_000).await;
    assert_eq!(w.withheld().await, -300_000);
    let s2 = w.open_shift().await;
    let r2_before = w.get(&format!("/api/v1/shifts/{s2}/report")).await;
    let till = w.balance(&w.till).await;
    let out = w
        .ok(
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 300_000, "account_id": w.bank, "comment": "QR" }),
        )
        .await;
    assert_eq!(out["effect"]["diff_after_tyiyn"], 0, "{out}");
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.withheld().await, 0);
    let r1 = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    assert_eq!(r1["after_close"]["after"]["diff_tyiyn"], 0);
    assert_eq!(r1["after_close"]["after"]["cash_to_bank_tyiyn"], 300_000);
    let r2 = w.get(&format!("/api/v1/shifts/{s2}/report")).await;
    assert_eq!(r2["cash"], r2_before["cash"]);
    assert_eq!(r2["shift"]["bank_split"], r2_before["shift"]["bank_split"]);
    // Прибыль дня: сумма слагаемых — чистая.
    let p = w.get("/api/v1/reports/profit").await;
    assert!(p["totals"]["net_tyiyn"].is_i64());
}

#[sqlx::test(migrations = "./migrations")]
async fn no_shift_then_counted_at_open_and_cost_from_original(pool: PgPool) {
    let w = W::new(pool).await;
    // Чек без смены: пересчёта не было — меняется «Касса».
    let (a, _) = w.sale(json!([cash(100_000)])).await;
    let cost: i64 =
        sqlx::query_scalar("select cost_tyiyn from sale_lines where sale_id = $1::uuid")
            .bind(&a)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    // После чека — приход по другой цене: средняя меняется, себестоимость нового чека — нет.
    let safe: String =
        sqlx::query_scalar("select id::text from cash_accounts where owner_only limit 1")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    w.ok(
        "/api/v1/receipts",
        json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": w.item, "qty": 10, "cost_tyiyn": 10 * 90_000 }] }),
    )
    .await;
    let till = w.balance(&w.till).await;
    let (s, out) = w
        .pay_change(
            &w.owner,
            &a,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert!(out["effect"]["counted"].is_null());
    assert_eq!(w.balance(&w.till).await, till - 100_000);
    let new_cost: i64 =
        sqlx::query_scalar("select cost_tyiyn from sale_lines where sale_id = $1::uuid")
            .bind(out["new_sale_id"].as_str().unwrap())
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(new_cost, cost);

    // Ещё чек без смены, потом смена открыта с «Пересчитал» — поправка пересчёта при открытии,
    // удержания нет, «Касса» не меняется.
    let (b, _) = w.sale(json!([cash(100_000)])).await;
    let balance = w.balance(&w.till).await;
    w.ok(
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": w.cashier, "cashier_pin": "1234",
                "counted_tyiyn": balance - 100_000 }),
    )
    .await;
    let till = w.balance(&w.till).await;
    let withheld = w.withheld().await;
    let (s, out) = w
        .pay_change(
            &w.owner,
            &b,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["effect"]["counted"]["at"], "open");
    assert_eq!(out["effect"]["diff_before_tyiyn"], -100_000);
    assert_eq!(out["effect"]["diff_after_tyiyn"], 0);
    assert_eq!(out["effect"]["withhold_tyiyn"], 0);
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.withheld().await, withheld);
}

// ---------- Наличные ↔ QR в обе стороны и право администратора (задача 003 Н1) ----------

impl W {
    async fn get_as(&self, who: &str, uri: &str) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, "GET", uri, Some(who), None).await;
        (s, v)
    }

    async fn grant_admin(&self, permission: &str) {
        sqlx::query(
            "insert into user_permissions (user_id, branch_id, permission)
             select id, branch_id, $1 from users where login = 'admin'",
        )
        .bind(permission)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn expected(&self) -> i64 {
        self.get("/api/v1/shifts/current").await["expected_tyiyn"]
            .as_i64()
            .unwrap()
    }
}

fn lines_of(v: &Value) -> String {
    v["effect"]["lines"].to_string()
}

#[sqlx::test(migrations = "./migrations")]
async fn qr_to_cash_in_open_and_closed_shift(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    for _ in 0..3 {
        w.sale(json!([card(100_000, &w.bank)])).await;
    }
    w.sale(json!([cash(100_000)])).await;
    let uri = format!("/api/v1/shifts/{s1}/cash-to-bank");
    let info = w.get(&uri).await;
    let bank_limit = info["bank_limits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["account_id"] == json!(w.bank))
        .cloned()
        .unwrap();
    assert_eq!(bank_limit["limit_tyiyn"], 300_000, "{info}");
    assert_eq!(info["limit_tyiyn"], 100_000);
    assert_eq!(info["sales"].as_array().unwrap().len(), 4);
    assert!(info["sales"][0]["fixable"].as_bool().unwrap(), "{info}");

    // Больше предела по счёту — 422.
    let (s, e) = w
        .post(
            &w.owner,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "direction": "to_cash", "amount_tyiyn": 300_001,
                    "account_id": w.bank, "comment": "наличными" }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");

    let till = w.balance(&w.till).await;
    let bank = w.balance(&w.bank).await;
    let expected = w.expected().await;
    let gross = w.get("/api/v1/reports/profit").await["totals"]["gross_tyiyn"].clone();
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "direction": "to_cash", "amount_tyiyn": 100_000,
                       "account_id": w.bank, "comment": "клиент дал наличные, нажали QR" });
    let out = w.ok(&uri, body.clone()).await;
    // Касса +, счёт −, комиссия возвращена; «Должно быть» больше; выручка и валовая те же.
    assert_eq!(w.balance(&w.till).await, till + 100_000);
    assert_eq!(w.balance(&w.bank).await, bank - 100_000 + 500);
    assert_eq!(w.expected().await, expected + 100_000);
    assert_eq!(
        w.get("/api/v1/reports/profit").await["totals"]["gross_tyiyn"],
        gross
    );
    assert_eq!(out["effect"]["fee_tyiyn"], -500, "{out}");
    assert!(lines_of(&out).contains("больше"), "{out}");
    // Повтор `op_id` — тот же перенос.
    assert_eq!(w.ok(&uri, body).await, out);
    assert_eq!(w.scalar("select count(*) from pay_fixes").await, 1);
    // Предел «из наличных в QR» вырос на обратный перенос.
    let info = w.get(&uri).await;
    assert_eq!(info["limit_tyiyn"], 200_000, "{info}");
    assert_eq!(info["items"][0]["amount_tyiyn"], -100_000);
    // Отчёт смены: поступление «Из QR в наличные», безнал −, отдельная строка в «Деньгах».
    let r = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    let income = r["cash"]["income"].as_array().unwrap();
    let line = income.iter().find(|l| l["key"] == json!("bank_to_cash"));
    assert_eq!(line.unwrap()["amount_tyiyn"], 100_000, "{r}");
    let split = r["shift"]["bank_split"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["account_id"] == json!(w.bank))
        .cloned()
        .unwrap();
    assert_eq!(split["cash_to_bank_tyiyn"], -100_000);
    assert_eq!(r["pay_fix_items"].as_array().unwrap().len(), 1);
    let money = r["money"].as_array().unwrap();
    let row = money.iter().find(|m| m["kind"] == json!("bank_to_cash"));
    assert_eq!(row.unwrap()["amount_tyiyn"], 100_000, "{r}");
    // Безнал (банк) по дням — отдельная строка; сводка дня владельца — по счёту.
    let b = w.get("/api/v1/cash/bank").await;
    assert_eq!(b["cash_to_bank_tyiyn"], -100_000, "{b}");
    let d = w.get("/api/v1/owner/dashboard").await;
    let acc = d["noncash"]["by_account"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["account_id"] == json!(w.bank))
        .cloned()
        .unwrap();
    assert_eq!(acc["cash_to_bank_tyiyn"], -100_000, "{d}");
    // Сверка с банком: перенос закрывает нашу несопоставленную оплату картой в окне смены.
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let recon = w
        .get(&format!("/api/v1/bank/reconcile?from={today}&to={today}"))
        .await;
    assert_eq!(recon["days"][0]["unmatched"], 2, "{recon}");

    // Отмена — всё обратно.
    let fix = out["id"].as_str().unwrap().to_string();
    w.ok(
        &format!("/api/v1/cash-to-bank/{fix}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибся" }),
    )
    .await;
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.balance(&w.bank).await, bank);
    assert_eq!(w.expected().await, expected);
    let recon = w
        .get(&format!("/api/v1/bank/reconcile?from={today}&to={today}"))
        .await;
    assert_eq!(recon["days"][0]["unmatched"], 3, "{recon}");

    // Закрытая смена без расхождения: «из QR в наличные» — недостача 1 000, удержание 1 000;
    // без галочки — удержания нет. «Касса» не меняется.
    w.close_shift(&s1, 0).await;
    let s2 = w.open_shift().await;
    let till = w.balance(&w.till).await;
    let (s, dry) = w
        .post(
            &w.owner,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "direction": "to_cash", "amount_tyiyn": 100_000,
                    "account_id": w.bank, "comment": "", "dry_run": true, "fix_withholding": false }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{dry}");
    assert_eq!(dry["effect"]["withhold_tyiyn"], 0);
    assert_eq!(dry["effect"]["withhold_possible_tyiyn"], -100_000);
    let out = w
        .ok(
            &uri,
            json!({ "op_id": Uuid::now_v7(), "direction": "to_cash", "amount_tyiyn": 100_000,
                    "account_id": w.bank, "comment": "наличными" }),
        )
        .await;
    assert_eq!(out["effect"]["diff_after_tyiyn"], -100_000, "{out}");
    assert_eq!(out["effect"]["withhold_tyiyn"], -100_000);
    assert_eq!(w.balance(&w.till).await, till);
    assert_eq!(w.withheld().await, -100_000);
    let r1 = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    assert_eq!(r1["after_close"]["after"]["cash_to_bank_tyiyn"], -100_000);
    let last = r1["after_close"]["items"]
        .as_array()
        .unwrap()
        .last()
        .cloned()
        .unwrap();
    assert!(
        last["new_text"].as_str().unwrap().contains("в наличные"),
        "{r1}"
    );
    // Отчёт открытой сейчас смены не меняется.
    let r2 = w.get(&format!("/api/v1/shifts/{s2}/report")).await;
    assert!(r2["pay_fix_items"].as_array().unwrap().is_empty());
    // Отмена поправки удержания возвращает удержанное.
    let fix = out["id"].as_str().unwrap().to_string();
    w.ok(
        &format!("/api/v1/cash-to-bank/{fix}/reverse"),
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибся" }),
    )
    .await;
    assert_eq!(w.withheld().await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn closed_shift_surplus_goes_to_zero_after_qr_to_cash(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    w.sale(json!([card(100_000, &w.bank)])).await;
    // Кассир нажал QR, а взял наличные: при закрытии излишек 1 000.
    w.close_shift(&s1, 100_000).await;
    w.open_shift().await;
    let out = w
        .ok(
            &format!("/api/v1/shifts/{s1}/cash-to-bank"),
            json!({ "op_id": Uuid::now_v7(), "direction": "to_cash", "amount_tyiyn": 100_000,
                    "account_id": w.bank, "comment": "клиент платил наличными" }),
        )
        .await;
    assert_eq!(out["effect"]["diff_before_tyiyn"], 100_000, "{out}");
    assert_eq!(out["effect"]["diff_after_tyiyn"], 0);
    assert_eq!(out["effect"]["withhold_tyiyn"], 0);
    assert_eq!(w.withheld().await, 0);
    // Владельцу — прибыль дня: комиссия вернулась, излишек ушёл.
    assert!(lines_of(&out).contains("Прибыль дня"), "{out}");
}

#[sqlx::test(migrations = "./migrations")]
async fn admin_with_till_pay_fix_only_in_open_shift(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    let (a, _) = w.sale(json!([cash(100_000)])).await;
    let (b, _) = w.sale(json!([card(100_000, &w.bank)])).await;
    let (c, _) = w.sale(json!([cash(100_000)])).await;
    w.sale(json!([cash(100_000)])).await;
    let uri = format!("/api/v1/shifts/{s1}/cash-to-bank");

    // Без права — 403 «Нет права: …».
    let no = "Нет права: Наличные ↔ QR в открытой смене";
    let (s, e) = w.get_as(&w.admin, &uri).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"]["message"], no);
    let (s, e) = w
        .pay_change(
            &w.admin,
            &a,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(e["error"]["message"], no);

    w.grant_admin("till_pay_fix").await;
    let (s, info) = w.get_as(&w.admin, &uri).await;
    assert_eq!(s, StatusCode::OK, "{info}");
    assert!(info["admin_max_tyiyn"].is_null());
    // Перенос в обе стороны; администратору — без прибыли (I13).
    let (s, out) = w
        .post(
            &w.admin,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 50_000, "account_id": w.bank,
                    "comment": "QR" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["effect"]["profit_tyiyn"], 0);
    assert!(out["effect"]["month_note"].is_null());
    assert!(!lines_of(&out).contains("Прибыль"), "{out}");
    let own = out["id"].as_str().unwrap().to_string();
    let (s, out) = w
        .post(
            &w.admin,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "direction": "to_cash", "amount_tyiyn": 30_000,
                    "account_id": w.bank, "comment": "наличными" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Каждый перенос администратора — владельцу событием.
    let by_admin: i64 = w
        .scalar(
            "select count(*) from audit_log l join users u on u.id = l.user_id
             where l.action = 'shift.cash_to_bank' and u.login = 'admin'",
        )
        .await;
    assert_eq!(by_admin, 2);
    // Чек открытой смены: наличные → карта; без прибыли.
    let (s, out) = w
        .pay_change(
            &w.admin,
            &a,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["effect"]["profit_tyiyn"], 0);
    assert!(!lines_of(&out).contains("Прибыль"), "{out}");
    let (s, info) = w
        .get_as(&w.admin, &format!("/api/v1/sales/{b}/pay-change"))
        .await;
    assert_eq!(s, StatusCode::OK, "{info}");
    // В долг — только владелец.
    let (s, e) = w
        .pay_change(
            &w.admin,
            &c,
            Uuid::now_v7(),
            json!({ "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");

    // Чужой перенос администратор не отменяет; свой — да.
    let owners = w
        .ok(
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 10_000, "account_id": w.bank,
                    "comment": "QR" }),
        )
        .await;
    let (s, _) = w
        .post(
            &w.admin,
            &format!(
                "/api/v1/cash-to-bank/{}/reverse",
                owners["id"].as_str().unwrap()
            ),
            json!({ "op_id": Uuid::now_v7(), "comment": "не то" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, e) = w
        .post(
            &w.admin,
            &format!("/api/v1/cash-to-bank/{own}/reverse"),
            json!({ "op_id": Uuid::now_v7(), "comment": "ошибся" }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{e}");

    // Предел владельца на администратора: исправлено 30 000 + 100 000 (чек), ещё 1 — 422.
    let (s, _, e) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/shift",
        Some(&w.admin),
        Some(json!({ "print_on_close": true, "admin_pay_fix_max_tyiyn": 1 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    let (s, _, e) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/shift",
        Some(&w.owner),
        Some(json!({ "print_on_close": true, "pay_fix_on_close": true,
                     "admin_pay_fix_max_tyiyn": 130_000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    let (s, info) = w.get_as(&w.admin, &uri).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(info["admin_used_tyiyn"], 130_000, "{info}");
    let (s, e) = w
        .post(
            &w.admin,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 100, "account_id": w.bank,
                    "comment": "QR" }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("разрешённого владельцем")
    );
    let settings = w.get("/api/v1/settings/shift").await;
    assert_eq!(settings["admin_pay_fix_max_tyiyn"], 130_000);
    assert_eq!(settings["pay_fix_on_close"], true);

    // Смена закрыта — только владелец: окно, перенос, чек этой смены.
    w.close_shift(&s1, 0).await;
    let (s, _) = w.get_as(&w.admin, &uri).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w
        .post(
            &w.admin,
            &uri,
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": 100, "account_id": w.bank,
                    "comment": "QR" }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    w.open_shift().await;
    let (s, _) = w
        .pay_change(
            &w.admin,
            &c,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, out) = w
        .pay_change(
            &w.owner,
            &c,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
}

#[sqlx::test(migrations = "./migrations")]
async fn shift_sales_tab_and_items_per_change(pool: PgPool) {
    let w = W::new(pool).await;
    let s1 = w.open_shift().await;
    let (a, na) = w.sale(json!([cash(100_000)])).await;
    let (b, _) = w.sale(json!([cash(100_000)])).await;
    // Чек с возвратом «По чекам» не меняется — причину говорит сервер.
    w.ok(
        &format!("/api/v1/sales/{b}/return"),
        json!({ "op_id": Uuid::now_v7(), "comment": "вернул",
                "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [cash(100_000)] }),
    )
    .await;
    let info = w.get(&format!("/api/v1/shifts/{s1}/cash-to-bank")).await;
    let sales = info["sales"].as_array().unwrap();
    let row = |id: &str| {
        sales
            .iter()
            .find(|s| s["id"] == json!(id))
            .cloned()
            .unwrap()
    };
    assert_eq!(row(&a)["fixable"], true, "{info}");
    assert_eq!(row(&b)["fixable"], false, "{info}");
    assert!(row(&b)["reason"].is_string());
    // Два изменения оплаты — две строки отчёта с номерами чеков; сумма — строка движения денег.
    let (s, out) = w
        .pay_change(
            &w.owner,
            &a,
            Uuid::now_v7(),
            json!({ "payments": [card(100_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let (c, _) = w.sale(json!([cash(100_000)])).await;
    let (s, out) = w
        .pay_change(
            &w.owner,
            &c,
            Uuid::now_v7(),
            json!({ "payments": [card(40_000, &w.bank), cash(60_000)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    let r = w.get(&format!("/api/v1/shifts/{s1}/report")).await;
    let items = r["pay_fix_items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{r}");
    assert_eq!(items[0]["original_number"], na);
    let sum: i64 = items
        .iter()
        .map(|i| i["cash_delta_tyiyn"].as_i64().unwrap())
        .sum();
    assert_eq!(sum, -140_000);
    // Строки «Изменение оплаты чеков» в движении денег: ушло минус пришло — то же.
    let line = |side: &str, key: &str| {
        r["cash"][side]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["key"] == json!(key))
            .map_or(0, |l| l["amount_tyiyn"].as_i64().unwrap())
    };
    assert_eq!(
        line("outgo", "pay_fix_out") - line("income", "pay_fix_in"),
        140_000,
        "{r}"
    );
    let fixes: Vec<&Value> = r["money"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["kind"] == json!("pay_fix"))
        .collect();
    assert_eq!(fixes.len(), 2, "{r}");
    assert!(
        fixes[0]["counterparty"]
            .as_str()
            .unwrap()
            .contains(&format!("№ {na}"))
    );
}

/// Администратор с `till_pay_fix` меняет оплату чека со скидкой (перекрёстная проверка задачи
/// 003): цены повтора те же, их проверили при продаже — право «Скидка» и предел администратора
/// не нужны; сторно повтора не требует «Возврат по чеку» — право проверено как `till_pay_fix`.
#[sqlx::test(migrations = "./migrations")]
async fn admin_changes_payment_of_discounted_sale(pool: PgPool) {
    let w = W::new(pool).await;
    w.open_shift().await;
    // Владелец продал со скидкой: 1000 с по прайсу → 800 с.
    let sale = w
        .ok(
            "/api/v1/sales",
            json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
                    "lines": [{ "kind": "piece", "product_id": w.item, "qty": 1, "unit_price_tyiyn": 80_000 }],
                    "payments": [cash(80_000)] }),
        )
        .await;
    let id = sale["id"].as_str().unwrap().to_string();
    // У администратора нет «Скидки» (по умолчанию) и снят «Возврат по чеку».
    w.grant_admin("till_pay_fix").await;
    sqlx::query(
        "delete from user_permissions where permission in ('discount', 'sale_return')
           and user_id = (select id from users where login = 'admin')",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    let (s, out) = w
        .pay_change(
            &w.admin,
            &id,
            Uuid::now_v7(),
            json!({ "payments": [card(80_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Обычный возврат без права — по-прежнему 403.
    let (s, _) = w
        .post(
            &w.admin,
            &format!(
                "/api/v1/sales/{}/return",
                out["new_sale_id"].as_str().unwrap()
            ),
            json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                    "payments": [card(80_000, &w.bank)] }),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

/// Администратор переносит «Наличные ↔ QR», а смену в это время закрывают (перекрёстная
/// проверка задачи 003): открытость смены проверяется под блокировкой кассы — после закрытия
/// перенос администратора не проходит как поправка закрытой смены (ADR-149).
#[sqlx::test(migrations = "./migrations")]
async fn admin_transfer_waits_for_close_and_sees_closed_shift(pool: PgPool) {
    let w = W::new(pool.clone()).await;
    let shift = w.open_shift().await;
    w.sale(json!([cash(100_000)])).await;
    w.grant_admin("till_pay_fix").await;
    let expected = w.expected().await;
    let waiting = |n: i64| {
        let pool = pool.clone();
        async move {
            for _ in 0..200 {
                let k: i64 = sqlx::query_scalar(
                    "select count(*) from pg_stat_activity
                     where datname = current_database() and wait_event_type = 'Lock'",
                )
                .fetch_one(&pool)
                .await
                .unwrap();
                if k >= n {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("запросы не встали в очередь за кассой");
        }
    };
    // Касса занята — закрытие смены встаёт в очередь первым, перенос администратора — за ним.
    let mut hold = pool.begin().await.unwrap();
    sqlx::query("select id from cash_accounts where id = $1::uuid for update")
        .bind(&w.till)
        .execute(&mut *hold)
        .await
        .unwrap();
    let close = {
        let (app, owner, uri) = (
            w.app.clone(),
            w.owner.clone(),
            format!("/api/v1/shifts/{shift}/close"),
        );
        tokio::spawn(async move {
            call(
                &app,
                "POST",
                &uri,
                Some(&owner),
                Some(json!({
                "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "пересчёт" })),
            )
            .await
        })
    };
    waiting(1).await;
    let transfer = {
        let (app, admin, uri, bank) = (
            w.app.clone(),
            w.admin.clone(),
            format!("/api/v1/shifts/{shift}/cash-to-bank"),
            w.bank.clone(),
        );
        tokio::spawn(async move {
            call(
                &app,
                "POST",
                &uri,
                Some(&admin),
                Some(json!({
                "op_id": Uuid::now_v7(), "amount_tyiyn": 50_000, "account_id": bank,
                "comment": "QR" })),
            )
            .await
        })
    };
    waiting(2).await;
    hold.commit().await.unwrap();
    let (s, out) = {
        let r = close.await.unwrap();
        (r.0, r.2)
    };
    assert_eq!(s, StatusCode::OK, "{out}");
    let (s, _, out) = transfer.await.unwrap();
    assert_eq!(s, StatusCode::FORBIDDEN, "{out}");
    assert_eq!(w.withheld().await, 0);
}
