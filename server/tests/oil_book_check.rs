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
        // Онлайн-чек — только при открытой смене (задача 005 Н4).
        ok(
            &app,
            "POST",
            "/api/v1/shifts/open",
            &owner,
            json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier["id"], "cashier_pin": "1234" }),
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

/// Услуга справочника для одной проверки: в `p.services` под ключом `key`.
async fn point_service(p: &mut Point, key: &'static str, body: Value) -> Value {
    let mut body = body;
    body["price_tyiyn"] = json!(20_000);
    body["master_fee_tyiyn"] = json!(3_000);
    let s = ok(&p.app, "POST", "/api/v1/services", &p.owner, body).await;
    p.services.insert(key, s.clone());
    s
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sale_check_has_goods_and_filters_follow_oil(pool: PgPool) {
    // П. 5, 14, 19: чек с маслом, антифризом и четырьмя фильтрами. Масло — 258 000 по услуге,
    // антифриз — 280 000 по услуге «Замена антифриза» (товар без услуги — доливка, ADR-131).
    // Задача 003 Н5: чек книжки по умолчанию — и фильтры; подпись — товар, а не строка книжки;
    // фильтры с маслом — под заменой масла и на её пробег, топливный — по своему сроку.
    let mut p = Point::new(pool).await;
    point_service(&mut p, "anti", json!({ "name": "Замена антифриза" })).await;
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
            &["oil", "anti"],
            json!([
                { "service_id": p.services["oil"]["id"], "next_mileage": 258_000 },
                { "service_id": p.services["anti"]["id"], "next_mileage": 280_000 },
                // Касса прежней версии прислала свою цель воздушному без услуги — её нет (ADR-131),
                // фильтр с маслом в чеке и так на цели масла.
                { "slot": "air_filter", "next_mileage": 265_000 },
            ]),
        )
        .await;
    let book = &sale["book"];
    let oil_service = p.services["oil"]["name"].clone();
    let filled: Vec<(Value, Value, Value)> = book["filled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["slot"].clone(), f["label"].clone(), f["group"].clone()))
        .collect();
    assert_eq!(
        filled,
        vec![
            (
                json!("oil"),
                json!("Масло 5W-30 4л · 1 кан. × 4 л"),
                oil_service.clone()
            ),
            (
                json!("antifreeze"),
                json!("Sintec G12 красный 5 кг · 2 шт"),
                json!("Замена антифриза")
            ),
            (
                json!("oil_filter"),
                json!("Фильтр W712 (масляный) · 1 шт"),
                oil_service.clone()
            ),
            (
                json!("air_filter"),
                json!("Фильтр C-110 (воздушный) · 1 шт"),
                oil_service.clone()
            ),
            (
                json!("cabin_filter"),
                json!("Фильтр CU-2 (салонный) · 1 шт"),
                oil_service.clone()
            ),
            (
                json!("fuel_filter"),
                json!("Фильтр WK-842 (топливный) · 1 шт"),
                Value::Null
            ),
        ]
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
            (
                json!("oil"),
                json!("Масло 5W-30 4л · 1 кан. × 4 л"),
                json!(258_000)
            ),
            (
                json!("antifreeze"),
                json!("Sintec G12 красный 5 кг · 2 шт"),
                json!(280_000)
            ),
            (
                json!("oil_filter"),
                json!("Фильтр W712 (масляный) · 1 шт"),
                json!(258_000)
            ),
            (
                json!("air_filter"),
                json!("Фильтр C-110 (воздушный) · 1 шт"),
                json!(258_000)
            ),
            (
                json!("cabin_filter"),
                json!("Фильтр CU-2 (салонный) · 1 шт"),
                json!(258_000)
            ),
            // Топливный — свой срок: 250 000 + 30 000, а не пробег масла (ADR-117).
            (
                json!("fuel_filter"),
                json!("Фильтр WK-842 (топливный) · 1 шт"),
                json!(280_000)
            ),
        ]
    );
    // «Только масла и жидкости» — как было (ADR-116).
    book_settings(&p, json!({ "with_filters": false })).await;
    let (status, again) = call(
        &p.app,
        "GET",
        &format!("/api/v1/sales/{}", sale["id"].as_str().unwrap()),
        &p.owner,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let slots: Vec<Value> = again["book"]["filled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["slot"].clone())
        .collect();
    assert_eq!(slots, vec![json!("oil"), json!("antifreeze")]);
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
    // Масло клиента — «масло клиента (двигатель)»; фильтр с ним — под той же заменой, без срока.
    let filled: Vec<(Value, Value, Value)> = sale["book"]["filled"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["slot"].clone(), f["label"].clone(), f["product"].clone()))
        .collect();
    assert_eq!(
        filled,
        vec![
            (
                json!("oil"),
                json!("масло клиента (двигатель)"),
                json!("масло клиента")
            ),
            (
                json!("air_filter"),
                json!("Фильтр C-110 (воздушный) · 1 шт"),
                json!("Фильтр C-110 · 1 шт")
            ),
        ]
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
    // Канистра моторного масла с «Доливкой КПП» — доливка коробки, а масло двигателя — замена
    // (ADR-136).
    let no_service = p
        .sell(211_000, &[("oil", "container", 1)], &["topup"], json!([]))
        .await;
    let lines = p
        .record(&no_service)
        .await
        .expect("масло двигателя — замена");
    assert!(lines.iter().all(|l| l.0 != "gear_oil"), "{lines:?}");
    // Цель кассир не прислал — сервер поставил пробег + интервал масла (ADR-144).
    assert_eq!(
        (slot(&lines, "oil").1, &slot(&lines, "oil").2),
        (true, &json!(219_000))
    );
    let b = p.book().await;
    let gear = b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "gear_oil")
        .unwrap();
    assert_eq!(gear["next_km"], json!(240_000));
    // Цель по товару без услуги (касса до 27.10.2026) и цель по доливке на вынос — не замена:
    // цель только по услуге этого чека (ADR-131).
    assert_eq!(p.services["topup"]["takeaway"], json!(true));
    for next in [
        json!([{ "slot": "gear_oil", "next_mileage": 250_000 }]),
        json!([{ "service_id": p.services["topup"]["id"], "slot": "gear_oil", "next_mileage": 250_000 }]),
    ] {
        let by_target = p
            .sell(212_000, &[("atf", "pour", 1000)], &["topup"], next)
            .await;
        assert!(
            p.record(&by_target).await.is_none(),
            "доливка записана заменой"
        );
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn sold_oil_is_change(pool: PgPool) {
    // ADR-136: проданное масло двигателя или коробки в чеке «В сервис» — замена и без услуги своей
    // строки, сколько бы литров ни было; доливку решает только услуга-доливка.
    let mut p = Point::new(pool).await;
    // Чек № 17: ATF и «Замена со своим маслом» (масло двигателя) — строка коробки по интервалу.
    let own = point_service(&mut p, "own", json!({ "name": "Замена со своим маслом" })).await;
    assert_eq!(own["book_slot"], "oil");
    let sale = p
        .sell(130_000, &[("atf", "container", 1)], &["own"], json!([]))
        .await;
    let lines = p.record(&sale).await.expect("запись книжки");
    // Цель кассир не прислал — сервер записал пробег + интервал коробки (ADR-144); то же число
    // в блоке книжки печатного чека.
    assert_eq!(slot(&lines, "gear_oil").2, json!(170_000), "{lines:?}");
    assert!(lines.iter().any(|l| l.0 == "oil"), "{lines:?}");
    assert!(
        sale["book"]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["slot"] == "gear_oil" && l["next_mileage"] == json!(170_000)),
        "{}",
        sale["book"]
    );
    let b = p.book().await;
    let gear = b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == "gear_oil")
        .cloned()
        .unwrap();
    assert_eq!(gear["next_km"], json!(170_000), "{gear}");
    // Чек «В сервис» — с видом работ: мойка (строка «прочее»).
    point_service(&mut p, "wash", json!({ "name": "Мойка двигателя" })).await;
    // Моторное масло без услуги замены с целью кассира по строке — цель масла и у фильтров (ADR-117).
    let sale = p
        .sell(
            140_000,
            &[("oil", "container", 1), ("air_f", "piece", 1)],
            &["wash"],
            json!([{ "slot": "oil", "next_mileage": 147_000 }]),
        )
        .await;
    let lines = p.record(&sale).await.expect("запись книжки");
    assert_eq!(slot(&lines, "oil").2, json!(147_000));
    assert_eq!(slot(&lines, "air_filter").2, json!(147_000));
    // Розлив 0,5 л ATF без доливки — тоже замена: объём не важен.
    let sale = p
        .sell(141_000, &[("atf", "pour", 500)], &["wash"], json!([]))
        .await;
    assert!(slot(&p.record(&sale).await.expect("запись"), "gear_oil").0 == "gear_oil");
    // Цель по строке без такого масла в чеке и по антифризу без услуги — не пишется.
    let sale = p
        .sell(
            142_000,
            &[("oil", "container", 1), ("anti", "piece", 1)],
            &["wash"],
            json!([
                { "slot": "gear_oil", "next_mileage": 182_000 },
                { "slot": "antifreeze", "next_mileage": 200_000 },
            ]),
        )
        .await;
    let lines = p.record(&sale).await.expect("запись книжки");
    assert!(
        lines.iter().all(|l| l.0 == "oil"),
        "лишние строки: {lines:?}"
    );
    // ATF при «Замене ГУР» и при «Замене масла мех. моста» — не замена коробки (ADR-131).
    point_service(&mut p, "gur", json!({ "name": "Замена ГУР" })).await;
    for svc in ["gur", "bridge"] {
        let sale = p
            .sell(143_000, &[("atf", "container", 1)], &[svc], json!([]))
            .await;
        let lines = p.record(&sale).await.unwrap_or_default();
        assert!(lines.iter().all(|l| l.0 != "gear_oil"), "{svc}: {lines:?}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn correction_keeps_next_changes(pool: PgPool) {
    // ADR-131: «Исправить» переносит в черновик следующие замены, заданные кассиром: по строке
    // книжки (и очищенную — «не напоминать») и строки «+ ещё» по услугам «прочее»; исправленный
    // чек пишет их в книжку снова.
    let mut p = Point::new(pool).await;
    let gur = point_service(&mut p, "gur", json!({ "name": "Замена ГУР" })).await;
    assert_eq!(gur["book_slot"], "other");
    let sale = p
        .sell(
            250_000,
            &[("oil", "container", 1), ("air_f", "piece", 1)],
            &["oil", "gear", "gur"],
            json!([
                { "service_id": p.services["oil"]["id"], "slot": "oil", "next_mileage": 258_000 },
                { "service_id": p.services["gear"]["id"], "slot": "gear_oil", "next_mileage": null },
                { "service_id": gur["id"], "slot": "other", "next_mileage": 290_000 },
            ]),
        )
        .await;
    // Исправление чека за наличные возвращает их в кассу — смена открыта с начала (Point::new).
    let (s, draft) = call(
        &p.app,
        "GET",
        &format!(
            "/api/v1/corrections/draft/sale/{}",
            sale["id"].as_str().unwrap()
        ),
        &p.owner,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{draft}");
    // Фильтр с маслом своей цели не имеет — в черновике его нет.
    assert_eq!(
        draft["doc"]["next_changes"],
        json!([
            { "service_id": p.services["oil"]["id"], "slot": "oil", "next_mileage": 258_000 },
            { "service_id": p.services["gear"]["id"], "slot": "gear_oil", "next_mileage": null },
            { "service_id": gur["id"], "slot": "other", "next_mileage": 290_000 },
        ])
    );
    let mut doc = draft["doc"].clone();
    doc["comment"] = json!("исправлен мастер");
    let fixed = ok(
        &p.app,
        "POST",
        "/api/v1/corrections",
        &p.owner,
        json!({ "op_id": Uuid::now_v7(), "kind": "sale", "original_id": sale["id"], "comment": "проверка", "doc": doc }),
    )
    .await;
    let rec = p.record(&json!({ "id": fixed["new_id"] })).await.unwrap();
    assert_eq!(slot(&rec, "oil").2, json!(258_000));
    let gear = slot(&rec, "gear_oil");
    assert_eq!((gear.1, &gear.2), (true, &Value::Null));
    assert_eq!(slot(&rec, "other").2, json!(290_000));
    assert_eq!(slot(&rec, "air_filter").2, json!(258_000));
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

/// «Масляная книжка и печать» филиала (задача 003 Н5): поля поверх значений по умолчанию.
async fn book_settings(p: &Point, patch: Value) -> Value {
    let mut body = ok(
        &p.app,
        "GET",
        "/api/v1/settings/oil-book",
        &p.owner,
        json!({}),
    )
    .await;
    for (k, v) in patch.as_object().unwrap() {
        body[k] = v.clone();
    }
    body["op_id"] = json!(Uuid::now_v7());
    ok(&p.app, "PUT", "/api/v1/settings/oil-book", &p.owner, body).await
}

fn next_of(book: &Value) -> Vec<(String, Value)> {
    book["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["slot"].as_str().unwrap().to_string(),
                l["next_mileage"].clone(),
            )
        })
        .collect()
}

fn slot_next(b: &Value, s: &str) -> Value {
    b["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["slot"] == s)
        .unwrap()["next_km"]
        .clone()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_book_next_without_cashier_target(pool: PgPool) {
    // Задача 003 Н5: кассир цель не прислал — сервер пишет пробег сейчас + интервал строки, и
    // печатный чек, книжка клиента и чек книжки показывают одно число (ADR-144).
    let mut p = Point::new(pool).await;
    point_service(&mut p, "anti", json!({ "name": "Замена антифриза" })).await;
    let sale = p
        .sell(
            130_000,
            &[("oil", "container", 1), ("anti", "piece", 1)],
            &["oil", "anti"],
            json!([]),
        )
        .await;
    assert_eq!(
        next_of(&sale["book"]),
        vec![
            ("oil".to_string(), json!(138_000)),
            ("antifreeze".to_string(), json!(190_000)),
        ]
    );
    assert_eq!(slot_next(&p.book().await, "oil"), json!(138_000));
    // Экран чека открывает книжку машины по `vehicle_id` чека.
    assert_eq!(sale["vehicle_id"], p.vehicle["id"]);
    // «Исправить» серверную цель не переносит: новый чек считает её заново.
    let (s, draft) = call(
        &p.app,
        "GET",
        &format!(
            "/api/v1/corrections/draft/sale/{}",
            sale["id"].as_str().unwrap()
        ),
        &p.owner,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{draft}");
    assert_eq!(draft["doc"]["next_changes"], json!([]));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn filters_follow_oil_without_cashier_target(pool: PgPool) {
    // ADR-117 и без цели кассира: масляный, воздушный, салонный — на пробег масла; топливный — по
    // своему интервалу. Владелец выключил — каждый фильтр по своему интервалу.
    let p = Point::new(pool).await;
    let all = [
        ("oil", "container", 1),
        ("oil_f", "piece", 1),
        ("air_f", "piece", 1),
        ("cab_f", "piece", 1),
        ("fuel_f", "piece", 1),
    ];
    let sale = p.sell(130_000, &all, &["oil"], json!([])).await;
    let rec = p.record(&sale).await.unwrap();
    for f in ["oil", "oil_filter", "air_filter", "cabin_filter"] {
        assert_eq!(slot(&rec, f).2, json!(138_000), "{f}");
    }
    assert!(!slot(&rec, "fuel_filter").1);
    let b = p.book().await;
    assert_eq!(slot_next(&b, "air_filter"), json!(138_000));
    assert_eq!(slot_next(&b, "fuel_filter"), json!(160_000));
    book_settings(&p, json!({ "filters_follow_oil": false })).await;
    let sale = p.sell(150_000, &all, &["oil"], json!([])).await;
    let rec = p.record(&sale).await.unwrap();
    assert_eq!(slot(&rec, "oil").2, json!(158_000));
    assert!(!slot(&rec, "air_filter").1);
    assert_eq!(slot_next(&p.book().await, "air_filter"), json!(165_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn container_and_pour_one_line(pool: PgPool) {
    // Задача 003 Н5: канистра и разлив одного масла — одной строкой книжки «4,5 л»; с настройкой —
    // «4,5 л (1 кан. × 4 л + 0,5 л)».
    let p = Point::new(pool).await;
    let goods = [("oil", "container", 1), ("oil", "pour", 500)];
    let sale = p.sell(130_000, &goods, &["oil"], json!([])).await;
    assert_eq!(
        sale["book"]["filled"],
        json!([{ "slot": "oil", "label": "Масло 5W-30 4л · 4,5 л", "product": "Масло 5W-30 4л · 4,5 л",
                 "service_id": null, "group": p.services["oil"]["name"] }])
    );
    let b = p.book().await;
    let rec = &b["records"][0];
    assert_eq!(rec["lines"].as_array().unwrap().len(), 1, "{rec}");
    assert_eq!(rec["oil_text"], json!("Масло 5W-30 4л · 4,5 л"));
    book_settings(&p, json!({ "qty_style": "split" })).await;
    let sale = p.sell(140_000, &goods, &["oil"], json!([])).await;
    assert_eq!(
        sale["book"]["filled"][0]["product"],
        json!("Масло 5W-30 4л · 4,5 л (1 кан. × 4 л + 0,5 л)")
    );
    // Неизвестный вид — 422.
    let (s, _) = call(
        &p.app,
        "PUT",
        "/api/v1/settings/oil-book",
        &p.owner,
        Some(json!({ "op_id": Uuid::now_v7(), "qty_style": "lines" })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn book_with_filters_and_old_record_by_interval(pool: PgPool) {
    // Настройка «с фильтрами» — фильтры в блоке книжки чека; запись без цели (до 31.10.2026) —
    // срок по интервалу при показе, записанное не меняется (I4).
    let p = Point::new(pool.clone()).await;
    book_settings(&p, json!({ "with_filters": true })).await;
    let sale = p
        .sell(
            130_000,
            &[("oil", "container", 1), ("air_f", "piece", 1)],
            &["oil"],
            json!([]),
        )
        .await;
    assert_eq!(
        next_of(&sale["book"]),
        vec![
            ("oil".to_string(), json!(138_000)),
            ("air_filter".to_string(), json!(138_000)),
        ]
    );
    assert_eq!(
        sale["book"]["filled"][1]["product"],
        json!("Фильтр C-110 · 1 шт")
    );
    // Старая запись: целей нет.
    sqlx::query(
        "update oil_change_items set target_set = false, target_auto = false, next_value = null,
                next_unit = null, next_km = null, next_date = null",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (s, again) = call(
        &p.app,
        "GET",
        &format!("/api/v1/sales/{}", sale["id"].as_str().unwrap()),
        &p.owner,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{again}");
    assert_eq!(
        next_of(&again["book"]),
        vec![
            ("oil".to_string(), json!(138_000)),
            ("air_filter".to_string(), json!(145_000)),
        ]
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn filter_type_switched_off_gives_category_slot(pool: PgPool) {
    // Задача 002 П10: «Тип» выключен у категории — строка книжки фильтра от категории (ADR-072).
    let p = Point::new(pool).await;
    let (_, cats) = call(&p.app, "GET", "/api/v1/categories", &p.owner, None).await;
    let filters = cats
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Фильтры")
        .unwrap()
        .clone();
    assert_eq!(filters["by_type"], json!(true));
    let mut attrs = filters["attributes"].clone();
    for a in attrs.as_array_mut().unwrap() {
        if a["key"] == "filter_type" {
            a["enabled"] = json!(false);
        }
    }
    let saved = ok(
        &p.app,
        "PATCH",
        &format!("/api/v1/categories/{}", filters["id"].as_str().unwrap()),
        &p.owner,
        json!({ "attributes": attrs }),
    )
    .await;
    assert_eq!(saved["by_type"], json!(false));
    let sale = p
        .sell(130_000, &[("air_f", "piece", 1)], &["oil"], json!([]))
        .await;
    let rec = p.record(&sale).await.unwrap();
    assert!(rec.iter().any(|l| l.0 == "oil_filter"), "{rec:?}");
    assert!(rec.iter().all(|l| l.0 != "air_filter"), "{rec:?}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn server_target_follows_interval_changed_later(pool: PgPool) {
    // Цель, которую поставил сервер, — расчёт по интервалу: владелец поменял интервал машины после
    // чека — книжка пересчитывается; фильтр с маслом — на интервал масла, свой интервал фильтра —
    // по нему. Печатный чек остаётся, каким был (I4).
    let p = Point::new(pool.clone()).await;
    let sale = p
        .sell(
            130_000,
            &[("oil", "container", 1), ("air_f", "piece", 1)],
            &["oil"],
            json!([]),
        )
        .await;
    let b = p.book().await;
    assert_eq!(slot_next(&b, "oil"), json!(138_000));
    assert_eq!(slot_next(&b, "air_filter"), json!(138_000));
    for (slot, km) in [("oil", 10_000), ("air_filter", 12_000)] {
        sqlx::query(
            "insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days)
             select id, branch_id, $2, $3, 90 from party_vehicles where id = $1",
        )
        .bind(Uuid::parse_str(p.vehicle["id"].as_str().unwrap()).unwrap())
        .bind(slot)
        .bind(km)
        .execute(&pool)
        .await
        .unwrap();
    }
    let b = p.book().await;
    assert_eq!(slot_next(&b, "oil"), json!(140_000));
    assert_eq!(slot_next(&b, "air_filter"), json!(142_000));
    let (_, again) = call(
        &p.app,
        "GET",
        &format!("/api/v1/sales/{}", sale["id"].as_str().unwrap()),
        &p.owner,
        None,
    )
    .await;
    // Печатный чек не меняется: масло и воздушный фильтр с ним — на 138 000 (I4).
    assert_eq!(
        next_of(&again["book"]),
        vec![
            ("oil".to_string(), json!(138_000)),
            ("air_filter".to_string(), json!(138_000))
        ]
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn titles_are_goods_and_fuel_filter_keeps_its_term(pool: PgPool) {
    // Задача 003 Н5 (жалоба 31.10.2026): в книжке и чеке — что залили и поставили, а не «Масло в
    // коробке»; два товара одной строки — оба; топливный фильтр в чеке с заменой масла — на свой
    // срок (30 000 км), не на пробег масла (ADR-117), под своей услугой.
    let mut p = Point::new(pool).await;
    point_service(
        &mut p,
        "fuel",
        json!({ "name": "Замена топливного фильтра", "book_slot": "fuel_filter" }),
    )
    .await;
    let (_, cats) = call(&p.app, "GET", "/api/v1/categories", &p.owner, None).await;
    let gear_cat = cats
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Трансмиссионные масла")
        .unwrap()["id"]
        .clone();
    let shell = ok(
        &p.app,
        "POST",
        "/api/v1/products",
        &p.owner,
        json!({ "op_id": Uuid::now_v7(), "category_id": gear_cat, "name": "Shell ATF 134",
                "container_ml": 1000, "sale_price_tyiyn": 50_000 }),
    )
    .await;
    p.goods.insert("shell", (shell["id"].clone(), 50_000));
    let sale = p
        .sell(
            130_000,
            &[
                ("oil", "container", 1),
                ("atf", "container", 1),
                ("shell", "container", 1),
                ("fuel_f", "piece", 1),
            ],
            &["oil", "gear", "fuel"],
            json!([
                { "service_id": p.services["oil"]["id"], "next_mileage": 138_000 },
                { "service_id": p.services["gear"]["id"], "next_mileage": 170_000 },
            ]),
        )
        .await;
    let book = &sale["book"];
    let line = |slot: &str| {
        book["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["slot"] == slot)
            .cloned()
            .unwrap_or_else(|| panic!("нет {slot}: {book}"))
    };
    let gear = line("gear_oil");
    assert_eq!(
        gear["label"],
        json!("ATF Dexron III · 1 кан. × 4 л, Shell ATF 134 · 1 кан. × 1 л")
    );
    assert_eq!(gear["next_mileage"], json!(170_000));
    assert_eq!(gear["group"], json!("Замена масла в коробке"));
    let fuel = line("fuel_filter");
    assert_eq!(fuel["label"], json!("Фильтр WK-842 (топливный) · 1 шт"));
    assert_eq!(fuel["next_mileage"], json!(160_000));
    assert_eq!(fuel["group"], json!("Замена топливного фильтра"));
    assert_eq!(line("oil")["next_mileage"], json!(138_000));
    for l in book["lines"].as_array().unwrap() {
        let label = l["label"].as_str().unwrap();
        assert!(
            !["Масло в коробке", "Масло двигателя", "Фильтр топливный"].contains(&label),
            "{l}"
        );
    }
    // Книжка клиента: подпись следующей замены — товар; название строки — для интервала.
    let b = p.book().await;
    let slot_of = |s: &str| {
        b["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["slot"] == s)
            .cloned()
            .unwrap()
    };
    let gear = slot_of("gear_oil");
    assert_eq!(gear["label"], json!("Масло в коробке"));
    assert_eq!(
        gear["title"],
        json!("ATF Dexron III · 1 кан. × 4 л, Shell ATF 134 · 1 кан. × 1 л")
    );
    assert_eq!(gear["products"], json!(["ATF Dexron III", "Shell ATF 134"]));
    let fuel = slot_of("fuel_filter");
    assert_eq!(fuel["title"], json!("Фильтр WK-842 (топливный) · 1 шт"));
    assert_eq!(fuel["next_km"], json!(160_000));
    let rec = &b["records"][0];
    let titles: Vec<&str> = rec["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Фильтр WK-842 (топливный) · 1 шт"),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"Shell ATF 134 · 1 кан. × 1 л"),
        "{titles:?}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn fuel_filter_category_without_slot_gets_it_by_name(pool: PgPool) {
    // Задача 003 Н5: «Фильтры топливные», заведённые до строки по названию, — строка «Фильтр
    // топливный» (миграция 0085): выключили у неё «Тип» — топливный фильтр не становится масляным.
    let fuel = Uuid::now_v7();
    let plain = Uuid::now_v7();
    let chosen = Uuid::now_v7();
    for (id, name, slot) in [
        (fuel, "Фильтры топливные", None),
        (plain, "Фильтры", None),
        (chosen, "Фильтры топливные грузовые", Some("oil_filter")),
    ] {
        sqlx::query(
            "insert into categories (id, name, kind, book_slot) values ($1, $2, 'filter', $3)",
        )
        .bind(id)
        .bind(name)
        .bind(slot)
        .execute(&pool)
        .await
        .unwrap();
    }
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version == 85) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    assert_eq!(
        stored_book_slot(&pool, fuel).await.as_deref(),
        Some("fuel_filter")
    );
    // Без подсказки в названии и выбранная владельцем — как были.
    assert_eq!(stored_book_slot(&pool, plain).await, None);
    assert_eq!(
        stored_book_slot(&pool, chosen).await.as_deref(),
        Some("oil_filter")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn untyped_fuel_filter_in_plain_category_by_its_name(pool: PgPool) {
    // Задача 003 Н5: «Фильтр топливный WK 820» без «Типа» в общей категории «Фильтры» (строка
    // книжки не выбрана) — топливный по названию товара, как категория в миграции 0085, а не
    // масляный на срок масла. Товар без подсказки в названии — как был, масляный.
    let mut p = Point::new(pool).await;
    let (_, cats) = call(&p.app, "GET", "/api/v1/categories", &p.owner, None).await;
    let filters = cats
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Фильтры")
        .unwrap()
        .clone();
    assert_eq!(filters["slot_by_name"], json!(true));
    for (key, name) in [
        ("fuel_n", "Фильтр топливный WK 820"),
        ("plain_n", "Фильтр W920"),
    ] {
        let made = ok(
            &p.app,
            "POST",
            "/api/v1/products",
            &p.owner,
            json!({ "op_id": Uuid::now_v7(), "category_id": filters["id"], "name": name,
                    "sale_price_tyiyn": 40_000 }),
        )
        .await;
        p.goods.insert(key, (made["id"].clone(), 40_000));
    }
    let sale = p
        .sell(
            130_000,
            &[("fuel_n", "piece", 1), ("plain_n", "piece", 1)],
            &["oil"],
            json!([]),
        )
        .await;
    let b = p.book().await;
    let rec = b["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["sale_id"] == sale["id"])
        .unwrap()
        .clone();
    let lines: Vec<(&str, &str)> = rec["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["slot"].as_str().unwrap(), l["text"].as_str().unwrap()))
        .collect();
    assert!(
        lines.contains(&("fuel_filter", "Фильтр топливный WK 820 · 1 шт")),
        "{lines:?}"
    );
    assert!(
        lines.contains(&("oil_filter", "Фильтр W920 · 1 шт")),
        "{lines:?}"
    );
}
