//! Незавершённые приход и оприходование (задача 030, ADR-190): черновик свой у входа и филиала,
//! старая запись не затирает новую, проведение убирает черновик, остаток он не трогает.
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

const DRAFT: &str = "/api/v1/entry-drafts/stock_in";

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    branch: Option<Uuid>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(b) = branch {
        req = req.header("x-branch-id", b.to_string());
    }
    let res = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
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

struct World {
    app: Router,
    branch: Uuid,
    owner: String,
    /// С правом «Оприходование».
    admin: String,
    /// Без прав склада.
    plain: String,
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
        for login in ["admin1", "plain1"] {
            let id = Uuid::now_v7();
            let hash = hash_password("staff-pass-1".into()).await.unwrap();
            sqlx::query(
                "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $3)",
            )
            .bind(id)
            .bind(branch)
            .bind(login)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
            if login == "admin1" {
                sqlx::query(
                    "insert into user_permissions (user_id, branch_id, permission) values ($1, $2, 'stock_in')",
                )
                .bind(id)
                .bind(branch)
                .execute(&pool)
                .await
                .unwrap();
            }
        }
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin1", "staff-pass-1").await;
        let plain = login(&app, "plain1", "staff-pass-1").await;
        World {
            app,
            branch,
            owner,
            admin,
            plain,
        }
    }

    async fn put(&self, who: &str, op: Uuid, seq: i64, comment: &str) -> (StatusCode, Value) {
        call(
            &self.app,
            "PUT",
            DRAFT,
            who,
            None,
            json!({ "op_id": op, "seq": seq, "body": { "comment": comment } }),
        )
        .await
    }

    async fn saved(&self, who: &str) -> Value {
        let (status, body) = call(&self.app, "GET", DRAFT, who, None, Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn product(&self) -> String {
        let (_, cat) = call(
            &self.app,
            "POST",
            "/api/v1/categories",
            &self.owner,
            None,
            json!({ "name": "Прочее", "kind": "other" }),
        )
        .await;
        let (status, p) = call(
            &self.app,
            "POST",
            "/api/v1/products",
            &self.owner,
            None,
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Хомут", "sale_price_tyiyn": 10_000 }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{p}");
        p["id"].as_str().unwrap().to_string()
    }

    async fn stock(&self, product: &str) -> i64 {
        let (_, p) = call(
            &self.app,
            "GET",
            &format!("/api/v1/products/{product}"),
            &self.owner,
            None,
            Value::Null,
        )
        .await;
        p["stock_qty"].as_i64().unwrap()
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn saved_form_comes_back_and_old_save_does_not_overwrite(pool: PgPool) {
    let w = World::new(pool).await;
    assert_eq!(w.saved(&w.admin).await, Value::Null);
    let op = Uuid::now_v7();
    assert_eq!(w.put(&w.admin, op, 5, "пятая").await.1["saved"], true);
    assert_eq!(w.saved(&w.admin).await["body"]["comment"], "пятая");
    assert_eq!(w.saved(&w.admin).await["op_id"], op.to_string());
    // Опоздавшая запись с меньшим номером.
    let (status, res) = w.put(&w.admin, op, 4, "четвёртая").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(res["saved"], false);
    assert_eq!(w.saved(&w.admin).await["body"]["comment"], "пятая");
    assert_eq!(w.put(&w.admin, op, 6, "шестая").await.1["saved"], true);
    assert_eq!(w.saved(&w.admin).await["body"]["comment"], "шестая");
}

#[sqlx::test(migrations = "./migrations")]
async fn draft_is_own_and_needs_the_right(pool: PgPool) {
    let w = World::new(pool).await;
    w.put(&w.admin, Uuid::now_v7(), 1, "админа").await;
    // Чужой вход и владелец его черновика не видят (вопрос 2а).
    assert_eq!(w.saved(&w.owner).await, Value::Null);
    // Без права — 403, и на чтение, и на запись; у вида свой раздел.
    let (status, _) = call(&w.app, "GET", DRAFT, &w.plain, None, Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        w.put(&w.plain, Uuid::now_v7(), 1, "нет").await.0,
        StatusCode::FORBIDDEN
    );
    let (status, _) = call(
        &w.app,
        "GET",
        "/api/v1/entry-drafts/receipt",
        &w.admin,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &w.app,
        "GET",
        "/api/v1/entry-drafts/other",
        &w.owner,
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn draft_lives_in_its_branch(pool: PgPool) {
    let w = World::new(pool).await;
    let (status, other) = call(
        &w.app,
        "POST",
        "/api/v1/branches",
        &w.owner,
        None,
        json!({ "op_id": Uuid::now_v7(), "name": "Второй" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{other}");
    let other: Uuid = other["id"].as_str().unwrap().parse().unwrap();
    w.put(&w.owner, Uuid::now_v7(), 1, "главный").await;
    let (_, there) = call(&w.app, "GET", DRAFT, &w.owner, Some(other), Value::Null).await;
    assert_eq!(there, Value::Null);
    let (_, here) = call(&w.app, "GET", DRAFT, &w.owner, Some(w.branch), Value::Null).await;
    assert_eq!(here["body"]["comment"], "главный");
}

#[sqlx::test(migrations = "./migrations")]
async fn discard_removes_the_draft(pool: PgPool) {
    let w = World::new(pool).await;
    w.put(&w.admin, Uuid::now_v7(), 1, "лишнее").await;
    let (status, _) = call(
        &w.app,
        "POST",
        &format!("{DRAFT}/discard"),
        &w.admin,
        None,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(w.saved(&w.admin).await, Value::Null);
}

#[sqlx::test(migrations = "./migrations")]
async fn posting_removes_draft_and_is_idempotent(pool: PgPool) {
    let w = World::new(pool).await;
    let product = w.product().await;
    let op = Uuid::now_v7();
    w.put(&w.owner, op, 1, "привёз").await;
    // Черновик не двигает склад.
    assert_eq!(w.stock(&product).await, 0);
    let doc = json!({ "op_id": op, "comment": "привёз",
        "lines": [{ "product_id": product, "qty": 2, "price_tyiyn": 1_000 }] });
    // Неверный документ откатывается и черновик остаётся.
    let (status, _) = call(
        &w.app,
        "POST",
        "/api/v1/stock-ins",
        &w.owner,
        None,
        json!({ "op_id": op, "comment": "", "lines": doc["lines"] }),
    )
    .await;
    assert_eq!(status.as_u16(), 422);
    assert_eq!(w.saved(&w.owner).await["op_id"], op.to_string());

    let (status, first) = call(
        &w.app,
        "POST",
        "/api/v1/stock-ins",
        &w.owner,
        None,
        doc.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(w.saved(&w.owner).await, Value::Null);
    let (_, again) = call(&w.app, "POST", "/api/v1/stock-ins", &w.owner, None, doc).await;
    assert_eq!(first["id"], again["id"]);
    assert_eq!(w.stock(&product).await, 2);
    // Запоздавшее сохранение не возвращает проведённый документ.
    assert_eq!(
        w.put(&w.owner, op, 2, "поздно").await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(w.saved(&w.owner).await, Value::Null);
}

#[sqlx::test(migrations = "./migrations")]
async fn posting_a_receipt_removes_its_draft(pool: PgPool) {
    let w = World::new(pool).await;
    let product = w.product().await;
    let op = Uuid::now_v7();
    let uri = "/api/v1/entry-drafts/receipt";
    let (status, _) = call(
        &w.app,
        "PUT",
        uri,
        &w.owner,
        None,
        json!({ "op_id": op, "seq": 1, "body": { "supplierDoc": "12" } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, supplier) = call(
        &w.app,
        "POST",
        "/api/v1/suppliers",
        &w.owner,
        None,
        json!({ "op_id": Uuid::now_v7(), "name": "Поставщик" }),
    )
    .await;
    let (status, res) = call(
        &w.app,
        "POST",
        "/api/v1/receipts",
        &w.owner,
        None,
        json!({ "op_id": op, "supplier_id": supplier["id"], "payment": "debt", "lines": [{ "product_id": product, "qty": 3, "cost_tyiyn": 3_000 }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (_, left) = call(&w.app, "GET", uri, &w.owner, None, Value::Null).await;
    assert_eq!(left, Value::Null);
}
