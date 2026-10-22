//! Optima Business (SPEC-31, ADR-106): QR на сумму чека, опрос статуса, чек со ссылкой на QR,
//! «QR оплачен, чек не проведён», выписка, сверка и поправка комиссии. Банк — подставной
//! HTTP-сервер здесь же; настоящий в тестах не вызывается.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::optima::{Optima, bishkek};
use avtodom_server::api::{bank_qr, bank_recon};
use avtodom_server::config::SecretKey;
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const KEY: &str = "test-optima-key-7c2d";

// ---------- Подставной банк ----------

#[derive(Clone, Default)]
struct Bank {
    /// Запросы: путь, ключ из заголовка, тело.
    seen: Arc<Mutex<Vec<(String, String, String)>>>,
    /// Ответ статуса по номеру транзакции: код и тело; нет — 404.
    status: Arc<Mutex<HashMap<String, (u16, String)>>>,
    /// Тело выписки — текстом, суммы как у банка («990.00»).
    statement: Arc<Mutex<String>>,
    generated: Arc<Mutex<u32>>,
}

impl Bank {
    fn set_status(&self, tx: &str, code: u16, body: &str) {
        self.status
            .lock()
            .unwrap()
            .insert(tx.to_string(), (code, body.to_string()));
    }
    fn set_operations(&self, ops: &[String]) {
        *self.statement.lock().unwrap() = format!(
            r#"{{"statementCreatedDateTime":"2026-02-27T09:48:57.42530573","currency":"KGS","totalCommissionSum":0.00,"posOperations":[{}]}}"#,
            ops.join(",")
        );
    }
    fn calls(&self, path_part: &str) -> Vec<(String, String, String)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _, _)| p.contains(path_part))
            .cloned()
            .collect()
    }
}

fn key_ok(b: &Bank, path: String, headers: &HeaderMap, body: String) -> bool {
    let key = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let ok = key == KEY;
    b.seen.lock().unwrap().push((path, key, body));
    ok
}

fn denied() -> (StatusCode, String) {
    (
        StatusCode::UNAUTHORIZED,
        json!({ "error": "Invalid API key", "code": 401 }).to_string(),
    )
}

async fn points(
    State(b): State<Bank>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> (StatusCode, String) {
    if !key_ok(&b, format!("points/{id}"), &headers, String::new()) {
        return denied();
    }
    (
        StatusCode::OK,
        r#"[{"account":"1091820100040101","salePointInfoDtoList":[{"name":"Точка А","address":"Бишкек","code":101,"cashDtoList":[{"name":"Касса 1","code":1001},{"name":"Касса 2","code":"1002"}]}]}]"#.into(),
    )
}

async fn generate(State(b): State<Bank>, headers: HeaderMap, body: String) -> (StatusCode, String) {
    if !key_ok(&b, "generate".into(), &headers, body) {
        return denied();
    }
    let n = {
        let mut g = b.generated.lock().unwrap();
        *g += 1;
        *g
    };
    (
        StatusCode::OK,
        json!({ "qrBase64": "iVBORw0KGgo=", "qrUrl": "https://pay.example/qr", "transactionId": format!("tx-{n}") })
            .to_string(),
    )
}

async fn qr_status(
    State(b): State<Bank>,
    Path(tx): Path<String>,
    headers: HeaderMap,
) -> (StatusCode, String) {
    if !key_ok(&b, format!("status/{tx}"), &headers, String::new()) {
        return denied();
    }
    match b.status.lock().unwrap().get(&tx) {
        Some((code, body)) => (StatusCode::from_u16(*code).unwrap(), body.clone()),
        None => (
            StatusCode::NOT_FOUND,
            json!({ "error": "Operation not found by transactionId", "code": "1003" }).to_string(),
        ),
    }
}

async fn statement(
    State(b): State<Bank>,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
    headers: HeaderMap,
) -> (StatusCode, String) {
    if !key_ok(
        &b,
        format!("statement?{}", q.unwrap_or_default()),
        &headers,
        String::new(),
    ) {
        return denied();
    }
    (StatusCode::OK, b.statement.lock().unwrap().clone())
}

async fn fake_bank() -> (String, Bank) {
    let bank = Bank::default();
    bank.set_operations(&[]);
    let router = Router::new()
        .route(
            "/api/v2/get-sale-point-infos/{id}",
            axum::routing::get(points),
        )
        .route("/api/v2/generate/qr", axum::routing::post(generate))
        .route(
            "/api/v2/get-qr-transaction-info/{tx}",
            axum::routing::get(qr_status),
        )
        .route(
            "/api/v1/get-pos-operation-statement",
            axum::routing::get(statement),
        )
        .with_state(bank.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), bank)
}

// ---------- Программа ----------

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    bank_account: String,
    product: String,
    optima: Option<Optima>,
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

impl World {
    async fn new(pool: PgPool, optima: Option<Optima>) -> Self {
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
        let mut state = build_state(pool.clone(), false).await.unwrap();
        state.optima = optima.clone();
        let app = app(state, None);
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
        let bank_account = find(&|a| a["kind"] == json!("bank"));
        let safe = find(&|a| a["owner_only"] == json!(true));
        ok(
            &app,
            "POST",
            "/api/v1/shifts/open",
            &admin,
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
        )
        .await;
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
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 10_000 }),
        )
        .await;
        let product = p["id"].as_str().unwrap().to_string();
        ok(
            &app,
            "POST",
            "/api/v1/receipts",
            &owner,
            json!({
                "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": product, "qty": 500, "cost_tyiyn": 500 * 1_000 }]
            }),
        )
        .await;
        World {
            app,
            pool,
            owner,
            admin,
            cashier,
            bank_account,
            product,
            optima,
        }
    }

    async fn configure(&self) {
        ok(
            &self.app,
            "PUT",
            "/api/v1/settings/optima",
            &self.owner,
            json!({ "legal_party_id": 248, "account_number": "1091820100040101",
                    "sale_point_code": 101, "sale_point_name": "Точка А",
                    "cash_code": 1001, "cash_name": "Касса 1" }),
        )
        .await;
    }

    /// Чек на сумму `amount` (одна строка), оплата картой; `qr` — ссылка на QR банка.
    async fn card_sale(&self, amount: i64, qr: Option<&str>) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/sales",
            Some(&self.admin),
            Some(json!({
                "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
                "lines": [{ "kind": "piece", "product_id": self.product, "qty": 1, "unit_price_tyiyn": amount }],
                "payments": [{ "method": "card", "amount_tyiyn": amount, "bank_qr_id": qr }]
            })),
        )
        .await
    }

    async fn new_qr(&self, op_id: Uuid, amount: i64) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/bank-qr",
            Some(&self.admin),
            Some(json!({ "op_id": op_id, "amount_tyiyn": amount })),
        )
        .await
    }

    async fn poll(&self) {
        bank_qr::poll_once(&self.pool, self.optima.as_ref().unwrap())
            .await
            .unwrap();
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by created_at, id")
            .bind(action)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

async fn with_bank(pool: PgPool) -> (World, Bank) {
    let (url, bank) = fake_bank().await;
    let optima = Optima::new(SecretKey(KEY.into()), Some(url));
    (World::new(pool, Some(optima)).await, bank)
}

fn paid(tx: &str, sum: &str, op: &str) -> String {
    format!(
        r#"{{"note":"Оплата, чек кассы Avtodom","sum":{sum},"status":"PROCESSED","transactionProcessedDateTime":"{}","transactionId":"{tx}","operationId":"{op}","payerClientType":""}}"#,
        Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ")
    )
}

/// Время банка — время точки без зоны.
fn bank_time(shift_min: i64) -> String {
    (Utc::now() + Duration::minutes(shift_min))
        .with_timezone(&bishkek())
        .format("%Y-%m-%dT%H:%M:%S")
        .to_string()
}

fn pos_op(time: &str, sum: &str, fee: &str, rrn: &str, extra: &str) -> String {
    format!(
        r#"{{"operationProcessedDateTime":"{time}","operationTransferDate":"2026-10-22","posTerminalCode":"PR900482","posTerminalName":null,"authCode":"530248","requisite":"499001******6776","operationSum":{sum},"operationFeePercent":1.00,"operationFeeSum":{fee},"operationTransferSum":{sum},"currency":"KGS","departmentName":"","marketAddress":"","rrn":"{rrn}"{extra}}}"#
    )
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn without_key_nothing_is_visible_or_called(pool: PgPool) {
    let w = World::new(pool, None).await;
    let st = get(&w.app, "/api/v1/optima/status", &w.admin).await;
    assert_eq!(st, json!({ "enabled": false, "qr": false }));
    let s = get(&w.app, "/api/v1/settings/optima", &w.owner).await;
    assert_eq!(s["enabled"], false);
    let (code, _) = w.new_qr(Uuid::now_v7(), 10_000).await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    let today = Utc::now().with_timezone(&bishkek()).date_naive();
    let (code, _) = call(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        Some(&w.owner),
        Some(json!({ "from": today, "to": today })),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
    // Касса — как раньше: оплата картой ручной отметкой.
    let (code, sale) = w.card_sale(10_000, None).await;
    assert_eq!(code, StatusCode::OK, "{sale}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn qr_for_sale_amount_paid_and_used_once(pool: PgPool) {
    let (w, bank) = with_bank(pool).await;
    // Настройка — только владельцу.
    let (code, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/optima",
        Some(&w.admin),
        Some(json!({ "legal_party_id": 248 })),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&w.app, "/api/v1/optima/status", &w.admin).await,
        json!({ "enabled": true, "qr": false })
    );
    let points = get(
        &w.app,
        "/api/v1/settings/optima/sale-points?legal_party_id=248",
        &w.owner,
    )
    .await;
    assert_eq!(points[0]["code"], 101);
    assert_eq!(points[0]["cashes"][1]["code"], 1002);
    w.configure().await;
    assert_eq!(
        get(&w.app, "/api/v1/optima/status", &w.admin).await,
        json!({ "enabled": true, "qr": true })
    );
    let check = ok(
        &w.app,
        "POST",
        "/api/v1/settings/optima/check",
        &w.owner,
        json!({}),
    )
    .await;
    assert_eq!(check["ok"], true, "{check}");

    // QR на сумму к оплате картой: точка, касса, компания, сумма с двумя знаками.
    let op = Uuid::now_v7();
    let (code, qr) = w.new_qr(op, 150_050).await;
    assert_eq!(code, StatusCode::OK, "{qr}");
    assert_eq!(qr["status"], "waiting");
    assert_eq!(qr["qr_base64"], "iVBORw0KGgo=");
    assert!(qr["seconds_left"].as_i64().unwrap() > 590);
    let sent = bank.calls("generate");
    assert_eq!(sent.len(), 1);
    let (_, key, body) = &sent[0];
    assert_eq!(key, KEY);
    assert!(body.contains(r#""sum":1500.50"#), "{body}");
    let b: Value = serde_json::from_str(body).unwrap();
    assert_eq!(
        b["requisite"],
        json!({ "salePointCode": 101, "cashCode": 1001, "legalPartyId": 248 })
    );
    assert_eq!(b["qrGenerateType"], "WEB_PARTNER_BY_SALE_POINT");
    assert_eq!(b["transactionCount"], 1);
    assert_eq!(b["qrType"], "png");
    assert_eq!(b["qrSize"], 400);
    assert_eq!(b["note"], "Оплата, чек кассы Avtodom");
    // Повтор с тем же op_id — тот же QR, в банк второй раз не идём.
    let (_, again) = w.new_qr(op, 150_050).await;
    assert_eq!(again["id"], qr["id"]);
    assert_eq!(bank.calls("generate").len(), 1);
    let id = qr["id"].as_str().unwrap().to_string();

    // Пока не оплачен — 404 банка, ждём; чек с ним не проводится.
    w.poll().await;
    let st = get(&w.app, &format!("/api/v1/bank-qr/{id}"), &w.admin).await;
    assert_eq!(st["status"], "waiting");
    let (code, v) = w.card_sale(150_050, Some(&id)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    bank.set_status("tx-1", 200, &paid("tx-1", "1500.50", "op-1"));
    w.poll().await;
    let st = get(&w.app, &format!("/api/v1/bank-qr/{id}"), &w.admin).await;
    assert_eq!(st["status"], "paid", "{st}");
    assert!(st.get("qr_base64").is_none());
    assert_eq!(w.audit("bank_qr.paid").await.len(), 1);
    // Оплаченный не отменяется.
    let (code, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/bank-qr/{id}/cancel"),
        Some(&w.admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(code, StatusCode::CONFLICT);

    // Чужая сумма — 409; верная — чек проведён, деньги на счёт Optima.
    let (code, v) = w.card_sale(150_000, Some(&id)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    let (code, sale) = w.card_sale(150_050, Some(&id)).await;
    assert_eq!(code, StatusCode::OK, "{sale}");
    assert_eq!(sale["payments"][0]["account_id"], json!(w.bank_account));
    let (linked, qr_link): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "select q.sale_id, p.bank_qr_id from bank_qr q join sale_payments p on p.bank_qr_id = q.id where q.id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        linked.map(|x| x.to_string()),
        sale["id"].as_str().map(String::from)
    );
    assert_eq!(qr_link.map(|x| x.to_string()), Some(id.clone()));
    // Повторно — 409.
    let (code, v) = w.card_sale(150_050, Some(&id)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("другой чек"),
        "{v}"
    );

    // Истёкший: банк отвечает NOT_EXIST — «не оплачен», чек с ним — 409.
    let (_, qr2) = w.new_qr(Uuid::now_v7(), 20_000).await;
    let id2 = qr2["id"].as_str().unwrap().to_string();
    bank.set_status(
        "tx-2",
        200,
        r#"{"status":"NOT_EXIST","transactionId":"tx-2"}"#,
    );
    w.poll().await;
    assert_eq!(
        get(&w.app, &format!("/api/v1/bank-qr/{id2}"), &w.admin).await["status"],
        "waiting"
    );
    sqlx::query("update bank_qr set expires_at = now() - interval '1 second' where id = $1::uuid")
        .bind(&id2)
        .execute(&w.pool)
        .await
        .unwrap();
    w.poll().await;
    assert_eq!(
        get(&w.app, &format!("/api/v1/bank-qr/{id2}"), &w.admin).await["status"],
        "unpaid"
    );
    let (code, v) = w.card_sale(20_000, Some(&id2)).await;
    assert_eq!(code, StatusCode::CONFLICT, "{v}");

    // Банк отклонил — «ошибка»; «Отменить» — больше не ждём и не спрашиваем банк.
    let (_, qr3) = w.new_qr(Uuid::now_v7(), 30_000).await;
    bank.set_status("tx-3", 200, r#"{"status":"ERROR","transactionId":"tx-3"}"#);
    w.poll().await;
    assert_eq!(
        get(
            &w.app,
            &format!("/api/v1/bank-qr/{}", qr3["id"].as_str().unwrap()),
            &w.admin
        )
        .await["status"],
        "error"
    );
    let (_, qr4) = w.new_qr(Uuid::now_v7(), 40_000).await;
    let id4 = qr4["id"].as_str().unwrap();
    let c = ok(
        &w.app,
        "POST",
        &format!("/api/v1/bank-qr/{id4}/cancel"),
        &w.admin,
        json!({}),
    )
    .await;
    assert_eq!(c["status"], "cancelled");
    let before = bank.calls("status/tx-4").len();
    w.poll().await;
    assert_eq!(bank.calls("status/tx-4").len(), before);
    assert_eq!(w.audit("bank_qr.cancel").await.len(), 1);
    assert_eq!(w.audit("bank_qr.create").await.len(), 4);

    // Без QR — как раньше.
    let (code, v) = w.card_sale(10_000, None).await;
    assert_eq!(code, StatusCode::OK, "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn paid_qr_without_sale_alerts_owner(pool: PgPool) {
    let (w, bank) = with_bank(pool).await;
    w.configure().await;
    let (_, qr) = w.new_qr(Uuid::now_v7(), 99_000).await;
    let id = qr["id"].as_str().unwrap().to_string();
    bank.set_status("tx-1", 200, &paid("tx-1", "990.00", "op-1"));
    w.poll().await;
    // Строка в отчёте смены — сразу.
    let shift = get(&w.app, "/api/v1/shifts/current", &w.owner).await;
    let report = get(
        &w.app,
        &format!("/api/v1/shifts/{}/report", shift["id"].as_str().unwrap()),
        &w.admin,
    )
    .await;
    assert_eq!(
        report["qr_unclaimed"][0]["amount_tyiyn"], 99_000,
        "{}",
        report["qr_unclaimed"]
    );
    // Через 15 минут без чека — владельцу один раз.
    bank_qr::alert_unclaimed(&w.pool).await.unwrap();
    assert!(w.audit("bank_qr.unclaimed").await.is_empty());
    sqlx::query("update bank_qr set paid_at = now() - interval '16 minutes' where id = $1::uuid")
        .bind(&id)
        .execute(&w.pool)
        .await
        .unwrap();
    bank_qr::alert_unclaimed(&w.pool).await.unwrap();
    bank_qr::alert_unclaimed(&w.pool).await.unwrap();
    assert_eq!(w.audit("bank_qr.unclaimed").await.len(), 1);
    let n = get(&w.app, "/api/v1/notifications", &w.owner).await;
    assert!(
        n["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["title"] == "QR оплачен, чек не проведён: 990 с"),
        "{n}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn statement_matches_and_fixes_real_fee(pool: PgPool) {
    let (w, bank) = with_bank(pool).await;
    w.configure().await;
    let today = Utc::now().with_timezone(&bishkek()).date_naive();
    let yesterday = today - Duration::days(1);
    // Карта 990 с (комиссия по ставке 0,5 % — 4,95 с) и QR на 500 с (2,50 с).
    let (code, _) = w.card_sale(99_000, None).await;
    assert_eq!(code, StatusCode::OK);
    let (_, qr) = w.new_qr(Uuid::now_v7(), 50_000).await;
    bank.set_status("tx-1", 200, &paid("tx-1", "500", "jfu-g5iu-77"));
    w.poll().await;
    let (code, v) = w.card_sale(50_000, qr["id"].as_str()).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    let profit = |w: &World| {
        let app = w.app.clone();
        let owner = w.owner.clone();
        async move {
            get(
                &app,
                &format!("/api/v1/reports/profit?from={today}&to={today}"),
                &owner,
            )
            .await["totals"]["bank_fee_tyiyn"]
                .as_i64()
                .unwrap()
        }
    };
    let fee0 = profit(&w).await;
    assert_eq!(fee0, 495 + 250);

    // Банк: карта — 9,90 с комиссии; QR — по номеру операции, время другое.
    let card = pos_op(&bank_time(1), "990.00", "9.90", "111111111111", "");
    let qr_op = pos_op(
        &bank_time(-14),
        "500.00",
        "2.50",
        "",
        r#","stan":"jfu-g5iu-77""#,
    );
    bank.set_operations(&[card.clone(), qr_op.clone()]);
    // Админу сверка не доступна.
    let (code, _) = call(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        Some(&w.admin),
        Some(json!({ "from": today, "to": today })),
    )
    .await;
    assert_eq!(code, StatusCode::FORBIDDEN);
    let r = ok(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        &w.owner,
        json!({ "from": today, "to": today }),
    )
    .await;
    assert_eq!(r["run"]["operations"], 2, "{r}");
    assert_eq!(r["run"]["new_operations"], 2);
    assert_eq!(r["run"]["adjustments"], 1, "{r}");
    assert_eq!(r["run"]["adjusted_tyiyn"], 495);
    assert_eq!(r["ours_unmatched"], json!([]), "{r}");
    assert_eq!(r["bank_unmatched"], json!([]), "{r}");
    let day = &r["days"][0];
    assert_eq!(day["ours_count"], 2);
    assert_eq!(day["bank_tyiyn"], 149_000);
    assert_eq!(day["bank_fee_tyiyn"], 1_240);
    assert_eq!(day["diff_tyiyn"], 0);
    // Суммы банка — в тыйынах, без дробей.
    let stored: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "select amount_tyiyn, fee_tyiyn, transfer_tyiyn, kind from bank_operations order by amount_tyiyn",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        vec![
            (50_000, 250, 50_000, "card".into()),
            (99_000, 990, 99_000, "card".into())
        ]
    );
    let url = bank.calls("statement")[0].0.clone();
    assert!(url.contains("accountNumber=1091820100040101"), "{url}");
    // Прибыль учитывает поправку; на счёте — движение «комиссия банка».
    assert_eq!(profit(&w).await, fee0 + 495);
    let moves: Vec<(String, i64, String)> = sqlx::query_as(
        "select kind, amount_tyiyn, comment from cash_movements where doc_type = 'bank_fee_adjust'",
    )
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(moves.len(), 1);
    assert_eq!((moves[0].0.as_str(), moves[0].1), ("bank_fee", -495));
    assert!(
        moves[0]
            .2
            .starts_with("Комиссия банка: поправка по выписке за "),
        "{}",
        moves[0].2
    );
    assert_eq!(w.audit("bank_fee.adjust").await.len(), 1);
    assert_eq!(w.audit("bank_statement.load").await.len(), 1);

    // Повтор — не дублирует и не проводит второй раз.
    let r = ok(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        &w.owner,
        json!({ "from": today, "to": today }),
    )
    .await;
    assert_eq!(r["run"]["new_operations"], 0);
    assert_eq!(r["run"]["adjustments"], 0);
    assert_eq!(profit(&w).await, fee0 + 495);

    // Новая оплата: повторная сверка проводит только новую разницу.
    let (code, _) = w.card_sale(20_000, None).await;
    assert_eq!(code, StatusCode::OK);
    let third = pos_op(&bank_time(0), "200", "3.00", "222222222222", "");
    bank.set_operations(&[card.clone(), qr_op.clone(), third.clone()]);
    let r = ok(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        &w.owner,
        json!({ "from": today, "to": today }),
    )
    .await;
    assert_eq!(r["run"]["new_operations"], 1);
    assert_eq!(r["run"]["adjusted_tyiyn"], 200, "{r}");
    assert_eq!(profit(&w).await, fee0 + 100 + 495 + 200);

    // Расхождения в обе стороны: оплата без операции банка и операция без оплаты — день не
    // сошёлся, комиссию не правим.
    let (code, _) = w.card_sale(30_000, None).await;
    assert_eq!(code, StatusCode::OK);
    let stray = pos_op(&bank_time(0), "777.00", "7.77", "333333333333", "");
    let old = format!(
        r#"{{"operationProcessedDateTime":"{}T12:00:00","operationSum":45.5,"operationFeeSum":0.23,"operationTransferSum":45.27,"rrn":"444444444444"}}"#,
        yesterday
    );
    bank.set_operations(&[card, qr_op, third, stray, old]);
    let r = ok(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        &w.owner,
        json!({ "from": yesterday, "to": today }),
    )
    .await;
    assert_eq!(r["run"]["adjustments"], 0, "{r}");
    let ours: Vec<i64> = r["ours_unmatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["amount_tyiyn"].as_i64().unwrap())
        .collect();
    let theirs: Vec<i64> = r["bank_unmatched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["amount_tyiyn"].as_i64().unwrap())
        .collect();
    assert_eq!(ours, vec![30_000]);
    assert_eq!(theirs, vec![4_550, 77_700]);
    // Экран без запроса в банк показывает то же.
    let calls = bank.calls("statement").len();
    let g = get(
        &w.app,
        &format!("/api/v1/bank/reconcile?from={yesterday}&to={today}"),
        &w.owner,
    )
    .await;
    assert_eq!(g["bank_unmatched"].as_array().unwrap().len(), 2);
    assert_eq!(bank.calls("statement").len(), calls);
    // Расхождение прошедшего дня — владельцу (сегодняшний день ещё идёт).
    let m = w.audit("bank_recon.mismatch").await;
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0]["day"], json!(yesterday));
    assert_eq!(m[0]["bank_count"], 1);
    // Повтор того же расхождения — без второго уведомления.
    ok(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        &w.owner,
        json!({ "from": yesterday, "to": today }),
    )
    .await;
    assert_eq!(w.audit("bank_recon.mismatch").await.len(), 1);
    // Не больше 14 дней.
    let (code, _) = call(
        &w.app,
        "POST",
        "/api/v1/bank/reconcile",
        Some(&w.owner),
        Some(json!({ "from": today - Duration::days(14), "to": today })),
    )
    .await;
    assert_eq!(code, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn daily_reconcile_runs_once_after_six(pool: PgPool) {
    let (w, bank) = with_bank(pool).await;
    w.configure().await;
    let optima = w.optima.clone().unwrap();
    // 05:59 по Бишкеку — рано; 06:00 — сверка трёх прошедших дней, второй раз за день — нет.
    let early = chrono::DateTime::parse_from_rfc3339("2026-10-22T05:59:00+06:00")
        .unwrap()
        .with_timezone(&Utc);
    bank_recon::daily(&w.pool, &optima, early).await;
    assert!(bank.calls("statement").is_empty());
    let six = chrono::DateTime::parse_from_rfc3339("2026-10-22T06:00:00+06:00")
        .unwrap()
        .with_timezone(&Utc);
    bank_recon::daily(&w.pool, &optima, six).await;
    let calls = bank.calls("statement");
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .0
            .contains("startDate=2026-10-19&endDate=2026-10-21"),
        "{}",
        calls[0].0
    );
    bank_recon::daily(&w.pool, &optima, six + Duration::hours(1)).await;
    assert_eq!(bank.calls("statement").len(), 1);
    let load = w.audit("bank_statement.load").await;
    assert_eq!(load.len(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn wrong_key_tells_owner(pool: PgPool) {
    let (url, bank) = fake_bank().await;
    let optima = Optima::new(SecretKey("чужой-ключ".into()), Some(url));
    let w = World::new(pool, Some(optima)).await;
    w.configure().await;
    let (code, v) = w.new_qr(Uuid::now_v7(), 10_000).await;
    assert_eq!(code, StatusCode::FAILED_DEPENDENCY, "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Ключ Optima Business не подходит")
    );
    assert_eq!(bank.calls("generate").len(), 1);
    let (code, _) = w.new_qr(Uuid::now_v7(), 10_000).await;
    assert_eq!(code, StatusCode::FAILED_DEPENDENCY);
    // Владельцу — один раз в час.
    assert_eq!(w.audit("optima.key").await.len(), 1);
    let n = get(&w.app, "/api/v1/notifications", &w.owner).await;
    assert!(
        n["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["title"] == "⚠️ Ключ Optima Business не подходит"),
        "{n}"
    );
    // Банк недоступен — касса отмечает оплату вручную.
    let dead = Optima::new(SecretKey(KEY.into()), Some("http://127.0.0.1:9".into()));
    let w2 = World {
        app: app(
            {
                let mut s = build_state(w.pool.clone(), false).await.unwrap();
                s.optima = Some(dead);
                s
            },
            None,
        ),
        optima: None,
        ..w
    };
    let (code, v) = w2.new_qr(Uuid::now_v7(), 10_000).await;
    assert_eq!(code, StatusCode::FAILED_DEPENDENCY, "{v}");
    assert_eq!(
        v["error"]["message"],
        "Банк недоступен — отметьте оплату вручную"
    );
}
