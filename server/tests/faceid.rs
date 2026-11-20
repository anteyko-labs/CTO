//! Рабочий день по лицу (задача 039 A, ADR-200): кнопка в боте → событие терминала Dahua в окне →
//! день начат, работнику сообщение; вне окна — нет; чужой ключ и неизвестный номер — 200 без
//! действия; повтор события — одно действие; табель считает часы, незакрытый день — без часов,
//! закрытие владельцем.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::bot_staff::{BTN_WORK_START, BTN_WORK_STOP};
use avtodom_server::api::faceid;
use avtodom_server::api::telegram::{self, Keys};
use avtodom_server::{app, bootstrap, build_state};

const CHAT: i64 = 39_001;

struct World {
    pool: PgPool,
    app: Router,
    owner: String,
    branch: Uuid,
    employee: Uuid,
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

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Value {
    let (s, _, v) = call(app, method, uri, Some(cookie), body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
    v
}

async fn raw_post(app: &Router, uri: &str, body: &str) -> StatusCode {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap().status()
}

async fn world(pool: PgPool) -> World {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let (s, cookie, _) = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": "owner", "password": "owner-pass-1" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let employee = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master, phone) values ($1, $2, 'Эрбол', true, '+996700123456')")
        .bind(employee)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_employees (chat_id, employee_id, branch_id, phone) values ($1, $2, $3, '+996700123456')")
        .bind(CHAT)
        .bind(employee)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    World {
        pool,
        app,
        owner: cookie.unwrap(),
        branch,
        employee,
    }
}

impl World {
    async fn say(&self, text: &str) -> telegram::Reply {
        let mut conn = self.pool.acquire().await.unwrap();
        telegram::handle_message(&mut conn, CHAT, CHAT, Some(text), None)
            .await
            .unwrap()
    }

    /// Адрес терминала (заводится при первом открытии настроек) и номер сотрудника «12».
    async fn setup_terminal(&self) -> String {
        let s = ok(
            &self.app,
            "GET",
            "/api/v1/settings/faceid",
            &self.owner,
            None,
        )
        .await;
        assert_eq!(s["window_min"], 3);
        ok(
            &self.app,
            "PUT",
            &format!("/api/v1/employees/{}/faceid", self.employee),
            &self.owner,
            Some(json!({ "faceid_user_id": "12" })),
        )
        .await;
        s["path"].as_str().unwrap().to_string()
    }

    async fn sessions(&self) -> Vec<(Option<chrono::DateTime<chrono::Utc>>,)> {
        sqlx::query_as(
            "select ended_at from work_sessions where employee_id = $1 order by started_at",
        )
        .bind(self.employee)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn notices(&self) -> Vec<faceid::Notice> {
        let mut conn = self.pool.acquire().await.unwrap();
        let n = faceid::notices(&mut conn).await.unwrap();
        for x in &n {
            faceid::mark_notice(&mut conn, x.id).await.unwrap();
        }
        n
    }
}

fn event(user: &str, utc: i64) -> String {
    json!({ "Events": [{ "Code": "AccessControl", "Action": "Pulse",
        "Data": { "UserID": user, "Status": 1, "UTC": utc, "CardName": "Эрбол" } }] })
    .to_string()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn button_then_face_starts_and_ends_day(pool: PgPool) {
    let w = world(pool).await;
    // Терминал не настроен — кнопка объясняет.
    let r = w.say(BTN_WORK_START).await;
    assert!(r.text.contains("не настроен"), "{}", r.text);
    let path = w.setup_terminal().await;
    assert_eq!(w.say("/сегодня").await.keys, Keys::Employee);

    // Лицо без кнопки день не начинает — только журнал терминала.
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 1)).await,
        StatusCode::OK
    );
    assert!(w.sessions().await.is_empty());

    let r = w.say(BTN_WORK_START).await;
    assert!(r.text.contains("посмотрите в камеру (3 мин)"), "{}", r.text);
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 2)).await,
        StatusCode::OK
    );
    let s = w.sessions().await;
    assert_eq!(s.len(), 1);
    assert!(s[0].0.is_none());
    let n = w.notices().await;
    assert_eq!(n.len(), 1);
    assert!(
        n[0].text.starts_with("✅ Рабочий день начат в "),
        "{}",
        n[0].text
    );
    assert_eq!(n[0].keys, Some(Keys::EmployeeWorking));
    // Внизу теперь «Закончить».
    assert_eq!(w.say("/сегодня").await.keys, Keys::EmployeeWorking);
    assert!(w.say(BTN_WORK_START).await.text.contains("уже начат"));

    // Повтор того же события — без второго действия (заявку «закончить» он не трогает).
    w.say(BTN_WORK_STOP).await;
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 2)).await,
        StatusCode::OK
    );
    assert!(w.sessions().await[0].0.is_none());
    // Новое событие — день закончен.
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 3)).await,
        StatusCode::OK
    );
    assert!(w.sessions().await[0].0.is_some());
    let n = w.notices().await;
    assert!(
        n[0].text.starts_with("✅ Рабочий день закончен в "),
        "{}",
        n[0].text
    );
    assert_eq!(w.say("/сегодня").await.keys, Keys::Employee);

    // Событие владельцу staff.workday — два, в ленту бота по умолчанию не идёт.
    let feed: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'staff.workday'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(feed, 2);
    let outcomes: Vec<String> = sqlx::query_scalar(
        "select outcome from faceid_events where branch_id = $1 order by created_at",
    )
    .bind(w.branch)
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(outcomes, ["no_request", "applied", "applied"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn outside_window_and_strangers_do_nothing(pool: PgPool) {
    let w = world(pool).await;
    let path = w.setup_terminal().await;
    w.say(BTN_WORK_START).await;
    // Окно прошло.
    sqlx::query("update workday_requests set expires_at = now() - interval '1 second'")
        .execute(&w.pool)
        .await
        .unwrap();
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 10)).await,
        StatusCode::OK
    );
    assert!(w.sessions().await.is_empty());
    let n = w.notices().await;
    assert_eq!(n.len(), 1);
    assert!(n[0].text.contains("Не подтверждено лицом"), "{}", n[0].text);

    // Чужой ключ и неизвестный номер — 200 терминалу, без действия, запись в журнал.
    w.say(BTN_WORK_START).await;
    assert_eq!(
        raw_post(
            &w.app,
            "/api/v1/faceid/dahua/not-a-real-key-000000",
            &event("12", 11)
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        raw_post(&w.app, &path, &event("99", 12)).await,
        StatusCode::OK
    );
    // Не JSON вовсе — тоже 200.
    assert_eq!(raw_post(&w.app, &path, "garbage").await, StatusCode::OK);
    assert!(w.sessions().await.is_empty());
    let outcomes: Vec<String> =
        sqlx::query_scalar("select outcome from faceid_events order by created_at")
            .fetch_all(&w.pool)
            .await
            .unwrap();
    assert!(outcomes.contains(&"bad_key".to_string()), "{outcomes:?}");
    assert!(
        outcomes.contains(&"unknown_user".to_string()),
        "{outcomes:?}"
    );
    // Неудачное распознавание (Status 0) — тоже без действия.
    let failed = json!({ "Events": [{ "Code": "AccessControl",
        "Data": { "UserID": "12", "Status": 0, "UTC": 13 } }] })
    .to_string();
    assert_eq!(raw_post(&w.app, &path, &failed).await, StatusCode::OK);
    assert!(w.sessions().await.is_empty());
    // Сменили ключ — старый адрес больше не действует.
    let s = ok(
        &w.app,
        "PUT",
        "/api/v1/settings/faceid",
        &w.owner,
        Some(json!({ "new_key": true, "window_min": 5 })),
    )
    .await;
    assert_ne!(s["path"].as_str().unwrap(), path);
    assert_eq!(s["window_min"], 5);
    assert_eq!(
        raw_post(&w.app, &path, &event("12", 14)).await,
        StatusCode::OK
    );
    assert!(w.sessions().await.is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn timesheet_counts_hours_and_owner_closes(pool: PgPool) {
    let w = world(pool).await;
    w.setup_terminal().await;
    // Закрытый день 2026-10-05: 09:00–17:30 по Бишкеку; незакрытый 2026-10-06.
    sqlx::query(
        "insert into work_sessions (id, employee_id, branch_id, started_at, ended_at) values
         ($1, $3, $4, '2026-10-05 09:00+06', '2026-10-05 17:30+06'),
         ($2, $3, $4, '2026-10-06 09:00+06', null)",
    )
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .bind(w.employee)
    .bind(w.branch)
    .execute(&w.pool)
    .await
    .unwrap();
    let t = ok(
        &w.app,
        "GET",
        "/api/v1/staff/timesheet?month=2026-10",
        &w.owner,
        None,
    )
    .await;
    assert_eq!(t["days_in_month"], 31);
    let row = &t["rows"][0];
    assert_eq!(row["full_name"], "Эрбол");
    assert_eq!(row["total_minutes"], 510);
    let days = row["days"].as_array().unwrap();
    assert_eq!(days[0]["date"], "2026-10-05");
    assert_eq!(days[0]["minutes"], 510);
    assert_eq!(days[1]["minutes"], 0);
    assert_eq!(days[1]["unclosed"], true);
    let open = &days[1]["sessions"][0];
    assert_eq!(open["status"], "unclosed");
    let id = open["id"].as_str().unwrap();

    // Без причины — нельзя; с причиной — закрыт, часы считаются, в журнал.
    let (s, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/staff/work-sessions/{id}/close"),
        Some(&w.owner),
        Some(json!({ "ended_at": "2026-10-06T12:00:00Z", "reason": " " })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/staff/work-sessions/{id}/close"),
        &w.owner,
        Some(json!({ "ended_at": "2026-10-06T12:00:00Z", "reason": "забыл нажать" })),
    )
    .await;
    let t = ok(
        &w.app,
        "GET",
        "/api/v1/staff/timesheet?month=2026-10",
        &w.owner,
        None,
    )
    .await;
    // 09:00+06 = 03:00Z → 12:00Z: 9 часов.
    assert_eq!(t["rows"][0]["total_minutes"], 510 + 540);
    assert_eq!(
        t["rows"][0]["days"][1]["sessions"][0]["close_reason"],
        "забыл нажать"
    );
    let audit: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'staff.workday_close'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(audit, 1);
    // Закрыть второй раз — нельзя.
    let (s, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/staff/work-sessions/{id}/close"),
        Some(&w.owner),
        Some(json!({ "ended_at": "2026-10-06T12:00:00Z", "reason": "ещё" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_needs_timesheet_right(pool: PgPool) {
    let w = world(pool).await;
    let hash = avtodom_server::auth::hash_password("admin-pass-1".into())
        .await
        .unwrap();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'adm', $3, 'admin', 'adm')")
        .bind(Uuid::now_v7())
        .bind(w.branch)
        .bind(hash)
        .execute(&w.pool)
        .await
        .unwrap();
    let (_, cookie, _) = call(
        &w.app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": "adm", "password": "admin-pass-1" })),
    )
    .await;
    let cookie = cookie.unwrap();
    for uri in ["/api/v1/staff/timesheet", "/api/v1/settings/faceid"] {
        let (s, _, _) = call(&w.app, "GET", uri, Some(&cookie), None).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{uri}");
    }
}
