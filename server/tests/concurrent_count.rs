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
    call_from(app, method, uri, cookie, branch, body, Uuid::now_v7()).await
}

/// Запрос с устройства `device` (без него каждый запрос — с нового устройства).
async fn call_from(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    branch: Option<Uuid>,
    body: Option<Value>,
    device: Uuid,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", device.to_string());
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
    // Новый черновик заводит только владелец (ADR-186): администратор, опередивший его, получает
    // отказ, остальные присоединяются к одному черновику.
    assert_eq!(res[0].0, StatusCode::OK, "{}", res[0].1);
    let mut ids: Vec<String> = res
        .iter()
        .filter(|(s, v)| !(*s == StatusCode::FORBIDDEN && code(v) == "forbidden"))
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
    // Без флага (старая версия экрана) — тот же черновик: второго «весь склад» не бывает.
    let plain = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    assert_eq!(plain["head"]["id"], json!(ids[0]));
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
            json!({ "product_id": item, "counted_qty": 5, "mode": "set", "expected_empty": true, "op_id": Uuid::now_v7() }),
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
    // Начинает только владелец (задача 019): администратор, успевший раньше него, получает 403,
    // остальные — тот же черновик.
    let mut ids: Vec<String> = res
        .iter()
        .filter(|(s, v)| {
            *s != StatusCode::FORBIDDEN
                || v["error"]["message"] != json!("Ревизию начинает владелец")
        })
        .map(|(s, v)| {
            assert_eq!(*s, StatusCode::OK, "{v}");
            v["head"]["id"].as_str().unwrap().to_string()
        })
        .collect();
    assert!(!ids.is_empty(), "черновик владельца");
    ids.sort();
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
            *s == StatusCode::OK
                || (*s == StatusCode::CONFLICT && code(v) == "revision_open")
                || (*s == StatusCode::FORBIDDEN && code(v) == "forbidden"),
            "{v}"
        );
    }
    assert_eq!(drafts(&w).await, 1, "{res:?}");
}

/// Черновик со строкой одного товара: ревизия «весь склад», товар, путь строк.
async fn draft_with_item(w: &World, name: &str) -> (String, String, String) {
    let item = w.product(name).await;
    let rev = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/lines");
    (rid, item, uri)
}

/// Записать строку от человека `who`; ответ — статус и тело.
async fn put_line(w: &World, uri: &str, who: usize, body: Value) -> (StatusCode, Value) {
    call(&w.app, "PUT", uri, Some(&w.people[who]), None, Some(body)).await
}

/// Строка ревизии, как её видит сервер сейчас: `None` — строки нет.
async fn counted(w: &World, rid: &str, item: &str) -> Option<i64> {
    get(
        &w.app,
        &format!("/api/v1/revisions/{rid}/lines/{item}"),
        w.owner(),
    )
    .await["counted_qty"]
        .as_i64()
}

/// «Строки нет» и «посчитано 0» — разное: человек видел пустую строку и вписывает итог, а другой
/// тем временем записал «не нашли — 0» — 409, а не затирание; очистка строки, которую видели с
/// 5, когда другой записал 0, — тоже 409 (товар не выпадет из проведения).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn empty_line_differs_from_counted_zero(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча пустая").await;
    let set = |qty: i64, expected: Value| {
        let mut b = json!({ "product_id": item, "counted_qty": qty, "mode": "set", "op_id": Uuid::now_v7() });
        match expected {
            Value::Null => b["expected_empty"] = json!(true),
            v => b["expected_qty"] = v,
        }
        b
    };
    // Нурлан не нашёл товар — «= 0» по пустой строке.
    let (s, v) = put_line(&w, &uri, 2, set(0, Value::Null)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Владелец тоже видел пустую строку и вписывает 5: строка уже есть (0) — 409.
    let (s, v) = put_line(&w, &uri, 0, set(5, Value::Null)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "count_changed", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("сейчас 0 шт"),
        "{v}"
    );
    assert_eq!(counted(&w, &rid, &item).await, Some(0));
    // Видел 0 — записал 5; Нурлан видел 5 — записал 0.
    let (s, v) = put_line(&w, &uri, 0, set(5, json!(0))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = put_line(&w, &uri, 2, set(0, json!(5))).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Владелец видел 5 и очищает поле: сейчас 0 — это посчитанный ноль, не «строки нет».
    let one = format!("/api/v1/revisions/{rid}/lines/{item}");
    let (s, v) = call(
        &w.app,
        "DELETE",
        &format!("{one}?expected_qty=5"),
        Some(w.owner()),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "count_changed", "{v}");
    assert_eq!(counted(&w, &rid, &item).await, Some(0), "ноль Нурлана цел");
    // Видел 0 — убирает.
    ok(
        &w.app,
        "DELETE",
        &format!("{one}?expected_qty=0"),
        w.owner(),
        Value::Null,
    )
    .await;
    assert_eq!(counted(&w, &rid, &item).await, None);
    // Строки нет, а итог сверяют с «посчитано 0» — тоже 409.
    let (s, v) = put_line(&w, &uri, 1, set(3, json!(0))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(counted(&w, &rid, &item).await, None);
    // Видел пустую и очистил поле, а строку тем временем посчитали — 409, посчитанное цело.
    let (s, v) = put_line(&w, &uri, 2, set(2, Value::Null)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = call(
        &w.app,
        "DELETE",
        &format!("{one}?expected_empty=true"),
        Some(w.owner()),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(counted(&w, &rid, &item).await, Some(2));
}

/// Повтор того же «убрать строку» (`op_id`, ответ потерялся) — прежний ответ, а не 409 по чужой
/// свежей прибавке: чужой скан не удаляется переспрашиванием.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn repeated_line_delete_returns_same_answer(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча убрать").await;
    let (s, v) = put_line(
        &w,
        &uri,
        0,
        json!({ "product_id": item, "counted_qty": 3, "mode": "set", "expected_empty": true, "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let del = format!(
        "/api/v1/revisions/{rid}/lines/{item}?expected_qty=3&op_id={}",
        Uuid::now_v7()
    );
    let first = ok(&w.app, "DELETE", &del, w.owner(), Value::Null).await;
    // Айбек сканирует тот же товар после того, как строку убрали.
    let (s, v) = put_line(
        &w,
        &uri,
        1,
        json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Повтор удаления (обрыв связи) — прежний ответ, скан Айбека цел.
    let (s, again) = call(&w.app, "DELETE", &del, Some(w.owner()), None, None).await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(again, first);
    assert_eq!(counted(&w, &rid, &item).await, Some(1));
}

/// Охваты ревизий пересекаются: замок филиала и проверка — при любом `POST /revisions`, а не
/// только с `reuse_draft` (вкладка со старой версией экрана). Проведение не вычтет остаток дважды:
/// товар, посчитанный и в другом открытом черновике, — 409.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_scopes_never_overlap(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = w.category("filter").await;
    let whole = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let wid = whole["head"]["id"].as_str().unwrap().to_string();
    let again = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    assert_eq!(again["head"]["id"], json!(wid), "второго «весь склад» нет");
    let by_cat = ok(
        &w.app,
        "POST",
        "/api/v1/revisions",
        w.owner(),
        json!({ "category_id": cat }),
    )
    .await;
    assert_eq!(
        by_cat["head"]["id"],
        json!(wid),
        "категория — в «весь склад»"
    );
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/revisions/{wid}/cancel"),
        w.owner(),
        json!({}),
    )
    .await;
    let c1 = ok(
        &w.app,
        "POST",
        "/api/v1/revisions",
        w.owner(),
        json!({ "category_id": cat }),
    )
    .await;
    let c2 = ok(
        &w.app,
        "POST",
        "/api/v1/revisions",
        w.owner(),
        json!({ "category_id": cat }),
    )
    .await;
    assert_eq!(c1["head"]["id"], c2["head"]["id"], "одна ревизия категории");
    let (s, v) = call(
        &w.app,
        "POST",
        "/api/v1/revisions",
        Some(w.owner()),
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "revision_open", "{v}");
    assert_eq!(drafts(&w).await, 1);

    // Черновик, заведённый до этой проверки (или товар перенесли в другую категорию): товар
    // посчитан в двух открытых черновиках — проведение одного из них — 409 с названием и номером.
    let cid = c1["head"]["id"].as_str().unwrap().to_string();
    let item = w.product("Фильтр двойной").await;
    let uri = format!("/api/v1/revisions/{cid}/lines");
    let (s, v) = put_line(
        &w,
        &uri,
        0,
        json!({ "product_id": item, "counted_qty": 2, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let old = Uuid::now_v7();
    sqlx::query(
        r#"insert into revisions (id, branch_id, number, category_id, comment, user_id, device_id)
           select $1, $2, 990, null, '', id, $1 from users where login = 'owner'"#,
    )
    .bind(old)
    .bind(w.branch)
    .execute(&w.pool)
    .await
    .unwrap();
    sqlx::query(
        r#"insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count)
           select $1, $2, 1, id, 0 from users where login = 'owner'"#,
    )
    .bind(old)
    .bind(Uuid::parse_str(&item).unwrap())
    .execute(&w.pool)
    .await
    .unwrap();
    let (s, v) = call(
        &w.app,
        "POST",
        &format!("/api/v1/revisions/{cid}/post"),
        Some(w.owner()),
        None,
        Some(json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "revision_overlap", "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(m.contains("Фильтр двойной") && m.contains("№ 990"), "{m}");
}

/// Старая версия экрана шлёт «записать итог» без `expected_qty`: если строку в последние 10 минут
/// прибавляли с других устройств — 409 «обновите страницу», чужие сканы не затираются. Без чужих
/// прибавок — как раньше.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_screen_set_does_not_wipe_fresh_adds(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча старый экран").await;
    let tablet = Uuid::now_v7();
    let add =
        json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() });
    let (s, v) = put_line(&w, &uri, 1, add).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = call_from(
        &w.app,
        "PUT",
        &uri,
        Some(w.owner()),
        None,
        Some(json!({ "product_id": item, "counted_qty": 5 })),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "client_outdated", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Обновите страницу"),
        "{v}"
    );
    assert_eq!(counted(&w, &rid, &item).await, Some(1));
    // Строку без чужих прибавок старый экран пишет как раньше.
    let other = w.product("Свеча без прибавок").await;
    let (s, v) = call_from(
        &w.app,
        "PUT",
        &uri,
        Some(w.owner()),
        None,
        Some(json!({ "product_id": other, "counted_qty": 5 })),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
}

/// Журнал строки — в порядке записи (под замком черновика), а не по времени начала транзакции:
/// транзакция, ждавшая замок, не окажется в журнале раньше прибавки, после которой записана.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn count_log_follows_write_order(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, _) = draft_with_item(&w, "Свеча журнал").await;
    // Итог «= 5» начат раньше (время начала транзакции меньше), а записан после прибавки.
    for (login, mode, qty, ago) in [("admin1", "add", 2_i64, 0), ("owner", "set", 5, 30)] {
        sqlx::query(
            r#"insert into revision_line_adds (id, revision_id, product_id, mode, qty, user_id, device_id, created_at)
               select $1, $2, $3, $4, $5, id, $1, now() - make_interval(secs => $6) from users where login = $7"#,
        )
        .bind(Uuid::now_v7())
        .bind(Uuid::parse_str(&rid).unwrap())
        .bind(Uuid::parse_str(&item).unwrap())
        .bind(mode)
        .bind(qty)
        .bind(f64::from(ago))
        .bind(login)
        .execute(&w.pool)
        .await
        .unwrap();
    }
    sqlx::query(
        r#"insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count)
           select $1, $2, 5, id, 0 from users where login = 'owner'"#,
    )
    .bind(Uuid::parse_str(&rid).unwrap())
    .bind(Uuid::parse_str(&item).unwrap())
    .execute(&w.pool)
    .await
    .unwrap();
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    let log: Vec<String> = line(&full, &item)["counted_log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} {} {}",
                e["by"].as_str().unwrap(),
                e["mode"].as_str().unwrap(),
                e["qty"]
            )
        })
        .collect();
    assert_eq!(log, vec!["Айбек add 2", "Владелец set 5"]);
}

/// Версия строки: каждый ответ записи и перечитывания несёт номер последней записи строки —
/// экран показывает ответ, только если он новее показанного (старый итог не возвращается).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn line_version_grows_with_every_write(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча версия").await;
    let add =
        || json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() });
    let (_, a) = put_line(&w, &uri, 1, add()).await;
    let (_, b) = put_line(&w, &uri, 2, add()).await;
    let va = a["version"].as_i64().expect("версия в ответе записи");
    let vb = b["version"].as_i64().unwrap();
    assert!(vb > va, "{a} {b}");
    let full = get(&w.app, &format!("/api/v1/revisions/{rid}"), w.owner()).await;
    assert_eq!(line(&full, &item)["version"], json!(vb));
    let one = get(
        &w.app,
        &format!("/api/v1/revisions/{rid}/lines/{item}"),
        w.owner(),
    )
    .await;
    assert_eq!(one["version"], json!(vb));
    let d = ok(
        &w.app,
        "DELETE",
        &format!("/api/v1/revisions/{rid}/lines/{item}?expected_qty=2"),
        w.owner(),
        Value::Null,
    )
    .await;
    assert!(d["version"].as_i64().unwrap() > vb, "{d}");
    let one = get(
        &w.app,
        &format!("/api/v1/revisions/{rid}/lines/{item}"),
        w.owner(),
    )
    .await;
    assert_eq!(one, json!({ "counted_qty": null, "version": d["version"] }));
}

/// Проведение, пока другие ещё считают: прибавки за последние 60 с — 409 «Ещё считают: Нурлан
/// N с назад. Провести?»; подтвердил — проводится. Скан после проведения — понятный отказ.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn posting_while_others_count_asks_first(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча проведение").await;
    let add =
        || json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() });
    let (s, v) = put_line(&w, &uri, 2, add()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let post = format!("/api/v1/revisions/{rid}/post");
    let op = Uuid::now_v7();
    let (s, v) = call(
        &w.app,
        "POST",
        &post,
        Some(w.owner()),
        None,
        Some(json!({ "op_id": op, "comment": "пересчёт" })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "still_counting", "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(
        m.starts_with("Ещё считают: Нурлан") && m.contains(" с назад") && m.ends_with("Провести?"),
        "{m}"
    );
    // Подтвердил — проводится тем же op_id.
    ok(
        &w.app,
        "POST",
        &post,
        w.owner(),
        json!({ "op_id": op, "comment": "пересчёт", "force": true }),
    )
    .await;
    let (s, v) = put_line(&w, &uri, 1, add()).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "revision_closed", "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(
        m.contains("уже проведена") && m.contains("Владелец") && m.contains("ваш скан не учтён"),
        "{m}"
    );
}

/// Показ не откатывается назад: `GET /revisions/{id}` и `GET …/lines/{product_id}` читают итог
/// строки и её версию из одного снимка — итог ровно тот, что был после записи с этой версией.
/// Пока четыре человека сканируют, перечитывания идут непрерывно; каждый ответ сверяется с
/// журналом: прибавок по 1 с номером не больше версии — столько, сколько в итоге.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn reread_counted_and_version_come_from_one_snapshot(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча снимок").await;
    let mut writers = JoinSet::new();
    for who in 0..4 {
        let app = w.app.clone();
        let cookie = w.people[who].clone();
        let uri = uri.clone();
        let item = item.clone();
        writers.spawn(async move {
            for _ in 0..15 {
                let body = json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() });
                let (s, v) = call(&app, "PUT", &uri, Some(&cookie), None, Some(body)).await;
                assert_eq!(s, StatusCode::OK, "{v}");
            }
        });
    }
    let mut readers = JoinSet::new();
    for r in 0..4 {
        let app = w.app.clone();
        let cookie = w.owner().to_string();
        let full = format!("/api/v1/revisions/{rid}");
        let one = format!("/api/v1/revisions/{rid}/lines/{item}");
        let item = item.clone();
        readers.spawn(async move {
            let mut seen = Vec::new();
            for _ in 0..25 {
                let (s, v) = if r % 2 == 0 {
                    call(&app, "GET", &full, Some(&cookie), None, None).await
                } else {
                    call(&app, "GET", &one, Some(&cookie), None, None).await
                };
                assert_eq!(s, StatusCode::OK, "{v}");
                let l = if r % 2 == 0 {
                    // Строки ещё нет (никто не отсканировал): сверять нечего.
                    let Some(l) = v["lines"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|l| l["product_id"] == json!(item))
                    else {
                        continue;
                    };
                    l.clone()
                } else {
                    v
                };
                seen.push((
                    l["counted_qty"].as_i64().unwrap_or(0),
                    l["version"].as_i64().unwrap(),
                ));
            }
            seen
        });
    }
    while let Some(r) = writers.join_next().await {
        r.unwrap();
    }
    let mut seen = Vec::new();
    while let Some(r) = readers.join_next().await {
        seen.extend(r.unwrap());
    }
    let seqs: Vec<i64> = sqlx::query_scalar(
        "select seq from revision_line_adds where revision_id = $1 and product_id = $2 order by seq",
    )
    .bind(Uuid::parse_str(&rid).unwrap())
    .bind(Uuid::parse_str(&item).unwrap())
    .fetch_all(&w.pool)
    .await
    .unwrap();
    assert_eq!(seqs.len(), 60);
    for (counted, version) in seen {
        let by_version = i64::try_from(seqs.iter().filter(|s| **s <= version).count()).unwrap();
        assert_eq!(
            counted, by_version,
            "итог {counted} при версии {version}: по журналу {by_version}"
        );
    }
}

/// Экран до обновления очищает строку без `expected_*`: строку, которую за последние 10 минут
/// прибавляли с других устройств, он стёр бы вместе с чужими сканами — 409 «обновите страницу»,
/// как у «= итог». Без чужих прибавок — как раньше.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn old_screen_delete_does_not_wipe_fresh_adds(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item, uri) = draft_with_item(&w, "Свеча старый экран убрать").await;
    let tablet = Uuid::now_v7();
    let (s, v) = put_line(
        &w,
        &uri,
        1,
        json!({ "product_id": item, "counted_qty": 2, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let del = format!("/api/v1/revisions/{rid}/lines/{item}");
    let (s, v) = call_from(&w.app, "DELETE", &del, Some(w.owner()), None, None, tablet).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "client_outdated", "{v}");
    assert_eq!(counted(&w, &rid, &item).await, Some(2));
    // Свою строку без чужих прибавок старый экран убирает как раньше.
    let other = w.product("Свеча старый экран своя").await;
    let (s, v) = call_from(
        &w.app,
        "PUT",
        &uri,
        Some(w.owner()),
        None,
        Some(json!({ "product_id": other, "counted_qty": 4 })),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = call_from(
        &w.app,
        "DELETE",
        &format!("/api/v1/revisions/{rid}/lines/{other}"),
        Some(w.owner()),
        None,
        None,
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(counted(&w, &rid, &other).await, None);
}

/// «Товар не из программы»: «Завести» с 5 записалось, ответ потерян; человек исправил найденное
/// на 6 и нажал снова — это новый ввод (новый `op_id`), но прошлое нажатие (`prev_op_ids`) уже
/// завело карточку: сервер отвечает тем, что записано тогда (`earlier`), а не заводит второй
/// товар — и с кодом, и без кода (код выдаёт система). Прошлое нажатие не дошло — заводится.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn new_product_retry_with_changed_count_does_not_create_twice(pool: PgPool) {
    let w = World::new(pool).await;
    let cat = w.category("filter").await;
    let rev = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), json!({})).await;
    let rid = rev["head"]["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/revisions/{rid}/new-product");
    let products = |name: &'static str| {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select count(*) from products where name = $1")
                .bind(name)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    for (name, codes) in [
        ("Щётка с кодом", json!(["4600000000031"])),
        ("Щётка без кода", json!([])),
    ] {
        let first = Uuid::now_v7();
        let body = |op: Uuid, qty: i64, prev: Value| {
            json!({ "op_id": op, "counted_qty": qty, "category_id": cat, "name": name,
                    "barcodes": codes, "sale_price_tyiyn": 10_000, "prev_op_ids": prev })
        };
        let a = ok(&w.app, "POST", &uri, w.owner(), body(first, 5, json!([]))).await;
        let b = ok(
            &w.app,
            "POST",
            &uri,
            w.owner(),
            body(Uuid::now_v7(), 6, json!([first])),
        )
        .await;
        assert_eq!(b["earlier"], json!(true), "{b}");
        assert_eq!(b["product"]["id"], a["product"]["id"], "{b}");
        assert_eq!(b["counted_qty"], json!(5), "{b}");
        assert_eq!(products(name).await, 1, "{name}");
    }
    // Прошлое нажатие не дошло до сервера — заводится как обычно.
    let c = ok(
        &w.app,
        "POST",
        &uri,
        w.owner(),
        json!({ "op_id": Uuid::now_v7(), "counted_qty": 3, "category_id": cat, "name": "Щётка не дошла",
                "barcodes": [], "sale_price_tyiyn": 10_000, "prev_op_ids": [Uuid::now_v7()] }),
    )
    .await;
    assert!(c["earlier"].is_null(), "{c}");
    assert_eq!(c["counted_qty"], json!(3));
    assert_eq!(products("Щётка не дошла").await, 1);
}

/// Остаток товара на филиале сейчас.
async fn stock(w: &World, item: &str) -> i64 {
    sqlx::query_scalar(
        "select stock_qty from branch_products where branch_id = $1 and product_id = $2",
    )
    .bind(w.branch)
    .bind(Uuid::parse_str(item).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap()
}

/// Товар перенесли из категории X в Y, пока открыты ревизии обеих: строка в Y, пока товар
/// посчитан в X, — 409 `revision_overlap`; пока X проводится, запись в Y ждёт её конца и берёт
/// остаток уже после движения X — недостача не вычитается дважды (задача 003, Н14).
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn moved_product_is_not_subtracted_twice(pool: PgPool) {
    let w = World::new(pool).await;
    let (cat_x, cat_y) = (w.category("filter").await, w.category("filter").await);
    let p = ok(
        &w.app,
        "POST",
        "/api/v1/products",
        w.owner(),
        json!({ "op_id": Uuid::now_v7(), "category_id": cat_x, "name": "Фильтр перенесённый", "sale_price_tyiyn": 10_000 }),
    )
    .await;
    let item = p["id"].as_str().unwrap().to_string();
    let other = w.product("Фильтр соседний").await;
    for it in [&item, &other] {
        ok(
            &w.app,
            "POST",
            "/api/v1/stock-ins",
            w.owner(),
            json!({ "op_id": Uuid::now_v7(), "comment": "приход",
                    "lines": [{ "product_id": it, "qty": 10, "price_tyiyn": 5_000 }] }),
        )
        .await;
    }
    let rev = |cat: &str| json!({ "category_id": cat });
    let x = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), rev(&cat_x)).await;
    let xid = x["head"]["id"].as_str().unwrap().to_string();
    let (s, v) = put_line(
        &w,
        &format!("/api/v1/revisions/{xid}/lines"),
        0,
        json!({ "product_id": item, "counted_qty": 7, "mode": "set", "expected_empty": true, "op_id": Uuid::now_v7() }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    // Товар перенесли в категорию Y; ревизия Y — своя (охваты не пересекаются).
    sqlx::query("update products set category_id = $1 where id = $2")
        .bind(Uuid::parse_str(&cat_y).unwrap())
        .bind(Uuid::parse_str(&item).unwrap())
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("update products set category_id = $1 where id = $2")
        .bind(Uuid::parse_str(&cat_y).unwrap())
        .bind(Uuid::parse_str(&other).unwrap())
        .execute(&w.pool)
        .await
        .unwrap();
    let y = ok(&w.app, "POST", "/api/v1/revisions", w.owner(), rev(&cat_y)).await;
    let yid = y["head"]["id"].as_str().unwrap().to_string();
    assert_ne!(xid, yid);
    let y_lines = format!("/api/v1/revisions/{yid}/lines");
    let add = |it: &str| json!({ "product_id": it, "counted_qty": 7, "mode": "add", "op_id": Uuid::now_v7() });

    // Товар посчитан в открытой X — в Y не записывается; соседний товар — записывается.
    let (s, v) = put_line(&w, &y_lines, 1, add(&item)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "revision_overlap", "{v}");
    let m = v["error"]["message"].as_str().unwrap();
    assert!(
        m.contains("Фильтр перенесённый") && m.contains(&format!("№ {}", x["head"]["number"])),
        "{m}"
    );
    let (s, v) = put_line(&w, &y_lines, 1, add(&other)).await;
    assert_eq!(s, StatusCode::OK, "{v}");

    // X проводится (замок черновиков филиала держит «проведение» ниже), в Y тем временем сканируют
    // тот же товар: запись ждёт конца проведения и видит X проведённой.
    let mut held = w.pool.begin().await.unwrap();
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('revision.draft:' || $1::text, 0))")
        .bind(w.branch.to_string())
        .execute(&mut *held)
        .await
        .unwrap();
    let (app, owner, post_x) = (
        w.app.clone(),
        w.owner().to_string(),
        format!("/api/v1/revisions/{xid}/post"),
    );
    let posting = tokio::spawn(async move {
        call(
            &app,
            "POST",
            &post_x,
            Some(&owner),
            None,
            Some(json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт X" })),
        )
        .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let (app, admin, uri, body) = (
        w.app.clone(),
        w.people[1].clone(),
        y_lines.clone(),
        add(&item),
    );
    let scanning =
        tokio::spawn(async move { call(&app, "PUT", &uri, Some(&admin), None, Some(body)).await });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !posting.is_finished() && !scanning.is_finished(),
        "проведение и запись ждут замка черновиков филиала"
    );
    held.commit().await.unwrap();
    let (s, v) = posting.await.unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = scanning.await.unwrap();
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(stock(&w, &item).await, 7, "X: недостача 3");
    let at: Option<i64> = sqlx::query_scalar(
        "select stock_at_count from revision_lines where revision_id = $1 and product_id = $2",
    )
    .bind(Uuid::parse_str(&yid).unwrap())
    .bind(Uuid::parse_str(&item).unwrap())
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(at, Some(7), "остаток строки Y — после движения X");

    // Y проводится: в Y тоже 7 — расхождения нет, недостача не вычлась второй раз.
    ok(
        &w.app,
        "POST",
        &format!("/api/v1/revisions/{yid}/post"),
        w.owner(),
        json!({ "op_id": Uuid::now_v7(), "comment": "пересчёт Y", "force": true }),
    )
    .await;
    assert_eq!(stock(&w, &item).await, 7, "недостача вычтена один раз");
    assert_eq!(stock(&w, &other).await, 7);
}
