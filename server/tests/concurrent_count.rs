//! Один товар считают и заводят несколько человек с разных устройств одновременно (задача 003,
//! Н14): «масло лежит в 4 местах, 4 человека отсканировали — должно показать 4».
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::task::JoinSet;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    branch: Option<Uuid>,
    body: Option<Value>,
) -> (StatusCode, Value) {
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
    let mut json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(c) = cookie
        && uri.ends_with("/auth/login")
    {
        json = json!(c);
    }
    (status, json)
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, Some(cookie), None, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn get(app: &Router, uri: &str, cookie: &str) -> Value {
    let (s, v) = call(app, "GET", uri, Some(cookie), None, None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        None,
        Some(json!({ "login": login, "password": password })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    c.as_str().unwrap().to_string()
}

/// Владелец и три администратора филиала: четыре человека на четырёх устройствах.
struct World {
    app: Router,
    pool: PgPool,
    branch: Uuid,
    people: Vec<String>,
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
        let keys: Vec<String> = avtodom_server::auth::default_permissions()
            .into_iter()
            .map(String::from)
            .collect();
        for (login, name) in [
            ("admin1", "Айбек"),
            ("admin2", "Нурлан"),
            ("admin3", "Эрмек"),
        ] {
            let id = Uuid::now_v7();
            let hash = hash_password("admin-pass-1".into()).await.unwrap();
            sqlx::query(
                "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $5)",
            )
            .bind(id)
            .bind(branch)
            .bind(login)
            .bind(hash)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
            )
            .bind(id)
            .bind(branch)
            .bind(&keys)
            .execute(&pool)
            .await
            .unwrap();
        }
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let mut people = vec![login(&app, "owner", "owner-pass-1").await];
        for l in ["admin1", "admin2", "admin3"] {
            people.push(login(&app, l, "admin-pass-1").await);
        }
        World {
            app,
            pool,
            branch,
            people,
        }
    }

    fn owner(&self) -> &str {
        &self.people[0]
    }

    async fn category(&self, kind: &str) -> String {
        let c = ok(
            &self.app,
            "POST",
            "/api/v1/categories",
            self.owner(),
            json!({ "name": format!("Кат {}", Uuid::now_v7()), "kind": kind }),
        )
        .await;
        c["id"].as_str().unwrap().to_string()
    }

    async fn product(&self, name: &str) -> String {
        let cat = self.category("filter").await;
        let p = ok(
            &self.app,
            "POST",
            "/api/v1/products",
            self.owner(),
            json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": name, "sale_price_tyiyn": 10_000 }),
        )
        .await;
        p["id"].as_str().unwrap().to_string()
    }

    /// Один и тот же запрос от всех четырёх человек одновременно; тело — по номеру человека.
    async fn all_at_once(
        &self,
        method: &'static str,
        uri: &str,
        body: impl Fn(usize) -> Value,
    ) -> Vec<(StatusCode, Value)> {
        let mut set = JoinSet::new();
        for (i, cookie) in self.people.iter().enumerate() {
            let app = self.app.clone();
            let cookie = cookie.clone();
            let uri = uri.to_string();
            let b = body(i);
            set.spawn(async move { call(&app, method, &uri, Some(&cookie), None, Some(b)).await });
        }
        let mut out = Vec::new();
        while let Some(r) = set.join_next().await {
            out.push(r.unwrap());
        }
        out
    }
}

fn line<'a>(rev: &'a Value, product: &str) -> &'a Value {
    rev["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["product_id"] == json!(product))
        .unwrap()
}

/// Четыре человека сканируют один товар в одной ревизии: каждый скан прибавляет 1 — итог 4.
/// Повтор того же скана (обрыв связи) с тем же `op_id` не прибавляет второй раз. У строки — кто
/// считал последним.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn four_people_scan_one_product_into_one_revision(pool: PgPool) {
    let w = World::new(pool).await;
    let oil = w.product("Масло Shell 5W-30").await;
    let rev = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let ops: Vec<Uuid> = (0..4).map(|_| Uuid::now_v7()).collect();
    let res = w
        .all_at_once(
            "PUT",
            &uri,
            |i| json!({ "product_id": oil, "counted_qty": 1, "mode": "add", "op_id": ops[i] }),
        )
        .await;
    let mut totals: Vec<i64> = res
        .iter()
        .map(|(s, v)| {
            assert_eq!(*s, StatusCode::OK, "{v}");
            v["counted_qty"].as_i64().unwrap()
        })
        .collect();
    totals.sort_unstable();
    // Каждый получил свой итог — общий счёт рос по одному.
    assert_eq!(totals, vec![1, 2, 3, 4]);
    // Повтор скана второго человека — прежний ответ, не пятая штука.
    let (s, again) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.people[1]),
        None,
        Some(json!({ "product_id": oil, "counted_qty": 1, "mode": "add", "op_id": ops[1] })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    assert_eq!(line(&full, &oil)["counted_qty"], json!(4));

    // Прежний экран слал «set» с локальным числом + 1: четыре устройства затирали друг друга.
    let item = w.product("Фильтр масляный").await;
    w.all_at_once(
        "PUT",
        &uri,
        |_| json!({ "product_id": item, "counted_qty": 1, "op_id": Uuid::now_v7() }),
    )
    .await;
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    assert_eq!(line(&full, &item)["counted_qty"], json!(1));

    // Кто считал строку последним — видно каждому.
    ok(
        &w.app,
        "PUT",
        &uri,
        &w.people[2],
        json!({ "product_id": oil, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    assert_eq!(line(&full, &oil)["counted_qty"], json!(5));
    assert_eq!(line(&full, &oil)["counted_by"], json!("Нурлан"));
    assert_eq!(line(&full, &item)["counted_qty"], json!(1));
    assert!(line(&full, &item)["counted_by"].is_string());
}

/// «Сканер товаров» в режиме пересчёта: черновика «весь склад» нет, четыре человека начинают
/// одновременно — все попадают в один черновик, а не в четыре.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn four_people_open_one_whole_warehouse_draft(pool: PgPool) {
    let w = World::new(pool).await;
    let res = w
        .all_at_once(
            "POST",
            "/api/v1/revisions",
            |_| json!({ "category_id": null, "reuse_draft": true }),
        )
        .await;
    let mut ids: Vec<String> = res
        .iter()
        .map(|(s, v)| {
            assert_eq!(*s, StatusCode::OK, "{v}");
            v["head"]["id"].as_str().unwrap().to_string()
        })
        .collect();
    ids.dedup();
    assert_eq!(ids.len(), 1, "один черновик на всех");
    let drafts: i64 = sqlx::query_scalar(
        "select count(*) from revisions where status = 'draft' and branch_id = $1",
    )
    .bind(w.branch)
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(drafts, 1);
    // Ревизия категории при открытом «весь склад» присоединяется к нему: товар категории
    // входит в оба охвата, и две ревизии вычли бы один остаток дважды.
    let cat = w.category("filter").await;
    let by_cat = ok(
        &w.app,
        "POST",
        "/api/v1/revisions",
        w.owner(),
        json!({ "category_id": cat, "reuse_draft": true }),
    )
    .await;
    assert_eq!(by_cat["head"]["id"], json!(ids[0]));
    // Без флага — как раньше: новый черновик.
    let plain = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    assert_ne!(plain["head"]["id"], json!(ids[0]));
}

/// Четыре человека одновременно заводят незнакомый товар с одним штрихкодом: товар один,
/// остальным — 409 «штрихкод … уже у товара «…»» (не 500), и по коду находится заведённый.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn four_people_create_one_new_barcode(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = w.category("filter").await;
    let code = "4600000000017";
    let res = w
        .all_at_once("POST", "/api/v1/products", |i| {
            json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": format!("Фильтр {i}"),
                    "barcodes": [code], "sale_price_tyiyn": 10_000 })
        })
        .await;
    let created: Vec<&Value> = res
        .iter()
        .filter(|(s, _)| *s == StatusCode::OK)
        .map(|(_, v)| v)
        .collect();
    assert_eq!(created.len(), 1, "{res:?}");
    for (s, v) in res.iter().filter(|(s, _)| *s != StatusCode::OK) {
        assert_eq!(*s, StatusCode::CONFLICT, "{v}");
        let m = v["error"]["message"].as_str().unwrap();
        assert!(m.contains("уже у товара"), "{m}");
    }
    let found = get(
        &w.app,
        &format!("/api/v1/products/by-barcode/{code}"),
        w.owner(),
    )
    .await;
    assert_eq!(found["id"], created[0]["id"]);
    let n: i64 = sqlx::query_scalar("select count(*) from products where category_id = $1")
        .bind(Uuid::parse_str(&cat).unwrap())
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    // То же из ревизии («товар не из программы»): одна карточка, остальным 409, в строке — первое.
    let rev = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let code2 = "4600000000024";
    let res = w
        .all_at_once("POST", &format!("/api/v1/revisions/{rid}/new-product"), |i| {
            json!({ "op_id": Uuid::now_v7(), "counted_qty": 1, "category_id": cat,
                    "name": format!("Фильтр Б {i}"), "barcodes": [code2], "sale_price_tyiyn": 10_000 })
        })
        .await;
    assert_eq!(
        res.iter().filter(|(s, _)| *s == StatusCode::OK).count(),
        1,
        "{res:?}"
    );
    for (s, v) in res.iter().filter(|(s, _)| *s != StatusCode::OK) {
        assert_eq!(*s, StatusCode::CONFLICT, "{v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("уже у товара"),
            "{v}"
        );
    }
}

/// Тот же код заводят в двух филиалах одновременно: каталог общий, код один — второму 409 с
/// названием товара, а не «внутренняя ошибка» (вставка кода упирается в уникальность в базе).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn same_new_barcode_in_two_branches_at_once(pool: PgPool) {
    let w = World::new(pool).await;
    let nb = ok(
        &w.app,
        "POST",
        "/api/v1/branches",
        w.owner(),
        json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
    )
    .await;
    let b = Uuid::parse_str(nb["id"].as_str().unwrap()).unwrap();
    let cat = w.category("filter").await;
    let code = "4600000000031";
    for round in 0..5 {
        let code = format!("{code}{round}");
        let mut set = JoinSet::new();
        for (i, branch) in [w.branch, b, w.branch, b].into_iter().enumerate() {
            let app = w.app.clone();
            let cookie = w.owner().to_string();
            let body = json!({ "op_id": Uuid::now_v7(), "category_id": cat,
                               "name": format!("Фильтр {round}-{i}"), "barcodes": [code],
                               "sale_price_tyiyn": 10_000 });
            set.spawn(async move {
                call(
                    &app,
                    "POST",
                    "/api/v1/products",
                    Some(&cookie),
                    Some(branch),
                    Some(body),
                )
                .await
            });
        }
        let mut good = 0;
        while let Some(r) = set.join_next().await {
            let (s, v) = r.unwrap();
            if s == StatusCode::OK {
                good += 1;
            } else {
                assert_eq!(s, StatusCode::CONFLICT, "{v}");
                assert!(
                    v["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("уже у товара"),
                    "{v}"
                );
            }
        }
        assert_eq!(good, 1);
    }
}

/// Оприходование одного товара с четырёх устройств одновременно: каждый документ свой, остаток
/// — сумма всех.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn four_stock_ins_of_one_product_at_once_add_up(pool: PgPool) {
    let w = World::new(pool).await;
    let item = w.product("Свеча").await;
    let owner = w.owner().to_string();
    let mut set = JoinSet::new();
    for _ in 0..4 {
        let app = w.app.clone();
        let cookie = owner.clone();
        let body = json!({ "op_id": Uuid::now_v7(), "comment": "нашли на полке",
                           "lines": [{ "product_id": item, "qty": 1, "price_tyiyn": 5_000 }] });
        set.spawn(async move {
            call(
                &app,
                "POST",
                "/api/v1/stock-ins",
                Some(&cookie),
                None,
                Some(body),
            )
            .await
        });
    }
    while let Some(r) = set.join_next().await {
        let (s, v) = r.unwrap();
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let (qty, value): (i64, i64) = sqlx::query_as(
        "select stock_qty, stock_value_tyiyn from branch_products where branch_id = $1 and product_id = $2",
    )
    .bind(w.branch)
    .bind(Uuid::parse_str(&item).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!((qty, value), (4, 20_000));
}

/// Код ответа ошибки.
fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// Ручной ввод «= итог» несёт итог, который человек видел на экране (`expected_qty`): пока он
/// вводил, другие прибавили — 409 «сейчас N», и чужие сканы не затираются. Кто сколько прибавил —
/// журнал строки («Айбек +2, Нурлан +1, Владелец = 5»).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn set_with_stale_expected_qty_does_not_wipe_scans(pool: PgPool) {
    let w = World::new(pool).await;
    let item = w.product("Свеча NGK").await;
    let rev = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let add = |who: usize| {
        let (app, cookie, uri, item) = (
            w.app.clone(),
            w.people[who].clone(),
            uri.clone(),
            item.clone(),
        );
        async move {
            ok(
                &app,
                "PUT",
                &uri,
                &cookie,
                json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() }),
            )
            .await
        }
    };
    // Владелец открыл строку (пусто) и вводит итог; тем временем Айбек дважды сканирует, Нурлан — раз.
    add(1).await;
    add(1).await;
    add(2).await;
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(w.owner()),
        None,
        Some(
            json!({ "product_id": item, "counted_qty": 5, "mode": "set", "expected_qty": 0, "op_id": Uuid::now_v7() }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "count_changed", "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("сейчас 3 шт"), "{m}");
    assert!(m.contains("Записать итог 5 шт"), "{m}");
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    assert_eq!(
        line(&full, &item)["counted_qty"],
        json!(3),
        "чужие сканы целы"
    );
    // Перечитал — видит 3 и подтверждает итог 5.
    let set = ok(
        &w.app,
        "PUT",
        &uri,
        w.owner(),
        json!({ "product_id": item, "counted_qty": 5, "mode": "set", "expected_qty": 3, "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(set["counted_qty"], json!(5));
    // Повтор того же «= итог» (обрыв связи) — прежний ответ, а не 409.
    let op = Uuid::now_v7();
    let body = json!({ "product_id": item, "counted_qty": 6, "mode": "set", "expected_qty": 5, "op_id": op });
    ok(&w.app, "PUT", &uri, w.owner(), body.clone()).await;
    let again = ok(&w.app, "PUT", &uri, w.owner(), body).await;
    assert_eq!(again["counted_qty"], json!(6));
    // Убрать строку с устаревшим итогом — тоже 409; с верным — убирается.
    add(3).await;
    let (s, v) = call(
        &w.app,
        "DELETE",
        &format!("/api/v1/revisions/{rid}/lines/{item}?expected_qty=6"),
        Some(w.owner()),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "count_changed", "{v}");
    // Кто сколько прибавил: подряд идущие прибавки одного человека — одной записью.
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    let log: Vec<(String, String, i64)> = line(&full, &item)["counted_log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["by"].as_str().unwrap().to_string(),
                e["mode"].as_str().unwrap().to_string(),
                e["qty"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        log,
        vec![
            ("Айбек".into(), "add".into(), 2),
            ("Нурлан".into(), "add".into(), 1),
            ("Владелец".into(), "set".into(), 5),
            ("Владелец".into(), "set".into(), 6),
            ("Эрмек".into(), "add".into(), 1),
        ]
    );
    // Журнал только пополняется (инвариант 5).
    let r = sqlx::query("update revision_line_adds set qty = 100")
        .execute(&w.pool)
        .await;
    assert!(r.is_err(), "журнал прибавок не правится");
    ok(
        &w.app,
        "DELETE",
        &format!("/api/v1/revisions/{rid}/lines/{item}?expected_qty=7"),
        w.owner(),
        Value::Null,
    )
    .await;
    let n: i64 = sqlx::query_scalar(
        "select count(*) from revision_line_adds where revision_id = $1 and mode = 'clear'",
    )
    .bind(Uuid::parse_str(&rid).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

/// Число открытых черновиков филиала.
async fn drafts(w: &World) -> i64 {
    sqlx::query_scalar("select count(*) from revisions where status = 'draft' and branch_id = $1")
        .bind(w.branch)
        .fetch_one(&w.pool)
        .await
        .unwrap()
}

/// Двое нажали «Начать ревизию» одной категории — один черновик. «Весь склад» и категория —
/// пересекаются: двух черновиков, вычитающих один остаток, не бывает (категория присоединяется к
/// «весь склад», «весь склад» при открытой категории — 409 «Идёт ревизия «…»»).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn overlapping_revision_starts_share_one_draft(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = w.category("filter").await;
    let other = w.category("filter").await;
    let res = w
        .all_at_once(
            "POST",
            "/api/v1/revisions",
            |_| json!({ "category_id": cat, "reuse_draft": true }),
        )
        .await;
    let mut ids: Vec<String> = res
        .iter()
        .map(|(s, v)| {
            assert_eq!(*s, StatusCode::OK, "{v}");
            v["head"]["id"].as_str().unwrap().to_string()
        })
        .collect();
    ids.dedup();
    assert_eq!(ids.len(), 1, "один черновик категории");
    assert_eq!(drafts(&w).await, 1);
    // Другая категория — не пересекается: свой черновик.
    let other_rev = ok(
        &w.app,
        "POST",
        "/api/v1/revisions",
        w.owner(),
        json!({ "category_id": other, "reuse_draft": true }),
    )
    .await;
    assert_ne!(other_rev["head"]["id"], json!(ids[0]));
    // «Весь склад» при открытой категории — 409 с её названием, черновик не заводится.
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/revisions",
        Some(w.owner()),
        None,
        Some(json!({ "category_id": null, "reuse_draft": true })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "revision_open", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Идёт ревизия «Кат"),
        "{v}"
    );
    assert_eq!(drafts(&w).await, 2);
    for id in [
        ids[0].clone(),
        other_rev["head"]["id"].as_str().unwrap().to_string(),
    ] {
        ok(
            &w.app,
            "POST",
            &format!("/api/v1/revisions/{id}/cancel"),
            w.owner(),
            json!({}),
        )
        .await;
    }
    // Одновременно: двое — «весь склад», двое — категорию. Какой бы ни пришёл первым, открытый
    // черновик один (категория присоединилась к «весь склад» или «весь склад» получил 409).
    let res = w
        .all_at_once("POST", "/api/v1/revisions", |i| {
            if i % 2 == 0 {
                json!({ "category_id": null, "reuse_draft": true })
            } else {
                json!({ "category_id": cat, "reuse_draft": true })
            }
        })
        .await;
    for (s, v) in &res {
        assert!(
            *s == StatusCode::OK || (*s == StatusCode::CONFLICT && code(v) == "revision_open"),
            "{v}"
        );
    }
    assert_eq!(drafts(&w).await, 1, "{res:?}");
}
