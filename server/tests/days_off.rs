//! График выходных (задача 037 п.5, ADR-203): выходной ниже минимума — 409; минимум 0 — без
//! ограничений; снять можно всегда; администратору без права «График выходных» — 403, с правом —
//! можно; минимум задаёт только владелец; табель помечает выходные; событие `staff.day_off`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, bootstrap, build_state};

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
    let (s, cookie, _) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    cookie.unwrap()
}

async fn employee(pool: &PgPool, branch: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into employees (id, branch_id, full_name, is_master) values ($1, $2, $3, true)",
    )
    .bind(id)
    .bind(branch)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn off(employee: Uuid, day: &str, off: bool) -> Value {
    json!({ "op_id": Uuid::now_v7(), "employee_id": employee, "day": day, "off": off })
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn day_off_keeps_minimum_on_duty(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let a = employee(&pool, branch, "Азат").await;
    let b = employee(&pool, branch, "Бакыт").await;
    let c = employee(&pool, branch, "Чынгыз").await;
    let uri = "/api/v1/staff/days-off";

    // Минимум 0 (по умолчанию) — выходной хоть всем.
    for e in [a, b, c] {
        let (s, _, v) = call(
            &app,
            "PUT",
            uri,
            Some(&owner),
            Some(off(e, "2026-12-01", true)),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let (s, _, v) = call(
        &app,
        "GET",
        &format!("{uri}?month=2026-12"),
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["min_on_duty"], 0);
    assert_eq!(v["days_in_month"], 31);
    assert_eq!(v["employees"][0]["days_off"], json!(["2026-12-01"]));

    // Минимум задаёт только владелец; ставим 2.
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/staff",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "min_on_duty": 2 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Из трёх один выходной — остаётся 2, можно; второй — останется 1, 409.
    let (s, _, v) = call(
        &app,
        "PUT",
        uri,
        Some(&owner),
        Some(off(a, "2026-12-02", true)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["on_duty"], 2);
    let (s, _, v) = call(
        &app,
        "PUT",
        uri,
        Some(&owner),
        Some(off(b, "2026-12-02", true)),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(
        v["error"]["message"],
        "В 02.12 останется 1, нужно не менее 2"
    );
    // Снять выходной можно всегда.
    let (s, _, v) = call(
        &app,
        "PUT",
        uri,
        Some(&owner),
        Some(off(a, "2026-12-01", false)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // Событие в журнале — по каждому изменению.
    let n: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'staff.day_off'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 5);

    // Табель помечает выходные.
    let (s, _, t) = call(
        &app,
        "GET",
        "/api/v1/staff/timesheet?month=2026-12",
        Some(&owner),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let row = t["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["employee_id"] == json!(a))
        .unwrap()
        .clone();
    assert_eq!(row["days_off"], json!(["2026-12-02"]));

    // Администратор: без права — 403, с правом — можно; минимум менять нельзя.
    let (s, _, user) = call(
        &app,
        "POST",
        "/api/v1/users",
        Some(&owner),
        Some(json!({ "login": "adm", "password": "admin-pass-1", "full_name": "adm", "role": "admin" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{user}");
    let admin_id = user["id"].as_str().unwrap().to_string();
    let admin = login(&app, "adm", "admin-pass-1").await;
    let (s, _, _) = call(&app, "GET", uri, Some(&admin), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, v) = call(
        &app,
        "PUT",
        uri,
        Some(&admin),
        Some(off(c, "2026-12-03", true)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(v["error"]["message"], "Нет права: График выходных");
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/staff",
        Some(&admin),
        Some(json!({ "min_on_duty": 0 })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let mut perms: Vec<&str> = avtodom_server::auth::default_permissions();
    perms.push("staff_schedule");
    let (s, _, v) = call(
        &app,
        "PUT",
        &format!("/api/v1/users/{admin_id}/permissions"),
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "permissions": perms })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _, v) = call(
        &app,
        "PUT",
        uri,
        Some(&admin),
        Some(off(c, "2026-12-03", true)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}
