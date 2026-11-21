//! Объединение карточек клиентов и поставщиков (задача 043).
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
) -> (StatusCode, Value) {
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
    let mut json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(c) = cookie
        && uri.ends_with("/auth/login")
    {
        json = json!(c);
    }
    (status, json)
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, Some(cookie), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn get(app: &Router, uri: &str, cookie: &str) -> Value {
    let (s, v) = call(app, "GET", uri, Some(cookie), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    c.as_str().unwrap().to_string()
}

fn message(v: &Value) -> String {
    v["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

struct World {
    app: Router,
    pool: PgPool,
    branch: Uuid,
    owner: String,
    admin: String,
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
        let hash = hash_password("admin-pass-1".into()).await.unwrap();
        let admin_id = Uuid::now_v7();
        sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Эрбол')")
            .bind(admin_id)
            .bind(branch)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
        let keys: Vec<String> = avtodom_server::auth::default_permissions()
            .into_iter()
            .map(String::from)
            .collect();
        sqlx::query(
            "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
        )
        .bind(admin_id)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        World {
            app,
            pool,
            branch,
            owner,
            admin,
        }
    }

    async fn person(&self, last: &str, first: &str, phone: &str) -> String {
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/parties",
            &self.owner,
            json!({ "kind": "person", "first_name": first, "last_name": last, "phone": phone }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    async fn supplier(&self, name: &str) -> String {
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/parties",
            &self.owner,
            json!({ "role": "supplier", "kind": "company", "name": name }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    async fn debt(&self, party: &str, amount: i64) {
        ok(
            &self.app,
            "POST",
            "/api/v1/debts/adjust",
            &self.owner,
            json!({ "op_id": Uuid::now_v7(), "party_id": party, "amount_tyiyn": amount, "comment": "долг" }),
        )
        .await;
    }

    async fn balance(&self, party: &str) -> i64 {
        get(&self.app, &format!("/api/v1/parties/{party}"), &self.owner).await["balance_tyiyn"]
            .as_i64()
            .unwrap()
    }

    async fn merge(&self, cookie: &str, into: &str, body: Value) -> (StatusCode, Value) {
        call(
            &self.app,
            "POST",
            &format!("/api/v1/parties/{into}/merge"),
            Some(cookie),
            Some(body),
        )
        .await
    }

    async fn ledger_rows(&self, party: &str) -> i64 {
        sqlx::query_scalar("select count(*) from party_ledger where party_id = $1::uuid")
            .bind(party)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

/// Пример заказчика: два агента одной фирмы — две карточки с долгом; владелец сливает их в
/// фирму «IQOS» — долг общий, агенты стали работниками фирмы, машины перешли, дубль в архиве,
/// его записи журнала не тронуты, чеки видны в карточке фирмы.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn two_agents_become_one_company(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.person("Асанов", "Бакыт", "+996555111111").await;
    let b = w.person("Усенов", "Нурлан", "+996555222222").await;
    w.debt(&a, 30_000).await;
    w.debt(&b, 50_000).await;
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/parties/{b}/vehicles"),
        &w.owner,
        json!({ "plate": "01KG123ABC" }),
    )
    .await;
    let rows_before = w.ledger_rows(&b).await;

    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "duplicate_ids": [b], "kind": "company", "name": "IQOS" });
    let (s, out) = w.merge(&w.owner, &a, body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["name"], json!("IQOS"));
    assert_eq!(out["kind"], json!("company"));
    assert_eq!(out["balance_tyiyn"], json!(80_000));
    assert_eq!(w.balance(&b).await, 0);
    // Журнал влитой карточки не переписан: только встречная запись переноса.
    assert_eq!(w.ledger_rows(&b).await, rows_before + 1);
    let sum: i64 = sqlx::query_scalar(
        "select coalesce(sum(balance_tyiyn), 0)::bigint from party_balances where party_id = $1::uuid",
    )
    .bind(&a)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(sum, 80_000);

    // Повтор с тем же op_id — тот же ответ, долг не задвоился.
    let (s, again) = w.merge(&w.owner, &a, body).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(w.balance(&a).await, 80_000);

    let card = get(&w.app, &format!("/api/v1/parties/{a}/card"), &w.owner).await;
    let names: Vec<&str> = card["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["full_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"Усенов Нурлан"), "{names:?}");
    assert!(
        card["vehicles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["plate"] == json!("01KG123ABC"))
    );
    // Перенос в истории не сторнируется.
    let merge_row = card["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["title"] == json!("Объединение карточек"))
        .expect("перенос в истории");
    assert_eq!(merge_row["reversible"], json!(false));
    let lid = merge_row["ledger_id"].as_str().unwrap();
    let (s, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/debts/ledger/{lid}/reverse"),
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "ошибка" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);

    // Влитая карточка — в архиве и не находится в списке.
    let dup = get(&w.app, &format!("/api/v1/parties/{b}"), &w.owner).await;
    assert_eq!(dup["active"], json!(false));
    let list = get(&w.app, "/api/v1/parties?q=Усенов", &w.owner).await;
    let items = list["items"]
        .as_array()
        .or(list.as_array())
        .unwrap()
        .clone();
    assert!(items.iter().all(|p| p["id"] != json!(b)), "{items:?}");

    // Уведомление владельцу.
    let n: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'party.merge'")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

/// Аванс переносится так же; несколько карточек за раз.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn several_cards_and_advance(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.person("Асанов", "Бакыт", "").await;
    let b = w.person("Усенов", "Нурлан", "+996555333333").await;
    let c = w.person("Токтогулов", "Эмиль", "").await;
    w.debt(&a, 10_000).await;
    w.debt(&b, 40_000).await;
    w.debt(&c, -15_000).await;
    let (s, out) = w
        .merge(
            &w.owner,
            &a,
            json!({ "op_id": Uuid::now_v7(), "duplicate_ids": [b, c] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["balance_tyiyn"], json!(35_000));
    // Остался человек — имя прежнее, пустой телефон взят у влитой.
    assert_eq!(out["name"], json!("Асанов Бакыт"));
    assert_eq!(out["phone"], json!("+996555333333"));
    assert_eq!(w.balance(&c).await, 0);
}

/// Запреты: администратор, сам в себя, клиент с поставщиком, уже влитая карточка.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn merge_refusals(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.person("Асанов", "Бакыт", "").await;
    let b = w.person("Усенов", "Нурлан", "").await;
    let s1 = w.supplier("Автоснаб").await;
    let op = || Uuid::now_v7();
    let (s, _) = w
        .merge(&w.admin, &a, json!({ "op_id": op(), "duplicate_ids": [b] }))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w
        .merge(&w.owner, &a, json!({ "op_id": op(), "duplicate_ids": [a] }))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = w
        .merge(
            &w.owner,
            &a,
            json!({ "op_id": op(), "duplicate_ids": [s1] }),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = w
        .merge(&w.owner, &a, json!({ "op_id": op(), "duplicate_ids": [] }))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, v) = w
        .merge(&w.owner, &a, json!({ "op_id": op(), "duplicate_ids": [b] }))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = w
        .merge(&w.owner, &a, json!({ "op_id": op(), "duplicate_ids": [b] }))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert!(message(&v).contains("уже влита"));
    let _ = w.branch;
}

/// Поставщик: долг дубля переходит, оплата основной закрывает его.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn supplier_debt_moves(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.supplier("Автоснаб").await;
    let b = w.supplier("Автоснаб ОсОО").await;
    w.debt(&b, -70_000).await;
    let (s, out) = w
        .merge(
            &w.owner,
            &a,
            json!({ "op_id": Uuid::now_v7(), "duplicate_ids": [b] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["balance_tyiyn"], json!(-70_000));
    assert_eq!(w.balance(&b).await, 0);
    let active: Option<bool> =
        sqlx::query_scalar("select active from suppliers where id = $1::uuid")
            .bind(&b)
            .fetch_optional(&w.pool)
            .await
            .unwrap();
    assert_ne!(active, Some(true));
}

/// Баллы влитой карточки переходят на основную.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn points_move(pool: PgPool) {
    let w = World::new(pool).await;
    let a = w.person("Асанов", "Бакыт", "").await;
    let b = w.person("Усенов", "Нурлан", "").await;
    sqlx::query(
        "insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, doc_type, doc_id)
         values ($1, $2, $3::uuid, 'accrual', 5000, 'sale', $1)",
    )
    .bind(Uuid::now_v7())
    .bind(w.branch)
    .bind(&b)
    .execute(&w.pool)
    .await
    .unwrap();
    let (s, v) = w
        .merge(
            &w.owner,
            &a,
            json!({ "op_id": Uuid::now_v7(), "duplicate_ids": [b] }),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let pts = |p: String| {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select coalesce(sum(amount_tyiyn), 0)::bigint from loyalty_ledger where party_id = $1::uuid",
            )
            .bind(p)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(pts(a.clone()).await, 5000);
    assert_eq!(pts(b.clone()).await, 0);
}
