//! Задача 005 Н1: личный вход сотрудника (ADR-170). Вход заводит владелец из карточки
//! сотрудника; права — свои (урезание ADR-152 только у общего входа); отключение сотрудника режет
//! сессии; «Сменить свой пароль»; кассир открывает смену на себя без PIN и не пробивает онлайн-чек
//! в чужой смене; «Мой заработок» — только своё; предел попыток входа с одного адреса.
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

async fn try_login(app: &Router, login: &str, password: &str) -> (StatusCode, Option<String>) {
    let (s, c, _) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    (s, c)
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = try_login(app, login, password).await;
    assert_eq!(s, StatusCode::OK, "вход {login}");
    c.unwrap()
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    /// Общий вход кассы — администратор без сотрудника.
    kassa: String,
    kassa_id: Uuid,
}

impl W {
    async fn new(pool: PgPool) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let branch: Uuid = sqlx::query_scalar("select id from branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        let hash = hash_password("kassa-pass-1".into()).await.unwrap();
        let kassa_id = Uuid::now_v7();
        sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'kassa', $3, 'admin', 'Касса')")
            .bind(kassa_id)
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
            .bind(kassa_id)
            .bind(branch)
            .bind(keys)
            .execute(&pool)
            .await
            .unwrap();
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let kassa = login(&app, "kassa", "kassa-pass-1").await;
        W {
            app,
            pool,
            owner,
            kassa,
            kassa_id,
        }
    }

    async fn req(
        &self,
        method: &str,
        who: &str,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (s, _, v) = call(&self.app, method, uri, Some(who), body).await;
        (s, v)
    }

    async fn ok(&self, method: &str, who: &str, uri: &str, body: Value) -> Value {
        let (s, v) = self.req(method, who, uri, Some(body)).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn get(&self, who: &str, uri: &str) -> Value {
        let (s, v) = self.req("GET", who, uri, None).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    async fn scalar(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sql).fetch_one(&self.pool).await.unwrap()
    }

    async fn employee(&self, name: &str, cashier: bool, pin: Option<&str>) -> String {
        let mut body = json!({ "full_name": name, "is_cashier": cashier, "is_master": !cashier });
        if let Some(p) = pin {
            body["pin"] = json!(p);
        }
        let e = self
            .ok("POST", &self.owner, "/api/v1/employees", body)
            .await;
        e["id"].as_str().unwrap().to_string()
    }

    /// Вход из карточки сотрудника: ответ `POST /users`.
    async fn personal(&self, employee: &str, login: &str) -> Value {
        self.ok(
            "POST",
            &self.owner,
            "/api/v1/users",
            json!({ "op_id": Uuid::now_v7(), "login": login, "password": format!("{login}-pass-1"), "employee_id": employee }),
        )
        .await
    }

    async fn set_permissions(&self, user: &str, keys: &[&str]) {
        self.ok(
            "PUT",
            &self.owner,
            &format!("/api/v1/users/{user}/permissions"),
            json!({ "op_id": Uuid::now_v7(), "permissions": keys }),
        )
        .await;
    }
}

/// Товар с остатком: чек без минуса.
async fn stocked_item(w: &W) -> String {
    let cat = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/categories",
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
    let p = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 100_000 }),
        )
        .await;
    let accounts = w.get(&w.owner, "/api/v1/cash/accounts").await;
    let safe = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["owner_only"] == json!(true))
        .unwrap()["id"]
        .clone();
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/receipts",
        json!({ "op_id": Uuid::now_v7(), "paid_from": safe,
                "lines": [{ "product_id": p["id"], "qty": 20, "cost_tyiyn": 20 * 60_000 }] }),
    )
    .await;
    p["id"].as_str().unwrap().to_string()
}

fn sale_body(cashier: &str, item: &str, price: i64) -> Value {
    json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": price }],
            "payments": [{ "method": "cash", "amount_tyiyn": price }] })
}

#[sqlx::test(migrations = "./migrations")]
async fn owner_creates_login_from_employee_card(pool: PgPool) {
    let w = W::new(pool).await;
    let aibek = w.employee("Айбек", true, Some("2222")).await;

    // Только владелец заводит вход.
    let (s, _) = w
        .req(
            "POST",
            &w.kassa,
            "/api/v1/users",
            Some(json!({ "op_id": Uuid::now_v7(), "login": "aibek", "password": "aibek-pass-1", "employee_id": aibek })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Пароль короче 8 — отказ.
    let (s, _) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/users",
            Some(json!({ "op_id": Uuid::now_v7(), "login": "aibek", "password": "short", "employee_id": aibek })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Вход сотрудника — только администратор, не владелец.
    let (s, _) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/users",
            Some(
                json!({ "op_id": Uuid::now_v7(), "login": "aibek", "password": "aibek-pass-1",
                         "employee_id": aibek, "role": "owner" }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Двойное нажатие «Завести вход»: тот же ответ, а не «логин занят», один вход и одна запись.
    let op = Uuid::now_v7();
    let body =
        json!({ "op_id": op, "login": "aibek", "password": "aibek-pass-1", "employee_id": aibek });
    let first = w.ok("POST", &w.owner, "/api/v1/users", body.clone()).await;
    let again = w.ok("POST", &w.owner, "/api/v1/users", body).await;
    assert_eq!(first, again);
    assert_eq!(first["role"], json!("admin"));
    assert_eq!(first["full_name"], json!("Айбек"));
    assert_eq!(first["employee_id"], json!(aibek));
    assert_eq!(first["employee_name"], json!("Айбек"));
    // Права по умолчанию — как у нового администратора.
    assert_eq!(
        first["permissions"].as_array().unwrap().len(),
        avtodom_server::auth::default_permissions().len()
    );
    assert_eq!(
        w.scalar("select count(*) from users where login = 'aibek'")
            .await,
        1
    );
    assert_eq!(
        w.scalar("select count(*) from audit_log where action = 'user.create'")
            .await,
        1
    );
    let audit: Value =
        sqlx::query_scalar("select data from audit_log where action = 'user.create'")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(audit["employee_name"], json!("Айбек"));

    // Другой op_id с тем же логином — «логин занят»; второй вход тому же сотруднику — отказ.
    let (s, e) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/users",
            Some(
                json!({ "op_id": Uuid::now_v7(), "login": "aibek", "password": "aibek-pass-1",
                         "full_name": "Другой" }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    let (s, e) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/users",
            Some(
                json!({ "op_id": Uuid::now_v7(), "login": "aibek2", "password": "aibek-pass-1",
                         "employee_id": aibek }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("уже есть вход"),
        "{e}"
    );

    // Повтор правки входа с тем же op_id — тот же ответ.
    let uid = first["id"].as_str().unwrap();
    let edit = json!({ "op_id": Uuid::now_v7(), "login": "aibek.k" });
    let a = w
        .ok(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{uid}"),
            edit.clone(),
        )
        .await;
    let b = w
        .ok("PATCH", &w.owner, &format!("/api/v1/users/{uid}"), edit)
        .await;
    assert_eq!(a, b);
    assert_eq!(a["login"], json!("aibek.k"));
    assert_eq!(a["employee_name"], json!("Айбек"));
    // Вход сотрудника не становится владельцем.
    let (s, _) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{uid}"),
            Some(json!({ "op_id": Uuid::now_v7(), "role": "owner" })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Сотрудник входит своим логином: /auth/me знает его карточку.
    let me_cookie = login(&w.app, "aibek.k", "aibek-pass-1").await;
    let me = w.get(&me_cookie, "/api/v1/auth/me").await;
    assert_eq!(me["employee_id"], json!(aibek));
    assert_eq!(me["employee_name"], json!("Айбек"));
    assert_eq!(me["role"], json!("admin"));
    // Общий вход — без сотрудника.
    let kassa = w.get(&w.kassa, "/api/v1/auth/me").await;
    assert_eq!(kassa["employee_id"], Value::Null);

    // Карточка у владельца — с входом; у администратора — без.
    let list = w.get(&w.owner, "/api/v1/employees").await;
    let card = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(aibek))
        .unwrap();
    assert_eq!(card["login"]["login"], json!("aibek.k"));
    assert_eq!(card["login"]["active"], json!(true));
    let list = w.get(&w.kassa, "/api/v1/employees").await;
    let card = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == json!(aibek))
        .unwrap();
    assert!(card.get("login").is_none(), "{card}");
    // «Пользователи» показывают сотрудника входа.
    let users = w.get(&w.owner, "/api/v1/users").await;
    let row = users
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["id"] == json!(uid))
        .unwrap();
    assert_eq!(row["employee_name"], json!("Айбек"));

    // Переименовали сотрудника — имя входа то же.
    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/employees/{aibek}"),
        json!({ "full_name": "Айбек К." }),
    )
    .await;
    let me = w.get(&me_cookie, "/api/v1/auth/me").await;
    assert_eq!(me["full_name"], json!("Айбек К."));
    assert_eq!(me["employee_name"], json!("Айбек К."));
}

#[sqlx::test(migrations = "./migrations")]
async fn employee_of_another_branch_gets_no_login_here(pool: PgPool) {
    let w = W::new(pool).await;
    let b = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/branches",
            json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
        )
        .await;
    let other = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_cashier, is_master) values ($1, $2, 'Чужой', true, false)")
        .bind(other)
        .bind(Uuid::parse_str(b["id"].as_str().unwrap()).unwrap())
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, e) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/users",
            Some(json!({ "op_id": Uuid::now_v7(), "login": "chuzhoi", "password": "chuzhoi-pass-1", "employee_id": other })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    // И привязать существующий вход к чужому сотруднику нельзя.
    let (s, _) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{}", w.kassa_id),
            Some(json!({ "op_id": Uuid::now_v7(), "employee_id": other })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Вход сотрудника в другой филиал не переводится.
    let mine = w.employee("Свой", false, None).await;
    let u = w.personal(&mine, "svoi").await;
    let (s, e) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{}", u["id"].as_str().unwrap()),
            Some(json!({ "op_id": Uuid::now_v7(), "branch_id": b["id"] })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    // Отвязать (`null`) — можно: вход остаётся, сотрудника у него нет.
    let out = w
        .ok(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{}", u["id"].as_str().unwrap()),
            json!({ "op_id": Uuid::now_v7(), "employee_id": null }),
        )
        .await;
    assert_eq!(out["employee_id"], Value::Null);
}

#[sqlx::test(migrations = "./migrations")]
async fn disabling_employee_disables_login_and_cuts_sessions(pool: PgPool) {
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let u = w.personal(&bakyt, "bakyt").await;
    let uid = u["id"].as_str().unwrap().to_string();
    let s1 = login(&w.app, "bakyt", "bakyt-pass-1").await;
    let s2 = login(&w.app, "bakyt", "bakyt-pass-1").await;
    w.get(&s1, "/api/v1/auth/me").await;

    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/employees/{bakyt}"),
        json!({ "active": false }),
    )
    .await;
    for s in [&s1, &s2] {
        let (st, _) = w.req("GET", s, "/api/v1/auth/me", None).await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    let (st, _) = try_login(&w.app, "bakyt", "bakyt-pass-1").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        w.scalar(&format!(
            "select count(*) from sessions where user_id = '{uid}'"
        ))
        .await,
        0
    );
    let reason: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'user.update' and entity_id = $1",
    )
    .bind(Uuid::parse_str(&uid).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(reason["reason"], json!("сотрудник отключён"));

    // Включить вход отключённого сотрудника нельзя; сначала — сотрудника.
    let (st, _) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/users/{uid}"),
            Some(json!({ "op_id": Uuid::now_v7(), "active": true })),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/employees/{bakyt}"),
        json!({ "active": true }),
    )
    .await;
    // Сотрудника включили — вход остаётся выключенным, пока его не включит владелец.
    let (st, _) = try_login(&w.app, "bakyt", "bakyt-pass-1").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/users/{uid}"),
        json!({ "op_id": Uuid::now_v7(), "active": true }),
    )
    .await;
    login(&w.app, "bakyt", "bakyt-pass-1").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn anyone_changes_own_password(pool: PgPool) {
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    w.personal(&bakyt, "bakyt").await;
    let here = login(&w.app, "bakyt", "bakyt-pass-1").await;
    let there = login(&w.app, "bakyt", "bakyt-pass-1").await;

    // Неверный прежний и короткий новый — отказ, пароль прежний.
    let (s, _) = w
        .req(
            "POST",
            &here,
            "/api/v1/auth/password",
            Some(json!({ "op_id": Uuid::now_v7(), "old_password": "wrong-pass-1", "new_password": "bakyt-new-pass" })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = w
        .req(
            "POST",
            &here,
            "/api/v1/auth/password",
            Some(json!({ "op_id": Uuid::now_v7(), "old_password": "bakyt-pass-1", "new_password": "short" })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Смена: прочие сессии закрыты, эта остаётся; повтор op_id — тот же ответ.
    let body = json!({ "op_id": Uuid::now_v7(), "old_password": "bakyt-pass-1", "new_password": "bakyt-new-pass" });
    let out = w
        .ok("POST", &here, "/api/v1/auth/password", body.clone())
        .await;
    assert_eq!(out["sessions_closed"], json!(1));
    assert_eq!(
        w.ok("POST", &here, "/api/v1/auth/password", body).await,
        out
    );
    w.get(&here, "/api/v1/auth/me").await;
    let (s, _) = w.req("GET", &there, "/api/v1/auth/me", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = try_login(&w.app, "bakyt", "bakyt-pass-1").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    login(&w.app, "bakyt", "bakyt-new-pass").await;
    assert_eq!(
        w.scalar("select count(*) from audit_log where action = 'user.password_self'")
            .await,
        1
    );

    // Общий вход и владелец — тоже.
    w.ok(
        "POST",
        &w.kassa,
        "/api/v1/auth/password",
        json!({ "op_id": Uuid::now_v7(), "old_password": "kassa-pass-1", "new_password": "kassa-pass-2" }),
    )
    .await;
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/auth/password",
        json!({ "op_id": Uuid::now_v7(), "old_password": "owner-pass-1", "new_password": "owner-pass-2" }),
    )
    .await;
    login(&w.app, "owner", "owner-pass-2").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn shift_cashier_limits_only_shared_login(pool: PgPool) {
    let w = W::new(pool).await;
    let item = stocked_item(&w).await;
    let nurlan = w.employee("Нурлан", true, Some("1111")).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let personal = w.personal(&bakyt, "bakyt").await;
    let bakyt_login = login(&w.app, "bakyt", "bakyt-pass-1").await;
    // Скидка выдана и общему входу, и входу Бакыта; у кассира Нурлана владелец её снял.
    for u in [
        w.kassa_id.to_string(),
        personal["id"].as_str().unwrap().to_string(),
    ] {
        sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, branch_id, 'discount' from users where id = $1")
            .bind(Uuid::parse_str(&u).unwrap())
            .execute(&w.pool)
            .await
            .unwrap();
    }
    w.ok(
        "PUT",
        &w.owner,
        &format!("/api/v1/employees/{nurlan}/permissions"),
        json!({ "op_id": Uuid::now_v7(), "denied": ["discount"] }),
    )
    .await;
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": nurlan, "cashier_pin": "1111" }),
    )
    .await;

    // Общий вход — «Нет права у кассира Нурлан» (ADR-152, текст 403 прежний).
    let (s, e) = w
        .req(
            "POST",
            &w.kassa,
            "/api/v1/sales",
            Some(sale_body(&nurlan, &item, 90_000)),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(
        e["error"]["message"],
        json!("Нет права у кассира Нурлан: Скидка в чеке")
    );
    // Личный вход — по своим правам: снятое у кассира смены его не урезает.
    let (s, e) = w
        .req(
            "POST",
            &bakyt_login,
            "/api/v1/sales",
            Some(sale_body(&nurlan, &item, 90_000)),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{e}");
    // Своих прав не добавляет: без права у входа — отказ.
    w.set_permissions(personal["id"].as_str().unwrap(), &["view_sales"])
        .await;
    let (s, _) = w
        .req(
            "POST",
            &bakyt_login,
            "/api/v1/sales",
            Some(sale_body(&nurlan, &item, 90_000)),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn cashier_opens_own_shift_without_pin(pool: PgPool) {
    let w = W::new(pool).await;
    let aibek = w.employee("Айбек", true, Some("2222")).await;
    let nurlan = w.employee("Нурлан", true, Some("1111")).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    w.personal(&aibek, "aibek").await;
    w.personal(&bakyt, "bakyt").await;
    let me = login(&w.app, "aibek", "aibek-pass-1").await;
    let master = login(&w.app, "bakyt", "bakyt-pass-1").await;

    // Общий вход без кассира и PIN — как раньше: выбрать кассира и подтвердить.
    let (s, _) = w
        .req(
            "POST",
            &w.kassa,
            "/api/v1/shifts/open",
            Some(json!({ "op_id": Uuid::now_v7() })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Личный вход на чужого кассира — только с его PIN.
    let (s, _) = w
        .req(
            "POST",
            &me,
            "/api/v1/shifts/open",
            Some(json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": nurlan })),
        )
        .await;
    assert_ne!(s, StatusCode::OK);
    // Мастер не кассир — смену на себя не открывает.
    let (s, _) = w
        .req(
            "POST",
            &master,
            "/api/v1/shifts/open",
            Some(json!({ "op_id": Uuid::now_v7() })),
        )
        .await;
    assert_ne!(s, StatusCode::OK);

    // Кассир своим входом — на себя, без PIN; повтор op_id — та же смена.
    let body = json!({ "op_id": Uuid::now_v7() });
    let shift = w.ok("POST", &me, "/api/v1/shifts/open", body.clone()).await;
    assert_eq!(w.ok("POST", &me, "/api/v1/shifts/open", body).await, shift);
    let cur = w.get(&w.owner, "/api/v1/shifts/current").await;
    assert_eq!(cur["cashier_employee_id"], json!(aibek));
    assert_eq!(cur["id"], shift["id"]);

    // Закрывает её сам кассир.
    let expected = cur["expected_tyiyn"].as_i64().unwrap();
    w.ok(
        "POST",
        &me,
        &format!("/api/v1/shifts/{}/close", shift["id"].as_str().unwrap()),
        json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "" }),
    )
    .await;
}

#[sqlx::test(migrations = "./migrations")]
async fn cashier_sells_online_only_in_own_shift(pool: PgPool) {
    let w = W::new(pool).await;
    let item = stocked_item(&w).await;
    let nurlan = w.employee("Нурлан", true, Some("1111")).await;
    let aibek = w.employee("Айбек", true, Some("2222")).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    w.personal(&aibek, "aibek").await;
    w.personal(&bakyt, "bakyt").await;
    let me = login(&w.app, "aibek", "aibek-pass-1").await;
    let master = login(&w.app, "bakyt", "bakyt-pass-1").await;
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/shifts/open",
        json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": nurlan, "cashier_pin": "1111" }),
    )
    .await;

    let (s, e) = w
        .req(
            "POST",
            &me,
            "/api/v1/sales",
            Some(sale_body(&aibek, &item, 100_000)),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{e}");
    assert_eq!(
        e["error"]["message"],
        json!("Смена открыта на Нурлан: закройте её и откройте свою")
    );
    // Чек без сети уже состоялся — не отклоняется (I9, I10).
    let mut offline = sale_body(&aibek, &item, 100_000);
    offline["offline"] = json!(true);
    offline["client_time"] = json!(chrono::Utc::now());
    let (s, e) = w.req("POST", &me, "/api/v1/sales", Some(offline)).await;
    assert_eq!(s, StatusCode::OK, "{e}");
    // …но владелец узнаёт, что чек пробит в чужой смене (`sale.offline_bypass`, ADR-062).
    let reasons: Value = sqlx::query_scalar(
        "select data->'reasons' from audit_log where action = 'sale.offline_bypass' and entity_id = $1::uuid",
    )
    .bind(e["id"].as_str().unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert!(
        reasons
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("в смене Нурлан")),
        "{reasons}"
    );
    // Владелец, общий вход и мастер со своим входом — как раньше.
    for who in [&w.owner, &w.kassa, &master] {
        let (s, e) = w
            .req(
                "POST",
                who,
                "/api/v1/sales",
                Some(sale_body(&nurlan, &item, 100_000)),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{e}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn my_earnings_show_only_own(pool: PgPool) {
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let other = w.employee("Эрлан", false, None).await;
    let u = w.personal(&bakyt, "bakyt").await;
    // Мастер без прав: даже «Расчёт» снят.
    w.set_permissions(u["id"].as_str().unwrap(), &[]).await;
    let me = login(&w.app, "bakyt", "bakyt-pass-1").await;
    for (emp, amount) in [(&bakyt, 5_000), (&other, 7_000)] {
        w.ok(
            "POST",
            &w.owner,
            "/api/v1/payroll/accruals",
            json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "kind": "bonus",
                    "amount_tyiyn": amount, "comment": "за план" }),
        )
        .await;
    }
    let accounts = w.get(&w.owner, "/api/v1/cash/accounts").await;
    let safe = accounts
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["owner_only"] == json!(true))
        .unwrap()["id"]
        .clone();
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": bakyt, "account_id": safe,
                "lines": [{ "purpose": "bonus", "amount_tyiyn": 2_000 }] }),
    )
    .await;

    let (s, _) = w.req("GET", &me, "/api/v1/payroll/month", None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let out = w.get(&me, "/api/v1/me/earnings").await;
    assert_eq!(out["employee_id"], json!(bakyt));
    assert_eq!(out["accrued_tyiyn"], json!(5_000));
    assert_eq!(out["paid_tyiyn"], json!(2_000));
    assert_eq!(out["closing_tyiyn"], json!(3_000));
    assert_eq!(out["owed_now_tyiyn"], json!(3_000));
    assert_eq!(out["payouts"].as_array().unwrap().len(), 1);
    // Чужого не запросить: лишние параметры ничего не меняют.
    let out2 = w
        .get(&me, &format!("/api/v1/me/earnings?employee_id={other}"))
        .await;
    assert_eq!(out2["employee_id"], json!(bakyt));
    // Далёкий месяц — отказ.
    let (s, _) = w
        .req("GET", &me, "/api/v1/me/earnings?month=2000-01-01", None)
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Без сотрудника — нечего показывать.
    for who in [&w.kassa, &w.owner] {
        let (s, _) = w.req("GET", who, "/api/v1/me/earnings", None).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn feed_names_login_with_employee(pool: PgPool) {
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let u = w.personal(&bakyt, "bakyt").await;
    w.set_permissions(u["id"].as_str().unwrap(), &["employees"])
        .await;
    let me = login(&w.app, "bakyt", "bakyt-pass-1").await;
    w.ok(
        "POST",
        &me,
        "/api/v1/employees",
        json!({ "full_name": "Новый мастер", "is_master": true }),
    )
    .await;
    let feed = w.get(&w.owner, "/api/v1/notifications?limit=50").await;
    assert!(
        feed["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["user_name"] == json!("bakyt (Бакыт)")),
        "{feed}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn one_cashiers_typos_do_not_lock_others_on_same_till(pool: PgPool) {
    let w = W::new(pool).await;
    let aibek = w.employee("Айбек", true, None).await;
    let nurlan = w.employee("Нурлан", true, None).await;
    w.personal(&aibek, "aibek").await;
    w.personal(&nurlan, "nurlan").await;
    // Айбек ошибается паролем и жмёт «Войти» снова и снова — его вход закрыт.
    let mut last = StatusCode::OK;
    for _ in 0..60 {
        last = try_login(&w.app, "aibek", "wrong-pass-1").await.0;
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    let (s, _) = try_login(&w.app, "aibek", "aibek-pass-1").await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    // Нурлан на том же моноблоке входит.
    login(&w.app, "nurlan", "nurlan-pass-1").await;
    login(&w.app, "kassa", "kassa-pass-1").await;
}

/// Ответ 8а (ADR-170): вход сотрудника, привязанный к боту, — один чат: кнопки по его правам и
/// «💼 Мой заработок» (тот же расчёт, что кабинет работника), без подключения по телефону.
/// Подключился тем же чатом и по телефону — ничего не ломается, заработок тот же.
#[sqlx::test(migrations = "./migrations")]
async fn personal_login_chat_has_rights_and_own_earnings(pool: PgPool) {
    use avtodom_server::api::telegram::{Keys, handle_callback, handle_message, handle_update};
    use avtodom_server::domain::money::format_som;
    const CHAT: i64 = 740_001;
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let other = w.employee("Эрлан", false, None).await;
    w.personal(&bakyt, "bakyt").await;
    let me = login(&w.app, "bakyt", "bakyt-pass-1").await;
    for (emp, amount) in [(&bakyt, 5_000), (&other, 7_000)] {
        w.ok(
            "POST",
            &w.owner,
            "/api/v1/payroll/accruals",
            json!({ "op_id": Uuid::now_v7(), "employee_id": emp, "kind": "bonus",
                    "amount_tyiyn": amount, "comment": "за план" }),
        )
        .await;
    }
    let code = w
        .ok(
            "POST",
            &me,
            "/api/v1/me/telegram/code",
            json!({ "op_id": Uuid::now_v7() }),
        )
        .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let mut conn = w.pool.acquire().await.unwrap();
    let r = handle_update(
        &mut conn,
        None,
        json!({
            "update_id": 1,
            "message": {
                "chat": { "id": CHAT, "type": "private" },
                "from": { "id": CHAT, "first_name": "Бакыт" },
                "text": format!("/start {code}")
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(r.text.starts_with("Готово"), "{}", r.text);

    for phone in ["без телефона", "с телефоном"] {
        if phone == "с телефоном" {
            // Тот же человек подключил тот же чат и по телефону (SPEC-23) — не мешает.
            sqlx::query("update employees set phone = '+996700123456' where id = $1::uuid")
                .bind(&bakyt)
                .execute(&w.pool)
                .await
                .unwrap();
            sqlx::query("insert into telegram_employees (chat_id, employee_id, branch_id, phone) select $1, id, branch_id, phone from employees where id = $2::uuid")
                .bind(CHAT)
                .bind(&bakyt)
                .execute(&w.pool)
                .await
                .unwrap();
        }
        // Свободный текст — меню по правам, первым рядом «💼 Мой заработок».
        let r = handle_message(&mut conn, CHAT, CHAT, Some("привет"), None)
            .await
            .unwrap();
        let Keys::Admin(menu) = r.keys else {
            panic!("{phone}: не кнопки администратора: {:?} {}", r.keys, r.text)
        };
        assert!(menu.has("shift"), "{phone}: {menu:?}");
        assert_eq!(menu.rows()[0], vec!["💼 Мой заработок"], "{phone}");
        assert!(r.text.contains("💼 Мой заработок"), "{phone}: {}", r.text);
        // Заработок — свой, кнопки внизу — администратора, разделы — под сообщением.
        let r = handle_message(&mut conn, CHAT, CHAT, Some("💼 Мой заработок"), None)
            .await
            .unwrap();
        assert!(r.text.starts_with("💼 Сегодня"), "{phone}: {}", r.text);
        assert!(r.text.contains("Бакыт"), "{phone}: {}", r.text);
        assert!(r.text.contains(&format_som(5_000)), "{phone}: {}", r.text);
        assert!(!r.text.contains(&format_som(7_000)), "{phone}: {}", r.text);
        assert!(matches!(r.keys, Keys::Admin(_)), "{phone}: {:?}", r.keys);
        assert!(!r.inline.is_empty(), "{phone}: разделы под сообщением");
        // Месяц кнопкой под сообщением — своё, кнопки администратора.
        let r = handle_callback(&mut conn, CHAT, &format!("e:m:{bakyt}"))
            .await
            .unwrap();
        assert!(r.text.contains("Бакыт"), "{phone}: {}", r.text);
        assert!(matches!(r.keys, Keys::Admin(_)), "{phone}: {:?}", r.keys);
        // Чужой сотрудник в кнопке — ничего не показывается.
        let r = handle_callback(&mut conn, CHAT, &format!("e:m:{other}"))
            .await
            .unwrap();
        assert!(!r.text.contains("Эрлан"), "{phone}: {}", r.text);
        // Кнопка меню по правам работает.
        let r = handle_message(&mut conn, CHAT, CHAT, Some("🧾 Смена сейчас"), None)
            .await
            .unwrap();
        assert!(r.text.starts_with("🧾 Смена"), "{phone}: {}", r.text);
    }

    // Общий вход кассы (без сотрудника) — без «Мой заработок».
    let kassa_code = w
        .ok(
            "POST",
            &w.kassa,
            "/api/v1/me/telegram/code",
            json!({ "op_id": Uuid::now_v7() }),
        )
        .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let r = handle_update(
        &mut conn,
        None,
        json!({
            "update_id": 2,
            "message": {
                "chat": { "id": CHAT + 1, "type": "private" },
                "from": { "id": CHAT + 1, "first_name": "Касса" },
                "text": format!("/start {kassa_code}")
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(r.text.starts_with("Готово"), "{}", r.text);
    let r = handle_message(&mut conn, CHAT + 1, CHAT + 1, Some("привет"), None)
        .await
        .unwrap();
    let Keys::Admin(menu) = r.keys else {
        panic!("не кнопки администратора: {:?}", r.keys)
    };
    assert!(!menu.earnings(), "{menu:?}");
    let r = handle_message(
        &mut conn,
        CHAT + 1,
        CHAT + 1,
        Some("💼 Мой заработок"),
        None,
    )
    .await
    .unwrap();
    assert!(!r.text.starts_with("💼 Сегодня"), "{}", r.text);
}

/// Вход перевели на другого сотрудника — прежний держатель входа не видит чужого заработка:
/// его сессии сброшены, чат в Телеграме отвязан (ADR-170).
#[sqlx::test(migrations = "./migrations")]
async fn moving_login_to_another_employee_cuts_sessions_and_chat(pool: PgPool) {
    let w = W::new(pool).await;
    let ivan = w.employee("Иван", false, None).await;
    let petr = w.employee("Пётр", false, None).await;
    let u = w.personal(&ivan, "ivan").await;
    let uid = u["id"].as_str().unwrap().to_string();
    let s = login(&w.app, "ivan", "ivan-pass-1").await;
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id) select 740100, id, branch_id from users where id = $1::uuid")
        .bind(&uid)
        .execute(&w.pool)
        .await
        .unwrap();

    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/users/{uid}"),
        json!({ "op_id": Uuid::now_v7(), "employee_id": petr }),
    )
    .await;
    let (st, _) = w.req("GET", &s, "/api/v1/me/earnings", None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        w.scalar(&format!(
            "select count(*) from telegram_links where user_id = '{uid}'"
        ))
        .await,
        0
    );
    assert_eq!(
        w.scalar(&format!(
            "select count(*) from sessions where user_id = '{uid}'"
        ))
        .await,
        0
    );
}

/// Вход заводит и меняет только владелец: сотрудник с правом «Сотрудники» не отключает
/// карточку коллеги со входом — иначе выключил бы ей вход (ADR-170).
#[sqlx::test(migrations = "./migrations")]
async fn only_owner_disables_employee_with_login(pool: PgPool) {
    let w = W::new(pool).await;
    let bakyt = w.employee("Бакыт", false, None).await;
    let aigul = w.employee("Айгуль", false, None).await;
    let plain = w.employee("Эрлан", false, None).await;
    w.personal(&bakyt, "bakyt").await;
    w.personal(&aigul, "aigul").await;
    let master = login(&w.app, "bakyt", "bakyt-pass-1").await;
    let her = login(&w.app, "aigul", "aigul-pass-1").await;
    let (st, e) = w
        .req(
            "PATCH",
            &master,
            &format!("/api/v1/employees/{aigul}"),
            Some(json!({ "active": false })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{e}");
    w.get(&her, "/api/v1/auth/me").await;
    // Сотрудника без входа отключает и не-владелец — как раньше.
    w.ok(
        "PATCH",
        &master,
        &format!("/api/v1/employees/{plain}"),
        json!({ "active": false }),
    )
    .await;
}
