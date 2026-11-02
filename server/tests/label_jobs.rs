//! Станция этикеток (задача 006, ADR-177): очередь заданий, повтор op_id, предел копий,
//! одна станция из двух берёт задание, чужой филиал не видит, возврат в очередь и устаревание,
//! отметка «жив» станции.
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

struct Api {
    app: Router,
}

impl Api {
    async fn call(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        branch: Option<Uuid>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-device-id", Uuid::now_v7().to_string())
            .header(header::COOKIE, cookie);
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
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, json)
    }

    async fn ok(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        branch: Option<Uuid>,
        body: Value,
    ) -> Value {
        let (s, v) = self.call(method, uri, cookie, branch, Some(body)).await;
        assert!(s.is_success(), "{method} {uri}: {s} {v}");
        v
    }

    async fn get(&self, uri: &str, cookie: &str, branch: Option<Uuid>) -> Value {
        let (s, v) = self.call("GET", uri, cookie, branch, None).await;
        assert!(s.is_success(), "GET {uri}: {s} {v}");
        v
    }

    async fn login(&self, login: &str, password: &str) -> String {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "login": login, "password": password }).to_string(),
            ))
            .unwrap();
        let res = self.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        res.headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::to_string)
            .unwrap()
    }
}

fn id(v: &Value) -> String {
    v["id"]
        .as_str()
        .unwrap_or_else(|| panic!("нет id: {v}"))
        .to_string()
}

fn op() -> Uuid {
    Uuid::now_v7()
}

/// Администратор филиала со всеми правами (SPEC-30 §5): обход проверяет филиал, а не права.
async fn admin_in(api: &Api, owner: &str, branch: Uuid, login: &str) -> String {
    let u = api
        .ok(
            "POST",
            "/api/v1/users",
            owner,
            Some(branch),
            json!({ "login": login, "password": "admin-pass-1", "full_name": format!("Админ {login}"), "role": "admin" }),
        )
        .await;
    let all: Vec<&str> = avtodom_server::auth::PERMISSIONS
        .iter()
        .map(|p| p.key)
        .collect();
    api.ok(
        "PUT",
        &format!("/api/v1/users/{}/permissions", id(&u)),
        owner,
        Some(branch),
        json!({ "op_id": op(), "permissions": all }),
    )
    .await;
    api.login(login, "admin-pass-1").await
}

struct World {
    api: Api,
    pool: PgPool,
    a: Uuid,
    b: Uuid,
    admin_a: String,
    admin_b: String,
    item: String,
}

async fn world(pool: PgPool) -> World {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let api = Api {
        app: app(build_state(pool.clone(), false).await.unwrap(), None),
    };
    let owner = api.login("owner", "owner-pass-1").await;
    let a: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let nb = api
        .ok(
            "POST",
            "/api/v1/branches",
            &owner,
            None,
            json!({ "op_id": op(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
        )
        .await;
    let b = Uuid::parse_str(&id(&nb)).unwrap();
    let admin_a = admin_in(&api, &owner, a, "admin_a").await;
    let admin_b = admin_in(&api, &owner, b, "admin_b").await;
    let cat = api
        .ok(
            "POST",
            "/api/v1/categories",
            &owner,
            Some(a),
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
    let item = id(&api
        .ok(
            "POST",
            "/api/v1/products",
            &owner,
            Some(a),
            json!({ "op_id": op(), "category_id": id(&cat), "name": "Фильтр масляный", "sale_price_tyiyn": 50_000, "barcodes": ["2000000000015"] }),
        )
        .await);
    World {
        api,
        pool,
        a,
        b,
        admin_a,
        admin_b,
        item,
    }
}

async fn create(w: &World, cookie: &str, op_id: Uuid, copies: i64) -> (StatusCode, Value) {
    w.api
        .call(
            "POST",
            "/api/v1/label-jobs",
            cookie,
            None,
            Some(json!({ "op_id": op_id, "items": [{ "product_id": w.item, "copies": copies }] })),
        )
        .await
}

async fn take(w: &World, cookie: &str, station: Uuid) -> Value {
    w.api
        .ok(
            "POST",
            "/api/v1/label-jobs/take",
            cookie,
            None,
            json!({ "station_id": station }),
        )
        .await
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn create_repeat_and_copies_limit(pool: PgPool) {
    let w = world(pool).await;
    let op_id = op();
    let (s, first) = create(&w, &w.admin_a, op_id, 3).await;
    assert_eq!(s, StatusCode::OK, "{first}");
    assert_eq!(first["status"], json!("queued"));
    let (s, again) = create(&w, &w.admin_a, op_id, 3).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["id"], first["id"], "повтор op_id — то же задание");
    let n: i64 = sqlx::query_scalar("select count(*) from label_jobs")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let audits: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'label_job.create'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(audits, 1);
    let (s, _) = create(&w, &w.admin_a, op(), 501).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = create(&w, &w.admin_a, op(), 500).await;
    assert_eq!(s, StatusCode::OK);
    let st = w
        .api
        .get(
            &format!("/api/v1/label-jobs/{}", id(&first)),
            &w.admin_a,
            None,
        )
        .await;
    assert_eq!(st["status"], json!("queued"));
    assert_eq!(st["copies"], json!(3));
    assert_eq!(st["what"], json!("Фильтр масляный"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn one_station_of_two_takes_the_job(pool: PgPool) {
    let w = world(pool).await;
    let (_, job) = create(&w, &w.admin_a, op(), 2).await;
    let (s1, s2) = (op(), op());
    let (x, y) = tokio::join!(take(&w, &w.admin_a, s1), take(&w, &w.admin_a, s2));
    let got: Vec<&Value> = [&x, &y]
        .into_iter()
        .filter(|v| !v["job"].is_null())
        .collect();
    assert_eq!(got.len(), 1, "{x} {y}");
    let j = &got[0]["job"];
    assert_eq!(j["id"], job["id"]);
    assert_eq!(j["items"][0]["copies"], json!(2));
    assert_eq!(j["items"][0]["product"]["name"], json!("Фильтр масляный"));
    assert_eq!(
        j["items"][0]["product"]["barcodes"][0],
        json!("2000000000015")
    );
    let jid = id(j);
    let station = if x["job"].is_null() { s2 } else { s1 };
    let done = w
        .api
        .ok(
            "POST",
            &format!("/api/v1/label-jobs/{jid}/done"),
            &w.admin_a,
            None,
            json!({ "station_id": station }),
        )
        .await;
    assert_eq!(done["status"], json!("done"));
    // Повтор отметки — тот же итог.
    w.api
        .ok(
            "POST",
            &format!("/api/v1/label-jobs/{jid}/done"),
            &w.admin_a,
            None,
            json!({ "station_id": station }),
        )
        .await;
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{jid}"), &w.admin_a, None)
        .await;
    assert_eq!(st["status"], json!("done"));
    assert!(take(&w, &w.admin_a, s1).await["job"].is_null());
    // Ошибка печати — текст на телефоне.
    let (_, j2) = create(&w, &w.admin_a, op(), 1).await;
    let t = take(&w, &w.admin_a, s1).await;
    assert_eq!(t["job"]["id"], j2["id"]);
    w.api
        .ok(
            "POST",
            &format!("/api/v1/label-jobs/{}/failed", id(&j2)),
            &w.admin_a,
            None,
            json!({ "station_id": s1, "error": "нет бумаги" }),
        )
        .await;
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{}", id(&j2)), &w.admin_a, None)
        .await;
    assert_eq!(st["status"], json!("failed"));
    assert_eq!(st["error"], json!("нет бумаги"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn other_branch_neither_sees_nor_takes(pool: PgPool) {
    let w = world(pool).await;
    let (_, job) = create(&w, &w.admin_a, op(), 1).await;
    let jid = id(&job);
    let (s, _) = w
        .api
        .call(
            "GET",
            &format!("/api/v1/label-jobs/{jid}"),
            &w.admin_b,
            None,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let list = w.api.get("/api/v1/label-jobs", &w.admin_b, None).await;
    assert_eq!(list["jobs"], json!([]));
    let sb = op();
    assert!(take(&w, &w.admin_b, sb).await["job"].is_null());
    let (s, _) = w
        .api
        .call(
            "POST",
            &format!("/api/v1/label-jobs/{jid}/done"),
            &w.admin_b,
            None,
            Some(json!({ "station_id": sb })),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // Станция Б не «в сети» для филиала А.
    let list = w.api.get("/api/v1/label-jobs", &w.admin_a, None).await;
    assert_eq!(list["station_online"], json!(false));
    assert_eq!(list["jobs"][0]["id"], job["id"]);
    // Номер станции Б из филиала А не перехватывается.
    let (s, _) = w
        .api
        .call(
            "POST",
            "/api/v1/label-jobs/take",
            &w.admin_a,
            None,
            Some(json!({ "station_id": sb })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let _ = (w.a, w.b);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stuck_printing_requeues_and_old_queued_expires(pool: PgPool) {
    let w = world(pool).await;
    let (_, job) = create(&w, &w.admin_a, op(), 1).await;
    let jid = id(&job);
    let s1 = op();
    assert_eq!(take(&w, &w.admin_a, s1).await["job"]["id"], job["id"]);
    // Станция перезагрузилась: задание в «печатается» дольше 2 минут — снова в очереди.
    sqlx::query(
        "update label_jobs set taken_at = now() - interval '3 minutes' where id = $1::uuid",
    )
    .bind(&jid)
    .execute(&w.pool)
    .await
    .unwrap();
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{jid}"), &w.admin_a, None)
        .await;
    assert_eq!(st["status"], json!("queued"));
    assert_eq!(take(&w, &w.admin_a, op()).await["job"]["id"], job["id"]);
    // Старше суток в очереди — «устарело».
    let (_, old) = create(&w, &w.admin_a, op(), 1).await;
    sqlx::query(
        "update label_jobs set created_at = now() - interval '25 hours' where id = $1::uuid",
    )
    .bind(id(&old))
    .execute(&w.pool)
    .await
    .unwrap();
    let st = w
        .api
        .get(
            &format!("/api/v1/label-jobs/{}", id(&old)),
            &w.admin_a,
            None,
        )
        .await;
    assert_eq!(st["status"], json!("failed"));
    assert_eq!(st["error"], json!("устарело"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn station_last_seen_and_offline(pool: PgPool) {
    let w = world(pool).await;
    let (_, job) = create(&w, &w.admin_a, op(), 1).await;
    let jid = id(&job);
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{jid}"), &w.admin_a, None)
        .await;
    assert_eq!(st["station_online"], json!(false), "станции ещё не было");
    let s1 = op();
    take(&w, &w.admin_a, s1).await;
    let seen: i64 = sqlx::query_scalar(
        "select count(*) from label_stations where id = $1 and last_seen > now() - interval '5 seconds'",
    )
    .bind(s1)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(seen, 1);
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{jid}"), &w.admin_a, None)
        .await;
    assert_eq!(st["station_online"], json!(true));
    sqlx::query("update label_stations set last_seen = now() - interval '31 seconds'")
        .execute(&w.pool)
        .await
        .unwrap();
    let st = w
        .api
        .get(&format!("/api/v1/label-jobs/{jid}"), &w.admin_a, None)
        .await;
    assert_eq!(
        st["station_online"],
        json!(false),
        "не отмечалась > 30 с — не в сети"
    );
}
