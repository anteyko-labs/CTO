//! Общие сотрудники филиалов (задача 045): сотрудник филиала А виден в списке филиала Б и
//! узнаётся терминалом лица Б; у филиала со «Своими сотрудниками» — только свои.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::{branches, faceid};
use avtodom_server::{app, bootstrap, build_state};

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    branch: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, Option<String>, Value) {
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
    (
        status,
        cookie,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn names(app: &Router, owner: &str, branch: Uuid) -> Vec<String> {
    let (s, _, v) = call(
        app,
        "GET",
        "/api/v1/employees",
        Some(owner),
        Some(branch),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| e["full_name"].as_str().unwrap().to_string())
        .collect()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn staff_shared_between_branches(pool: PgPool) {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let (s, cookie, _) = call(
        &app,
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({ "login": "owner", "password": "owner-pass-1" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let owner = cookie.unwrap();
    let a: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (s, _, v) = call(
        &app,
        "POST",
        "/api/v1/branches",
        Some(&owner),
        None,
        Some(json!({ "op_id": Uuid::now_v7(), "name": "Второй" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let b: Uuid = v["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(v["own_staff"], false);

    let erbol = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master, faceid_user_id) values ($1, $2, 'Эрбол', true, '12')")
        .bind(erbol).bind(a).execute(&pool).await.unwrap();

    // Виден и в Б.
    assert!(names(&app, &owner, b).await.contains(&"Эрбол".to_string()));
    // Номер терминала занят в общей группе — второму сотруднику Б не даётся.
    let other = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master) values ($1, $2, 'Азамат', true)")
        .bind(other).bind(b).execute(&pool).await.unwrap();
    let (s, _, _) = call(
        &app,
        "PUT",
        &format!("/api/v1/employees/{other}/faceid"),
        Some(&owner),
        Some(b),
        Some(json!({ "faceid_user_id": "12" })),
    )
    .await;
    assert_ne!(s, StatusCode::OK);

    // Терминал Б узнаёт сотрудника А.
    sqlx::query("insert into faceid_devices (branch_id, key) values ($1, 'key-of-branch-b-0001')")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    let body = json!({ "Events": [{ "Code": "AccessControl", "Action": "Pulse",
        "Data": { "UserID": "12", "Status": 1, "UTC": 1, "CardName": "Эрбол" } }] })
    .to_string();
    let mut conn = pool.acquire().await.unwrap();
    let out = faceid::receive(&mut conn, "key-of-branch-b-0001", body.as_bytes())
        .await
        .unwrap();
    assert_ne!(out, vec!["unknown_user".to_string()]);
    let seen: Option<Uuid> =
        sqlx::query_scalar("select employee_id from faceid_events where branch_id = $1")
            .bind(b)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seen, Some(erbol));

    // «Свои сотрудники» у Б: только свои.
    sqlx::query("update branches set own_staff = true where id = $1")
        .bind(b)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        branches::staff_branch_ids(&mut conn, b).await.unwrap(),
        vec![b]
    );
    let nb = names(&app, &owner, b).await;
    assert!(
        !nb.contains(&"Эрбол".to_string()) && nb.contains(&"Азамат".to_string()),
        "{nb:?}"
    );
    assert!(!names(&app, &owner, a).await.contains(&"Азамат".to_string()));
    let body = body
        .replace("\"UTC\": 1", "\"UTC\": 2")
        .replace("\"UTC\":1", "\"UTC\":2");
    let out = faceid::receive(&mut conn, "key-of-branch-b-0001", body.as_bytes())
        .await
        .unwrap();
    assert_eq!(out, vec!["unknown_user".to_string()]);
}
