//! Коды из старой базы Умаг — везде (SPEC-35, ADR-110): связь строки Умага с товаром, перенос
//! кодов и конфликты, потерянные нули, автосвязывание и похожие пары, новая выгрузка, права,
//! журнал, повтор `op_id`, филиалы и снимок кассы без сети.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

struct T {
    app: Router,
    pool: PgPool,
    owner: String,
    /// Администратор с правами по умолчанию — «Товары и штрихкоды» у него есть.
    admin: String,
    /// Администратор без права «Товары и штрихкоды» (ревизия есть).
    noprod: String,
}

async fn add_admin(pool: &PgPool, login: &str, branch: Uuid, without: &[&str]) {
    let hash = hash_password("admin-pass-1".into()).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $3)",
    )
    .bind(id)
    .bind(branch)
    .bind(login)
    .bind(hash)
    .execute(pool)
    .await
    .unwrap();
    let keys: Vec<String> = avtodom_server::auth::default_permissions()
        .into_iter()
        .filter(|k| !without.contains(k))
        .map(String::from)
        .collect();
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
        .bind(id)
        .bind(branch)
        .bind(keys)
        .execute(pool)
        .await
        .unwrap();
}

async fn setup(pool: PgPool) -> T {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    add_admin(&pool, "admin", branch, &[]).await;
    add_admin(&pool, "noprod", branch, &["products"]).await;
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let mut t = T {
        app,
        pool,
        owner: String::new(),
        admin: String::new(),
        noprod: String::new(),
    };
    t.owner = t.login("owner", "owner-pass-1").await;
    t.admin = t.login("admin", "admin-pass-1").await;
    t.noprod = t.login("noprod", "admin-pass-1").await;
    t
}

impl T {
    async fn login(&self, login: &str, password: &str) -> String {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
            .header("x-device-id", Uuid::now_v7().to_string())
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
            .unwrap()
            .to_string()
    }

    async fn call_in(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        branch: Option<Uuid>,
        body: Option<Value>,
    ) -> (StatusCode, HeaderMap, Value) {
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
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            headers,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn call(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (s, _, v) = self.call_in(method, uri, cookie, None, body).await;
        (s, v)
    }

    async fn ok(&self, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
        let (s, v) = self.call(method, uri, cookie, Some(body)).await;
        assert!(s.is_success(), "{method} {uri}: {s} {v}");
        v
    }

    async fn get(&self, uri: &str, cookie: &str) -> Value {
        let (s, v) = self.call("GET", uri, cookie, None).await;
        assert!(s.is_success(), "GET {uri}: {s} {v}");
        v
    }

    /// Выгрузка Умага владельцем в его текущем филиале.
    async fn import(&self, rows: Value) -> Value {
        let (s, v) = self
            .call("PUT", "/api/v1/old-catalog", &self.owner, Some(rows))
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v
    }

    /// Id строки Умага по коду.
    async fn row(&self, code: &str) -> String {
        let hits = self
            .get(
                &format!("/api/v1/old-catalog/search?barcode={code}"),
                &self.owner,
            )
            .await;
        assert_eq!(hits[0]["exact"], true, "{code}: {hits}");
        hits[0]["id"].as_str().unwrap().to_string()
    }

    async fn category(&self, name: &str, kind: &str) -> String {
        let c = self
            .ok(
                "POST",
                "/api/v1/categories",
                &self.owner,
                json!({ "name": name, "kind": kind }),
            )
            .await;
        c["id"].as_str().unwrap().to_string()
    }

    async fn product(&self, cookie: &str, mut body: Value) -> String {
        body["op_id"] = json!(Uuid::now_v7());
        let p = self.ok("POST", "/api/v1/products", cookie, body).await;
        p["id"].as_str().unwrap().to_string()
    }

    /// Скан на кассе и в приходе: товар по коду.
    async fn by_code(&self, code: &str, cookie: &str) -> (StatusCode, Value) {
        self.call(
            "GET",
            &format!("/api/v1/products/by-barcode/{code}"),
            cookie,
            None,
        )
        .await
    }

    async fn codes(&self, product: &str) -> Vec<String> {
        let p = self
            .get(&format!("/api/v1/products/{product}"), &self.owner)
            .await;
        let mut c: Vec<String> = p["barcodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        c.sort();
        c
    }

    /// Подсказка Умага по коду — первая строка.
    async fn hit(&self, code: &str) -> Value {
        self.get(
            &format!("/api/v1/old-catalog/search?barcode={code}"),
            &self.owner,
        )
        .await[0]
            .clone()
    }

    async fn link(&self, row: &str, product: &str, cookie: &str) -> (StatusCode, Value) {
        self.call(
            "POST",
            &format!("/api/v1/old-catalog/{row}/link"),
            cookie,
            Some(json!({ "op_id": Uuid::now_v7(), "product_id": product })),
        )
        .await
    }

    async fn journal(&self, action: &str) -> Vec<(Option<Uuid>, Value)> {
        sqlx::query_as::<_, (Option<Uuid>, Value)>(
            "select entity_id, data from audit_log where action = $1 order by created_at, id",
        )
        .bind(action)
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }
}

fn has(list: &Value, code: &str) -> bool {
    list.as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some(code))
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn linked_umag_codes_find_the_product_everywhere(pool: PgPool) {
    let t = setup(pool).await;
    t.import(json!([
        { "name": "TOTACHI NIRO 5W30 4Л", "barcode": "2760488", "extra_code": "1489;8809451090361",
          "unit": "литр", "price_tyiyn": 55_000, "cost_tyiyn": 32_350 },
        { "name": "Фильтр C-110", "barcode": "123456789012", "unit": "шт" }
    ]))
    .await;
    let oil = t.category("Масла", "oil").await;
    let pid = t
        .product(
            &t.owner,
            json!({ "category_id": oil, "name": "Totachi NIRO 5W-30", "container_ml": 4000,
                    "sale_price_tyiyn": 220_000 }),
        )
        .await;
    // Не связана — касса код Умага не знает: «Код не найден».
    assert_eq!(
        t.by_code("8809451090361", &t.admin).await.0,
        StatusCode::NOT_FOUND
    );
    // «Товары» и «Приход» видят строку Умага по коду.
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=8809451090361", &t.admin)
        .await;
    assert_eq!(hits[0]["exact"], true);
    assert_eq!(hits[0]["product_id"], Value::Null);
    assert_eq!(hits[0]["linked_product_id"], Value::Null);
    assert_eq!(
        hits[0]["codes"],
        json!(["2760488", "1489", "8809451090361"])
    );
    assert_eq!(hits[0]["conflicts"], json!([]));
    let row = hits[0]["id"].as_str().unwrap().to_string();

    // «Это уже есть — выбрать товар»: связать, коды переходят товару.
    let body = json!({ "op_id": Uuid::now_v7(), "product_id": pid });
    let uri = format!("/api/v1/old-catalog/{row}/link");
    let first = t.ok("POST", &uri, &t.admin, body.clone()).await;
    assert_eq!(first["copied"], json!(["2760488", "1489", "8809451090361"]));
    assert_eq!(first["conflicts"], json!([]));
    assert_eq!(first["product"]["id"], json!(pid));
    assert!(has(&first["product"]["barcodes"], "8809451090361"));
    // Повтор того же op_id — тот же ответ, вторая запись в журнал не пишется.
    assert_eq!(t.ok("POST", &uri, &t.admin, body).await, first);
    let journal = t.journal("old_catalog.link").await;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].0, Some(Uuid::parse_str(&pid).unwrap()));
    assert_eq!(journal[0].1["how"], "manual");
    assert_eq!(journal[0].1["umag_name"], "TOTACHI NIRO 5W30 4Л");

    // Код Умага находит товар: скан на кассе и в приходе, и с нулями впереди.
    for code in ["2760488", "1489", "8809451090361", "0008809451090361"] {
        let (s, found) = t.by_code(code, &t.admin).await;
        assert_eq!(
            (s, found["id"].clone()),
            (StatusCode::OK, json!(pid)),
            "{code}"
        );
    }
    // Поиск товара и «Товары».
    let found = t.get("/api/v1/products?q=1489", &t.admin).await;
    assert_eq!(found[0]["id"], json!(pid));
    let found = t.get("/api/v1/products?q=08809451090361", &t.admin).await;
    assert_eq!(found[0]["id"], json!(pid));
    let list = t
        .get("/api/v1/products/list?q=8809451090361", &t.admin)
        .await;
    assert_eq!(list["items"][0]["id"], json!(pid));
    // Касса без сети: перенесённые коды — в снимке.
    let snap = t.get("/api/v1/offline/snapshot", &t.admin).await;
    let sp = snap["products"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == json!(pid))
        .unwrap();
    for code in ["2760488", "1489", "8809451090361"] {
        assert!(has(&sp["barcodes"], code), "{code}: {sp}");
    }
    // Коды — с источником «Умаг», свой код программы — «свой».
    let sources: Vec<(String, String)> = sqlx::query_as(
        "select code, source from product_barcodes where product_id = $1 and source = 'umag' order by code",
    )
    .bind(Uuid::parse_str(&pid).unwrap())
    .fetch_all(&t.pool)
    .await
    .unwrap();
    assert_eq!(sources.len(), 3);
    // Подсказка показывает связанный товар.
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1489", &t.admin)
        .await;
    assert_eq!(
        (
            hits[0]["product_id"].clone(),
            hits[0]["product_name"].clone()
        ),
        (json!(pid), json!("Totachi NIRO 5W-30"))
    );
    // Карточка товара — блок «Умаг»; закупка Умага — только владельцу (ADR-008).
    let card = t.get(&format!("/api/v1/products/{pid}"), &t.owner).await;
    assert_eq!(card["umag"]["id"], json!(row));
    assert_eq!(
        card["umag"]["codes"],
        json!(["2760488", "1489", "8809451090361"])
    );
    assert_eq!(
        (
            card["umag"]["price_tyiyn"].clone(),
            card["umag"]["cost_tyiyn"].clone()
        ),
        (json!(55_000), json!(32_350))
    );
    let card = t.get(&format!("/api/v1/products/{pid}"), &t.admin).await;
    assert_eq!(card["umag"]["cost_tyiyn"], Value::Null);
    assert_eq!(card["umag"]["price_tyiyn"], 55_000);
    // Товар без связи — блока нет.
    let other = t
        .product(
            &t.owner,
            json!({ "category_id": oil, "name": "Другое масло", "container_ml": 1000, "sale_price_tyiyn": 50_000 }),
        )
        .await;
    let card = t.get(&format!("/api/v1/products/{other}"), &t.owner).await;
    assert_eq!(card["umag"], Value::Null);
    // Сводка экрана «Связать с Умагом».
    let stats = t.get("/api/v1/old-catalog", &t.admin).await;
    assert_eq!(
        (
            stats["rows"].clone(),
            stats["linked"].clone(),
            stats["unlinked"].clone(),
            stats["conflicts"].clone()
        ),
        (json!(2), json!(1), json!(1), json!(0))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn search_puts_codes_first_and_not_yet_created_above(pool: PgPool) {
    // Блок «Из старой базы Умаг» берёт из подсказок не заведённые строки — они выше связанных.
    let t = setup(pool).await;
    t.import(json!([
        { "name": "Свеча зажигания NGK 1", "barcode": "2760001" },
        { "name": "Свеча зажигания NGK 2", "barcode": "2760002" },
        { "name": "Свеча зажигания NGK 3", "barcode": "2760003", "extra_code": "1550" }
    ]))
    .await;
    let cat = t.category("Свечи", "other").await;
    let p = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Свеча NGK", "sale_price_tyiyn": 30_000 }),
        )
        .await;
    let row = t.row("2760001").await;
    assert_eq!(t.link(&row, &p, &t.owner).await.0, StatusCode::OK);
    let hits = t
        .get("/api/v1/old-catalog/search?q=свеча+зажигания", &t.admin)
        .await;
    let names: Vec<&str> = hits
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Свеча зажигания NGK 2",
            "Свеча зажигания NGK 3",
            "Свеча зажигания NGK 1"
        ]
    );
    assert_eq!(hits[2]["linked_product_id"], json!(p));
    // Код в строке поиска — точное совпадение сверху.
    let hits = t.get("/api/v1/old-catalog/search?q=1550", &t.admin).await;
    assert_eq!(
        (hits[0]["name"].clone(), hits[0]["exact"].clone()),
        (json!("Свеча зажигания NGK 3"), json!(true))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn product_from_umag_row_is_linked_and_needs_the_right(pool: PgPool) {
    let t = setup(pool).await;
    t.import(json!([
        { "name": "Свеча NGK BKR6E", "barcode": "4800000000202", "article": "BKR6E",
          "extra_code": "1550", "cost_tyiyn": 20_000, "price_tyiyn": 30_000, "unit": "шт", "category": "Свечи" },
        { "name": "Свеча Bosch", "barcode": "4800000000301" }
    ]))
    .await;
    let cat = t.category("Свечи", "other").await;
    let row = t.row("4800000000202").await;
    // Без права «Товары и штрихкоды» строку Умага видно, а завести из неё нельзя.
    let hits = t
        .get(
            "/api/v1/old-catalog/search?barcode=4800000000202",
            &t.noprod,
        )
        .await;
    assert_eq!(hits[0]["id"], json!(row));
    let card = json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Свеча NGK BKR6E",
                       "barcodes": ["4800000000202"], "sale_price_tyiyn": 30_000,
                       "initial_cost_tyiyn": 20_000, "old_catalog_id": row });
    let (s, _) = t
        .call("POST", "/api/v1/products", &t.noprod, Some(card.clone()))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Ревизия заводит карточки по своему праву, но строку Умага без права не связывает.
    let rev = t
        .ok("POST", "/api/v1/revisions", &t.noprod, json!({}))
        .await;
    let mut in_rev = card.clone();
    in_rev["counted_qty"] = json!(2);
    let (s, v) = t
        .call(
            "POST",
            &format!(
                "/api/v1/revisions/{}/new-product",
                rev["head"]["id"].as_str().unwrap()
            ),
            &t.noprod,
            Some(in_rev),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");

    // «Завести»: карточка из строки Умага — связана, все коды у товара.
    let p = t
        .ok("POST", "/api/v1/products", &t.admin, card.clone())
        .await;
    let pid = p["id"].as_str().unwrap().to_string();
    for code in ["4800000000202", "BKR6E", "1550"] {
        assert!(has(&p["barcodes"], code), "{code}: {p}");
    }
    assert_eq!(p["last_purchase_price_tyiyn"], 20_000);
    // Повтор того же op_id — тот же товар, без второй карточки.
    let again = t.ok("POST", "/api/v1/products", &t.admin, card).await;
    assert_eq!(again["id"], json!(pid));
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1550", &t.admin)
        .await;
    assert_eq!(hits[0]["product_id"], json!(pid));
    let journal = t.journal("old_catalog.link").await;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].1["how"], "created");
    // Строка уже связана: вторая карточка из неё — 409, карточка не заводится.
    let (s, v) = t
        .call(
            "POST",
            "/api/v1/products",
            &t.admin,
            Some(
                json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Свеча дубль",
                         "sale_price_tyiyn": 30_000, "old_catalog_id": row }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let dups: i64 = sqlx::query_scalar("select count(*) from products where name = 'Свеча дубль'")
        .fetch_one(&t.pool)
        .await
        .unwrap();
    assert_eq!(dups, 0);
    // Несуществующая строка — понятный отказ.
    let (s, _) = t
        .call(
            "POST",
            "/api/v1/products",
            &t.admin,
            Some(
                json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Свеча X",
                         "sale_price_tyiyn": 30_000, "old_catalog_id": Uuid::now_v7() }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Строка уже связана с другим товаром — «Связать» тоже 409.
    let other = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Свеча Bosch", "sale_price_tyiyn": 25_000 }),
        )
        .await;
    assert_eq!(t.link(&row, &other, &t.admin).await.0, StatusCode::CONFLICT);
    // С тем же товаром — можно, ничего не меняется.
    let (s, v) = t.link(&row, &pid, &t.admin).await;
    assert_eq!((s, v["copied"].clone()), (StatusCode::OK, json!([])));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn code_of_another_product_is_not_taken(pool: PgPool) {
    let t = setup(pool).await;
    let cat = t.category("Хомуты", "other").await;
    let a = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Хомут A", "barcodes": ["4607001234567"], "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let c = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Хомут C", "barcodes": ["0123456789012"], "sale_price_tyiyn": 1_000 }),
        )
        .await;
    t.import(json!([
        { "name": "Хомут Б", "barcode": "2760999", "extra_code": "4607001234567;1500" },
        { "name": "Хомут Д", "barcode": "2760998", "extra_code": "123456789012" }
    ]))
    .await;
    // Строки Умага с кодом другого товара — в подсказке «уже заведён».
    let row = t.row("2760999").await;
    let b = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Хомут Б", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let (s, linked) = t.link(&row, &b, &t.owner).await;
    assert_eq!(s, StatusCode::OK, "{linked}");
    assert_eq!(linked["copied"], json!(["2760999", "1500"]));
    assert_eq!(
        linked["conflicts"],
        json!([{ "code": "4607001234567", "product_id": a, "product_name": "Хомут A" }])
    );
    // Код не украден: скан находит прежний товар.
    assert_eq!(t.by_code("4607001234567", &t.admin).await.1["id"], json!(a));
    // Тот же числовой код без нуля впереди у другого товара — тоже конфликт.
    let row2 = t.row("2760998").await;
    let d = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Хомут Д", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let (_, linked) = t.link(&row2, &d, &t.owner).await;
    assert_eq!(linked["copied"], json!(["2760998"]));
    assert_eq!(linked["conflicts"][0]["product_id"], json!(c));
    assert_eq!(t.by_code("123456789012", &t.admin).await.1["id"], json!(c));
    // Отметка видна: в сводке и в подсказке.
    let stats = t.get("/api/v1/old-catalog", &t.owner).await;
    assert_eq!(stats["conflicts"], 2);
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1500", &t.owner)
        .await;
    assert_eq!(hits[0]["conflicts"][0]["product_name"], "Хомут A");
    assert_eq!(t.codes(&a).await, ["4607001234567"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn auto_link_by_code_and_name_then_similar_pairs(pool: PgPool) {
    let t = setup(pool).await;
    let oil = t.category("Масла", "oil").await;
    let filters = t.category("Фильтры", "filter").await;
    let oil_product = |name: &str, codes: Value| json!({ "category_id": oil, "name": name, "container_ml": 4000, "sale_price_tyiyn": 200_000, "barcodes": codes });
    let p1 = t
        .product(
            &t.owner,
            oil_product("Масло «Ёлка» Super 5W-30", json!(["4600000000011"])),
        )
        .await;
    let p2 = t
        .product(&t.owner, oil_product("ZIC X7 5W-30", json!([])))
        .await;
    let p3 = t
        .product(
            &t.owner,
            oil_product("Castrol Magnatec 5W-30 4 л", json!([])),
        )
        .await;
    t.product(&t.owner, oil_product("Shell Helix HX7 10W-40", json!([])))
        .await;
    let p5 = t
        .product(
            &t.owner,
            json!({ "category_id": filters, "name": "Фильтр общий", "sale_price_tyiyn": 30_000 }),
        )
        .await;
    let p6 = t
        .product(
            &t.owner,
            json!({ "category_id": filters, "name": "Фильтр общий", "sale_price_tyiyn": 30_000 }),
        )
        .await;
    // Свой код программы у ZIC — в Умаге такой же код у чужого товара.
    let internal = t.codes(&p2).await[0].clone();
    t.import(json!([
        { "name": "ЁЛКА СУПЕР", "barcode": "2760001", "extra_code": "4600000000011" },
        { "name": "zic  x7 5W-30", "barcode": "2760002" },
        { "name": "CASTROL MAGNATEC 5W30 4Л", "barcode": "2760003", "price_tyiyn": 75_000 },
        { "name": "CASTROL MAGNATEC 5W30 1Л", "barcode": "2760004" },
        { "name": "SHELL HELIX HX8 5W40 SN 4л", "barcode": "2760005" },
        { "name": "Фильтр общий", "barcode": "2760006" },
        { "name": "Чужой товар", "barcode": internal }
    ]))
    .await;
    // Без права — 403.
    for (method, uri) in [
        ("POST", "/api/v1/old-catalog/auto-link"),
        ("GET", "/api/v1/old-catalog/pairs"),
    ] {
        let body = (method == "POST").then(|| json!({ "op_id": Uuid::now_v7() }));
        assert_eq!(
            t.call(method, uri, &t.noprod, body).await.0,
            StatusCode::FORBIDDEN,
            "{uri}"
        );
    }
    let body = json!({ "op_id": Uuid::now_v7() });
    let auto = t
        .ok(
            "POST",
            "/api/v1/old-catalog/auto-link",
            &t.admin,
            body.clone(),
        )
        .await;
    assert_eq!(auto, json!({ "linked": 2, "by_code": 1, "by_name": 1 }));
    // Повтор op_id — тот же ответ.
    assert_eq!(
        t.ok("POST", "/api/v1/old-catalog/auto-link", &t.admin, body)
            .await,
        auto
    );
    assert_eq!(t.journal("old_catalog.auto_link").await.len(), 1);
    assert_eq!(t.hit("2760001").await["product_id"], json!(p1));
    assert_eq!(t.hit("2760002").await["product_id"], json!(p2));
    // Коды перенесены: «ЁЛКА СУПЕР» по своему коду находит товар.
    assert_eq!(t.by_code("2760001", &t.admin).await.1["id"], json!(p1));
    // Два товара с одним названием — автоматически не связываем; код программы — не код Умага.
    assert_eq!(t.hit("2760006").await["product_id"], Value::Null);
    assert_eq!(t.hit(&internal).await["product_id"], Value::Null);
    let again = t
        .ok(
            "POST",
            "/api/v1/old-catalog/auto-link",
            &t.admin,
            json!({ "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(again["linked"], 0);

    // Похожие пары: объём и вязкость не расходятся, связанные не предлагаются.
    let (s, headers, list) = t
        .call_in("GET", "/api/v1/old-catalog/pairs", &t.admin, None, None)
        .await;
    assert_eq!(s, StatusCode::OK, "{list}");
    let pairs: Vec<(String, String)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["product"]["id"].as_str().unwrap().to_string(),
                p["umag"]["name"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut filters_first = vec![
        (p5.clone(), "Фильтр общий".to_string()),
        (p6.clone(), "Фильтр общий".to_string()),
    ];
    filters_first.sort();
    let mut head = pairs[..2].to_vec();
    head.sort();
    assert_eq!(head, filters_first);
    assert_eq!(
        pairs[2..],
        [(p3.clone(), "CASTROL MAGNATEC 5W30 4Л".to_string())]
    );
    assert_eq!(headers.get("x-total-count").unwrap(), "3");
    let castrol = &list[2];
    let sim = castrol["similarity"].as_f64().unwrap();
    assert!((0.45..=1.0).contains(&sim), "{castrol}");
    assert_eq!(castrol["umag"]["price_tyiyn"], 75_000);
    assert_eq!(castrol["product"]["sale_price_tyiyn"], 200_000);
    assert_eq!(list[0]["similarity"].as_f64(), Some(1.0));
    // Страницы: размер и номер с нуля, или limit/offset.
    let (_, headers, page) = t
        .call_in(
            "GET",
            "/api/v1/old-catalog/pairs?size=1&page=2",
            &t.admin,
            None,
            None,
        )
        .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "3");
    assert_eq!(page.as_array().unwrap().len(), 1);
    assert_eq!(page[0]["product"]["id"], json!(p3));
    let page = t
        .get("/api/v1/old-catalog/pairs?limit=2&offset=0", &t.admin)
        .await;
    assert_eq!(page.as_array().unwrap().len(), 2);
    // Поиск по парам.
    let found = t
        .get("/api/v1/old-catalog/pairs?q=magnatec", &t.admin)
        .await;
    assert_eq!(found.as_array().unwrap().len(), 1);

    // «Не то» — пара больше не предлагается и автоматически не связывается.
    let castrol_row = castrol["umag"]["id"].as_str().unwrap().to_string();
    let body = json!({ "op_id": Uuid::now_v7(), "product_id": p3 });
    let uri = format!("/api/v1/old-catalog/{castrol_row}/reject");
    assert_eq!(
        t.ok("POST", &uri, &t.admin, body.clone()).await,
        json!({ "ok": true })
    );
    assert_eq!(
        t.ok("POST", &uri, &t.admin, body).await,
        json!({ "ok": true })
    );
    assert_eq!(t.journal("old_catalog.reject").await.len(), 1);
    let (_, headers, _) = t
        .call_in("GET", "/api/v1/old-catalog/pairs", &t.admin, None, None)
        .await;
    assert_eq!(headers.get("x-total-count").unwrap(), "2");
    // «Связать» в паре — пара уходит, второй товар с тем же названием больше не предлагается.
    let filter_row = list[0]["umag"]["id"].as_str().unwrap().to_string();
    assert_eq!(t.link(&filter_row, &p5, &t.admin).await.0, StatusCode::OK);
    let rest = t.get("/api/v1/old-catalog/pairs", &t.admin).await;
    assert_eq!(rest, json!([]));
    let stats = t.get("/api/v1/old-catalog", &t.admin).await;
    assert_eq!(
        (stats["linked"].clone(), stats["unlinked"].clone()),
        (json!(3), json!(4))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn new_upload_keeps_links_and_new_codes(pool: PgPool) {
    let t = setup(pool).await;
    t.import(json!([
        { "name": "Масло A", "barcode": "2760001", "extra_code": "1489" },
        { "name": "Масло B", "barcode": "2760002" }
    ]))
    .await;
    let cat = t.category("Масла", "other").await;
    let p = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Товар A", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let q = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Товар B", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let r1 = t.row("2760001").await;
    let r2 = t.row("2760002").await;
    assert_eq!(t.link(&r1, &p, &t.owner).await.0, StatusCode::OK);
    assert_eq!(t.link(&r2, &q, &t.owner).await.0, StatusCode::OK);
    // Владелец убрал из карточки код Умага — новая выгрузка его не вернёт.
    t.ok(
        "DELETE",
        &format!("/api/v1/products/{p}/barcodes/1489"),
        &t.owner,
        json!({}),
    )
    .await;
    let out = t
        .import(json!([
            { "name": "Масло A", "barcode": "2760001", "extra_code": "1489;8800000000001" },
            { "name": "Масло B (новый код)", "barcode": "2760099", "extra_code": "2760002" },
            { "name": "Новинка", "barcode": "2760100" }
        ]))
        .await;
    assert_eq!(out, json!({ "rows": 3, "linked": 2, "copied": 2 }));
    let pc = t.codes(&p).await;
    assert!(pc.contains(&"8800000000001".to_string()), "{pc:?}");
    assert!(!pc.contains(&"1489".to_string()), "{pc:?}");
    assert!(t.codes(&q).await.contains(&"2760099".to_string()));
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=2760099", &t.owner)
        .await;
    assert_eq!(hits[0]["product_id"], json!(q));
    let stats = t.get("/api/v1/old-catalog", &t.owner).await;
    assert_eq!(
        (
            stats["rows"].clone(),
            stats["linked"].clone(),
            stats["unlinked"].clone()
        ),
        (json!(3), json!(2), json!(1))
    );
    // Выгрузку меняет только владелец.
    assert_eq!(
        t.call("PUT", "/api/v1/old-catalog", &t.admin, Some(json!([])))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn unlink_keeps_codes_and_needs_the_right(pool: PgPool) {
    let t = setup(pool).await;
    t.import(json!([{ "name": "Масло C", "barcode": "2760001", "extra_code": "1489" }]))
        .await;
    let cat = t.category("Масла", "other").await;
    let p = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Масло C", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let row = t.row("2760001").await;
    // Без права — 403 на всё, что меняет связи.
    assert_eq!(t.link(&row, &p, &t.noprod).await.0, StatusCode::FORBIDDEN);
    for action in ["unlink", "reject"] {
        let (s, _) = t
            .call(
                "POST",
                &format!("/api/v1/old-catalog/{row}/{action}"),
                &t.noprod,
                Some(json!({ "op_id": Uuid::now_v7(), "product_id": p })),
            )
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{action}");
    }
    assert_eq!(t.link(&row, &p, &t.admin).await.0, StatusCode::OK);
    let body = json!({ "op_id": Uuid::now_v7() });
    let uri = format!("/api/v1/old-catalog/{row}/unlink");
    assert_eq!(
        t.ok("POST", &uri, &t.admin, body.clone()).await,
        json!({ "ok": true })
    );
    // Повтор op_id — тот же ответ; новая отвязка несвязанной строки — 409.
    assert_eq!(
        t.ok("POST", &uri, &t.admin, body).await,
        json!({ "ok": true })
    );
    let (s, _) = t
        .call(
            "POST",
            &uri,
            &t.admin,
            Some(json!({ "op_id": Uuid::now_v7() })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let journal = t.journal("old_catalog.unlink").await;
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].0, Some(Uuid::parse_str(&p).unwrap()));
    // Коды остаются у товара.
    assert_eq!(t.by_code("1489", &t.admin).await.1["id"], json!(p));
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1489", &t.admin)
        .await;
    assert_eq!(hits[0]["product_id"], Value::Null);
    // Автосвязывание отвязанную пару не возвращает — ни по коду, ни по названию.
    let auto = t
        .ok(
            "POST",
            "/api/v1/old-catalog/auto-link",
            &t.admin,
            json!({ "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(auto["linked"], 0);
    // Связать снова можно руками.
    assert_eq!(t.link(&row, &p, &t.admin).await.0, StatusCode::OK);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn umag_list_is_per_branch_and_codes_are_shared(pool: PgPool) {
    let t = setup(pool.clone()).await;
    let a: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let nb = t
        .ok(
            "POST",
            "/api/v1/branches",
            &t.owner,
            json!({ "op_id": Uuid::now_v7(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
        )
        .await;
    let b = Uuid::parse_str(nb["id"].as_str().unwrap()).unwrap();
    let (s, _, u) = t
        .call_in(
            "POST",
            "/api/v1/users",
            &t.owner,
            Some(b),
            Some(json!({ "login": "admin_b", "password": "admin-pass-1", "full_name": "Админ Б", "role": "admin" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{u}");
    let all: Vec<&str> = avtodom_server::auth::PERMISSIONS
        .iter()
        .map(|p| p.key)
        .collect();
    let (s, _, v) = t
        .call_in(
            "PUT",
            &format!("/api/v1/users/{}/permissions", u["id"].as_str().unwrap()),
            &t.owner,
            Some(b),
            Some(json!({ "op_id": Uuid::now_v7(), "permissions": all })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let admin_b = t.login("admin_b", "admin-pass-1").await;
    t.import(json!([{ "name": "Масло A", "barcode": "2760001", "extra_code": "1489" }]))
        .await;
    let (s, _, _) = t
        .call_in(
            "PUT",
            "/api/v1/old-catalog",
            &t.owner,
            Some(b),
            Some(json!([{ "name": "Масло Б", "barcode": "2760500" }])),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let cat = t.category("Масла", "other").await;
    let p = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Масло общее", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let row_a = t.row("2760001").await;
    // Филиал Б списка А не видит и не связывает.
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=2760001", &admin_b)
        .await;
    assert_eq!(hits, json!([]));
    assert_eq!(t.link(&row_a, &p, &admin_b).await.0, StatusCode::NOT_FOUND);
    let (s, _) = t
        .call(
            "POST",
            "/api/v1/products",
            &admin_b,
            Some(
                json!({ "op_id": Uuid::now_v7(), "category_id": cat, "name": "Масло из А",
                         "sale_price_tyiyn": 1_000, "old_catalog_id": row_a }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Связали в А — коды товара общие: скан в Б находит товар.
    let (s, _, _) = t
        .call_in(
            "POST",
            &format!("/api/v1/old-catalog/{row_a}/link"),
            &t.owner,
            Some(a),
            Some(json!({ "op_id": Uuid::now_v7(), "product_id": p })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(t.by_code("1489", &admin_b).await.1["id"], json!(p));
    // Блок «Умаг» карточки — по списку своего филиала.
    let card = t.get(&format!("/api/v1/products/{p}"), &admin_b).await;
    assert_eq!(card["umag"], Value::Null);
    let card = t.get(&format!("/api/v1/products/{p}"), &t.owner).await;
    assert_eq!(card["umag"]["id"], json!(row_a));
    let stats = t.get("/api/v1/old-catalog", &admin_b).await;
    assert_eq!(
        (stats["rows"].clone(), stats["linked"].clone()),
        (json!(1), json!(0))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bound_code_and_merge_keep_umag_link(pool: PgPool) {
    let t = setup(pool).await;
    t.import(json!([
        { "name": "Антифриз 5 л", "barcode": "2760001", "extra_code": "1489;4600000000028" },
        { "name": "Антифриз 1 л", "barcode": "2760002", "extra_code": "1500" },
        { "name": "Антифриз 10 л", "barcode": "2760003" }
    ]))
    .await;
    let cat = t.category("Антифризы", "other").await;
    let dup = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Антифриз (дубль)", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    let main = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Антифриз", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    // «Это он — привязать код» (SPEC-20): строка Умага с этим кодом — тот же товар.
    t.ok(
        "POST",
        &format!("/api/v1/products/{main}/barcodes"),
        &t.admin,
        json!({ "code": "2760002" }),
    )
    .await;
    assert!(t.codes(&main).await.contains(&"1500".to_string()));
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1500", &t.admin)
        .await;
    assert_eq!(hits[0]["product_id"], json!(main));
    assert_eq!(t.journal("old_catalog.link").await[0].1["how"], "code");
    // Дубль влили в основную карточку — строка Умага дубля связана с основной.
    let row = t.row("2760001").await;
    assert_eq!(t.link(&row, &dup, &t.owner).await.0, StatusCode::OK);
    t.ok(
        "POST",
        &format!("/api/v1/products/{dup}/merge"),
        &t.owner,
        json!({ "op_id": Uuid::now_v7(), "into_product_id": main }),
    )
    .await;
    let hits = t
        .get("/api/v1/old-catalog/search?barcode=1489", &t.admin)
        .await;
    assert_eq!(hits[0]["product_id"], json!(main));
    assert_eq!(
        t.by_code("4600000000028", &t.admin).await.1["id"],
        json!(main)
    );
    // Влитый дубль — это основная карточка: связь с ним ложится на неё.
    let (s, v) = t.link(&t.row("2760002").await, &dup, &t.owner).await;
    assert_eq!(
        (s, v["product"]["id"].clone()),
        (StatusCode::OK, json!(main))
    );
    // С карточкой в архиве не связывают.
    let old = t
        .product(
            &t.owner,
            json!({ "category_id": cat, "name": "Антифриз старый", "sale_price_tyiyn": 1_000 }),
        )
        .await;
    t.ok(
        "PATCH",
        &format!("/api/v1/products/{old}"),
        &t.owner,
        json!({ "archived": true }),
    )
    .await;
    let (s, _) = t.link(&t.row("2760003").await, &old, &t.owner).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shared_article_is_a_code_but_does_not_tie_rows(pool: PgPool) {
    // Артикул, похожий на код, переходит товару, но строку не «заводит» и не связывает:
    // у разных строк Умага он бывает одинаковым (свеча BKR6E разных поставщиков).
    let t = setup(pool).await;
    t.import(json!([
        { "name": "Свеча NGK BKR6E", "barcode": "4800000000202", "article": "BKR6E" },
        { "name": "Свеча NGK BKR6E (другой поставщик)", "barcode": "4800000000301", "article": "BKR6E" },
        { "name": "Свеча BKR6E без штрихкода у нас", "barcode": "4800000000400", "article": "BKR6E" }
    ]))
    .await;
    let cat = t.category("Свечи", "other").await;
    let first = t.row("4800000000202").await;
    let p1 = t
        .product(
            &t.admin,
            json!({ "category_id": cat, "name": "Свеча NGK", "barcodes": ["4800000000202"],
                    "sale_price_tyiyn": 30_000, "old_catalog_id": first }),
        )
        .await;
    assert!(t.codes(&p1).await.contains(&"BKR6E".to_string()));
    // Вторая строка с тем же артикулом — ещё не заведена: блок Умага её покажет.
    let second = t.hit("4800000000301").await;
    assert_eq!(second["linked_product_id"], Value::Null);
    // Товар по её штрихкоду связывается с ней; артикул — конфликт с первой свечой.
    let p2 = t
        .product(
            &t.admin,
            json!({ "category_id": cat, "name": "Свеча NGK (Б)", "barcodes": ["4800000000301"],
                    "sale_price_tyiyn": 30_000 }),
        )
        .await;
    let second = t.hit("4800000000301").await;
    assert_eq!(second["product_id"], json!(p2));
    assert_eq!(second["conflicts"][0]["code"], "BKR6E");
    assert_eq!(second["conflicts"][0]["product_id"], json!(p1));
    // Автосвязывание по одному артикулу не связывает.
    let auto = t
        .ok(
            "POST",
            "/api/v1/old-catalog/auto-link",
            &t.owner,
            json!({ "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(auto["by_code"], 0);
    assert_eq!(t.hit("4800000000400").await["product_id"], Value::Null);
    // Скан артикула находит первую свечу: код у товара.
    assert_eq!(t.by_code("BKR6E", &t.admin).await.1["id"], json!(p1));
}
