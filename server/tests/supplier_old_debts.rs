//! Старый долг поставщику из тетради (задача 003 Н15, ADR-{g1.1}): наш долг растёт с даты долга,
//! оплата гасит его, как накладную (самые старые первыми), акт сверки, «Долги», сводка, сторно и
//! «Исправить» владельцем, право `old_debts`, повтор и `op_id`, филиалы, бот владельца. Деньги,
//! склад и прибыль не меняются.
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

use avtodom_server::api::bot_debtors::BTN_ADD_DEBT;
use avtodom_server::api::telegram::Reply;
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

/// Дата долга из тетради.
const FEB1: &str = "2026-02-01";

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

async fn call_in(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    branch: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string());
    if let Some(c) = cookie {
        req = req.header(header::COOKIE, c);
    }
    if let Some(b) = branch {
        req = req.header("x-branch-id", b.to_string());
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

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call_in(app, method, uri, cookie, None, body).await
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
    supplier: String,
    item: String,
    today: NaiveDate,
}

impl World {
    /// Владелец, администратор с правами по умолчанию, поставщик и товар.
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
        let s = ok(
            &app,
            "POST",
            "/api/v1/suppliers",
            &owner,
            json!({ "name": "Автоснаб" }),
        )
        .await;
        let supplier = s["id"].as_str().unwrap().to_string();
        let cat = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
        let p = ok(
            &app,
            "POST",
            "/api/v1/products",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 50_000 }),
        )
        .await;
        let item = p["id"].as_str().unwrap().to_string();
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
            supplier,
            item,
            today,
        }
    }

    async fn old_debt(&self, cookie: &str, body: Value) -> (StatusCode, Value) {
        let mut body = body;
        if body.get("op_id").is_none() {
            body["op_id"] = json!(Uuid::now_v7());
        }
        call(
            &self.app,
            "POST",
            "/api/v1/debts/old-supplier",
            Some(cookie),
            Some(body),
        )
        .await
    }

    /// Старый долг поставщику владельцем: ответ сервера.
    async fn owner_old_debt(&self, amount: i64, date: &str, doc: &str, comment: &str) -> Value {
        let (s, v) = self
            .old_debt(
                &self.owner,
                json!({ "party_id": self.supplier, "amount_tyiyn": amount, "date": date,
                        "supplier_doc": doc, "comment": comment }),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }

    /// Приход в долг на сумму `total` сегодня.
    async fn receipt_on_debt(&self, total: i64) -> String {
        let r = ok(
            &self.app,
            "POST",
            "/api/v1/receipts",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "supplier_id": self.supplier, "payment": "debt",
                    "lines": [{ "product_id": self.item, "qty": 1, "cost_tyiyn": total }] }),
        )
        .await;
        r["id"].as_str().unwrap().to_string()
    }

    /// Оплата поставщику владельцем переводом — без выбора накладной.
    async fn pay(&self, amount: i64) -> Value {
        ok(
            &self.app,
            "POST",
            "/api/v1/debts/repayments",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "party_id": self.supplier, "amount_tyiyn": amount,
                    "method": "transfer" }),
        )
        .await
    }

    async fn invoices(&self) -> Value {
        get(
            &self.app,
            &format!("/api/v1/parties/{}/invoices", self.supplier),
            &self.owner,
        )
        .await
    }

    async fn balance(&self) -> i64 {
        sqlx::query_scalar(
            "select coalesce((select balance_tyiyn from party_balances
                              where party_id = $1::uuid and branch_id = $2), 0)::bigint",
        )
        .bind(&self.supplier)
        .bind(self.branch)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// Владелец выдаёт администратору права: по умолчанию и `extra`.
    async fn grant(&self, extra: &[&str]) {
        let mut list: Vec<String> = avtodom_server::auth::default_permissions()
            .into_iter()
            .map(String::from)
            .collect();
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

    /// Деньги, склад и прибыль: что не должно меняться от старого долга.
    async fn untouched(&self) -> Vec<String> {
        let mut out = Vec::new();
        for sql in [
            "select count(*)::bigint from cash_movements",
            "select count(*)::bigint from stock_movements",
            "select count(*)::bigint from expenses",
            "select coalesce(sum(balance_tyiyn), 0)::bigint from cash_accounts",
        ] {
            let v: i64 = sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap();
            out.push(format!("{sql} = {v}"));
        }
        for (from, to) in [
            (FEB1.to_string(), FEB1.to_string()),
            (FEB1.to_string(), self.today.to_string()),
        ] {
            // «Прибыль и убытки» с раскрытием (задача 003 Н12): старый долг — не расход.
            let p = get(
                &self.app,
                &format!("/api/v1/reports/profit?from={from}&to={to}&detail=1"),
                &self.owner,
            )
            .await;
            out.push(format!("{from}..{to}: {} {}", p["totals"], p["detail"]));
        }
        out
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_supplier_debt_counts_from_its_date_and_moves_no_money(pool: PgPool) {
    let w = World::new(pool).await;
    let before = w.untouched().await;
    let out = w
        .owner_old_debt(1_200_000, FEB1, "№ 457", "масло, стр. 4")
        .await;
    assert_eq!(out["balance_tyiyn"], json!(-1_200_000));
    let (kind, doc, amount, date, comment, sdoc): (String, String, i64, NaiveDate, String, String) =
        sqlx::query_as(
            "select kind, doc_type, amount_tyiyn, business_date, comment, supplier_doc
             from party_ledger where id = $1::uuid",
        )
        .bind(out["ledger_id"].as_str().unwrap())
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(
        (
            kind.as_str(),
            doc.as_str(),
            amount,
            date,
            comment.as_str(),
            sdoc.as_str()
        ),
        (
            "debt",
            "old_debt",
            -1_200_000,
            day(FEB1),
            "Старый долг (из тетради): масло, стр. 4",
            "№ 457"
        )
    );
    // Деньги, склад, расходы и «Прибыль и убытки» — без изменений: это прошлый долг за товар.
    assert_eq!(w.untouched().await, before);

    // Карточка поставщика: долг из тетради с датой, номером накладной и кто занёс.
    let inv = w.invoices().await;
    assert_eq!(inv["debt_tyiyn"], json!(1_200_000));
    let row = &inv["invoices"][0];
    assert_eq!(row["kind"], json!("old_debt"));
    assert_eq!(row["business_date"], json!(FEB1));
    assert_eq!(row["supplier_doc"], json!("№ 457"));
    assert_eq!(row["comment"], json!("масло, стр. 4"));
    assert_eq!(row["user_name"], json!("Владелец"));
    assert_eq!(row["status"], json!("unpaid"));
    assert_eq!(row["receipt_id"], Value::Null);

    // «Долги → Должны мы» и сводка владельца: наш долг поставщику.
    let list = get(&w.app, "/api/v1/debts/list", &w.owner).await;
    let r = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(w.supplier))
        .unwrap();
    assert_eq!(r["balance_tyiyn"], json!(-1_200_000));
    let dash = get(&w.app, "/api/v1/owner/dashboard", &w.owner).await;
    assert_eq!(dash["debts_out_tyiyn"], json!(1_200_000));
    let card = get(&w.app, &format!("/api/v1/parties/{}", w.supplier), &w.owner).await;
    assert_eq!(card["balance_tyiyn"], json!(-1_200_000));

    // Журнал и уведомление владельцу.
    let feed = get(&w.app, "/api/v1/notifications?limit=20", &w.owner).await;
    let text = feed.to_string();
    assert!(
        text.contains("Старый долг поставщику: Автоснаб — 12 000 с от 01.02.2026 (из тетради)"),
        "{text}"
    );
    assert!(text.contains("накладная № 457"), "{text}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn payment_pays_old_supplier_debt_first_like_an_invoice(pool: PgPool) {
    let w = World::new(pool).await;
    // Накладная в долг сегодня и старый долг от 01.02: оплата без выбора накладной — сначала
    // старый долг (самые старые первыми, как между накладными).
    let receipt = w.receipt_on_debt(300_000).await;
    let old = w.owner_old_debt(500_000, FEB1, "", "").await;
    let old_id = old["ledger_id"].as_str().unwrap().to_string();
    assert_eq!(w.balance().await, -800_000);
    let pay = w.pay(600_000).await;
    assert_eq!(pay["balance_tyiyn"], json!(-200_000));
    let inv = w.invoices().await;
    let rows = inv["invoices"].as_array().unwrap();
    assert_eq!(rows[0]["ledger_id"], json!(old_id));
    assert_eq!(
        (&rows[0]["paid_tyiyn"], &rows[0]["status"]),
        (&json!(500_000), &json!("paid"))
    );
    assert_eq!(rows[1]["receipt_id"], json!(receipt));
    assert_eq!(
        (&rows[1]["paid_tyiyn"], &rows[1]["left_tyiyn"]),
        (&json!(100_000), &json!(200_000))
    );
    assert_eq!(inv["debt_tyiyn"], json!(200_000));
    let mut applied: Vec<(String, i64)> = inv["payments"][0]["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["ledger_id"].as_str().unwrap().to_string(),
                a["amount_tyiyn"].as_i64().unwrap(),
            )
        })
        .collect();
    applied.sort_by_key(|a| a.1);
    assert_eq!(
        applied,
        [
            (rows[1]["ledger_id"].as_str().unwrap().to_string(), 100_000),
            (old_id.clone(), 500_000)
        ]
    );
    // Накладная видит свою часть оплаты.
    let r = get(&w.app, &format!("/api/v1/receipts/{receipt}"), &w.owner).await;
    assert_eq!(r["debt"]["left_tyiyn"], json!(200_000));

    // Оплата — движение денег, не расход: «Прибыль и убытки» её не вычитает.
    let p = get(
        &w.app,
        &format!("/api/v1/reports/profit?from={}&to={}", w.today, w.today),
        &w.owner,
    )
    .await;
    assert_eq!(p["totals"]["expenses_tyiyn"], json!(0), "{}", p["totals"]);

    // Сторно старого долга владельцем: оплата ложится на накладную, остальное — аванс.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/debts/ledger/{old_id}/reverse"),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "comment": "ошибка в тетради" }),
    )
    .await;
    let inv = w.invoices().await;
    assert_eq!(inv["invoices"].as_array().unwrap().len(), 1);
    assert_eq!(inv["invoices"][0]["status"], json!("paid"));
    assert_eq!(inv["debt_tyiyn"], json!(0));
    assert_eq!(inv["advance_tyiyn"], json!(300_000));
    assert_eq!(w.balance().await, 300_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn reconciliation_shows_supplier_old_debt_on_its_date(pool: PgPool) {
    let w = World::new(pool).await;
    w.owner_old_debt(1_200_000, FEB1, "457", "стр. 4").await;
    w.pay(200_000).await;
    let act = get(
        &w.app,
        &format!("/api/v1/parties/{}/reconciliation", w.supplier),
        &w.owner,
    )
    .await;
    assert_eq!(act["from"], json!(FEB1));
    let row = &act["rows"][0];
    assert_eq!(row["date"], json!(FEB1));
    assert_eq!(
        row["document"],
        json!("Старый долг (из тетради) (док. поставщика 457)")
    );
    // В пользу поставщика: он привёз — мы должны.
    assert_eq!(
        (&row["debit_tyiyn"], &row["credit_tyiyn"]),
        (&json!(0), &json!(1_200_000))
    );
    assert_eq!(row["comment"], json!("стр. 4"));
    assert_eq!(act["closing_tyiyn"], json!(-1_000_000));
    // Сальдо на начало следующего периода его включает.
    let act = get(
        &w.app,
        &format!(
            "/api/v1/parties/{}/reconciliation?from=2026-03-01&to={}",
            w.supplier, w.today
        ),
        &w.owner,
    )
    .await;
    assert_eq!(act["opening_tyiyn"], json!(-1_200_000));
    assert_eq!(act["closing_tyiyn"], json!(-1_000_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn rights_repeats_op_id_and_dates(pool: PgPool) {
    let w = World::new(pool).await;
    let body = || json!({ "party_id": w.supplier, "amount_tyiyn": 300_000, "date": FEB1, "supplier_doc": "12" });
    // Администратор без права «Старые долги из тетради» — 403.
    let (s, v) = w.old_debt(&w.admin, body()).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert_eq!(message(&v), "Нет права: Старые долги из тетради");
    // С правом — проводится от его имени; повтор `op_id` — та же запись.
    w.grant(&["old_debts"]).await;
    let op = Uuid::now_v7();
    let mut b = body();
    b["op_id"] = json!(op);
    let (s, first) = w.old_debt(&w.admin, b.clone()).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    let (s, again) = w.old_debt(&w.admin, b).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(first["ledger_id"], again["ledger_id"]);
    assert_eq!(w.balance().await, -300_000);
    let who: Uuid = sqlx::query_scalar(
        "select user_id from party_ledger where party_id = $1::uuid and doc_type = 'old_debt'",
    )
    .bind(&w.supplier)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(who, w.admin_id);
    let n: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'debt.old_supplier'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Та же сумма и дата — 409 «Такой долг уже занесён …», с подтверждением — вторая запись.
    let (s, v) = w.old_debt(&w.admin, body()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        message(&v).starts_with("Такой долг уже занесён"),
        "{}",
        message(&v)
    );
    let mut b = body();
    b["confirm_duplicate"] = json!(true);
    let (s, _) = w.old_debt(&w.admin, b).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(w.balance().await, -600_000);
    // Сторно — только владелец.
    let (s, _) = call(
        &w.app,
        "POST",
        &format!(
            "/api/v1/debts/ledger/{}/reverse",
            first["ledger_id"].as_str().unwrap()
        ),
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Дата в будущем, раньше 10 лет, нулевая сумма, длинный номер накладной — 422.
    let tomorrow = (w.today + chrono::Duration::days(1)).to_string();
    for (amount, date, doc) in [
        (100, tomorrow.as_str(), ""),
        (100, "2010-01-01", ""),
        (0, FEB1, ""),
        (100, FEB1, &"1".repeat(101)[..]),
    ] {
        let (s, v) = w
            .old_debt(
                &w.owner,
                json!({ "party_id": w.supplier, "amount_tyiyn": amount, "date": date, "supplier_doc": doc }),
            )
            .await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{date} {amount}: {v}");
    }
    // Клиенту это окно долг не заносит.
    let client = ok(
        &w.app,
        "POST",
        "/api/v1/parties",
        &w.owner,
        json!({ "kind": "person", "first_name": "Пётр", "last_name": "Иванов" }),
    )
    .await;
    let (s, v) = w
        .old_debt(
            &w.owner,
            json!({ "party_id": client["id"], "amount_tyiyn": 100, "date": FEB1 }),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // А старый долг клиента — не поставщику.
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/debts/old",
        Some(&w.owner),
        Some(
            json!({ "op_id": Uuid::now_v7(), "party_id": w.supplier, "amount_tyiyn": 100, "date": FEB1 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(message(&v).contains("в его карточке"), "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_corrects_supplier_old_debt(pool: PgPool) {
    let w = World::new(pool).await;
    let out = w.owner_old_debt(500_000, FEB1, "457", "стр. 4").await;
    let id = out["ledger_id"].as_str().unwrap().to_string();
    let draft = get(
        &w.app,
        &format!("/api/v1/corrections/draft/ledger/{id}"),
        &w.owner,
    )
    .await;
    assert_eq!(
        draft["doc"],
        json!({ "type": "supplier_old_debt", "party_id": w.supplier, "amount_tyiyn": 500_000,
                "date": FEB1, "supplier_doc": "457", "comment": "стр. 4" })
    );
    let fixed = ok(
        &w.app,
        "POST",
        "/api/v1/corrections",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "kind": "ledger", "original_id": id, "comment": "не та сумма",
                "doc": { "type": "supplier_old_debt", "party_id": w.supplier, "amount_tyiyn": 450_000,
                         "date": "2026-02-03", "supplier_doc": "458", "comment": "стр. 4" } }),
    )
    .await;
    let what: Vec<&str> = fixed["diff"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["what"].as_str().unwrap())
        .collect();
    assert_eq!(what, ["Сумма", "Дата долга", "Накладная"]);
    assert_eq!(w.balance().await, -450_000);
    let inv = w.invoices().await;
    let rows = inv["invoices"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (&rows[0]["business_date"], &rows[0]["supplier_doc"]),
        (&json!("2026-02-03"), &json!("458"))
    );
    // Администратору «Исправить» не дают.
    w.grant(&["old_debts"]).await;
    let new_id = fixed["new_id"].as_str().unwrap();
    let (s, _) = call(
        &w.app,
        "GET",
        &format!("/api/v1/corrections/draft/ledger/{new_id}"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn other_branch_neither_sees_nor_pays_it(pool: PgPool) {
    let w = World::new(pool).await;
    let nb = ok(
        &w.app,
        "POST",
        "/api/v1/branches",
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
    )
    .await;
    let b = Uuid::parse_str(nb["id"].as_str().unwrap()).unwrap();
    w.owner_old_debt(700_000, FEB1, "", "").await;
    // В филиале Б долга перед поставщиком нет: свой долг у каждого филиала (SPEC-33).
    let (s, inv) = call_in(
        &w.app,
        "GET",
        &format!("/api/v1/parties/{}/invoices", w.supplier),
        Some(&w.owner),
        Some(b),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{inv}");
    assert_eq!(inv["invoices"], json!([]));
    assert_eq!(inv["debt_tyiyn"], json!(0));
    let (_, act) = call_in(
        &w.app,
        "GET",
        &format!("/api/v1/parties/{}/reconciliation", w.supplier),
        Some(&w.owner),
        Some(b),
        None,
    )
    .await;
    assert_eq!(act["rows"], json!([]), "{act}");
    // Тот же долг в Б — не повтор (другой филиал); оплата в Б гасит только долг Б.
    let (s, v) = call_in(
        &w.app,
        "POST",
        "/api/v1/debts/old-supplier",
        Some(&w.owner),
        Some(b),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": w.supplier, "amount_tyiyn": 700_000, "date": FEB1 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["balance_tyiyn"], json!(-700_000));
    let (s, v) = call_in(
        &w.app,
        "POST",
        "/api/v1/debts/repayments",
        Some(&w.owner),
        Some(b),
        Some(json!({ "op_id": Uuid::now_v7(), "party_id": w.supplier, "amount_tyiyn": 700_000, "method": "transfer" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(w.balance().await, -700_000);
    assert_eq!(w.invoices().await["debt_tyiyn"], json!(700_000));
    // Сторно записи филиала А из филиала Б — не найдено.
    let ledger: Uuid = sqlx::query_scalar(
        "select id from party_ledger where branch_id = $1 and doc_type = 'old_debt'",
    )
    .bind(w.branch)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    let (s, _) = call_in(
        &w.app,
        "POST",
        &format!("/api/v1/debts/ledger/{ledger}/reverse"),
        Some(&w.owner),
        Some(b),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "чужой" })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

// ---------- Бот владельца: «➕ Занести долг» → «🚚 Мы должны поставщику» ----------

impl World {
    async fn link_bot(&self, chat: i64, login: &str) {
        sqlx::query(
            "insert into telegram_links (chat_id, user_id, branch_id, tg_user_id)
             select $1, id, branch_id, $1 from users where login = $2",
        )
        .bind(chat)
        .bind(login)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn say(&self, chat: i64, text: &str) -> Reply {
        let mut conn = self.pool.acquire().await.unwrap();
        avtodom_server::api::telegram::handle_message(&mut conn, chat, chat, Some(text), None)
            .await
            .unwrap()
    }

    async fn press(&self, chat: i64, data: &str) -> Reply {
        let mut conn = self.pool.acquire().await.unwrap();
        avtodom_server::api::telegram::handle_callback_from(&mut conn, None, chat, chat, data)
            .await
            .unwrap()
    }
}

fn data_of(r: &Reply, text: &str) -> String {
    r.inline
        .iter()
        .flatten()
        .find(|b| b.text.starts_with(text))
        .unwrap_or_else(|| panic!("нет кнопки «{text}»: {:?}", r.inline))
        .data
        .clone()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_bot_enters_supplier_old_debt_step_by_step(pool: PgPool) {
    let w = World::new(pool).await;
    let chat = 96_001i64;
    w.link_bot(chat, "owner").await;
    let r = w.say(chat, BTN_ADD_DEBT).await;
    assert!(r.text.contains("Кто должен?"), "{}", r.text);
    let r = w.press(chat, &data_of(&r, "🚚 Мы должны поставщику")).await;
    assert!(r.text.contains("название поставщика"), "{}", r.text);
    let r = w.say(chat, "Нетакого").await;
    assert!(r.text.starts_with("Не нашли поставщика"), "{}", r.text);
    let r = w.say(chat, "Автос").await;
    let r = w.press(chat, &data_of(&r, "Автоснаб")).await;
    assert!(r.text.contains("Сколько мы должны"), "{}", r.text);
    let r = w.say(chat, "12 000").await;
    assert!(r.text.contains("когда получили товар"), "{}", r.text);
    let r = w.say(chat, "01.02.2026").await;
    assert!(r.text.contains("Номер накладной"), "{}", r.text);
    let r = w.say(chat, "№ 457").await;
    assert!(r.text.contains("Примечание"), "{}", r.text);
    let r = w.say(chat, "масло").await;
    assert!(
        r.text.starts_with("Занести старый долг поставщику?"),
        "{}",
        r.text
    );
    assert!(r.text.contains("Накладная: № 457"), "{}", r.text);
    assert!(
        r.text
            .contains("Наш долг поставщику сейчас 0 с → станет 12 000 с"),
        "{}",
        r.text
    );
    let r = w.press(chat, "od:go").await;
    assert!(
        r.text
            .starts_with("✅ Занесено: долг поставщику Автоснаб — 12 000 с от 01.02.2026"),
        "{}",
        r.text
    );
    assert_eq!(w.balance().await, -1_200_000);
    // Повтор нажатия — не второй долг.
    let r = w.press(chat, "od:go").await;
    assert!(r.text.contains("устарела"), "{}", r.text);
    let (doc, date): (String, NaiveDate) = sqlx::query_as(
        "select supplier_doc, business_date from party_ledger where doc_type = 'old_debt'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!((doc.as_str(), date), ("№ 457", day(FEB1)));

    // Тот же долг ещё раз — «уже занесён», второе подтверждение; без накладной — «Пропустить».
    let r = w.say(chat, BTN_ADD_DEBT).await;
    w.press(chat, &data_of(&r, "🚚")).await;
    let r = w.say(chat, "Автоснаб").await;
    w.press(chat, &data_of(&r, "Автоснаб")).await;
    w.say(chat, "12000").await;
    w.say(chat, "01.02.2026").await;
    w.press(chat, "od:skip").await;
    w.press(chat, "od:skip").await;
    let r = w.press(chat, "od:go").await;
    assert!(r.text.starts_with("Такой долг уже занесён"), "{}", r.text);
    let r = w.press(chat, &data_of(&r, "Да, занести")).await;
    assert!(r.text.starts_with("✅ Занесено"), "{}", r.text);
    assert_eq!(w.balance().await, -2_400_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_bot_supplier_debt_needs_the_right(pool: PgPool) {
    let w = World::new(pool).await;
    let chat = 96_002i64;
    w.link_bot(chat, "admin").await;
    let r = w.say(chat, BTN_ADD_DEBT).await;
    assert!(r.text.starts_with("Нет права"), "{}", r.text);
    w.grant(&["old_debts"]).await;
    let r = w.say(chat, BTN_ADD_DEBT).await;
    w.press(chat, &data_of(&r, "🚚")).await;
    let r = w.say(chat, "Автоснаб").await;
    w.press(chat, &data_of(&r, "Автоснаб")).await;
    w.say(chat, "2 500").await;
    w.press(chat, "od:today").await;
    w.press(chat, "od:skip").await;
    w.press(chat, "od:skip").await;
    // Право сняли перед «Провести» — не проводится.
    let mut list: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .map(String::from)
        .collect();
    list.retain(|p| p != "old_debts");
    ok(
        &w.app,
        "PUT",
        &format!("/api/v1/users/{}/permissions", w.admin_id),
        &w.owner,
        json!({ "op_id": Uuid::now_v7(), "permissions": list }),
    )
    .await;
    let r = w.press(chat, "od:go").await;
    assert!(r.text.starts_with("Нет права"), "{}", r.text);
    assert_eq!(w.balance().await, 0);
}
