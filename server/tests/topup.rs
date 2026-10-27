//! Доливка на вынос (SPEC-36 §4, ADR-118): флажок услуги, чек «На вынос» с доливкой без клиента,
//! мастер по желанию, остальные услуги на вынос — 422, доливка в чеке «В сервис» не пишется в
//! книжку, флажок в снимке кассы.
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

async fn setup(pool: PgPool) -> Router {
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
    app(build_state(pool, false).await.unwrap(), None)
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

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (status, cookie, _) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    cookie.unwrap()
}

async fn send(app: &Router, method: &str, who: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, method, uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn get(app: &Router, who: &str, uri: &str) -> Value {
    let (s, _, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: Value,
    master: Value,
    party: Value,
    vehicle: Value,
    atf: Value,
    /// Доливка КПП снизу (флажок по названию) и замена масла в моторе (без флажка).
    topup: Value,
    change: Value,
}

async fn world(pool: PgPool) -> W {
    let app = setup(pool.clone()).await;
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let gear = send(
        &app,
        "POST",
        &owner,
        "/api/v1/categories",
        json!({ "name": "Трансмиссионные масла", "kind": "oil" }),
    )
    .await;
    let atf = send(
        &app,
        "POST",
        &owner,
        "/api/v1/products",
        json!({ "op_id": Uuid::now_v7(), "category_id": gear["id"], "name": "ATF Dexron III",
                "container_ml": 4000, "sale_price_tyiyn": 160_000 }),
    )
    .await;
    let svc =
        |name: &str| json!({ "name": name, "price_tyiyn": 20_000, "master_fee_tyiyn": 3_000 });
    let topup = send(
        &app,
        "POST",
        &owner,
        "/api/v1/services",
        svc("Доливка КПП снизу"),
    )
    .await;
    let change = send(
        &app,
        "POST",
        &owner,
        "/api/v1/services",
        svc("Замена масла в моторе"),
    )
    .await;
    let cashier = send(
        &app,
        "POST",
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Кассир Доливки", "is_cashier": true, "pin": "1234" }),
    )
    .await;
    let master = send(
        &app,
        "POST",
        &owner,
        "/api/v1/employees",
        json!({ "full_name": "Мастер Доливки", "is_master": true }),
    )
    .await;
    let party = send(
        &app,
        "POST",
        &owner,
        "/api/v1/parties",
        json!({ "kind": "person", "first_name": "Бакыт", "last_name": "Доливкин", "phone": "0700223344" }),
    )
    .await;
    let vehicle = send(
        &app,
        "POST",
        &owner,
        &format!("/api/v1/parties/{}/vehicles", party["id"].as_str().unwrap()),
        json!({ "plate": "01KG118TOP", "brand": "Toyota", "model": "Camry" }),
    )
    .await;
    W {
        app,
        pool,
        owner,
        admin,
        cashier,
        master,
        party,
        vehicle,
        atf,
        topup,
        change,
    }
}

impl W {
    fn service_line(s: &Value) -> Value {
        json!({ "kind": "service", "gift": false, "service_id": s["id"], "qty": 1, "unit_price_tyiyn": s["price_tyiyn"] })
    }

    /// Чек «На вынос»: услуги и розлив ATF (мл), мастер по желанию.
    fn takeaway(&self, services: &[&Value], atf_ml: i64, master: bool) -> Value {
        let mut lines: Vec<Value> = services.iter().map(|s| Self::service_line(s)).collect();
        let mut total: i64 = services
            .iter()
            .map(|s| s["price_tyiyn"].as_i64().unwrap())
            .sum();
        if atf_ml > 0 {
            lines.push(json!({ "kind": "pour", "gift": false, "product_id": self.atf["id"], "qty": atf_ml, "unit_price_tyiyn": 40_000 }));
            total += 40_000 * atf_ml / 1000;
        }
        let mut body = json!({
            "op_id": Uuid::now_v7(),
            "sale_type": "takeaway",
            "cashier_id": self.cashier["id"],
            "lines": lines,
            "payments": [{ "method": "cash", "amount_tyiyn": total }],
        });
        if master {
            body["master_id"] = self.master["id"].clone();
        }
        body
    }

    async fn post_sale(&self, body: Value) -> (StatusCode, Value) {
        let (s, _, v) = call(
            &self.app,
            "POST",
            "/api/v1/sales",
            Some(&self.owner),
            Some(body),
        )
        .await;
        (s, v)
    }

    async fn master_accrued(&self, sale_id: &Value) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where doc_id = $1 and kind = 'service_fee'",
        )
        .bind(Uuid::parse_str(sale_id.as_str().unwrap()).unwrap())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn migration_marks_active_topup_services(pool: PgPool) {
    // Услуги, заведённые до миграции 0070 (флажка ещё не было): ставим их и снимаем флажок, затем
    // повторяем её `update`.
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    for (name, active) in [
        ("Доливка масла в мотор", true),
        ("ДОЛИВКА антифриза", true),
        ("Доливка старая", false),
        ("Замена масла в моторе", true),
    ] {
        sqlx::query("insert into services (id, branch_id, name, price_tyiyn, active) values ($1, $2, $3, 0, $4)")
            .bind(Uuid::now_v7())
            .bind(branch)
            .bind(name)
            .bind(active)
            .execute(&pool)
            .await
            .unwrap();
    }
    let sql = include_str!("../migrations/0070_service_takeaway.sql");
    let update = sql
        .lines()
        .find(|l| l.starts_with("update services"))
        .unwrap();
    sqlx::query(update).execute(&pool).await.unwrap();
    let marked: Vec<String> =
        sqlx::query_scalar("select name from services where takeaway order by name")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(marked, vec!["ДОЛИВКА антифриза", "Доливка масла в мотор"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn flag_by_name_and_only_owner_changes_it(pool: PgPool) {
    let w = world(pool).await;
    assert_eq!(w.topup["takeaway"], true);
    assert_eq!(w.change["takeaway"], false);
    let list = get(&w.app, &w.owner, "/api/v1/services").await;
    assert!(
        list.as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == w.topup["id"] && s["takeaway"] == true)
    );
    // Администратор флажок не меняет, а форма с прежним значением проходит.
    let uri = format!("/api/v1/services/{}", w.change["id"].as_str().unwrap());
    let (s, _, _) = call(
        &w.app,
        "PATCH",
        &uri,
        Some(&w.admin),
        Some(json!({ "takeaway": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    send(
        &w.app,
        "PATCH",
        &w.admin,
        &uri,
        json!({ "name": "Замена масла в моторе", "takeaway": false }),
    )
    .await;
    let (s, _, _) = call(
        &w.app,
        "POST",
        "/api/v1/services",
        Some(&w.admin),
        Some(json!({ "name": "Мойка", "price_tyiyn": 1_000, "takeaway": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Администратор заводит «Доливку» — флажок по названию.
    let admin_topup = send(
        &w.app,
        "POST",
        &w.admin,
        "/api/v1/services",
        json!({ "name": "Доливка тосола", "price_tyiyn": 1_000 }),
    )
    .await;
    assert_eq!(admin_topup["takeaway"], true);
    // Владелец ставит и снимает.
    let out = send(&w.app, "PATCH", &w.owner, &uri, json!({ "takeaway": true })).await;
    assert_eq!(out["takeaway"], true);
    let out = send(
        &w.app,
        "PATCH",
        &w.owner,
        &uri,
        json!({ "takeaway": false }),
    )
    .await;
    assert_eq!(out["takeaway"], false);
    let out = send(
        &w.app,
        "POST",
        &w.owner,
        "/api/v1/services",
        json!({ "name": "Доливка по просьбе", "price_tyiyn": 1_000, "takeaway": false }),
    )
    .await;
    assert_eq!(out["takeaway"], false);
    // Флажок в снимке кассы — работает без сети.
    let snap = get(&w.app, &w.owner, "/api/v1/offline/snapshot").await;
    let snap_topup = snap["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == w.topup["id"])
        .unwrap()
        .clone();
    assert_eq!(snap_topup["takeaway"], true);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn topup_sells_takeaway_master_optional(pool: PgPool) {
    let w = world(pool).await;
    // Без мастера: чек без клиента, ставки мастеру нет.
    let (s, sale) = w.post_sale(w.takeaway(&[&w.topup], 1000, false)).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(sale["sale_type"], "takeaway");
    assert_eq!(sale["master_fee_tyiyn"], 0);
    assert!(
        sale["lines"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["master_fee_tyiyn"] == 0)
    );
    assert_eq!(w.master_accrued(&sale["id"]).await, 0);
    // С мастером — ставка услуги.
    let (s, sale) = w.post_sale(w.takeaway(&[&w.topup], 0, true)).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    assert_eq!(sale["master_fee_tyiyn"], 3_000);
    assert_eq!(w.master_accrued(&sale["id"]).await, 3_000);
    // Мастер на вынос без услуги — по-прежнему нельзя.
    let (s, v) = w.post_sale(w.takeaway(&[], 500, true)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    // Книжки у чека на вынос нет.
    let book = get(
        &w.app,
        &w.owner,
        &format!(
            "/api/v1/vehicles/{}/oil-book",
            w.vehicle["id"].as_str().unwrap()
        ),
    )
    .await;
    assert!(book["records"].as_array().unwrap().is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn other_services_takeaway_rejected_online_accepted_offline(pool: PgPool) {
    let w = world(pool).await;
    let (s, v) = w
        .post_sale(w.takeaway(&[&w.topup, &w.change], 0, false))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(
        v.to_string().contains("Замена масла в моторе"),
        "в ошибке — услуга: {v}"
    );
    // Без сети чек уже состоялся (флажок могли снять, пока касса была без связи): проводится,
    // владельцу — причина.
    let mut body = w.takeaway(&[&w.change], 0, false);
    body["offline"] = json!(true);
    body["client_time"] = json!(chrono::Utc::now());
    let (s, sale) = w.post_sale(body).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
    let reasons: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid and data::text like '%на вынос с услугой%'",
    )
    .bind(sale["id"].as_str().unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(reasons, 1);
    // Доливка без сети — как онлайн.
    let mut body = w.takeaway(&[&w.topup], 500, false);
    body["offline"] = json!(true);
    body["client_time"] = json!(chrono::Utc::now());
    let (s, sale) = w.post_sale(body).await;
    assert_eq!(s, StatusCode::OK, "{sale}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn topup_in_service_sale_not_in_book(pool: PgPool) {
    let w = world(pool).await;
    // Владелец дал доливке строку книжки «коробка» — доливка всё равно не замена.
    send(
        &w.app,
        "PATCH",
        &w.owner,
        &format!("/api/v1/services/{}", w.topup["id"].as_str().unwrap()),
        json!({ "book_slot": "gear_oil" }),
    )
    .await;
    let lines = vec![
        W::service_line(&w.change),
        W::service_line(&w.topup),
        json!({ "kind": "pour", "gift": false, "product_id": w.atf["id"], "qty": 1000, "unit_price_tyiyn": 40_000 }),
    ];
    let sale = send(
        &w.app,
        "POST",
        &w.owner,
        "/api/v1/sales",
        json!({
            "op_id": Uuid::now_v7(),
            "client_time": chrono::Utc::now(),
            "sale_type": "service",
            "cashier_id": w.cashier["id"],
            "master_id": w.master["id"],
            "party_id": w.party["id"],
            "vehicle_id": w.vehicle["id"],
            "mileage": 120_000,
            "mileage_unit": "km",
            "lines": lines,
            "payments": [{ "method": "cash", "amount_tyiyn": 80_000 }],
        }),
    )
    .await;
    // Мастеру — ставки обеих услуг.
    assert_eq!(w.master_accrued(&sale["id"]).await, 6_000);
    let book = get(
        &w.app,
        &w.owner,
        &format!(
            "/api/v1/vehicles/{}/oil-book",
            w.vehicle["id"].as_str().unwrap()
        ),
    )
    .await;
    let rec = book["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["sale_id"] == sale["id"])
        .unwrap();
    let slots: Vec<&str> = rec["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["slot"].as_str().unwrap())
        .collect();
    assert_eq!(slots, vec!["oil"], "{rec}");
}
