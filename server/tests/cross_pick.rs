//! Подбор кросс-номеров фильтра (задача 002 П10, ADR-145): сервис разбора подставной (настоящий
//! в тестах не вызывается); «записывать сразу» и «предлагать», пометка «подобрано автоматически»,
//! удаление в карточке, дневной лимит, повтор `op_id`, права.
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

use avtodom_server::api::assistant::Assistant;
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

// ---------- Подставной сервис ----------

#[derive(Clone, Default)]
struct Fake {
    replies: Arc<Mutex<VecDeque<(u16, Value)>>>,
    seen: Arc<Mutex<Vec<Value>>>,
}

impl Fake {
    fn push(&self, code: u16, body: Value) {
        self.replies.lock().unwrap().push_back((code, body));
    }
    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

async fn fake_messages(
    axum::extract::State(f): axum::extract::State<Fake>,
    axum::Json(body): axum::Json<Value>,
) -> (StatusCode, String) {
    f.seen.lock().unwrap().push(body);
    match f.replies.lock().unwrap().pop_front() {
        Some((code, v)) => (StatusCode::from_u16(code).unwrap(), v.to_string()),
        None => (
            StatusCode::BAD_REQUEST,
            json!({ "error": { "code": 400, "message": "test", "status": "INVALID_ARGUMENT" } })
                .to_string(),
        ),
    }
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

/// Ответ сервиса: JSON с номерами.
fn answer(codes: &[(&str, &str)]) -> Value {
    let list: Vec<Value> = codes
        .iter()
        .map(|(b, c)| json!({ "brand": b, "code": c }))
        .collect();
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": json!({ "codes": list }).to_string() }] },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 300, "candidatesTokenCount": 80 }
    })
}

// ---------- Программа ----------

struct World {
    app: Router,
    owner: String,
    admin: String,
    filter: Uuid,
    oil: Uuid,
}

async fn setup(pool: PgPool, assistant: Option<Assistant>) -> World {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select branch_id from users where login = 'owner'")
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
    // Права по умолчанию, но без `products`: подбор правит карточку товара.
    let keys: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .filter(|k| *k != "products")
        .map(String::from)
        .collect();
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
        .bind(admin)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
    let (cat_f, cat_o) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into categories (id, name, kind) values ($1, 'Фильтры', 'filter'), ($2, 'Масла', 'oil')")
        .bind(cat_f)
        .bind(cat_o)
        .execute(&pool)
        .await
        .unwrap();
    let (filter, oil) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query(
        "insert into products (id, category_id, name, brand, article, unit, container_ml) values
                 ($1, $3, 'Фильтр масляный Mann W712/75', 'Mann', 'W712/75', 'piece', null),
                 ($2, $4, 'Mobil Super 3000 5W-30 4л', 'Mobil', 'MOB 5W30 4', 'ml', 4000)",
    )
    .bind(filter)
    .bind(oil)
    .bind(cat_f)
    .bind(cat_o)
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
        owner,
        admin,
        filter,
        oil,
    }
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "login": login, "password": password }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie);
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn assistant(url: String) -> Assistant {
    Assistant::new("test-cross-key".into(), Some(url), None, 100).with_retry_pauses(Vec::new())
}

async fn pick(w: &World, product: Uuid, op: Uuid) -> (StatusCode, Value) {
    call(
        &w.app,
        "POST",
        &format!("/api/v1/products/{product}/cross/pick"),
        &w.owner,
        Some(json!({ "op_id": op })),
    )
    .await
}

async fn settings(w: &World, mode: &str, limit: i64) {
    let (s, v) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/cross-pick",
        &w.owner,
        Some(json!({ "op_id": Uuid::now_v7(), "mode": mode, "daily_limit": limit })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

async fn audit_auto(pool: &PgPool) -> Vec<Value> {
    sqlx::query_scalar(
        "select data from audit_log where action = 'product.cross' and data ? 'auto' order by created_at",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn auto_mode_writes_codes_with_mark(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool.clone(), Some(assistant(url))).await;
    // По умолчанию — «записывать сразу», лимит 50.
    let (_, st) = call(&w.app, "GET", "/api/v1/settings/cross-pick", &w.owner, None).await;
    assert_eq!(
        (&st["mode"], &st["daily_limit"], &st["available"]),
        (&json!("auto"), &json!(50), &json!(true))
    );
    fake.push(
        200,
        answer(&[
            ("Toyota", "90915-YZZE1"),
            ("MANN", "W 712/75"),
            ("Mahle", "OC 217"),
        ]),
    );
    let op = Uuid::now_v7();
    let (s, out) = pick(&w, w.filter, op).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // Свой артикул (W712/75 в другом написании) не пишется.
    assert_eq!(out["added"], json!(["90915-YZZE1", "OC 217"]));
    assert_eq!(out["auto"], json!(["90915-YZZE1", "OC 217"]));
    assert_eq!(out["used_today"], json!(1));
    // Повтор `op_id` — тот же ответ, без второго обращения к сервису.
    let (s, again) = pick(&w, w.filter, op).await;
    assert_eq!((s, &again), (StatusCode::OK, &out));
    assert_eq!(fake.calls(), 1);
    // Журнал: «подобрано автоматически».
    let log = audit_auto(&pool).await;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["note"], json!("подобрано автоматически"));
    // По номеру находится товар; удалить можно в карточке — пометка у оставшихся сохраняется.
    let (s, v) = call(
        &w.app,
        "PUT",
        &format!("/api/v1/products/{}/cross", w.filter),
        &w.owner,
        Some(json!({ "codes": ["OC 217", "LF-1"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["codes"], json!(["LF-1", "OC 217"]));
    assert_eq!(v["auto"], json!(["OC 217"]));
    // Не фильтр — 422; сервис не ответил — ошибка в ответе, номера не тронуты.
    let (s, _) = pick(&w, w.oil, Uuid::now_v7()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, out) = pick(&w, w.filter, Uuid::now_v7()).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert!(out["error"].is_string(), "{out}");
    assert_eq!(out["codes"], json!(["LF-1", "OC 217"]));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn suggest_mode_saved_by_person_and_daily_limit(pool: PgPool) {
    let (url, fake) = fake_service().await;
    let w = setup(pool.clone(), Some(assistant(url))).await;
    settings(&w, "suggest", 1).await;
    fake.push(200, answer(&[("Toyota", "90915-YZZE1")]));
    let (s, out) = pick(&w, w.filter, Uuid::now_v7()).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    // «Предлагать»: номера вернулись, в карточку не записаны.
    assert_eq!(
        (&out["found"], &out["added"], &out["codes"]),
        (&json!(["90915-YZZE1"]), &json!([]), &json!([]))
    );
    // Человек сохраняет — с пометкой и в журнал.
    let (s, v) = call(
        &w.app,
        "PUT",
        &format!("/api/v1/products/{}/cross", w.filter),
        &w.owner,
        Some(json!({ "codes": ["90915-YZZE1"], "auto": ["90915-YZZE1"] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["auto"], json!(["90915-YZZE1"]));
    assert_eq!(audit_auto(&pool).await.len(), 1);
    // Лимит дня исчерпан — 409, к сервису не ходим.
    let (s, v) = pick(&w, w.filter, Uuid::now_v7()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(
        v["error"]["message"].as_str().unwrap().contains("лимит"),
        "{v}"
    );
    assert_eq!(fake.calls(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cross_pick_rights_and_no_key(pool: PgPool) {
    let w = setup(pool, None).await;
    // Без ключа сервиса — 422 с понятным текстом.
    let (s, v) = pick(&w, w.filter, Uuid::now_v7()).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Администратор без права «Товары» — 403; настройку правит только владелец.
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/products/{}/cross/pick", w.filter),
        &w.admin,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = call(&w.app, "GET", "/api/v1/settings/cross-pick", &w.admin, None).await;
    assert_eq!((s, &v["available"]), (StatusCode::OK, &json!(false)));
    let (s, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/cross-pick",
        &w.admin,
        Some(json!({ "op_id": Uuid::now_v7(), "mode": "suggest", "daily_limit": 5 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/oil-book",
        &w.admin,
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = call(&w.app, "GET", "/api/v1/settings/oil-book", &w.admin, None).await;
    assert_eq!((s, &v["auto_book"]), (StatusCode::OK, &json!(true)));
    // Книжка для чека без сети — печатать после отправки (задача 003, хвост Н5): по умолчанию да,
    // владелец выключает, и выключенное сохраняется.
    assert_eq!(v["offline_book"], json!(true), "{v}");
    // QR-наклейка после книжки (задача 024): по умолчанию да, выключенное сохраняется.
    assert_eq!(v["qr_sticker"], json!(true), "{v}");
    let mut off = v.clone();
    off["offline_book"] = json!(false);
    off["qr_sticker"] = json!(false);
    off["op_id"] = json!(Uuid::now_v7());
    let (s, saved) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/oil-book",
        &w.owner,
        Some(off),
    )
    .await;
    assert_eq!(
        (s, &saved["offline_book"]),
        (StatusCode::OK, &json!(false)),
        "{saved}"
    );
    let (_, again) = call(&w.app, "GET", "/api/v1/settings/oil-book", &w.admin, None).await;
    assert_eq!(again["offline_book"], json!(false), "{again}");
    assert_eq!(again["qr_sticker"], json!(false), "{again}");
    // Неверный лимит — 422.
    let (s, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/cross-pick",
        &w.owner,
        Some(json!({ "op_id": Uuid::now_v7(), "mode": "auto", "daily_limit": -1 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn filter_type_switch_is_owner_only(pool: PgPool) {
    // «Тип» решает строку книжки (ADR-072): администратор с правом «Товары» правит категорию, но
    // выключить «Тип» не может — 403; владелец может.
    let w = setup(pool.clone(), None).await;
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission)
         select id, branch_id, 'products' from users where login = 'admin'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, cats) = call(&w.app, "GET", "/api/v1/categories", &w.owner, None).await;
    let filters = cats
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Фильтры")
        .unwrap()
        .clone();
    let id = filters["id"].as_str().unwrap().to_string();
    let mut attrs: Value = json!([
        { "key": "filter_type", "label": "Тип", "type": "select",
          "options": ["масляный", "воздушный"], "filterable": true }
    ]);
    let (s, v) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/categories/{id}"),
        &w.admin,
        Some(json!({ "attributes": attrs })),
    )
    .await;
    assert_eq!((s, &v["by_type"]), (StatusCode::OK, &json!(true)), "{v}");
    attrs[0]["enabled"] = json!(false);
    let (s, _) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/categories/{id}"),
        &w.admin,
        Some(json!({ "attributes": attrs })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, v) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/categories/{id}"),
        &w.owner,
        Some(json!({ "attributes": attrs })),
    )
    .await;
    assert_eq!((s, &v["by_type"]), (StatusCode::OK, &json!(false)), "{v}");
    let off: Value = sqlx::query_scalar(
        "select data -> 'attributes_off' from audit_log where action = 'category.update' order by created_at desc limit 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(off["off"]["new"], json!(["filter_type"]), "{off}");
}
