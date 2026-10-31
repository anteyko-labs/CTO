//! Накладная по фото (SPEC-30 §9, ADR-098): черновик прихода из фото, распознавание через
//! подставной сервис (настоящий в тестах не вызывается), подсказки «Нового прихода», фото
//! к накладной после проведения, без ключа, лимит часа, фото в боте.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::assistant::{Assistant, DEFAULT_INVOICE_MODEL};
use avtodom_server::api::receipt_drafts::{self, BotFile};
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const KEY: &str = "test-invoice-key-5b1e";
/// Начало JPEG и немного данных: сервер проверяет вид по первым байтам.
const JPEG: [u8; 12] = [0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1];

// ---------- Подставной сервис распознавания ----------

#[derive(Clone, Default)]
struct Fake {
    /// Ответы по очереди: код и тело; `Value::Null` — пустое тело.
    replies: Arc<Mutex<VecDeque<(u16, Value)>>>,
    seen: Arc<Mutex<Vec<(axum::http::HeaderMap, Value)>>>,
    paths: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn push(&self, code: u16, body: Value) {
        self.replies.lock().unwrap().push_back((code, body));
    }
    fn requests(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect()
    }
    fn key(&self, i: usize) -> String {
        self.seen.lock().unwrap()[i]
            .0
            .get("x-goog-api-key")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
    fn path(&self, i: usize) -> String {
        self.paths.lock().unwrap()[i].clone()
    }
}

async fn fake_messages(
    axum::extract::State(f): axum::extract::State<Fake>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, String) {
    f.paths.lock().unwrap().push(path);
    f.seen.lock().unwrap().push((headers, body));
    match f.replies.lock().unwrap().pop_front() {
        Some((code, Value::Null)) => (StatusCode::from_u16(code).unwrap(), String::new()),
        Some((code, v)) => (StatusCode::from_u16(code).unwrap(), v.to_string()),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            error(500, "INTERNAL").to_string(),
        ),
    }
}

/// Ошибка сервиса: `{"error": {"code", "message", "status"}}`.
fn error(code: u16, status: &str) -> Value {
    json!({ "error": { "code": code, "message": "test", "status": status } })
}

/// Подключение к подставному сервису; повторы — без пауз.
fn assistant(url: String) -> Assistant {
    Assistant::new(KEY.into(), Some(url), None, 100)
        .with_retry_pauses(vec![std::time::Duration::ZERO; 2])
}

async fn fake_service() -> (String, Fake) {
    let fake = Fake::default();
    let router = Router::new()
        .route("/v1beta/models/{*rest}", axum::routing::post(fake_messages))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

/// Ответ сервиса: размышление и текст — JSON накладной (с подписью размышления).
fn answer(invoice: &Value) -> Value {
    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [
                    { "text": "Читаю накладную.", "thought": true },
                    { "text": invoice.to_string(), "thoughtSignature": "sig" }
                ]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 1500, "candidatesTokenCount": 250, "thoughtsTokenCount": 50 }
    })
}

fn invoice() -> Value {
    json!({
        "readable": true,
        "supplier_name": "ОсОО \"Автоснаб\"",
        "supplier_inn": "01234567890123",
        "doc_number": "15",
        "doc_date": "2026-10-20",
        "lines": [
            // Штрихкод совпал.
            { "name": "Фильтр масляный W712/75", "code": "", "barcode": "4011558738001",
              "quantity": "2", "unit": "шт", "price": "350", "sum": "700" },
            // Артикул совпал; литры → канистры по 4 л.
            { "name": "Масло моторное 5W-30 бочка", "code": "MOB-5W30-4", "barcode": "",
              "quantity": "20", "unit": "л", "price": "450", "sum": "9000" },
            // Похожее название.
            { "name": "Свеча зажигания NGK BKR6E", "code": "", "barcode": "",
              "quantity": "4", "unit": "шт", "price": "250", "sum": "1000" },
            // Такого товара нет.
            { "name": "Жидкость тормозная DOT-4", "code": "", "barcode": "",
              "quantity": "1", "unit": "шт", "price": "500", "sum": "500" }
        ],
        "total": "11 200"
    })
}

// ---------- Программа ----------

struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    branch: Uuid,
    owner_id: Uuid,
    filter: Uuid,
    oil: Uuid,
    plug: Uuid,
}

async fn setup(pool: PgPool, assistant: Option<Assistant>) -> World {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let (branch, owner_id): (Uuid, Uuid) =
        sqlx::query_as("select branch_id, id from users where login = 'owner'")
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
    let (cat_f, cat_o, cat_x) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into categories (id, name, kind) values ($1, 'Фильтры', 'filter'), ($2, 'Масла', 'oil'), ($3, 'Прочее', 'other')")
        .bind(cat_f)
        .bind(cat_o)
        .bind(cat_x)
        .execute(&pool)
        .await
        .unwrap();
    let (filter, oil, plug) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "insert into products (id, category_id, name, brand, article, unit, container_ml) values
                 ($1, $4, 'Фильтр масляный Mann W712/75', 'Mann', 'W712/75', 'piece', null),
                 ($2, $5, 'Mobil Super 3000 5W-30 4л', 'Mobil', 'MOB 5W30 4', 'ml', 4000),
                 ($3, $6, 'Свеча NGK BKR6E', 'NGK', '', 'piece', null)",
    )
    .bind(filter)
    .bind(oil)
    .bind(plug)
    .bind(cat_f)
    .bind(cat_o)
    .bind(cat_x)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("insert into product_barcodes (code, product_id) values ('4011558738001', $1)")
        .bind(filter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into branch_products (branch_id, product_id, sale_price_tyiyn) values ($1, $2, 50000), ($1, $3, 250000), ($1, $4, 40000)")
        .bind(branch)
        .bind(filter)
        .bind(oil)
        .bind(plug)
        .execute(&pool)
        .await
        .unwrap();
    let mut state = build_state(pool.clone(), false).await.unwrap();
    state.assistant = assistant;
    let app = app(state, None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    World {
        app,
        pool,
        owner,
        admin,
        branch,
        owner_id,
        filter,
        oil,
        plug,
    }
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let cookie = res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::to_string);
    let bytes = res.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, cookie, bytes)
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
    let (status, _, bytes) = send(app, req).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn upload(app: &Router, cookie: &str, op_id: Uuid, data: &[u8]) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/receipt-drafts?op_id={op_id}&name=nakladnaya.jpg"
        ))
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(Body::from(data.to_vec()))
        .unwrap();
    let (status, _, bytes) = send(app, req).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "login": login, "password": password }).to_string(),
        ))
        .unwrap();
    let (status, cookie, _) = send(app, req).await;
    assert_eq!(status, StatusCode::OK);
    cookie.unwrap()
}

/// Поставщик «Автоснаб» с ИНН (или без).
async fn supplier(w: &World, name: &str, inn: &str) -> String {
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/suppliers",
        Some(&w.owner),
        Some(json!({ "name": name })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let id = v["id"].as_str().unwrap().to_string();
    sqlx::query("update parties set inn = $2 where id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .bind(inn)
        .execute(&w.pool)
        .await
        .unwrap();
    id
}

async fn link_owner_chat(w: &World, chat: i64) {
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $1)")
        .bind(chat)
        .bind(w.owner_id)
        .bind(w.branch)
        .execute(&w.pool)
        .await
        .unwrap();
}

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("select count(*) from audit_log where action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------- Экран ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn photo_draft_fills_receipt_and_photo_goes_to_it(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool, Some(assistant(url))).await;
    let supplier_id = supplier(&w, "Автоснаб", "0123-4567-8901-23").await;
    fake.push(200, answer(&invoice()));

    // Администратор присылает фото с экрана — распознано, подсказки заполнены.
    let op = Uuid::now_v7();
    let (s, d) = upload(&w.app, &w.admin, op, &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["recognized"], true, "{d}");
    assert_eq!(d["source"], "screen");
    assert_eq!(d["supplier"]["id"], supplier_id.as_str());
    assert_eq!(d["supplier"]["by"], "inn");
    assert_eq!(d["invoice"]["doc_number"], "15");
    assert_eq!(d["total_tyiyn"], 1_120_000);
    assert_eq!(d["lines_sum_tyiyn"], 1_120_000);
    let lines = d["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["match"], "exact");
    assert_eq!(lines[0]["product"]["id"], w.filter.to_string());
    assert_eq!(
        (lines[0]["count"].as_i64(), lines[0]["each_tyiyn"].as_i64()),
        (Some(2), Some(35_000))
    );
    // Артикул «MOB-5W30-4» = «MOB 5W30 4»; 20 л по 450 с → 5 канистр по 1 800 с.
    assert_eq!(lines[1]["match"], "exact");
    assert_eq!(lines[1]["product"]["id"], w.oil.to_string());
    assert_eq!(
        (lines[1]["count"].as_i64(), lines[1]["each_tyiyn"].as_i64()),
        (Some(5), Some(180_000))
    );
    assert!(
        lines[1]["note"]
            .as_str()
            .unwrap()
            .contains("20 л = 5 кан. по 4 л"),
        "{}",
        lines[1]
    );
    assert_eq!(lines[1]["qty_milli"], 20_000);
    assert_eq!(lines[1]["liters"], true);
    assert_eq!(lines[2]["match"], "similar");
    assert_eq!(lines[2]["product"]["id"], w.plug.to_string());
    assert_eq!(lines[3]["match"], "none");
    assert!(lines[3]["product"].is_null());
    // Администратор не видит себестоимость товаров (инвариант 6).
    assert!(lines[0]["product"].get("avg_cost_tyiyn").is_none());

    // Запрос к сервису: модель для накладных, снимок блоком image, ответ по схеме.
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 1);
    let r = &reqs[0];
    assert_eq!(
        fake.path(0),
        format!("{DEFAULT_INVOICE_MODEL}:generateContent")
    );
    assert_eq!(fake.key(0), KEY);
    assert!(
        r["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("накладн")
    );
    let block = &r["contents"][0]["parts"][1]["inline_data"];
    assert_eq!(block["mime_type"], "image/jpeg");
    assert!(block["data"].as_str().unwrap().starts_with("/9j/4A"));
    let config = &r["generationConfig"];
    assert_eq!(config["responseMimeType"], "application/json");
    assert_eq!(config["responseJsonSchema"]["type"], "object");
    assert_eq!(config["thinkingConfig"]["thinkingLevel"], "low");
    assert_eq!(config["responseJsonSchema"]["additionalProperties"], false);

    // Повтор той же загрузки — тот же черновик, сервис второй раз не спрашивают.
    let (s, again) = upload(&w.app, &w.admin, op, &JPEG).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], d["id"]);
    assert_eq!(fake.requests().len(), 1);

    // Список «Накладные по фото»: один черновик с поставщиком, строками и итогом.
    let (s, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["enabled"], true);
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["supplier_name"], "ОсОО \"Автоснаб\"");
    assert_eq!(items[0]["lines"], 4);
    assert_eq!(items[0]["total_tyiyn"], 1_120_000);
    let id = d["id"].as_str().unwrap().to_string();
    let req = Request::builder()
        .uri(format!("/api/v1/receipt-drafts/{id}/file"))
        .header(header::COOKIE, &w.owner)
        .body(Body::empty())
        .unwrap();
    let (s, _, bytes) = send(&w.app, req).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(bytes, JPEG.to_vec());

    // Приход по черновику: фото прикрепляется к накладной, черновик — «использован».
    let receipt_op = Uuid::now_v7();
    let body = json!({
        "op_id": receipt_op,
        "supplier_id": supplier_id,
        "payment": "debt",
        "supplier_doc": "15",
        "lines": [
            { "product_id": w.filter, "qty": 2, "cost_tyiyn": 70_000 },
            { "product_id": w.oil, "qty": 20_000, "cost_tyiyn": 900_000 },
            { "product_id": w.plug, "qty": 4, "cost_tyiyn": 100_000 }
        ],
        "draft_id": id,
    });
    let (s, receipt) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        Some(&w.admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{receipt}");
    let rid = receipt["id"].as_str().unwrap().to_string();
    let (s, files) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let files = files.as_array().unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0]["name"], "nakladnaya.jpg");
    assert_eq!(files[0]["content_type"], "image/jpeg");
    let fid = files[0]["id"].as_str().unwrap();
    let req = Request::builder()
        .uri(format!("/api/v1/receipt-files/{fid}"))
        .header(header::COOKIE, &w.owner)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&w.app, req).await.2, JPEG.to_vec());
    let (_, used) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipt-drafts/{id}"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(used["status"], "used");
    assert_eq!(used["receipt_id"], rid.as_str());
    assert_eq!(used["receipt_number"], receipt["number"]);
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    assert!(list["items"].as_array().unwrap().is_empty());
    // Фото не хранится дважды: у черновика осталась ссылка на файл накладной.
    let (data_null, file_id): (bool, Option<Uuid>) =
        sqlx::query_as("select data is null, file_id from receipt_drafts where id = $1")
            .bind(Uuid::parse_str(&id).unwrap())
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert!(data_null);
    assert_eq!(file_id.map(|f| f.to_string()).as_deref(), Some(fid));
    // И по черновику фото по-прежнему открывается.
    let req = Request::builder()
        .uri(format!("/api/v1/receipt-drafts/{id}/file"))
        .header(header::COOKIE, &w.owner)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&w.app, req).await.2, JPEG.to_vec());
    assert_eq!(audit_count(&w.pool, "receipt_draft.use").await, 1);
    assert_eq!(audit_count(&w.pool, "receipt_draft.add").await, 1);
    assert_eq!(audit_count(&w.pool, "receipt_draft.recognize").await, 1);

    // Повтор проведения — та же накладная, второго фото нет.
    let (s, again) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        Some(&w.admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again["id"], receipt["id"]);
    let (_, files) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(files.as_array().unwrap().len(), 1);
    // Второй приход по тому же черновику — 409, приход не проведён.
    let mut second = body;
    second["op_id"] = json!(Uuid::now_v7());
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        Some(&w.owner),
        Some(second),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let receipts: i64 = sqlx::query_scalar("select count(*) from receipts")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(receipts, 1);
    // Использованный черновик не убирается.
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-drafts/{id}/remove"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_by_name_and_unknown_supplier(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool, Some(assistant(url))).await;
    let supplier_id = supplier(&w, "Автоснаб", "").await;
    // ИНН не совпал ни с кем — по названию без кавычек и формы собственности.
    fake.push(200, answer(&invoice()));
    let (_, d) = upload(&w.app, &w.owner, Uuid::now_v7(), &JPEG).await;
    assert_eq!(d["supplier"]["id"], supplier_id.as_str(), "{d}");
    assert_eq!(d["supplier"]["by"], "name");
    // Незнакомый поставщик — пусто, а в накладной — его название для подсказки.
    let mut other = invoice();
    other["supplier_name"] = json!("ИП Совсем Другой");
    other["supplier_inn"] = json!("");
    fake.push(200, answer(&other));
    let (_, d) = upload(&w.app, &w.owner, Uuid::now_v7(), &JPEG).await;
    assert!(d["supplier"].is_null(), "{d}");
    assert_eq!(d["invoice"]["supplier_name"], "ИП Совсем Другой");
    // «Убрать» — черновик пропадает из списка, запись остаётся (журнал).
    let id = d["id"].as_str().unwrap();
    let (s, r) = call(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-drafts/{id}/remove"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["status"], "removed");
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(audit_count(&w.pool, "receipt_draft.remove").await, 1);
    // По убранному черновику приход не проводится.
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        Some(&w.owner),
        Some(json!({
            "op_id": Uuid::now_v7(), "payment": "debt", "supplier_id": supplier_id,
            "lines": [{ "product_id": w.filter, "qty": 1, "cost_tyiyn": 35_000 }],
            "draft_id": id,
        })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    // Черновик не удаляется и фото не меняется — ни через программу, ни в базе.
    let del = sqlx::query("delete from receipt_drafts where id = $1")
        .bind(Uuid::parse_str(id).unwrap())
        .execute(&w.pool)
        .await;
    assert!(del.is_err());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn without_key_failure_retries_and_hour_limit(pool: PgPool) {
    // Без ключа: и экран, и бот сохраняют фото черновиком без распознавания (задача 003, Н6) —
    // приход заполняют вручную.
    let w = setup(pool.clone(), None).await;
    let op = Uuid::now_v7();
    let (s, v) = upload(&w.app, &w.admin, op, &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["message"].as_str().unwrap().contains("не настроено"),
        "{v}"
    );
    assert!(v["lines"].as_array().unwrap().is_empty());
    // Повтор того же op_id — тот же черновик (I8).
    let (_, again) = upload(&w.app, &w.admin, op, &JPEG).await;
    assert_eq!(again["id"], v["id"]);
    let screen = v["id"].as_str().unwrap().to_string();
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-drafts/{screen}/remove"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(list["enabled"], false);
    assert!(list["items"].as_array().unwrap().is_empty(), "{list}");
    let chat = 950_001i64;
    link_owner_chat(&w, chat).await;
    let file = || BotFile {
        data: JPEG.to_vec(),
        name: "фото из бота".into(),
        file_key: None,
    };
    let text = receipt_drafts::from_bot(&w.pool, None, chat, chat, file())
        .await
        .unwrap();
    assert!(
        text.starts_with("Распознавание накладных не настроено"),
        "{text}"
    );
    assert!(text.contains("«Приход» → «Накладные по фото»"), "{text}");
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["source"], "bot");
    assert_eq!(items[0]["recognition"], "off");
    let id = items[0]["id"].as_str().unwrap();
    let (_, d) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipt-drafts/{id}"),
        Some(&w.owner),
        None,
    )
    .await;
    assert!(
        d["message"].as_str().unwrap().contains("не настроено"),
        "{d}"
    );
    assert!(d["lines"].as_array().unwrap().is_empty());
    // Журнал: черновик из бота — источник «бот» и чат.
    let data: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'receipt_draft.add' and data->>'from' = 'bot'",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(data["source"], "bot");
    assert_eq!(data["chat"], chat);
    // Не из чата владельца — вежливый отказ, ничего не сохраняется.
    let text = receipt_drafts::from_bot(&w.pool, None, 950_002, 950_002, file())
        .await
        .unwrap();
    assert_eq!(text, receipt_drafts::NOT_OWNER);
    // Чат владельца, но прислал не тот, кто его привязал (ADR-070).
    let text = receipt_drafts::from_bot(&w.pool, None, chat, 777, file())
        .await
        .unwrap();
    assert_eq!(text, receipt_drafts::NOT_OWNER);
    let n: i64 = sqlx::query_scalar("select count(*) from receipt_drafts where source = 'bot'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // Не снимок — просьба прислать фото.
    let text = receipt_drafts::from_bot(
        &w.pool,
        None,
        chat,
        chat,
        BotFile {
            data: b"<html>".to_vec(),
            name: String::new(),
            file_key: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(text, receipt_drafts::BAD_KIND_BOT);

    // Сервис ответил ошибкой — черновик с фото без строк и «Не удалось распознать».
    let (url, fake) = fake_service().await;
    let a = assistant(url).with_invoice_model(Some("invoice-test-model".into()));
    fake.push(400, error(400, "INVALID_ARGUMENT"));
    let text = receipt_drafts::from_bot(&w.pool, Some(&a), chat, chat, file())
        .await
        .unwrap();
    assert!(text.starts_with(receipt_drafts::FAILED), "{text}");
    assert_eq!(fake.path(0), "invoice-test-model:generateContent");
    // Ошибка в запросе не повторяется.
    assert_eq!(fake.requests().len(), 1);
    let failed: (String, Option<String>) =
        sqlx::query_as("select recognition, error from receipt_drafts where recognition <> 'off'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(failed.0, "failed");
    assert!(failed.1.unwrap().contains("400 INVALID_ARGUMENT"));
    // Ключ сервиса никуда не записан.
    let leaked: i64 = sqlx::query_scalar(
        "select (select count(*) from receipt_drafts where receipt_drafts::text like '%' || $1 || '%')
              + (select count(*) from audit_log where audit_log::text like '%' || $1 || '%')",
    )
    .bind(KEY)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(leaked, 0);

    // Перегрузка и пустой 404 сервиса — повтор, с третьей попытки распознано.
    fake.push(503, error(503, "UNAVAILABLE"));
    fake.push(404, Value::Null);
    fake.push(200, answer(&invoice()));
    let text = receipt_drafts::from_bot(&w.pool, Some(&a), chat, chat, file())
        .await
        .unwrap();
    assert!(
        text.starts_with("Накладная распознана: ОсОО \"Автоснаб\", 4 строки, итого 11 200"),
        "{text}"
    );
    assert_eq!(fake.requests().len(), 4);
    // Ответ оборван по длине — не распознано, повтора нет.
    let mut cut = answer(&invoice());
    cut["candidates"][0]["finishReason"] = json!("MAX_TOKENS");
    fake.push(200, cut);
    let text = receipt_drafts::from_bot(&w.pool, Some(&a), chat, chat, file())
        .await
        .unwrap();
    assert!(text.starts_with(receipt_drafts::FAILED), "{text}");
    assert_eq!(fake.requests().len(), 5);

    // Лимит часа общий с вопросами: 16 вопросов + 3 накладные = 19 — ещё можно; 20 — нет.
    for _ in 0..16 {
        sqlx::query("insert into bot_questions (id, chat_id, user_id, branch_id, question) values ($1, $2, $3, $4, 'вопрос')")
            .bind(Uuid::now_v7())
            .bind(chat)
            .bind(w.owner_id)
            .bind(w.branch)
            .execute(&w.pool)
            .await
            .unwrap();
    }
    fake.push(200, answer(&invoice()));
    let text = receipt_drafts::from_bot(&w.pool, Some(&a), chat, chat, file())
        .await
        .unwrap();
    assert!(text.starts_with("Накладная распознана"), "{text}");
    let text = receipt_drafts::from_bot(&w.pool, Some(&a), chat, chat, file())
        .await
        .unwrap();
    assert!(
        text.starts_with("Лимит распознаваний на этот час исчерпан"),
        "{text}"
    );
    assert_eq!(fake.requests().len(), 6);
    let limited: i64 =
        sqlx::query_scalar("select count(*) from receipt_drafts where recognition = 'limit'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(limited, 1);
    // Вопрос боту в этот час — тоже лимит: накладные считаются вместе с вопросами.
    let mut conn = w.pool.acquire().await.unwrap();
    let ans = avtodom_server::api::assistant::answer(
        &mut conn,
        &a,
        chat,
        w.owner_id,
        w.branch,
        "выручка сегодня",
    )
    .await
    .unwrap();
    assert!(
        ans.text.contains("Лимит вопросов на этот час"),
        "{}",
        ans.text
    );
    assert_eq!(fake.requests().len(), 6);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn screen_limit_is_429(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool, Some(assistant(url))).await;
    for _ in 0..20 {
        sqlx::query("insert into bot_questions (id, chat_id, user_id, branch_id, question) values ($1, 1, $2, $3, 'вопрос')")
            .bind(Uuid::now_v7())
            .bind(w.owner_id)
            .bind(w.branch)
            .execute(&w.pool)
            .await
            .unwrap();
    }
    let (s, v) = upload(&w.app, &w.owner, Uuid::now_v7(), &JPEG).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
    assert!(fake.requests().is_empty());
    let n: i64 = sqlx::query_scalar("select count(*) from receipt_drafts")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    // Не снимок — 422 ещё до сервиса.
    let (s, _) = upload(&w.app, &w.owner, Uuid::now_v7(), b"%PDF").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

// ---------- Бот: фото в чате ----------

#[derive(Clone, Default)]
struct FakeTelegram {
    file: Arc<Mutex<Vec<u8>>>,
}

async fn fake_get_file(
    axum::extract::State(f): axum::extract::State<FakeTelegram>,
    axum::Json(body): axum::Json<Value>,
) -> axum::Json<Value> {
    let size = f.file.lock().unwrap().len();
    axum::Json(json!({ "ok": true, "result": {
        "file_id": body["file_id"], "file_path": "photos/file_7.jpg", "file_size": size
    } }))
}

async fn fake_file(axum::extract::State(f): axum::extract::State<FakeTelegram>) -> Vec<u8> {
    f.file.lock().unwrap().clone()
}

fn photo_update(chat: i64, extra: Value) -> Value {
    let mut message = json!({
        "message_id": 1,
        "chat": { "id": chat, "type": "private" },
        "from": { "id": chat },
    });
    for (k, v) in extra.as_object().unwrap() {
        message[k] = v.clone();
    }
    json!({ "update_id": 1, "message": message })
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_photo_from_owner_is_accepted_others_refused(pool: PgPool) {
    use avtodom_server::api::telegram::{Api, Keys, handle_update};
    let (url, _fake) = fake_service().await;
    let a = assistant(url);
    let w = setup(pool, Some(a.clone())).await;
    let chat = 960_001i64;
    link_owner_chat(&w, chat).await;
    let mut conn = w.pool.acquire().await.unwrap();
    let photo = json!({ "photo": [
        { "file_id": "small", "file_unique_id": "s", "width": 90, "height": 120, "file_size": 1000 },
        { "file_id": "big", "file_unique_id": "b", "width": 1280, "height": 1706, "file_size": 200_000 }
    ], "caption": "накладная" });
    let r = handle_update(&mut conn, Some(&a), photo_update(chat, photo.clone()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.text, receipt_drafts::ACCEPTED);
    assert!(matches!(r.keys, Keys::OwnerAsk), "{:?}", r.keys);
    // Не владелец — вежливый отказ, кнопки его раздела.
    let r = handle_update(&mut conn, Some(&a), photo_update(960_002, photo))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.text, receipt_drafts::NOT_OWNER);
    assert!(matches!(r.keys, Keys::Contact));
    // Файл больше 5 МБ и не картинка — объяснение сразу, без скачивания.
    let big = json!({ "document": { "file_id": "d1", "file_name": "scan.png", "mime_type": "image/png", "file_size": 6_000_000 } });
    let r = handle_update(&mut conn, Some(&a), photo_update(chat, big))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.text, receipt_drafts::TOO_BIG_BOT);
    let doc = json!({ "document": { "file_id": "d2", "file_name": "a.txt", "mime_type": "text/plain", "file_size": 100 } });
    let r = handle_update(&mut conn, Some(&a), photo_update(chat, doc))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r.text, receipt_drafts::BAD_KIND_BOT);
    // Без сервиса — фото тоже принимается (сохранится без распознавания).
    let pdf = json!({ "document": { "file_id": "d3", "file_name": "n.pdf", "mime_type": "application/pdf", "file_size": 100 } });
    let r = handle_update(&mut conn, None, photo_update(chat, pdf))
        .await
        .unwrap()
        .unwrap();
    assert!(r.text.starts_with("Принял фото накладной"), "{}", r.text);

    // Скачивание из Телеграма: getFile, затем файл; больше предела — отказ.
    let tg = FakeTelegram::default();
    *tg.file.lock().unwrap() = JPEG.to_vec();
    let router = Router::new()
        .route("/{bot}/getFile", axum::routing::post(fake_get_file))
        .route("/file/{bot}/{*path}", axum::routing::get(fake_file))
        .with_state(tg.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let api = Api::new(&format!("http://{addr}"), "123:tg-token");
    assert_eq!(api.download("big", 1024).await.unwrap(), JPEG.to_vec());
    assert!(api.download("big", 5).await.is_err());
}

// ---------- Задача 002: «Ещё фото», варианты товара, запомненный выбор, поиск приходов ----------

/// «Ещё фото» в открытый черновик: загрузка с `parent_id`.
async fn upload_more(
    app: &Router,
    cookie: &str,
    op_id: Uuid,
    parent: &str,
    data: &[u8],
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/receipt-drafts?op_id={op_id}&name=page2.jpg&parent_id={parent}"
        ))
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(Body::from(data.to_vec()))
        .unwrap();
    let (status, _, bytes) = send(app, req).await;
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Вторая страница: другой поставщик и номер, фильтр ещё раз и свеча.
fn invoice_page2() -> Value {
    json!({
        "readable": true,
        "supplier_name": "ОсОО \"Запчасти Плюс\"",
        "supplier_inn": "",
        "doc_number": "16",
        "doc_date": "2026-10-20",
        "lines": [
            { "name": "Фильтр масляный W712/75", "code": "", "barcode": "4011558738001",
              "quantity": "1", "unit": "шт", "price": "350", "sum": "350" },
            { "name": "Свеча зажигания NGK BKR6E", "code": "", "barcode": "",
              "quantity": "2", "unit": "шт", "price": "250", "sum": "500" }
        ],
        "total": "850"
    })
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn more_photo_appends_same_draft_and_all_photos_go_to_receipt(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool, Some(assistant(url))).await;
    let supplier_id = supplier(&w, "Автоснаб", "01234567890123").await;
    fake.push(200, answer(&invoice()));
    fake.push(200, answer(&invoice_page2()));

    let (s, d) = upload(&w.app, &w.admin, Uuid::now_v7(), &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let id = d["id"].as_str().unwrap().to_string();
    assert_eq!(d["photos"].as_array().unwrap().len(), 1);
    assert!(d["added_photo"].is_null());
    assert_eq!(d["warnings"], json!([]));

    // «Ещё фото»: тот же черновик, строки второго фото дописаны после первых.
    let op = Uuid::now_v7();
    let (s, more) = upload_more(&w.app, &w.admin, op, &id, &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{more}");
    assert_eq!(more["id"], id.as_str());
    assert_eq!(more["added_photo"], 2);
    assert_eq!(more["photos"].as_array().unwrap().len(), 2);
    let lines = more["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 6);
    assert_eq!(lines[3]["photo"], 1);
    assert_eq!(
        (lines[4]["photo"].as_i64(), lines[4]["no"].as_i64()),
        (Some(2), Some(5))
    );
    assert_eq!(lines[4]["product"]["id"], w.filter.to_string());
    // Поставщик — по первому фото; другой поставщик и номер — предупреждение, не отказ.
    assert_eq!(more["supplier"]["id"], supplier_id.as_str());
    let warnings = more["warnings"].to_string();
    assert!(warnings.contains("другой поставщик"), "{warnings}");
    assert!(warnings.contains("другой № накладной — 16"), "{warnings}");
    // Разные накладные — итоги складываются: 11 200 + 850.
    assert_eq!(more["total_tyiyn"], 1_205_000);
    assert_eq!(more["lines_sum_tyiyn"], 1_205_000);

    // Повтор того же «Ещё фото» — без дубля и без второго распознавания.
    let (s, again) = upload_more(&w.app, &w.admin, op, &id, &JPEG).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["lines"].as_array().unwrap().len(), 6);
    assert_eq!(again["added_photo"], 2);
    assert_eq!(fake.requests().len(), 2);
    let photos: i64 = sqlx::query_scalar("select count(*) from receipt_drafts")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(photos, 2);

    // В «Накладных по фото» — одна накладная с двумя фото; открыть — те же 6 строк.
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["photos"], 2);
    let (_, opened) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipt-drafts/{id}"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(opened["lines"].as_array().unwrap().len(), 6);
    // Дописанное фото само по себе не проводится и не убирается.
    let child = more["photos"][1]["id"].as_str().unwrap().to_string();
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-drafts/{child}/remove"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Проведение: оба фото — к накладной, оба черновика — «использованы».
    let (s, receipt) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        Some(&w.admin),
        Some(json!({
            "op_id": Uuid::now_v7(),
            "supplier_id": supplier_id,
            "payment": "debt",
            "lines": [{ "product_id": w.filter, "qty": 3, "cost_tyiyn": 105_000 }],
            "draft_id": id,
        })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{receipt}");
    let rid = receipt["id"].as_str().unwrap();
    let (_, files) = call(
        &w.app,
        "GET",
        &format!("/api/v1/receipts/{rid}/files"),
        Some(&w.owner),
        None,
    )
    .await;
    let names: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["nakladnaya.jpg", "page2.jpg"]);
    let used: i64 = sqlx::query_scalar(
        "select count(*) from receipt_drafts where status = 'used' and receipt_id = $1",
    )
    .bind(Uuid::parse_str(rid).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(used, 2);
    // В проведённую накладную по фото «Ещё фото» уже не дописать.
    let (s, err) = upload_more(&w.app, &w.admin, Uuid::now_v7(), &id, &JPEG).await;
    assert_eq!(s, StatusCode::CONFLICT, "{err}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn several_exact_similar_candidates_and_remembered_choice(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool, Some(assistant(url))).await;
    let supplier_id = supplier(&w, "Автоснаб", "01234567890123").await;
    // Ещё один товар с тем же артикулом и ещё одна свеча — варианты на выбор.
    let (twin, plug2) = (Uuid::now_v7(), Uuid::now_v7());
    let cat: Uuid = sqlx::query_scalar("select category_id from products where id = $1")
        .bind(w.plug)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into products (id, category_id, name, brand, article, unit) values
                 ($1, $3, 'Фильтр масляный Mann W712/75 (старый)', 'Mann', 'W 712-75', 'piece'),
                 ($2, $3, 'Свеча NGK BKR6E-11', 'NGK', '', 'piece')",
    )
    .bind(twin)
    .bind(plug2)
    .bind(cat)
    .execute(&w.pool)
    .await
    .unwrap();
    sqlx::query("update products set article = 'W712/75' where id = $1")
        .bind(w.filter)
        .execute(&w.pool)
        .await
        .unwrap();
    let inv = json!({
        "readable": true, "supplier_name": "Автоснаб", "supplier_inn": "01234567890123",
        "doc_number": "20", "doc_date": "", "total": "",
        "lines": [
            { "name": "Фильтр W712/75", "code": "W712/75", "barcode": "",
              "quantity": "1", "unit": "шт", "price": "350", "sum": "350" },
            { "name": "Свеча зажигания NGK BKR6E", "code": "", "barcode": "",
              "quantity": "4", "unit": "шт", "price": "250", "sum": "1000" },
            { "name": "Жидкость тормозная DOT-4", "code": "", "barcode": "",
              "quantity": "1", "unit": "шт", "price": "500", "sum": "500" }
        ]
    });
    fake.push(200, answer(&inv));
    let (s, d) = upload(&w.app, &w.admin, Uuid::now_v7(), &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let lines = d["lines"].as_array().unwrap();
    // Два точных совпадения по артикулу — оба на выбор, а не «без товара».
    assert_eq!(lines[0]["match"], "several", "{}", lines[0]);
    assert!(lines[0]["product"].is_null());
    let ids: Vec<&str> = lines[0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(
        ids.contains(&w.filter.to_string().as_str()) && ids.contains(&twin.to_string().as_str())
    );
    // «Похож» — выбранный и другие похожие с остатком и ценой последнего прихода.
    assert_eq!(lines[1]["match"], "similar");
    let more = lines[1]["candidates"].as_array().unwrap();
    assert!(!more.is_empty() && more.len() <= 3, "{more:?}");
    let mut seen: Vec<&str> = more.iter().map(|p| p["id"].as_str().unwrap()).collect();
    seen.push(lines[1]["product"]["id"].as_str().unwrap());
    assert!(
        seen.contains(&w.plug.to_string().as_str()) && seen.contains(&plug2.to_string().as_str())
    );
    assert!(more[0].get("stock_qty").is_some());
    assert!(more[0].get("last_purchase_price_tyiyn").is_some());
    assert_eq!(lines[2]["match"], "none");

    // Выбор запоминается: «Жидкость тормозная DOT-4» у Автоснаба → свеча (для проверки).
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "supplier_id": supplier_id,
                       "name": "  Жидкость   тормозная DOT-4 ", "product_id": w.plug });
    let (s, m) = call(
        &w.app,
        "POST",
        "/api/v1/supplier-line-map",
        Some(&w.admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["name"], "Жидкость   тормозная DOT-4");
    let (s, again) = call(
        &w.app,
        "POST",
        "/api/v1/supplier-line-map",
        Some(&w.admin),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], m["id"]);
    // Тот же артикул — тоже выбор: двойное совпадение запоминается.
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/supplier-line-map",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "supplier_id": supplier_id,
                     "name": "Фильтр W712/75", "product_id": twin })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(audit_count(&w.pool, "supplier_line_map.set").await, 2);
    let rows: i64 = sqlx::query_scalar("select count(*) from supplier_line_map")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);

    // Следующая накладная этого поставщика: запомненные строки совпадают точно.
    fake.push(200, answer(&inv));
    let (s, d2) = upload(&w.app, &w.admin, Uuid::now_v7(), &JPEG).await;
    assert_eq!(s, StatusCode::OK, "{d2}");
    let l2 = d2["lines"].as_array().unwrap();
    assert_eq!(l2[0]["match"], "exact");
    assert_eq!(l2[0]["remembered"], true);
    assert_eq!(l2[0]["product"]["id"], twin.to_string());
    assert_eq!(l2[2]["match"], "exact");
    assert_eq!(l2[2]["product"]["id"], w.plug.to_string());
    assert_eq!(l2[1]["remembered"], false);

    // Список в карточке поставщика и удаление (право «Товары»).
    let (s, list) = call(
        &w.app,
        "GET",
        &format!("/api/v1/suppliers/{supplier_id}/line-map"),
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 2, "{list}");
    let mid = m["id"].as_str().unwrap();
    let rm = json!({ "op_id": Uuid::now_v7() });
    let uri = format!("/api/v1/supplier-line-map/{mid}/remove");
    let (s, r) = call(&w.app, "POST", &uri, Some(&w.admin), Some(rm.clone())).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    // Повтор с тем же op_id — тот же ответ.
    let (s, _) = call(&w.app, "POST", &uri, Some(&w.admin), Some(rm)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(audit_count(&w.pool, "supplier_line_map.remove").await, 1);
    let (_, list) = call(
        &w.app,
        "GET",
        &format!("/api/v1/suppliers/{supplier_id}/line-map"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    // Другой филиал запомненного не видит и не удаляет (инвариант 16).
    let other = Uuid::now_v7();
    sqlx::query("insert into branches (id, name) values ($1, 'Вторая точка')")
        .bind(other)
        .execute(&w.pool)
        .await
        .unwrap();
    let hash = hash_password("admin2-pass-1".into()).await.unwrap();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin2', $3, 'admin', 'Админ 2')")
        .bind(Uuid::now_v7())
        .bind(other)
        .bind(hash)
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select id, $1, 'products' from users where login = 'admin2'")
        .bind(other)
        .execute(&w.pool)
        .await
        .unwrap();
    let admin2 = login(&w.app, "admin2", "admin2-pass-1").await;
    let (_, list) = call(
        &w.app,
        "GET",
        &format!("/api/v1/suppliers/{supplier_id}/line-map"),
        Some(&admin2),
        None,
    )
    .await;
    assert_eq!(list, json!([]));
    let left = list_first_id(&w, &supplier_id).await;
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/supplier-line-map/{left}/remove"),
        Some(&admin2),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

async fn list_first_id(w: &World, supplier_id: &str) -> String {
    let (_, list) = call(
        &w.app,
        "GET",
        &format!("/api/v1/suppliers/{supplier_id}/line-map"),
        Some(&w.owner),
        None,
    )
    .await;
    list[0]["id"].as_str().unwrap().to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipts_search_by_doc_goods_amount_and_photo(pool: PgPool) {
    let w = setup(pool, None).await;
    let supplier_id = supplier(&w, "Автоснаб", "").await;
    let oil_trade = supplier(&w, "Ойл Трейд", "").await;
    let post = |lines: Value, doc: &str, supplier: Option<&str>| {
        let body = json!({
            "op_id": Uuid::now_v7(), "supplier_id": supplier, "payment": "debt",
            "supplier_doc": doc, "lines": lines,
        });
        let app = w.app.clone();
        let cookie = w.owner.clone();
        async move {
            let (s, r) = call(&app, "POST", "/api/v1/receipts", Some(&cookie), Some(body)).await;
            assert_eq!(s, StatusCode::OK, "{r}");
            r["id"].as_str().unwrap().to_string()
        }
    };
    // № НК-777 от Автоснаба: фильтр на 700 с; второй — масло на 1 234,50 с от Ойл Трейда.
    let r1 = post(
        json!([{ "product_id": w.filter, "qty": 2, "cost_tyiyn": 70_000 }]),
        "НК-777 от 20.10.2026",
        Some(&supplier_id),
    )
    .await;
    let r2 = post(
        json!([{ "product_id": w.oil, "qty": 4000, "cost_tyiyn": 123_450 }]),
        "",
        Some(&oil_trade),
    )
    .await;
    // Фото — только у первой.
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/receipts/{r1}/files?op_id={}&name=p.jpg",
            Uuid::now_v7()
        ))
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, &w.owner)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .body(Body::from(JPEG.to_vec()))
        .unwrap();
    assert_eq!(send(&w.app, req).await.0, StatusCode::OK);

    let find = |q: &str| {
        let app = w.app.clone();
        let cookie = w.admin.clone();
        let uri = format!("/api/v1/receipts?kind=all&q={}", urlencode(q));
        async move {
            let (s, v) = call(&app, "GET", &uri, Some(&cookie), None).await;
            assert_eq!(s, StatusCode::OK, "{v}");
            v.as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(find("нк-777").await, std::slice::from_ref(&r1));
    assert_eq!(find("автоснаб").await, std::slice::from_ref(&r1));
    assert_eq!(find("масляный").await, std::slice::from_ref(&r1));
    assert_eq!(find("Mobil").await, std::slice::from_ref(&r2));
    // Сумма в сомах — сравнение в тыйынах.
    assert_eq!(find("700").await, std::slice::from_ref(&r1));
    assert_eq!(find("1 234,50").await, std::slice::from_ref(&r2));
    assert_eq!(find("1234.5").await, std::slice::from_ref(&r2));
    assert!(find("100%").await.is_empty());
    // Без поиска — обе; «с фото» — только первая, со значком фото.
    assert_eq!(find("").await.len(), 2);
    let (_, v) = call(
        &w.app,
        "GET",
        "/api/v1/receipts?kind=all&photo=true",
        Some(&w.admin),
        None,
    )
    .await;
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 1, "{v}");
    assert_eq!(rows[0]["id"], r1.as_str());
    assert_eq!(rows[0]["files"], 1);

    // Приход другого филиала не находится (инвариант 16).
    let other = Uuid::now_v7();
    sqlx::query("insert into branches (id, name) values ($1, 'Вторая точка')")
        .bind(other)
        .execute(&w.pool)
        .await
        .unwrap();
    let hash = hash_password("admin2-pass-1".into()).await.unwrap();
    let admin2_id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin2', $3, 'admin', 'Админ 2')")
        .bind(admin2_id)
        .bind(other)
        .bind(hash)
        .execute(&w.pool)
        .await
        .unwrap();
    // Право «Приход товара» есть (список — по праву, задача 003): чужих накладных всё равно не видно.
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) values ($1, $2, 'receipts'), ($1, $2, 'stock_in')")
        .bind(admin2_id)
        .bind(other)
        .execute(&w.pool)
        .await
        .unwrap();
    let admin2 = login(&w.app, "admin2", "admin2-pass-1").await;
    let (_, v) = call(
        &w.app,
        "GET",
        "/api/v1/receipts?kind=all&q=777",
        Some(&admin2),
        None,
    )
    .await;
    assert_eq!(v, json!([]));
}

/// Текст в адрес запроса.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ---------- Задача 003, Н6: склейка фото накладной в боте ----------

async fn bot_roots(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "select count(*) from receipt_drafts where source = 'bot' and parent_id is null",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Склейка выключена (0, по умолчанию) — каждое фото отдельно, как было; включена — фото того же
/// человека за N минут дописывается к открытой накладной, «➕ Это другая накладная» отделяет его.
/// Настройку меняет только владелец, изменение — в журнал.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_photos_merge_into_one_invoice_when_set(pool: PgPool) {
    let w = setup(pool.clone(), None).await;
    let chat = 951_001i64;
    link_owner_chat(&w, chat).await;
    let file = || BotFile {
        data: JPEG.to_vec(),
        name: "фото из бота".into(),
        file_key: None,
    };
    // По умолчанию — как раньше: два фото — две накладные, кнопки «другая накладная» нет.
    let (_, st) = call(
        &w.app,
        "GET",
        "/api/v1/settings/bot-receipts",
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(st["merge_minutes"], 0, "{st}");
    for _ in 0..2 {
        let a = receipt_drafts::from_bot_answer(&w.pool, None, chat, chat, file())
            .await
            .unwrap();
        assert!(a.split.is_none());
    }
    assert_eq!(bot_roots(&w.pool).await, 2);
    // Администратору настройка не меняется; владельцу — да, в журнал.
    let body = json!({ "merge_minutes": 10, "invoice_hint": "Снимите накладную целиком." });
    let (s, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/bot-receipts",
        Some(&w.admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/bot-receipts",
        Some(&w.owner),
        Some(json!({ "merge_minutes": 2000, "invoice_hint": "x" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (s, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/bot-receipts",
        Some(&w.owner),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(audit_count(&w.pool, "receipt_draft.bot_settings").await, 1);
    let mut conn = w.pool.acquire().await.unwrap();
    let hint = receipt_drafts::invoice_hint_text(&mut conn, w.branch)
        .await
        .unwrap();
    assert!(hint.starts_with("Снимите накладную целиком."), "{hint}");
    assert!(hint.contains("10 мин."), "{hint}");
    // Третье фото — к последней открытой накладной, с кнопкой «другая накладная».
    let a = receipt_drafts::from_bot_answer(&w.pool, None, chat, chat, file())
        .await
        .unwrap();
    let merged = a.split.expect("фото дописано");
    assert!(
        a.text.starts_with("Фото 2 добавлено к накладной"),
        "{}",
        a.text
    );
    assert_eq!(bot_roots(&w.pool).await, 2);
    let (_, list) = call(
        &w.app,
        "GET",
        "/api/v1/receipt-drafts",
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(list["items"][0]["photos"], 2, "{list}");
    // Кнопка — только от того, кто привязал чат; отделяет фото в свою накладную, в журнал.
    let data = receipt_drafts::split_data(merged);
    let text = receipt_drafts::split_bot(&mut conn, chat, 777, &data)
        .await
        .unwrap();
    assert_eq!(text, receipt_drafts::NOT_OWNER);
    let text = receipt_drafts::split_bot(&mut conn, chat, chat, &data)
        .await
        .unwrap();
    assert!(text.starts_with("Готово"), "{text}");
    assert_eq!(bot_roots(&w.pool).await, 3);
    assert_eq!(audit_count(&w.pool, "receipt_draft.split").await, 1);
    let text = receipt_drafts::split_bot(&mut conn, chat, chat, &data)
        .await
        .unwrap();
    assert!(text.contains("уже отдельная"), "{text}");
    // Последнее фото старше 10 минут — следующее становится новой накладной.
    for sql in [
        "alter table receipt_drafts disable trigger receipt_drafts_immutable",
        "update receipt_drafts set created_at = created_at - interval '11 minutes'",
        "alter table receipt_drafts enable trigger receipt_drafts_immutable",
    ] {
        sqlx::query(sql).execute(&w.pool).await.unwrap();
    }
    let a = receipt_drafts::from_bot_answer(&w.pool, None, chat, chat, file())
        .await
        .unwrap();
    assert!(a.split.is_none());
    assert_eq!(bot_roots(&w.pool).await, 4);
    // Убранную накладную фото не дописывает.
    let last: Uuid = sqlx::query_scalar(
        "select id from receipt_drafts where source = 'bot' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/receipt-drafts/{last}/remove"),
        Some(&w.owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let a = receipt_drafts::from_bot_answer(&w.pool, None, chat, chat, file())
        .await
        .unwrap();
    assert!(a.split.is_none());
    assert_eq!(bot_roots(&w.pool).await, 5);
    // Ссылку на первый черновик не поставить и не сменить в базе — только снять кнопкой.
    let first: Uuid = sqlx::query_scalar(
        "select id from receipt_drafts where source = 'bot' order by created_at limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    let r = sqlx::query("update receipt_drafts set parent_id = $1 where id = $2")
        .bind(last)
        .bind(first)
        .execute(&w.pool)
        .await;
    assert!(r.is_err());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn same_bot_photo_within_a_day_is_not_a_new_draft(pool: PgPool) {
    // Задача 003: то же фото (тот же ключ Телеграма) в тот же чат за сутки — ответ «уже принято»,
    // второго черновика нет; другое фото — новый черновик; через сутки — снова новый.
    let w = setup(pool, None).await;
    let chat = 950_101i64;
    link_owner_chat(&w, chat).await;
    let photo = |key: &str| BotFile {
        data: JPEG.to_vec(),
        name: "фото из бота".into(),
        file_key: Some(key.into()),
    };
    let pool = w.pool.clone();
    let drafts = move || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from receipt_drafts where source = 'bot'")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let text = receipt_drafts::from_bot(&w.pool, None, chat, chat, photo("AQADuniq1"))
        .await
        .unwrap();
    assert!(text.contains("Фото сохранено"), "{text}");
    assert_eq!(drafts().await, 1);
    let text = receipt_drafts::from_bot(&w.pool, None, chat, chat, photo("AQADuniq1"))
        .await
        .unwrap();
    assert!(
        text.starts_with("Это фото уже принято: черновик от "),
        "{text}"
    );
    assert!(text.contains("ждёт проверки"), "{text}");
    assert_eq!(drafts().await, 1);
    // Проверка до скачивания — тот же ответ; другое фото не встречалось.
    let mut conn = w.pool.acquire().await.unwrap();
    let seen = receipt_drafts::seen_photo(&mut conn, chat, Some("AQADuniq1"))
        .await
        .unwrap();
    assert!(seen.is_some());
    assert!(
        receipt_drafts::seen_photo(&mut conn, chat, Some("AQADother"))
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    // Другое фото — новый черновик.
    receipt_drafts::from_bot(&w.pool, None, chat, chat, photo("AQADother"))
        .await
        .unwrap();
    assert_eq!(drafts().await, 2);
    // Через сутки то же фото — снова черновик.
    sqlx::query(
        "update bot_photo_seen set created_at = now() - interval '25 hours' where file_key = 'AQADuniq1'",
    )
    .execute(&w.pool)
    .await
    .unwrap();
    let text = receipt_drafts::from_bot(&w.pool, None, chat, chat, photo("AQADuniq1"))
        .await
        .unwrap();
    assert!(text.contains("Фото сохранено"), "{text}");
    assert_eq!(drafts().await, 3);
    // Не из чата владельца — фото не занимается: отказ, как раньше.
    let text = receipt_drafts::from_bot(&w.pool, None, 950_102, 950_102, photo("AQADuniq9"))
        .await
        .unwrap();
    assert_eq!(text, receipt_drafts::NOT_OWNER);
    let held: i64 =
        sqlx::query_scalar("select count(*) from bot_photo_seen where file_key = 'AQADuniq9'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(held, 0);
}
