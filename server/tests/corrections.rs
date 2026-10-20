//! «Исправить» у документов владельца (SPEC-29 §24, ADR-085): сторно и верный документ одной
//! операцией. Главная проверка: исходный документ вместе с исправлением меняет остатки, деньги,
//! долги, начисления и прибыль ровно так же, как проведённый сразу верный документ.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

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
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Админ')")
        .bind(Uuid::now_v7())
        .bind(branch)
        .bind(hash)
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

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    till: String,
    bank: String,
    safe: String,
}

impl World {
    async fn new(pool: PgPool) -> Self {
        let app = setup(pool.clone()).await;
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
        let find = |pred: &dyn Fn(&Value) -> bool| {
            accounts
                .as_array()
                .unwrap()
                .iter()
                .find(|a| pred(a))
                .unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let till = find(&|a| a["kind"] == json!("register"));
        let bank = find(&|a| a["kind"] == json!("bank"));
        let safe = find(&|a| a["owner_only"] == json!(true));
        ok(
            &app,
            "POST",
            "/api/v1/shifts/open",
            &admin,
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
        )
        .await;
        World {
            app,
            pool,
            owner,
            admin,
            cashier,
            till,
            bank,
            safe,
        }
    }

    async fn product(&self, name: &str, price: i64) -> String {
        let cat = ok(
            &self.app,
            "POST",
            "/api/v1/categories",
            &self.owner,
            json!({ "name": format!("Кат {name}"), "kind": "filter" }),
        )
        .await;
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/products",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name, "sale_price_tyiyn": price }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    /// Приход за деньги из сейфа: товар на склад по цене `cost` за штуку.
    async fn stock(&self, product: &str, qty: i64, cost: i64) {
        ok(
            &self.app,
            "POST",
            "/api/v1/receipts",
            &self.owner,
            json!({
                "op_id": Uuid::now_v7(), "paid_from": self.safe,
                "lines": [{ "product_id": product, "qty": qty, "cost_tyiyn": qty * cost }]
            }),
        )
        .await;
    }

    async fn supplier(&self, name: &str) -> String {
        let s = ok(
            &self.app,
            "POST",
            "/api/v1/suppliers",
            &self.owner,
            json!({ "name": name }),
        )
        .await;
        s["id"].as_str().unwrap().to_string()
    }

    async fn draft(&self, kind: &str, id: &str) -> Value {
        get(
            &self.app,
            &format!("/api/v1/corrections/draft/{kind}/{id}"),
            &self.owner,
        )
        .await
    }

    async fn correct(&self, op_id: Uuid, kind: &str, id: &str, doc: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/corrections",
            Some(&self.owner),
            Some(json!({ "op_id": op_id, "kind": kind, "original_id": id, "comment": "ошибка ввода", "doc": doc })),
        )
        .await
    }

    /// Всё, что меняет проведение: остатки, деньги, долги, начисления, выплаты, расходы,
    /// оплаты чеков, прибыль дня.
    async fn snapshot(&self) -> BTreeMap<String, i64> {
        let mut m = BTreeMap::new();
        let rows: Vec<(String, i64)> =
            sqlx::query_as("select 'acc:' || name, balance_tyiyn from cash_accounts")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        m.extend(rows);
        let rows: Vec<(String, i64)> =
            sqlx::query_as("select 'party:' || name, balance_tyiyn from parties")
                .fetch_all(&self.pool)
                .await
                .unwrap();
        m.extend(rows);
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "select p.name, bp.stock_qty, bp.stock_value_tyiyn from branch_products bp join products p on p.id = bp.product_id",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap();
        for (name, qty, value) in rows {
            m.insert(format!("stock:{name}"), qty);
            m.insert(format!("value:{name}"), value);
        }
        for sql in [
            "select 'accrual:' || e.full_name || ':' || a.kind, sum(a.amount_tyiyn)::bigint from payroll_accruals a join employees e on e.id = a.employee_id group by 1",
            "select 'payout:' || e.full_name, sum(p.amount_tyiyn)::bigint from payouts p join employees e on e.id = p.employee_id group by 1",
            "select 'expense:' || a.name, sum(x.amount_tyiyn)::bigint from expenses x join expense_articles a on a.id = x.article_id group by 1",
            "select 'pay:' || method, sum(amount_tyiyn)::bigint from sale_payments group by 1",
            "select 'fee:' || method, sum(fee_tyiyn)::bigint from sale_payments group by 1",
        ] {
            let rows: Vec<(String, i64)> = sqlx::query_as(sql).fetch_all(&self.pool).await.unwrap();
            m.extend(rows);
        }
        let profit = get(&self.app, "/api/v1/reports/profit", &self.owner).await;
        for key in [
            "goods_tyiyn",
            "services_tyiyn",
            "cost_tyiyn",
            "gross_tyiyn",
            "payroll_tyiyn",
            "bank_fee_tyiyn",
            "expenses_tyiyn",
            "bonus_tyiyn",
            "net_tyiyn",
        ] {
            m.insert(
                format!("profit:{key}"),
                profit["totals"][key].as_i64().unwrap(),
            );
        }
        m
    }

    async fn verify_stock(&self) {
        let v = get(&self.app, "/api/v1/stock/verify", &self.owner).await;
        assert_eq!(v, json!([]), "остаток = сумма движений");
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("select count(*) from {table}"))
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn delta(a: &BTreeMap<String, i64>, b: &BTreeMap<String, i64>) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for k in a.keys().chain(b.keys()) {
        let d = b.get(k).copied().unwrap_or(0) - a.get(k).copied().unwrap_or(0);
        if d != 0 {
            out.insert(k.clone(), d);
        }
    }
    out
}

/// Исходный документ + исправление = сразу проведённый верный документ: `post` проводит его
/// обычным путём, и разница снимков должна совпасть.
async fn assert_as_if<F, Fut>(w: &World, before: &BTreeMap<String, i64>, post: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let corrected = w.snapshot().await;
    post().await;
    let direct = w.snapshot().await;
    assert!(
        !delta(&corrected, &direct).is_empty(),
        "верный документ что-то меняет"
    );
    assert_eq!(
        delta(before, &corrected),
        delta(&corrected, &direct),
        "исходный + исправление ≠ верный документ"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_correction_is_as_if_the_right_sale_was_posted(pool: PgPool) {
    let w = World::new(pool).await;
    let oil = w.product("Масло", 50_000).await;
    let filter = w.product("Фильтр", 30_000).await;
    w.stock(&oil, 20, 20_000).await;
    w.stock(&filter, 20, 10_000).await;
    let s0 = w.snapshot().await;

    // Кассир пробил 2 масла наличными, а было 3 масла по 450 с и фильтр, часть картой.
    let sale = ok(
        &w.app,
        "POST",
        "/api/v1/sales",
        &w.admin,
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
            "lines": [{ "kind": "piece", "product_id": oil, "qty": 2, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 100_000 }]
        }),
    )
    .await;
    let sid = sale["id"].as_str().unwrap().to_string();

    // Администратору «Исправить» нет — ни формы, ни проведения.
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/corrections/draft/sale/{sid}"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut draft = w.draft("sale", &sid).await;
    assert_eq!(draft["number"], sale["number"]);
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/corrections",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "sale", "original_id": sid, "comment": "x", "doc": draft["doc"] })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let doc = &mut draft["doc"];
    assert_eq!(doc["cashier_id"], json!(w.cashier));
    doc["lines"] = json!([
        { "kind": "piece", "product_id": oil, "qty": 3, "unit_price_tyiyn": 45_000 },
        { "kind": "piece", "product_id": filter, "qty": 1, "unit_price_tyiyn": 30_000 }
    ]);
    doc["payments"] = json!([
        { "method": "cash", "amount_tyiyn": 65_000 },
        { "method": "card", "amount_tyiyn": 100_000 }
    ]);
    let right = doc.clone();
    // Без комментария — 422, ничего не проведено.
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/corrections",
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "sale", "original_id": sid, "comment": " ", "doc": right })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let op = Uuid::now_v7();
    let (s, fixed) = w.correct(op, "sale", &sid, right.clone()).await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    assert_eq!(fixed["original_number"], sale["number"]);
    let new_id = fixed["new_id"].as_str().unwrap().to_string();
    let what: Vec<&str> = fixed["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["what"].as_str().unwrap())
        .collect();
    assert!(what.contains(&"Масло: количество"), "{what:?}");
    assert!(what.contains(&"Фильтр: строка"), "{what:?}");
    assert!(what.contains(&"Итого"), "{what:?}");
    // Повтор с тем же op_id — то же исправление, второго нет.
    let (s, again) = w.correct(op, "sale", &sid, right.clone()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], fixed["id"]);
    assert_eq!(w.count("doc_corrections").await, 1);
    assert_eq!(w.count("sales").await, 3);
    // Исправленный второй раз не исправляется; новый — можно.
    let (s, e) = w.correct(Uuid::now_v7(), "sale", &sid, right.clone()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("уже исправлен")
    );

    // Связи в истории: исходный «исправлен → № N», новый «исправление № M».
    let links = get(&w.app, &format!("/api/v1/corrections/doc/{sid}"), &w.owner).await;
    assert_eq!(links["corrected_by"]["new_number"], fixed["new_number"]);
    let links = get(
        &w.app,
        &format!("/api/v1/corrections/doc/{new_id}"),
        &w.admin,
    )
    .await;
    assert_eq!(links["correction_of"]["original_number"], sale["number"]);
    assert_eq!(
        links["correction_of"]["diff"],
        json!([]),
        "администратору без сравнения"
    );
    // Новый чек — в тот же учётный день, кассир тот же.
    let new_sale = get(&w.app, &format!("/api/v1/sales/{new_id}"), &w.owner).await;
    assert_eq!(new_sale["cashier_id"], json!(w.cashier));
    assert_eq!(new_sale["total_tyiyn"], 165_000);
    // Владельцу — уведомление «Исправлен документ».
    let n = get(&w.app, "/api/v1/notifications", &w.owner).await;
    assert!(
        n["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["action"] == "correction.post"
                && i["title"]
                    .as_str()
                    .unwrap()
                    .starts_with("Исправлен документ: чек")),
        "{n}"
    );
    w.verify_stock().await;

    assert_as_if(&w, &s0, || async {
        let mut body = right.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/sales", &w.admin, body).await;
    })
    .await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn failed_correction_changes_nothing(pool: PgPool) {
    let w = World::new(pool).await;
    let oil = w.product("Масло", 50_000).await;
    w.stock(&oil, 5, 20_000).await;
    let sale = ok(
        &w.app,
        "POST",
        "/api/v1/sales",
        &w.admin,
        json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": w.cashier,
            "lines": [{ "kind": "piece", "product_id": oil, "qty": 1, "unit_price_tyiyn": 50_000 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }]
        }),
    )
    .await;
    let sid = sale["id"].as_str().unwrap().to_string();
    let before = w.snapshot().await;
    let (sales, returns_audit) = (w.count("sales").await, w.count("audit_log").await);

    // Сторно прошло бы, а верный чек не проводится (продать больше, чем заплатили) — откатывается всё.
    let mut draft = w.draft("sale", &sid).await;
    draft["doc"]["lines"][0]["qty"] = json!(2);
    let (s, e) = w
        .correct(Uuid::now_v7(), "sale", &sid, draft["doc"].clone())
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert_eq!(w.snapshot().await, before);
    assert_eq!(w.count("sales").await, sales);
    assert_eq!(w.count("audit_log").await, returns_audit);
    assert_eq!(w.count("doc_corrections").await, 0);

    // Сторно не проходит: приход, по товару которого отмечен брак от поставщика (ADR-070).
    let supplier = w.supplier("Поставщик А").await;
    let receipt = ok(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        json!({
            "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt",
            "lines": [{ "product_id": oil, "qty": 4, "cost_tyiyn": 80_000 }]
        }),
    )
    .await;
    ok(
        &w.app,
        "POST",
        "/api/v1/defects",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "product_id": oil, "qty": 1, "reason": "течёт", "supplier_id": supplier }),
    )
    .await;
    let rid = receipt["id"].as_str().unwrap().to_string();
    let before = w.snapshot().await;
    // Отказ — ещё до формы (SPEC-29 §24): черновик пробует сторно и откатывает его.
    let (s, e) = call(
        &w.app,
        "GET",
        &format!("/api/v1/corrections/draft/receipt/{rid}"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(
        e["error"]["message"].as_str().unwrap().contains("брак"),
        "{e}"
    );
    // И само исправление — тот же отказ.
    let doc = json!({ "supplier_id": supplier, "payment": "debt",
                      "lines": [{ "product_id": oil, "qty": 5, "cost_tyiyn": 100_000 }] });
    let (s, e) = w.correct(Uuid::now_v7(), "receipt", &rid, doc).await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(w.snapshot().await, before);
    assert_eq!(w.count("doc_corrections").await, 0);

    // Чек с возвратом не исправляется — понятный отказ ещё до формы.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/sales/{sid}/return"),
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }], "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }] }),
    )
    .await;
    let (s, e) = call(
        &w.app,
        "GET",
        &format!("/api/v1/corrections/draft/sale/{sid}"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(e["error"]["message"].as_str().unwrap().contains("возврат"));
    w.verify_stock().await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_correction_keeps_one_payment_and_right_debt(pool: PgPool) {
    let w = World::new(pool).await;
    let oil = w.product("Масло", 50_000).await;
    let filter = w.product("Фильтр", 30_000).await;
    let supplier = w.supplier("Поставщик А").await;
    // Деньги в сейф — из него платим «часть сразу».
    ok(
        &w.app,
        "POST",
        "/api/v1/cash/movements",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "account_id": w.safe, "kind": "cash_in", "amount_tyiyn": 1_000_000, "comment": "свои" }),
    )
    .await;
    let s0 = w.snapshot().await;
    let receipt = ok(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        json!({
            "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt",
            "paid_from": w.safe, "paid_now_tyiyn": 30_000, "supplier_doc": "Н-1",
            "lines": [{ "product_id": oil, "qty": 4, "cost_tyiyn": 80_000 }]
        }),
    )
    .await;
    let rid = receipt["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("receipt", &rid).await;
    let doc = &mut draft["doc"];
    assert_eq!(doc["payment"], "debt");
    assert_eq!(doc["paid_now_tyiyn"], 30_000);
    assert_eq!(doc["paid_from"], json!(w.safe));
    doc["lines"] = json!([
        { "product_id": oil, "qty": 5, "cost_tyiyn": 100_000, "sale_price_tyiyn": 55_000 },
        { "product_id": filter, "qty": 2, "cost_tyiyn": 20_000 }
    ]);
    doc["supplier_doc"] = json!("Н-1/2");
    let right = doc.clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "receipt", &rid, right.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let what: Vec<&str> = fixed["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["what"].as_str().unwrap())
        .collect();
    assert!(what.contains(&"Накладная поставщика"), "{what:?}");
    assert!(what.contains(&"Масло: количество"), "{what:?}");
    assert!(what.contains(&"Фильтр: строка"), "{what:?}");
    // Должны поставщику 120 000 − 30 000, из сейфа ушло 30 000 один раз.
    let new_id = fixed["new_id"].as_str().unwrap();
    let new_receipt = get(&w.app, &format!("/api/v1/receipts/{new_id}"), &w.owner).await;
    assert_eq!(new_receipt["debt"]["paid_tyiyn"], 30_000, "{new_receipt}");
    let original = get(&w.app, &format!("/api/v1/receipts/{rid}"), &w.owner).await;
    assert!(original["reversed_by"].is_string());
    w.verify_stock().await;

    assert_as_if(&w, &s0, || async {
        let mut body = right.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/receipts", &w.owner, body).await;
    })
    .await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn money_debt_and_payroll_corrections_are_as_if(pool: PgPool) {
    let w = World::new(pool).await;
    ok(
        &w.app,
        "POST",
        "/api/v1/cash/movements",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 500_000, "comment": "размен" }),
    )
    .await;
    let articles = get(&w.app, "/api/v1/expense-articles", &w.owner).await;
    let article = |name: &str| {
        articles
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["name"] == json!(name))
            .unwrap()["id"]
            .clone()
    };

    // Расход: 100 с хозтоваров из кассы, а был налог 150 с со счёта.
    let s0 = w.snapshot().await;
    let exp = ok(
        &w.app,
        "POST",
        "/api/v1/expenses",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "article_id": article("Хозтовары"), "amount_tyiyn": 10_000, "comment": "швабра" }),
    )
    .await;
    let mut draft = w.draft("expense", exp["id"].as_str().unwrap()).await;
    draft["doc"]["article_id"] = article("Налоги");
    draft["doc"]["amount_tyiyn"] = json!(15_000);
    draft["doc"]["account_id"] = json!(w.bank);
    let right = draft["doc"].clone();
    let (s, fixed) = w
        .correct(
            Uuid::now_v7(),
            "expense",
            exp["id"].as_str().unwrap(),
            right.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    assert_as_if(&w, &s0, || async {
        let mut body = right.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/expenses", &w.owner, body).await;
    })
    .await;

    // Оплата поставщику: 400 с наличными, а было 500 с со счёта.
    let oil = w.product("Масло", 50_000).await;
    let supplier = w.supplier("Поставщик Б").await;
    ok(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        json!({
            "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt",
            "lines": [{ "product_id": oil, "qty": 10, "cost_tyiyn": 200_000 }]
        }),
    )
    .await;
    let s0 = w.snapshot().await;
    let pay = ok(
        &w.app,
        "POST",
        "/api/v1/debts/repayments",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "party_id": supplier, "amount_tyiyn": 40_000, "method": "cash" }),
    )
    .await;
    let ledger = pay["ledger_id"].as_str().unwrap().to_string();
    let mut draft = w.draft("ledger", &ledger).await;
    assert_eq!(draft["doc"]["type"], "repayment");
    draft["doc"]["amount_tyiyn"] = json!(50_000);
    draft["doc"]["method"] = json!("transfer");
    draft["doc"]["account_id"] = json!(w.bank);
    let right = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "ledger", &ledger, right.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    assert_as_if(&w, &s0, || async {
        let mut body = right.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/debts/repayments", &w.owner, body).await;
    })
    .await;

    // Премия 50 с, а была 70 с; выплата 30 с из кассы, а было 40 с со счёта.
    let s0 = w.snapshot().await;
    let bonus = ok(
        &w.app,
        "POST",
        "/api/v1/payroll/accruals",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "kind": "bonus", "amount_tyiyn": 5_000, "comment": "за чистоту" }),
    )
    .await;
    let accrual = bonus["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("accrual", &accrual).await;
    assert_eq!(draft["doc"]["type"], "accrual");
    draft["doc"]["amount_tyiyn"] = json!(7_000);
    let right_bonus = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "accrual", &accrual, right_bonus.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let payout = ok(
        &w.app,
        "POST",
        "/api/v1/payouts",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "amount_tyiyn": 3_000, "account_id": w.till }),
    )
    .await;
    let pid = payout["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("payout", &pid).await;
    draft["doc"]["amount_tyiyn"] = json!(4_000);
    draft["doc"]["account_id"] = json!(w.bank);
    let right_payout = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "payout", &pid, right_payout.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    assert_eq!(fixed["diff"].as_array().unwrap().len(), 2, "{fixed}");
    assert_as_if(&w, &s0, || async {
        let mut body = right_bonus.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/payroll/accruals", &w.owner, body).await;
        let mut body = right_payout.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/payouts", &w.owner, body).await;
    })
    .await;

    // Внесение, перемещение и оприходование.
    let s0 = w.snapshot().await;
    let m = ok(
        &w.app,
        "POST",
        "/api/v1/cash/movements",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 20_000, "comment": "размен" }),
    )
    .await;
    let mid = m["movement_id"].as_str().unwrap().to_string();
    let mut draft = w.draft("cash_movement", &mid).await;
    draft["doc"]["kind"] = json!("cash_out");
    let right_move = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "cash_movement", &mid, right_move.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let t = ok(
        &w.app,
        "POST",
        "/api/v1/cash/transfers",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "from_account_id": w.till, "to_account_id": w.safe, "amount_tyiyn": 10_000 }),
    )
    .await;
    let tid = t["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("cash_transfer", &tid).await;
    draft["doc"]["to_account_id"] = json!(w.bank);
    let right_transfer = draft["doc"].clone();
    let (s, fixed) = w
        .correct(
            Uuid::now_v7(),
            "cash_transfer",
            &tid,
            right_transfer.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let si = ok(
        &w.app,
        "POST",
        "/api/v1/stock-ins",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "product_id": oil, "qty": 3, "price_tyiyn": 20_000, "comment": "нашли" }),
    )
    .await;
    let siid = si["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("stock_in", &siid).await;
    draft["doc"]["lines"][0]["qty"] = json!(2);
    let right_in = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "stock_in", &siid, right_in.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    assert_as_if(&w, &s0, || async {
        for (uri, doc) in [
            ("/api/v1/cash/movements", &right_move),
            ("/api/v1/cash/transfers", &right_transfer),
            ("/api/v1/stock-ins", &right_in),
        ] {
            let mut body = doc.clone();
            body["op_id"] = json!(Uuid::now_v7());
            ok(&w.app, "POST", uri, &w.owner, body).await;
        }
    })
    .await;

    // Список исправлений по виду — для отметок в списках.
    let list = get(&w.app, "/api/v1/corrections?kind=expense", &w.owner).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(w.count("doc_corrections").await, 7);
    // Связь не правится и не удаляется (инвариант 4).
    let e = sqlx::query("update doc_corrections set comment = 'x'")
        .execute(&w.pool)
        .await;
    assert!(e.is_err());
    w.verify_stock().await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn goods_and_salary_corrections_are_as_if(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = ok(
        &w.app,
        "POST",
        "/api/v1/categories",
        &w.owner,
        json!({ "name": "Масла", "kind": "oil" }),
    )
    .await;
    let mut oils = Vec::new();
    for name in ["Mobil 5W-30", "Shell 5W-30"] {
        let p = ok(
            &w.app,
            "POST",
            "/api/v1/products",
            &w.owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name, "container_ml": 4000, "sale_price_tyiyn": 200_000 }),
        )
        .await;
        oils.push(p["id"].as_str().unwrap().to_string());
    }
    let supplier = w.supplier("Поставщик В").await;
    ok(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        json!({
            "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt",
            "lines": [{ "product_id": oils[0], "qty": 40_000, "cost_tyiyn": 1_000_000 }]
        }),
    )
    .await;
    let s0 = w.snapshot().await;

    // Перелив 2 л, а было 3 л.
    let t = ok(
        &w.app,
        "POST",
        "/api/v1/oil/transfers",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "from_product_id": oils[0], "to_product_id": oils[1], "qty_ml": 2000 }),
    )
    .await;
    let tid = t["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("oil_transfer", &tid).await;
    draft["doc"]["qty_ml"] = json!(3000);
    let right_oil = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "oil_transfer", &tid, right_oil.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");

    // Возврат поставщику 1 л в счёт долга, а было 2 л деньгами в кассу.
    let r = ok(
        &w.app,
        "POST",
        "/api/v1/supplier-returns",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "supplier_id": supplier, "settle": "debt",
                "lines": [{ "product_id": oils[0], "qty": 1000 }] }),
    )
    .await;
    let rid = r["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("supplier_return", &rid).await;
    draft["doc"]["settle"] = json!("money");
    draft["doc"]["account_id"] = json!(w.till);
    draft["doc"]["lines"][0]["qty"] = json!(2000);
    draft["doc"]["lines"][0]["amount_tyiyn"] = json!(50_000);
    let right_return = draft["doc"].clone();
    let (s, fixed) = w
        .correct(
            Uuid::now_v7(),
            "supplier_return",
            &rid,
            right_return.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");

    // Приём аккумуляторов 10 кг, а было 12,5 кг.
    let b = ok(
        &w.app,
        "POST",
        "/api/v1/batteries/intake",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "grams": 10_000, "price_per_kg_tyiyn": 10_000 }),
    )
    .await;
    let bid = b["id"].as_str().unwrap().to_string();
    let mut draft = w.draft("battery_intake", &bid).await;
    draft["doc"]["grams"] = json!(12_500);
    let right_battery = draft["doc"].clone();
    let (s, fixed) = w
        .correct(
            Uuid::now_v7(),
            "battery_intake",
            &bid,
            right_battery.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");

    // Оклад 30 000 с, а договорились на 32 000 с; правка долга поставщику.
    let month = chrono::Utc::now().format("%Y-%m-01").to_string();
    ok(
        &w.app,
        "POST",
        "/api/v1/payroll/salary",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.cashier, "month": month, "amount_tyiyn": 3_000_000 }),
    )
    .await;
    let salary: Uuid =
        sqlx::query_scalar("select id from payroll_accruals where kind = 'monthly_salary'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let mut draft = w.draft("accrual", &salary.to_string()).await;
    assert_eq!(draft["doc"]["type"], "salary");
    draft["doc"]["amount_tyiyn"] = json!(3_200_000);
    let right_salary = draft["doc"].clone();
    let (s, fixed) = w
        .correct(
            Uuid::now_v7(),
            "accrual",
            &salary.to_string(),
            right_salary.clone(),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let adj = ok(
        &w.app,
        "POST",
        "/api/v1/debts/adjust",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "party_id": supplier, "amount_tyiyn": -10_000, "comment": "старый долг" }),
    )
    .await;
    let lid = adj["ledger_id"].as_str().unwrap().to_string();
    let mut draft = w.draft("ledger", &lid).await;
    assert_eq!(draft["doc"]["type"], "adjust");
    draft["doc"]["amount_tyiyn"] = json!(-12_000);
    let right_adjust = draft["doc"].clone();
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "ledger", &lid, right_adjust.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    w.verify_stock().await;

    // Оклад за месяц один: прямое проведение того же оклада — 409, поэтому его сравниваем
    // отдельно, а остальное — как сразу проведённые верные документы.
    let accrued: i64 = sqlx::query_scalar(
        "select sum(amount_tyiyn)::bigint from payroll_accruals where kind = 'monthly_salary'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(accrued, 3_200_000);
    let mut s0 = s0;
    *s0.entry("accrual:Айбек:monthly_salary".to_string())
        .or_insert(0) += 3_200_000;
    *s0.entry("profit:payroll_tyiyn".into()).or_insert(0) += 3_200_000;
    *s0.entry("profit:net_tyiyn".into()).or_insert(0) -= 3_200_000;
    assert_as_if(&w, &s0, || async {
        for (uri, doc, who) in [
            ("/api/v1/oil/transfers", &right_oil, &w.owner),
            ("/api/v1/supplier-returns", &right_return, &w.owner),
            ("/api/v1/batteries/intake", &right_battery, &w.admin),
            ("/api/v1/debts/adjust", &right_adjust, &w.owner),
        ] {
            let mut body = doc.clone();
            body["op_id"] = json!(Uuid::now_v7());
            ok(&w.app, "POST", uri, who, body).await;
        }
    })
    .await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn multi_line_stock_in_correction_is_as_if(pool: PgPool) {
    // Оприходование строками (ADR-086): исправление меняет количество одной строки, убирает
    // другую и добавляет третью — как будто сразу провели верное; цена продажи, поставленная
    // документом, остаётся в форме.
    let w = World::new(pool).await;
    let a = w.product("Хомут", 30_000).await;
    let b = w.product("Шайба", 5_000).await;
    let c = w.product("Болт", 2_000).await;
    let s0 = w.snapshot().await;
    let si = ok(
        &w.app,
        "POST",
        "/api/v1/stock-ins",
        &w.admin,
        json!({ "op_id": Uuid::now_v7(), "comment": "нашли", "lines": [
            { "product_id": a, "qty": 3, "price_tyiyn": 20_000, "sale_price_tyiyn": 32_000 },
            { "product_id": b, "qty": 2, "price_tyiyn": 1_000 },
        ] }),
    )
    .await;
    let id = si["id"].as_str().unwrap().to_string();
    let draft = w.draft("stock_in", &id).await;
    assert_eq!(draft["doc"]["lines"][0]["sale_price_tyiyn"], 32_000);
    assert_eq!(draft["doc"]["lines"][1]["sale_price_tyiyn"], Value::Null);
    let mut doc = draft["doc"].clone();
    doc["lines"] = json!([
        doc["lines"][0].clone(),
        { "product_id": c, "qty": 5, "price_tyiyn": 1_500 },
    ]);
    doc["lines"][0]["qty"] = json!(4);
    let right = doc.clone();
    // Администратору исправление недоступно.
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/corrections",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "kind": "stock_in", "original_id": id, "comment": "x", "doc": doc })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, fixed) = w
        .correct(Uuid::now_v7(), "stock_in", &id, right.clone())
        .await;
    assert_eq!(s, StatusCode::OK, "{fixed}");
    let diff = fixed["diff"].to_string();
    assert!(
        diff.contains("Хомут: количество")
            && diff.contains("Шайба: строка")
            && diff.contains("Болт: строка"),
        "{diff}"
    );
    let new_doc = get(
        &w.app,
        &format!("/api/v1/stock-ins/{}", fixed["new_id"].as_str().unwrap()),
        &w.owner,
    )
    .await;
    assert_eq!(new_doc["lines"].as_array().unwrap().len(), 2);
    assert_eq!(new_doc["cost_tyiyn"], 87_500);
    assert_as_if(&w, &s0, || async {
        let mut body = right.clone();
        body["op_id"] = json!(Uuid::now_v7());
        ok(&w.app, "POST", "/api/v1/stock-ins", &w.owner, body).await;
    })
    .await;
    w.verify_stock().await;
}
