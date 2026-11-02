//! Свои строки книжки точки (задача 016, ADR-184): создание из окна записи, проверки названия и
//! интервала, только своя точка, `other` + своя строка, скрытие и переименование, права, журнал,
//! срок по своему интервалу и напоминание бота её названием.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::Days;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::auth::hash_password;
use avtodom_server::{app, bootstrap, build_state};

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    branch: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie);
    if let Some(b) = branch {
        req = req.header("x-branch-id", b);
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
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
    let (s, v) = call(app, method, uri, cookie, None, body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
    v
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

/// Администратор точки с данными правами.
async fn admin(pool: &PgPool, app: &Router, name: &str, perms: &[&str]) -> String {
    let branch: Uuid = sqlx::query_scalar("select id from branches order by created_at limit 1")
        .fetch_one(pool)
        .await
        .unwrap();
    let hash = hash_password("pass-12345".into()).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $3)")
        .bind(id)
        .bind(branch)
        .bind(name)
        .bind(hash)
        .execute(pool)
        .await
        .unwrap();
    let perms: Vec<String> = perms.iter().map(|p| p.to_string()).collect();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
    )
    .bind(id)
    .bind(branch)
    .bind(perms)
    .execute(pool)
    .await
    .unwrap();
    login(app, name, "pass-12345").await
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    party: String,
    vehicle: String,
}

async fn world(pool: PgPool) -> W {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let p = ok(
        &app,
        "POST",
        "/api/v1/parties",
        &owner,
        Some(json!({ "kind": "person", "first_name": "Азиз", "last_name": "Свечкин", "phone": "0700116016" })),
    )
    .await;
    let party = p["id"].as_str().unwrap().to_string();
    let v = ok(
        &app,
        "POST",
        &format!("/api/v1/parties/{party}/vehicles"),
        &owner,
        Some(json!({ "plate": "01KG016ABC" })),
    )
    .await;
    W {
        app,
        pool,
        owner,
        party,
        vehicle: v["id"].as_str().unwrap().to_string(),
    }
}

impl W {
    async fn create(&self, body: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            "/api/v1/book-slots",
            &self.owner,
            None,
            Some(body),
        )
        .await
    }

    /// Запись книжки: дата, пробег (без него — `None`), строки.
    async fn record(&self, date: &str, mileage: Option<i64>, lines: Value) -> Value {
        let mut body = json!({ "change_date": date, "lines": lines });
        if let Some(m) = mileage {
            body["mileage"] = json!(m);
            body["mileage_unit"] = json!("km");
        }
        ok(
            &self.app,
            "POST",
            &format!("/api/v1/vehicles/{}/oil-changes", self.vehicle),
            &self.owner,
            Some(body),
        )
        .await
    }

    async fn book(&self) -> Value {
        ok(
            &self.app,
            "GET",
            &format!("/api/v1/vehicles/{}/oil-book", self.vehicle),
            &self.owner,
            None,
        )
        .await
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar("select data from audit_log where action = $1 order by created_at")
            .bind(action)
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }
}

fn slot_of<'a>(book: &'a Value, id: &str) -> Option<&'a Value> {
    book["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["custom_slot_id"] == json!(id))
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn names_intervals_and_branch(pool: PgPool) {
    let w = world(pool).await;
    let (s, plugs) = w
        .create(json!({ "name": "  Свечи ", "interval_km": 30_000, "interval_days": 365 }))
        .await;
    assert_eq!(s, StatusCode::OK, "{plugs}");
    assert_eq!(plugs["name"], "Свечи");
    assert_eq!(plugs["interval_km"], 30_000);
    assert_eq!(plugs["interval_days"], 365);
    assert_eq!(plugs["hidden"], false);
    let (s, belt) = w.create(json!({ "name": "Ремень ГРМ" })).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(belt["interval_km"], Value::Null);

    // Повтор без учёта регистра — со своей и со встроенной строкой.
    for name in ["свечи", "АНТИФРИЗ", "прочие работы"] {
        let (s, v) = w.create(json!({ "name": name })).await;
        assert_eq!(s, StatusCode::CONFLICT, "{name}: {v}");
    }
    // Длина и пределы интервалов.
    for body in [
        json!({ "name": "Я" }),
        json!({ "name": " " }),
        json!({ "name": "щ".repeat(41) }),
        json!({ "name": "Жидкость ГУР", "interval_km": 499 }),
        json!({ "name": "Жидкость ГУР", "interval_km": 200_001 }),
        json!({ "name": "Жидкость ГУР", "interval_days": 0 }),
        json!({ "name": "Жидкость ГУР", "interval_days": 1096 }),
    ] {
        let (s, v) = w.create(body.clone()).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{body}: {v}");
    }

    let list = ok(&w.app, "GET", "/api/v1/book-slots", &w.owner, None).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Ремень ГРМ", "Свечи"]);

    // Другая точка своих строк первой не видит, ни взять их в запись, ни править.
    let nb = ok(
        &w.app,
        "POST",
        "/api/v1/branches",
        &w.owner,
        Some(json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 016" })),
    )
    .await;
    let b = nb["id"].as_str().unwrap();
    let (s, other) = call(&w.app, "GET", "/api/v1/book-slots", &w.owner, Some(b), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(other, json!([]));
    // Там то же название свободно.
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/book-slots",
        &w.owner,
        Some(b),
        Some(json!({ "name": "Свечи" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let pid = plugs["id"].as_str().unwrap();
    let (s, _) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/book-slots/{pid}"),
        &w.owner,
        Some(b),
        Some(json!({ "hidden": true })),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let book = w
        .record(
            "2026-10-01",
            Some(100_000),
            json!([{ "slot": "oil", "text": "Mobil" }]),
        )
        .await;
    let rid = book["records"][0]["id"].as_str().unwrap();
    let (s, v) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/oil-changes/{rid}"),
        &w.owner,
        Some(b),
        Some(
            json!({ "lines": [{ "index": 0, "slot": "oil", "text": "Mobil" },
                               { "slot": "other", "text": "NGK", "custom_slot_id": pid }] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    let (s, v) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/oil-changes/{rid}"),
        &w.owner,
        None,
        Some(json!({ "lines": [{ "slot": "other", "text": "NGK", "custom_slot_id": Uuid::now_v7() }] })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn record_line_term_hide_rename_and_log(pool: PgPool) {
    let w = world(pool).await;
    let (_, plugs) = w
        .create(json!({ "name": "Свечи", "interval_km": 30_000, "interval_days": 365 }))
        .await;
    let (_, belt) = w.create(json!({ "name": "Ремень ГРМ" })).await;
    let pid = plugs["id"].as_str().unwrap();
    let bid = belt["id"].as_str().unwrap();
    let book = w
        .record(
            "2026-10-01",
            Some(100_000),
            json!([{ "slot": "oil", "text": "Mobil" }]),
        )
        .await;
    let rid = book["records"][0]["id"].as_str().unwrap();
    // Своя строка при слоте не `other` — всё равно `other`.
    let out = ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/oil-changes/{rid}"),
        &w.owner,
        Some(
            json!({ "lines": [{ "index": 0, "slot": "oil", "text": "Mobil" },
                               { "slot": "oil", "text": "NGK BKR6E", "custom_slot_id": pid },
                               { "slot": "other", "text": "Gates", "custom_slot_id": bid }] }),
        ),
    )
    .await;
    let lines = out["records"][0]["lines"].as_array().unwrap();
    assert_eq!(lines[0]["custom_slot_id"], Value::Null);
    assert_eq!(lines[0]["custom_slot_name"], Value::Null);
    assert_eq!(lines[1]["slot"], "other");
    assert_eq!(lines[1]["custom_slot_id"], json!(pid));
    assert_eq!(lines[1]["custom_slot_name"], "Свечи");
    assert_eq!(lines[1]["title"], "NGK BKR6E (Свечи)");
    assert_eq!(lines[1]["group"], "Свечи");

    // База не даёт своей строке другой вид.
    let bad =
        sqlx::query("update oil_change_items set slot = 'oil' where custom_slot_id is not null")
            .execute(&w.pool)
            .await;
    assert!(bad.is_err());

    // Срок: пробег записи + свой интервал км, дата + свои дни; без интервала — без срока.
    let n = slot_of(&out, pid).unwrap();
    assert_eq!(n["slot"], "other");
    assert_eq!(n["label"], "Свечи");
    assert_eq!(n["title"], "Свечи");
    assert_eq!(n["next_km"], 130_000);
    assert_eq!(n["next_date"], "2027-10-01");
    let n = slot_of(&out, bid).unwrap();
    assert_eq!(n["label"], "Ремень ГРМ");
    assert_eq!(n["next_km"], Value::Null);
    assert_eq!(n["next_date"], Value::Null);
    // Свои строки не попадают в общие «Прочие работы».
    assert!(
        !out["slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["slot"] == "other" && n["custom_slot_id"].is_null())
    );

    // Правка записи — было → стало с названием своей строки.
    let edit = w.audit("vehicle.oil_edit").await;
    let last = edit.last().unwrap().to_string();
    assert!(last.contains("Свечи: NGK BKR6E"), "{last}");
    assert!(last.contains("Ремень ГРМ: Gates"), "{last}");

    // Скрыли и переименовали: в списке — скрытая, в записи — новое название.
    let up = ok(
        &w.app,
        "PATCH",
        &format!("/api/v1/book-slots/{pid}"),
        &w.owner,
        Some(json!({ "name": "Свечи зажигания", "hidden": true, "interval_km": null })),
    )
    .await;
    assert_eq!(up["hidden"], true);
    assert_eq!(up["interval_km"], Value::Null);
    assert_eq!(up["interval_days"], 365);
    let list = ok(&w.app, "GET", "/api/v1/book-slots", &w.owner, None).await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == json!(pid) && c["hidden"] == true)
    );
    let book = w.book().await;
    let line = &book["records"][0]["lines"][1];
    assert_eq!(line["custom_slot_name"], "Свечи зажигания");
    let n = slot_of(&book, pid).unwrap();
    assert_eq!(n["next_km"], Value::Null);
    assert_eq!(n["next_date"], "2027-10-01");
    // Имя занято другой своей строкой — 409.
    let (s, _) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/book-slots/{pid}"),
        &w.owner,
        None,
        Some(json!({ "name": "ремень грм" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = call(
        &w.app,
        "PATCH",
        &format!("/api/v1/book-slots/{pid}"),
        &w.owner,
        None,
        Some(json!({ "interval_days": 2000 })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Журнал: создание и правка с было → стало.
    let created = w.audit("book_slot.create").await;
    assert_eq!(created.len(), 2);
    assert_eq!(created[0]["name"], "Свечи");
    let updated = w.audit("book_slot.update").await;
    assert_eq!(updated.len(), 1);
    let ch = &updated[0]["changes"];
    assert_eq!(
        ch["name"],
        json!({ "old": "Свечи", "new": "Свечи зажигания" })
    );
    assert_eq!(ch["hidden"], json!({ "old": false, "new": true }));
    assert_eq!(ch["interval_km"], json!({ "old": 30_000, "new": null }));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn rights(pool: PgPool) {
    let w = world(pool.clone()).await;
    let editor = admin(&pool, &w.app, "editor", &["clients"]).await;
    let nobody = admin(&pool, &w.app, "nobody", &[]).await;
    let manager = admin(&pool, &w.app, "manager", &["services"]).await;
    // Создаёт тот, кто правит записи книжки.
    let (s, _) = call(
        &w.app,
        "POST",
        "/api/v1/book-slots",
        &nobody,
        None,
        Some(json!({ "name": "Свечи" })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, c) = call(
        &w.app,
        "POST",
        "/api/v1/book-slots",
        &editor,
        None,
        Some(json!({ "name": "Свечи" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    let uri = format!("/api/v1/book-slots/{}", c["id"].as_str().unwrap());
    // Правит и скрывает — владелец или с правом «Услуги».
    let (s, _) = call(
        &w.app,
        "PATCH",
        &uri,
        &editor,
        None,
        Some(json!({ "hidden": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &w.app,
        "PATCH",
        &uri,
        &manager,
        None,
        Some(json!({ "hidden": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(
        &w.app,
        "PATCH",
        &uri,
        &w.owner,
        None,
        Some(json!({ "hidden": false })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Повтор с тем же `op_id` — та же строка, не дубль.
    let op = Uuid::now_v7();
    let a = ok(
        &w.app,
        "POST",
        "/api/v1/book-slots",
        &w.owner,
        Some(json!({ "op_id": op, "name": "ГУР" })),
    )
    .await;
    let b = ok(
        &w.app,
        "POST",
        "/api/v1/book-slots",
        &w.owner,
        Some(json!({ "op_id": op, "name": "ГУР" })),
    )
    .await;
    assert_eq!(a, b);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_reminds_by_name(pool: PgPool) {
    let w = world(pool).await;
    let (_, plugs) = w
        .create(json!({ "name": "Свечи", "interval_days": 30 }))
        .await;
    let pid = plugs["id"].as_str().unwrap();
    // Запись без пробега: напоминание по дням; срок — завтра.
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let date = today.checked_sub_days(Days::new(29)).unwrap();
    w.record(
        &date.to_string(),
        None,
        json!([{ "slot": "other", "text": "NGK", "custom_slot_id": pid }]),
    )
    .await;
    sqlx::query(
        "insert into telegram_customers (chat_id, party_id, phone) values (16016, $1::uuid, '0700116016')",
    )
    .bind(&w.party)
    .execute(&w.pool)
    .await
    .unwrap();
    let mut conn = w.pool.acquire().await.unwrap();
    let due = avtodom_server::api::telegram::due_reminders(&mut conn)
        .await
        .unwrap();
    let r = due
        .iter()
        .find(|r| r.chat_id == 16016)
        .expect("напоминание по своей строке");
    assert!(r.text.contains("свечи"), "{}", r.text);
    assert_eq!(r.slots, vec![format!("other:{pid}")]);
    // «Сейчас в машине» в боте — названием своей строки.
    let books = avtodom_server::api::oil_book::books(
        &mut conn,
        Some(Uuid::parse_str(&w.party).unwrap()),
        None,
    )
    .await
    .unwrap();
    let now = avtodom_server::api::oil_book::now_lines(&books[0], "");
    assert!(now.iter().any(|l| l.starts_with("Свечи — ")), "{now:?}");
}
