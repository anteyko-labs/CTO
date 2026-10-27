//! Книжка и чек книжки (SPEC-36 §2–3, ADR-116, ADR-117): чек книжки — только жидкости этого чека
//! с товаром; фильтры с заменой масла — на пробег масла; доливка — не замена; строка книжки
//! категории и услуги по названию.
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

async fn setup(pool: PgPool) -> Router {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    app(build_state(pool, false).await.unwrap(), None)
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::COOKIE, cookie);
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn ok(app: &Router, method: &str, uri: &str, cookie: &str, body: Value) -> Value {
    let (s, v) = call(app, method, uri, cookie, Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {v}");
    v
}

async fn login(app: &Router) -> String {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header("x-device-id", Uuid::now_v7().to_string())
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "login": "owner", "password": "owner-pass-1" }).to_string(),
        ))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .unwrap()
        .to_string()
}

/// Точка: категории, товары, услуги, кассир, мастер, клиент с машиной.
struct Point {
    app: Router,
    owner: String,
    cashier: Value,
    master: Value,
    party: Value,
    vehicle: Value,
    /// Товары по короткому имени: id и цена.
    goods: std::collections::HashMap<&'static str, (Value, i64)>,
    services: std::collections::HashMap<&'static str, Value>,
}

impl Point {
    async fn new(pool: PgPool) -> Self {
        let app = setup(pool).await;
        let owner = login(&app).await;
        let cat = |name: &str, kind: &str, book: Option<&str>| {
            let mut body = json!({ "name": name, "kind": kind });
            if let Some(slot) = book {
                body["book"] = json!({ "slot": slot });
            }
            body
        };
        let motor = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            cat("Моторные масла", "oil", None),
        )
        .await;
        let gear = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            cat("Трансмиссионные масла", "oil", None),
        )
        .await;
        assert_eq!(gear["book_slot"], "gear_oil");
        // Строка «Антифриз» — по названию категории (ADR-117).
        let anti = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            cat("Антифризы", "other", None),
        )
        .await;
        let filters = ok(
            &app,
            "POST",
            "/api/v1/categories",
            &owner,
            cat("Фильтры", "filter", None),
        )
        .await;
        let mut goods = std::collections::HashMap::new();
        for (key, category, name, extra, price) in [
            (
                "oil",
                &motor,
                "Масло 5W-30 4л",
                json!({ "container_ml": 4000 }),
                200_000i64,
            ),
            (
                "atf",
                &gear,
                "ATF Dexron III",
                json!({ "container_ml": 4000 }),
                160_000,
            ),
            ("anti", &anti, "Sintec G12 красный 5 кг", json!({}), 90_000),
            (
                "oil_f",
                &filters,
                "Фильтр W712",
                json!({ "attrs": { "filter_type": "масляный" } }),
                50_000,
            ),
            (
                "air_f",
                &filters,
                "Фильтр C-110",
                json!({ "attrs": { "filter_type": "воздушный" } }),
                60_000,
            ),
            (
                "cab_f",
                &filters,
                "Фильтр CU-2",
                json!({ "attrs": { "filter_type": "салонный" } }),
                70_000,
            ),
            (
                "fuel_f",
                &filters,
                "Фильтр WK-842",
                json!({ "attrs": { "filter_type": "топливный" } }),
                80_000,
            ),
        ] {
            let mut body = extra;
            body["op_id"] = json!(Uuid::now_v7());
            body["category_id"] = category["id"].clone();
            body["name"] = json!(name);
            body["sale_price_tyiyn"] = json!(price);
            let p = ok(&app, "POST", "/api/v1/products", &owner, body).await;
            goods.insert(key, (p["id"].clone(), price));
        }
        let mut services = std::collections::HashMap::new();
        for (key, name) in [
            ("oil", "Замена масла в моторе"),
            ("gear", "Замена масла в коробке"),
            ("topup", "Доливка КПП снизу"),
            ("bridge", "Замена масла мех. моста"),
        ] {
            let s = ok(
                &app,
                "POST",
                "/api/v1/services",
                &owner,
                json!({ "name": name, "price_tyiyn": 20_000, "master_fee_tyiyn": 3_000 }),
            )
            .await;
            services.insert(key, s);
        }
        let cashier = ok(
            &app,
            "POST",
            "/api/v1/employees",
            &owner,
            json!({ "full_name": "Кассир Книжки", "is_cashier": true, "pin": "1234" }),
        )
        .await;
        let master = ok(
            &app,
            "POST",
            "/api/v1/employees",
            &owner,
            json!({ "full_name": "Мастер Книжки", "is_master": true }),
        )
        .await;
        let party = ok(
            &app,
            "POST",
            "/api/v1/parties",
            &owner,
            json!({ "kind": "person", "first_name": "Азиз", "last_name": "Книжкин", "phone": "0700112233" }),
        )
        .await;
        let vehicle = ok(
            &app,
            "POST",
            &format!("/api/v1/parties/{}/vehicles", party["id"].as_str().unwrap()),
            &owner,
            json!({ "plate": "01KG258ABC", "brand": "Toyota", "model": "Camry" }),
        )
        .await;
        Point {
            app,
            owner,
            cashier,
            master,
            party,
            vehicle,
            goods,
            services,
        }
    }

    /// Чек «В сервис»: товары (имя, вид строки, количество), услуги и следующие замены.
    async fn sell(
        &self,
        mileage: i64,
        goods: &[(&str, &str, i64)],
        services: &[&str],
        next: Value,
    ) -> Value {
        let mut lines = Vec::new();
        let mut total = 0i64;
        for (key, kind, qty) in goods {
            let (id, price) = &self.goods[key];
            // Розлив — цена за литр от канистры 4 л (ADR-074), количество в мл.
            let (unit, amount) = if *kind == "pour" {
                (price / 4, price / 4 * qty / 1000)
            } else {
                (*price, price * qty)
            };
            lines.push(json!({ "kind": kind, "gift": false, "product_id": id, "qty": qty, "unit_price_tyiyn": unit }));
            total += amount;
        }
        for key in services {
            let s = &self.services[key];
            lines.push(json!({ "kind": "service", "gift": false, "service_id": s["id"], "qty": 1, "unit_price_tyiyn": s["price_tyiyn"] }));
            total += s["price_tyiyn"].as_i64().unwrap();
        }
        ok(
            &self.app,
            "POST",
            "/api/v1/sales",
            &self.owner,
            json!({
                "op_id": Uuid::now_v7(),
                "client_time": chrono::Utc::now(),
                "sale_type": "service",
                "cashier_id": self.cashier["id"],
                "master_id": self.master["id"],
                "party_id": self.party["id"],
                "vehicle_id": self.vehicle["id"],
                "mileage": mileage,
                "mileage_unit": "km",
                "lines": lines,
                "next_changes": next,
                "payments": [{ "method": "cash", "amount_tyiyn": total }],
            }),
        )
        .await
    }

    async fn book(&self) -> Value {
        let (s, b) = call(
            &self.app,
            "GET",
            &format!(
                "/api/v1/vehicles/{}/oil-book",
                self.vehicle["id"].as_str().unwrap()
            ),
            &self.owner,
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        b
    }

    /// Запись книжки этого чека: строка книжки → (цель задана, пробег цели, дата цели).
    async fn record(&self, sale: &Value) -> Option<Vec<(String, bool, Value, Value)>> {
        let b = self.book().await;
        let rec = b["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["sale_id"] == sale["id"])?
            .clone();
        Some(
            rec["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| {
                    (
                        l["slot"].as_str().unwrap().to_string(),
                        l["target_set"].as_bool().unwrap(),
                        l["next_mileage"].clone(),
                        l["next_date"].clone(),
                    )
                })
                .collect(),
        )
    }
}

fn slot<'a>(
    lines: &'a [(String, bool, Value, Value)],
    s: &str,
) -> &'a (String, bool, Value, Value) {
    lines
        .iter()
        .find(|l| l.0 == s)
        .unwrap_or_else(|| panic!("нет строки {s}: {lines:?}"))
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_check_has_only_fluids_and_filters_follow_oil(pool: PgPool) {
    // П. 5, 14, 19: чек с маслом, антифризом и четырьмя фильтрами. Масло — 258 000 по услуге,
    // антифриз — «+ ещё» по товару чека 280 000.
    let p = Point::new(pool).await;
    let sale = p
        .sell(
            250_000,
            &[
                ("oil", "container", 1),
                ("anti", "piece", 2),
                ("oil_f", "piece", 1),
                ("air_f", "piece", 1),
                ("cab_f", "piece", 1),
                ("fuel_f", "piece", 1),
            ],
            &["oil"],
            json!([
                { "service_id": p.services["oil"]["id"], "next_mileage": 258_000 },
                { "slot": "antifreeze", "next_mileage": 280_000 },
                // Касса прежней версии прислала свою цель воздушному: с маслом в чеке её заменяет цель масла.
                { "slot": "air_filter", "next_mileage": 265_000 },
            ]),
        )
        .await;
    // Чек книжки: «Залито» — масло и антифриз с товаром и количеством; следующая — по ним.
    let book = &sale["book"];
    assert_eq!(
        book["filled"],
        json!([
            { "slot": "oil", "label": "Масло двигателя", "product": "Масло 5W-30 4л · 1 кан. × 4 л" },
            { "slot": "antifreeze", "label": "Антифриз", "product": "Sintec G12 красный 5 кг · 2 шт" },
        ])
    );
    let next: Vec<(Value, Value, Value)> = book["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["slot"].clone(),
                l["label"].clone(),
                l["next_mileage"].clone(),
            )
        })
        .collect();
    assert_eq!(
        next,
        vec![
            (json!("oil"), json!("Масло двигателя"), json!(258_000)),
            (json!("antifreeze"), json!("Антифриз"), json!(280_000)),
        ]
    );
    // Книжка: масляный, воздушный, салонный — на пробег и дату масла; топливный — по своему сроку.
    let rec = p.record(&sale).await.unwrap();
    let oil = slot(&rec, "oil").clone();
    assert_eq!((oil.1, &oil.2), (true, &json!(258_000)));
    for f in ["oil_filter", "air_filter", "cabin_filter"] {
        let l = slot(&rec, f);
        assert_eq!((l.1, &l.2, &l.3), (true, &oil.2, &oil.3), "{f}");
    }
    let fuel = slot(&rec, "fuel_filter");
    assert_eq!((fuel.1, &fuel.2), (false, &Value::Null));
    let b = p.book().await;
    let fuel_next = b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "fuel_filter")
        .unwrap();
    assert_eq!(fuel_next["next_km"], json!(280_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_without_reminder_leaves_filters_without_it(pool: PgPool) {
    // У масла «не напоминать» — у фильтров с ним напоминаний тоже нет; масло клиента в чеке книжки.
    let p = Point::new(pool).await;
    let sale = p
        .sell(
            250_000,
            &[("air_f", "piece", 1)],
            &["oil"],
            json!([{ "service_id": p.services["oil"]["id"], "next_mileage": null }]),
        )
        .await;
    let rec = p.record(&sale).await.unwrap();
    let air = slot(&rec, "air_filter");
    assert_eq!((air.1, &air.2), (true, &Value::Null));
    assert_eq!(
        sale["book"]["filled"],
        json!([{ "slot": "oil", "label": "Масло двигателя", "product": "масло клиента" }])
    );
    assert_eq!(sale["book"]["lines"], json!([]));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn top_up_is_not_a_change(pool: PgPool) {
    // П. 15: «Доливка КПП снизу» и 1 л ATF — записи «Масло в коробке» нет, срок не сдвинулся.
    let p = Point::new(pool).await;
    assert_eq!(p.services["topup"]["book_slot"], "other");
    let first = p
        .sell(
            200_000,
            &[("atf", "container", 1)],
            &["gear"],
            json!([{ "service_id": p.services["gear"]["id"], "next_mileage": 240_000 }]),
        )
        .await;
    assert!(p.record(&first).await.is_some());
    let topup = p
        .sell(210_000, &[("atf", "pour", 1000)], &["topup"], json!([]))
        .await;
    assert!(p.record(&topup).await.is_none(), "доливка записана заменой");
    // Канистра моторного масла без услуги масла и без цели — тоже не замена.
    let no_service = p
        .sell(211_000, &[("oil", "container", 1)], &["topup"], json!([]))
        .await;
    assert!(p.record(&no_service).await.is_none());
    let b = p.book().await;
    let gear = b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "gear_oil")
        .unwrap();
    assert_eq!(gear["next_km"], json!(240_000));
    // Цель кассира через «+ ещё» по товару чека — замена.
    let by_target = p
        .sell(
            212_000,
            &[("atf", "pour", 1000)],
            &["topup"],
            json!([{ "slot": "gear_oil", "next_mileage": 250_000 }]),
        )
        .await;
    let rec = p.record(&by_target).await.unwrap();
    assert_eq!(slot(&rec, "gear_oil").2, json!(250_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_slot_by_name_for_categories_and_services(pool: PgPool) {
    // П. 2, 11: «Антифризы» — антифриз, «Тормозные жидкости» — тормозная, «мех. моста» — прочее;
    // переименованная категория без строки получает её по названию, выбранную — не меняет.
    let p = Point::new(pool).await;
    assert_eq!(p.services["bridge"]["book_slot"], "other");
    let brake = ok(
        &p.app,
        "POST",
        "/api/v1/categories",
        &p.owner,
        json!({ "name": "Тормозные жидкости", "kind": "other" }),
    )
    .await;
    assert_eq!(brake["book_slot"], "brake_fluid");
    let fluids = ok(
        &p.app,
        "POST",
        "/api/v1/categories",
        &p.owner,
        json!({ "name": "Антифриз и жидкости 2", "kind": "other", "book": { "slot": "none" } }),
    )
    .await;
    assert_eq!(fluids["book_slot"], "none");
    let other = ok(
        &p.app,
        "POST",
        "/api/v1/categories",
        &p.owner,
        json!({ "name": "Жидкости", "kind": "other" }),
    )
    .await;
    assert_eq!(other["book_slot"], "none");
    let uri = |c: &Value| format!("/api/v1/categories/{}", c["id"].as_str().unwrap());
    let renamed = ok(
        &p.app,
        "PATCH",
        &uri(&other),
        &p.owner,
        json!({ "name": "Охлаждающие жидкости" }),
    )
    .await;
    assert_eq!(renamed["book_slot"], "antifreeze");
    // Строку выбрал владелец («не в книжку») — название её не меняет.
    let kept = ok(
        &p.app,
        "PATCH",
        &uri(&fluids),
        &p.owner,
        json!({ "name": "Антифризы старые" }),
    )
    .await;
    assert_eq!(kept["book_slot"], "none");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn axle_service_is_free_with_gear_oil(pool: PgPool) {
    // ADR-131: новая услуга моста, редуктора или раздатки — «прочее», «бесплатно с нашим маслом» —
    // от трансмиссионных масел, а не моторных; замена масла в моторе — от моторных.
    let p = Point::new(pool).await;
    let (status, cats) = call(&p.app, "GET", "/api/v1/categories", &p.owner, None).await;
    assert_eq!(status, StatusCode::OK);
    let cat_id = |name: &str| {
        cats.as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["id"]
            .clone()
    };
    for (name, slot, category) in [
        ("Замена масла в редукторе", "other", "Трансмиссионные масла"),
        ("Масло заднего моста", "other", "Трансмиссионные масла"),
        ("Замена масла в раздатке", "other", "Трансмиссионные масла"),
        ("Замена масла двигателя", "oil", "Моторные масла"),
        ("Замена масла самостоятельно", "oil", "Моторные масла"),
    ] {
        let s = ok(
            &p.app,
            "POST",
            "/api/v1/services",
            &p.owner,
            json!({ "name": name, "price_tyiyn": 20_000, "free_with_oil": true }),
        )
        .await;
        assert_eq!(s["book_slot"], slot, "{name}");
        assert_eq!(s["oil_category_id"], cat_id(category), "{name}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn category_saved_unchanged_keeps_its_book_slot(pool: PgPool) {
    // ADR-131: категория без строки книжки, заведённая до ADR-117, при сохранении с прежним
    // названием строку по названию не получает («Антифриз и жидкости» не становится целиком
    // «Антифризом»); переименованная — получает.
    let app = setup(pool.clone()).await;
    let owner = login(&app).await;
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into categories (id, name, kind) values ($1, 'Антифриз и жидкости', 'other')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    let uri = format!("/api/v1/categories/{id}");
    // Форма «Категория» присылает название всегда; пробелы по краям — то же название.
    for name in ["Антифриз и жидкости", "  Антифриз и жидкости "]
    {
        let saved = ok(
            &app,
            "PATCH",
            &uri,
            &owner,
            json!({ "name": name, "attributes": [] }),
        )
        .await;
        assert_eq!(saved["book_slot"], "none", "{name}");
        assert_eq!(stored_book_slot(&pool, id).await, None, "{name}");
    }
    let renamed = ok(&app, "PATCH", &uri, &owner, json!({ "name": "Антифризы" })).await;
    assert_eq!(renamed["book_slot"], "antifreeze");
    assert_eq!(
        stored_book_slot(&pool, id).await.as_deref(),
        Some("antifreeze")
    );
}

/// Строка книжки категории, как она записана (пусто — не выбрана).
async fn stored_book_slot(pool: &PgPool, id: Uuid) -> Option<String> {
    sqlx::query_scalar("select book_slot from categories where id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Миграция 0071: меняет только данные справочника услуг, поэтому её можно выполнить ещё раз.
async fn run_axle_migration(pool: &PgPool) {
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version == 71) {
        sqlx::raw_sql(m.sql.as_ref()).execute(pool).await.unwrap();
    }
}

async fn add_oil_category(pool: &PgPool, name: &str, slot: Option<&str>, days_ago: i32) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into categories (id, name, kind, book_slot, created_at)
         values ($1, $2, 'oil', $3, now() - make_interval(days => $4))",
    )
    .bind(id)
    .bind(name)
    .bind(slot)
    .bind(days_ago)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn add_service(
    pool: &PgPool,
    name: &str,
    slot: &str,
    free_with_oil: bool,
    category: Option<Uuid>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into services (id, branch_id, name, price_tyiyn, free_with_oil, oil_category_id, book_slot)
         select $1, id, $2, 20000, $3, $4, $5 from branches",
    )
    .bind(id)
    .bind(name)
    .bind(free_with_oil)
    .bind(category)
    .bind(slot)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Услуги: название, строка книжки, категория масла — по названию.
async fn services_now(pool: &PgPool) -> Vec<(String, String, Option<Uuid>)> {
    let mut rows: Vec<(String, String, Option<Uuid>)> =
        sqlx::query_as("select name, book_slot, oil_category_id from services")
            .fetch_all(pool)
            .await
            .unwrap();
    rows.sort();
    rows
}

#[sqlx::test(migrations = false)]
async fn migration_moves_axle_services_to_other(pool: PgPool) {
    // Миграция 0071 (ADR-131): услуги с «мост» (с начала слова), «редукт», «раздатк» на строке
    // масла двигателя или коробки — в «прочее»; «бесплатно с нашим маслом» — от категории
    // трансмиссионных масел (строка «Масло в коробке» или «трансмис», первая по заведению).
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 71) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let motor = add_oil_category(&pool, "Моторные масла", None, 30).await;
    // Трансмиссионной категории ещё нет — услуга уходит в «прочее», категория прежняя.
    let lone = add_service(&pool, "Замена масла мех. моста", "oil", true, Some(motor)).await;
    run_axle_migration(&pool).await;
    let row: (String, Option<Uuid>) =
        sqlx::query_as("select book_slot, oil_category_id from services where id = $1")
            .bind(lone)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row, ("other".to_string(), Some(motor)));

    // Трансмиссионные категории: со строкой «Масло в коробке» (как «Трансмиссионные масла» в
    // рабочей базе) и с «трансмис» в названии без строки; берётся первая по заведению.
    let atf = add_oil_category(&pool, "Масло ATF", Some("gear_oil"), 20).await;
    let gear = add_oil_category(&pool, "Трансмиссионные масла", None, 10).await;
    for (name, slot, free, category) in [
        ("ЗАМЕНА МАСЛА В МОСТУ", "oil", true, Some(motor)),
        ("Масло в редуктор", "gear_oil", true, Some(gear)),
        ("Замена масла в раздатке", "oil", false, Some(motor)),
        ("Мост: замена масла", "oil", true, None),
        ("Замена масла самостоятельно", "oil", true, Some(motor)),
        ("Замена масла в моторе", "oil", true, Some(motor)),
        ("Замена масла в коробке", "gear_oil", true, Some(gear)),
        ("Смазка моста", "other", true, Some(motor)),
        ("Тормозная жидкость моста", "brake_fluid", false, None),
    ] {
        add_service(&pool, name, slot, free, category).await;
    }
    run_axle_migration(&pool).await;
    let s = |name: &str, slot: &str, category: Option<Uuid>| {
        (name.to_string(), slot.to_string(), category)
    };
    let mut want = vec![
        // Моторная категория — на трансмиссионную; без категории — тоже.
        s("ЗАМЕНА МАСЛА В МОСТУ", "other", Some(atf)),
        s("Мост: замена масла", "other", Some(atf)),
        // Уже трансмиссионная — прежняя; без «бесплатно с нашим маслом» категория не меняется.
        s("Масло в редуктор", "other", Some(gear)),
        s("Замена масла в раздатке", "other", Some(motor)),
        // Переведена первым запуском — второй её не трогает.
        s("Замена масла мех. моста", "other", Some(motor)),
        // Остальные услуги не меняются.
        s("Замена масла самостоятельно", "oil", Some(motor)),
        s("Замена масла в моторе", "oil", Some(motor)),
        s("Замена масла в коробке", "gear_oil", Some(gear)),
        s("Смазка моста", "other", Some(motor)),
        s("Тормозная жидкость моста", "brake_fluid", None),
    ];
    want.sort();
    assert_eq!(services_now(&pool).await, want);
    // Повторный запуск ничего не меняет.
    run_axle_migration(&pool).await;
    assert_eq!(services_now(&pool).await, want);
}
