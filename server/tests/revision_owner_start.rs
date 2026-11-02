//! Ревизию начинает только владелец (задача 019, ADR-186): администратор с правом «Ревизия» и
//! личный вход сотрудника присоединяются к начатому черновику и считают в нём, но новый черновик
//! не заводят и начатый не отменяют.
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

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
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
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK, "вход {login}");
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

/// Владелец, администратор с правом «Ревизия» и личный вход сотрудника с тем же правом.
struct World {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    staff: String,
}

impl World {
    async fn new(pool: PgPool) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let branch: Uuid = sqlx::query_scalar("select id from branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        let employee = Uuid::now_v7();
        sqlx::query("insert into employees (id, branch_id, full_name, is_master) values ($1, $2, 'Алексей', true)")
            .bind(employee)
            .bind(branch)
            .execute(&pool)
            .await
            .unwrap();
        for (login, employee_id) in [("admin1", None), ("staff1", Some(employee))] {
            let id = Uuid::now_v7();
            let hash = hash_password("staff-pass-1".into()).await.unwrap();
            sqlx::query(
                "insert into users (id, branch_id, login, password_hash, role, full_name, employee_id) values ($1, $2, $3, $4, 'admin', $3, $5)",
            )
            .bind(id)
            .bind(branch)
            .bind(login)
            .bind(hash)
            .bind(employee_id)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into user_permissions (user_id, branch_id, permission) values ($1, $2, 'revision')",
            )
            .bind(id)
            .bind(branch)
            .execute(&pool)
            .await
            .unwrap();
        }
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin1", "staff-pass-1").await;
        let staff = login(&app, "staff1", "staff-pass-1").await;
        World {
            app,
            pool,
            owner,
            admin,
            staff,
        }
    }

    async fn ok(&self, who: &str, method: &str, uri: &str, body: Value) -> Value {
        let (s, v) = call(&self.app, method, uri, who, body).await;
        assert_eq!(s, StatusCode::OK, "{method} {uri}: {v}");
        v
    }

    async fn drafts(&self) -> i64 {
        sqlx::query_scalar("select count(*) from revisions where status = 'draft'")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn category(&self, name: &str) -> Value {
        let cat = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/categories",
                json!({ "name": name, "kind": "filter" }),
            )
            .await;
        cat["id"].clone()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn only_owner_starts_a_revision(pool: PgPool) {
    let w = World::new(pool).await;
    for who in [&w.admin, &w.staff] {
        let (s, v) = call(&w.app, "POST", "/api/v1/revisions", who, json!({})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"]["message"], "Ревизию начинает владелец");
    }
    assert_eq!(w.drafts().await, 0, "черновик не заведён");

    // Владелец начал — администратор и сотрудник получают тот же черновик и считают в нём.
    let cat = w.category("Фильтры").await;
    let p = w
        .ok(
            &w.owner,
            "POST",
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Фильтр", "sale_price_tyiyn": 10_000 }),
        )
        .await;
    let rev = w.ok(&w.owner, "POST", "/api/v1/revisions", json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    for who in [&w.admin, &w.staff] {
        let same = w.ok(who, "POST", "/api/v1/revisions", json!({})).await;
        assert_eq!(same["head"]["id"], rev["head"]["id"]);
        w.ok(
            who,
            "PUT",
            &format!("/api/v1/revisions/{rid}/lines"),
            json!({ "op_id": Uuid::now_v7(), "product_id": p["id"], "counted_qty": 2, "mode": "add" }),
        )
        .await;
    }
    let line = w
        .ok(
            &w.owner,
            "GET",
            &format!(
                "/api/v1/revisions/{rid}/lines/{}",
                p["id"].as_str().unwrap()
            ),
            json!(null),
        )
        .await;
    assert_eq!(line["counted_qty"], 4);
    assert_eq!(w.drafts().await, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_cannot_take_whole_store_over_open_category(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = w.category("Масла").await;
    w.ok(
        &w.owner,
        "POST",
        "/api/v1/revisions",
        json!({ "category_id": cat }),
    )
    .await;
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/revisions",
        &w.admin,
        json!({ "category_id": null }),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "revision_open");
    // Та же категория — присоединяется.
    let same = w
        .ok(
            &w.staff,
            "POST",
            "/api/v1/revisions",
            json!({ "category_id": cat }),
        )
        .await;
    assert_eq!(same["head"]["category_id"], cat);
    assert_eq!(w.drafts().await, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn only_owner_cancels_a_revision(pool: PgPool) {
    let w = World::new(pool).await;
    let rev = w.ok(&w.owner, "POST", "/api/v1/revisions", json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/cancel");
    for who in [&w.admin, &w.staff] {
        let (s, v) = call(&w.app, "POST", &uri, who, json!({})).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(
            v["error"]["message"],
            "Отменить ревизию может только владелец"
        );
    }
    assert_eq!(w.drafts().await, 1, "черновик открыт");
    w.ok(&w.owner, "POST", &uri, json!({})).await;
    assert_eq!(w.drafts().await, 0);
}
