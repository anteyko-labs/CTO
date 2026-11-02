//! Ревизию ведут несколько сотрудников, у каждого свой вход (задача 005, Н7): «нас 5 человек,
//! каждый со своим аккаунтом зашёл и провёл ревизию, в конце суммируется: я занёс 6, Алексей 4 —
//! в итоге 10». Ручные прибавки и сканы разных входов складываются в одну строку, повтор `op_id`
//! не удваивает, «= итог» сверяется с тем, что человек видел, и перекрывает, журнал — кто сколько.
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

/// Запрос с устройства `device`.
async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
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

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap_or_default()
}

/// Сотрудник со своим входом: роль администратора, право «Ревизия» и своё устройство.
struct Person {
    cookie: String,
    device: Uuid,
    name: &'static str,
}

/// Владелец и пять сотрудников филиала, каждый на своём устройстве.
struct World {
    app: Router,
    owner: Person,
    people: Vec<Person>,
}

const NAMES: [&str; 5] = ["Азиз", "Алексей", "Айбек", "Нурлан", "Эрмек"];

impl World {
    async fn new(pool: PgPool) -> Self {
        bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
            .await
            .unwrap();
        let branch: Uuid = sqlx::query_scalar("select id from branches")
            .fetch_one(&pool)
            .await
            .unwrap();
        for (i, name) in NAMES.iter().enumerate() {
            let id = Uuid::now_v7();
            let hash = hash_password("staff-pass-1".into()).await.unwrap();
            sqlx::query(
                "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'admin', $5)",
            )
            .bind(id)
            .bind(branch)
            .bind(format!("staff{i}"))
            .bind(hash)
            .bind(*name)
            .execute(&pool)
            .await
            .unwrap();
            // Только «Ревизия»: прибавлять в общую строку не нужно ни «Товаров», ни чужого входа.
            sqlx::query(
                "insert into user_permissions (user_id, branch_id, permission) values ($1, $2, 'revision')",
            )
            .bind(id)
            .bind(branch)
            .execute(&pool)
            .await
            .unwrap();
        }
        let app = app(build_state(pool.clone(), false).await.unwrap(), None);
        let owner = Person {
            cookie: login(&app, "owner", "owner-pass-1").await,
            device: Uuid::now_v7(),
            name: "Владелец",
        };
        let mut people = Vec::new();
        for (i, name) in NAMES.iter().enumerate() {
            people.push(Person {
                cookie: login(&app, &format!("staff{i}"), "staff-pass-1").await,
                device: Uuid::now_v7(),
                name,
            });
        }
        World { app, owner, people }
    }

    async fn ok(&self, who: &Person, method: &str, uri: &str, body: Value) -> Value {
        let (s, v) = call(
            &self.app,
            method,
            uri,
            Some(&who.cookie),
            Some(body),
            who.device,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{} {uri}: {v}", who.name);
        v
    }

    async fn get(&self, who: &Person, uri: &str) -> Value {
        let (s, v) = call(&self.app, "GET", uri, Some(&who.cookie), None, who.device).await;
        assert_eq!(s, StatusCode::OK, "{uri}: {v}");
        v
    }

    /// Черновик ревизии «весь склад» и штучный товар в нём.
    async fn draft(&self, name: &str) -> (String, String) {
        let cat = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/categories",
                json!({ "name": format!("Кат {}", Uuid::now_v7()), "kind": "filter" }),
            )
            .await;
        let p = self
            .ok(
                &self.owner,
                "POST",
                "/api/v1/products",
                json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": name, "sale_price_tyiyn": 10_000 }),
            )
            .await;
        // Ревизию начинает сотрудник своим входом, остальные присоединяются к тому же черновику.
        let rev = self
            .ok(&self.people[0], "POST", "/api/v1/revisions", json!({}))
            .await;
        for who in &self.people[1..] {
            let same = self.ok(who, "POST", "/api/v1/revisions", json!({})).await;
            assert_eq!(same["head"]["id"], rev["head"]["id"], "{}", who.name);
        }
        (
            rev["head"]["id"].as_str().unwrap().to_string(),
            p["id"].as_str().unwrap().to_string(),
        )
    }

    /// Строка ревизии глазами `who`: итог и журнал.
    async fn line(&self, who: &Person, rid: &str, item: &str) -> Value {
        let rev = self.get(who, &format!("/api/v1/revisions/{rid}")).await;
        rev["lines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["product_id"] == json!(item))
            .cloned()
            .unwrap()
    }
}

async fn login(app: &Router, login: &str, password: &str) -> String {
    let (s, c) = call(
        app,
        "POST",
        "/api/v1/auth/login",
        None,
        Some(json!({ "login": login, "password": password })),
        Uuid::now_v7(),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{c}");
    c.as_str().unwrap().to_string()
}

fn log_of(line: &Value) -> Vec<(String, String, i64)> {
    line["counted_log"]
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
        .collect()
}

/// Пять входов одновременно: каждый вписывает руками найденное в своём месте (1…5) и сканирует
/// одну штуку — итог строки = сумма (15 + 5 = 20). Повтор записи с тем же `op_id` (обрыв связи)
/// не прибавляет; чужой вход с тем же `op_id` — 409, а не прибавка.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn five_logins_add_by_hand_and_scan_into_one_line(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item) = w.draft("Фильтр масляный W 712").await;
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let hand: Vec<Uuid> = (0..5).map(|_| Uuid::now_v7()).collect();
    let mut set = JoinSet::new();
    for (i, who) in w.people.iter().enumerate() {
        for (qty, op) in [(i as i64 + 1, hand[i]), (1, Uuid::now_v7())] {
            let (app, cookie, device, uri, item) = (
                w.app.clone(),
                who.cookie.clone(),
                who.device,
                uri.clone(),
                item.clone(),
            );
            set.spawn(async move {
                call(
                    &app,
                    "PUT",
                    &uri,
                    Some(&cookie),
                    Some(json!({ "product_id": item, "counted_qty": qty, "mode": "add", "op_id": op })),
                    device,
                )
                .await
            });
        }
    }
    while let Some(r) = set.join_next().await {
        let (s, v) = r.unwrap();
        assert_eq!(s, StatusCode::OK, "{v}");
    }
    let line = w.line(&w.owner, &rid, &item).await;
    assert_eq!(
        line["counted_qty"],
        json!(20),
        "1+2+3+4+5 руками и 5 сканов"
    );
    // У каждого входа в журнале — его 1…5 руками и скан: прибавки входа складываются.
    let mut by: std::collections::HashMap<String, i64> = Default::default();
    for (who, mode, qty) in log_of(&line) {
        assert_eq!(mode, "add");
        *by.entry(who).or_default() += qty;
    }
    for (i, name) in NAMES.iter().enumerate() {
        assert_eq!(by.get(*name).copied(), Some(i as i64 + 2), "{name}");
    }
    // Повтор ручной прибавки Нурлана (4) после обрыва — прежний ответ, не 24.
    let again = w
        .ok(
            &w.people[3],
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": 4, "mode": "add", "op_id": hand[3] }),
        )
        .await;
    assert!(again["counted_qty"].as_i64().unwrap() <= 20, "{again}");
    // Тот же op_id с чужого входа — не прибавка, а 409.
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.people[4].cookie),
        Some(json!({ "product_id": item, "counted_qty": 4, "mode": "add", "op_id": hand[3] })),
        w.people[4].device,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    for who in &w.people {
        assert_eq!(
            w.line(who, &rid, &item).await["counted_qty"],
            json!(20),
            "{} видит общий итог",
            who.name
        );
    }
}

/// «Я занёс 6, Алексей 4 — в итоге 10»: у обоих входов итог 10, журнал — кто сколько; строку,
/// которую начал один вход, другой видит и прибавляет к ней, а не заводит свою.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn six_plus_four_is_ten_and_log_shows_who(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item) = w.draft("Масло Shell 1 л").await;
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let (aziz, aleksey) = (&w.people[0], &w.people[1]);
    let a = w
        .ok(
            aziz,
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": 6, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(a["counted_qty"], json!(6));
    // Алексей видит строку Азиза — итог 6 и версию — и прибавляет своё.
    let seen = w
        .get(aleksey, &format!("/api/v1/revisions/{rid}/lines/{item}"))
        .await;
    assert_eq!(seen["counted_qty"], json!(6));
    assert_eq!(seen["version"], a["version"]);
    let b = w
        .ok(
            aleksey,
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": 4, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(b["counted_qty"], json!(10));
    for who in [aziz, aleksey, &w.owner] {
        let line = w.line(who, &rid, &item).await;
        assert_eq!(line["counted_qty"], json!(10), "{}", who.name);
        assert_eq!(
            log_of(&line),
            vec![
                ("Азиз".into(), "add".into(), 6),
                ("Алексей".into(), "add".into(), 4)
            ],
            "{}",
            who.name
        );
        // Записи разных входов — с разными входами: экран складывает «кто сколько» по входу.
        let ids: Vec<&Value> = line["counted_log"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| &e["user_id"])
            .collect();
        assert!(ids[0].is_string() && ids[0] != ids[1], "{ids:?}");
    }
}

/// «= итог» одного входа сверяется с тем, что он видел: пока он вводил, другой вход прибавил —
/// 409 `count_changed` «сейчас N», чужая прибавка цела; перечитал и подтвердил — итог перекрывает
/// прибавки, следующие прибавки — уже к нему.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn set_by_one_login_is_checked_against_adds_of_another(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item) = w.draft("Свеча NGK BKR6E").await;
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let (aziz, aleksey, aibek) = (&w.people[0], &w.people[1], &w.people[2]);
    for (who, qty) in [(aziz, 6), (aleksey, 4)] {
        w.ok(
            who,
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": qty, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    }
    // Айбек видел 10 и вводит «= итог 12»; тем временем Азиз прибавил ещё 1.
    w.ok(
        aziz,
        "PUT",
        &uri,
        json!({ "product_id": item, "counted_qty": 1, "mode": "add", "op_id": Uuid::now_v7() }),
    )
    .await;
    let op = Uuid::now_v7();
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&aibek.cookie),
        Some(json!({ "product_id": item, "counted_qty": 12, "mode": "set", "expected_qty": 10, "op_id": op })),
        aibek.device,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "count_changed", "{v}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("сейчас 11 шт"),
        "{v}"
    );
    assert_eq!(
        w.line(aibek, &rid, &item).await["counted_qty"],
        json!(11),
        "чужая прибавка цела"
    );
    // Перечитал (11) и подтвердил итог 12: итог перекрывает прибавки у всех входов.
    let set = w
        .ok(
            aibek,
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": 12, "mode": "set", "expected_qty": 11, "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(set["counted_qty"], json!(12));
    let after = w
        .ok(
            aleksey,
            "PUT",
            &uri,
            json!({ "product_id": item, "counted_qty": 3, "mode": "add", "op_id": Uuid::now_v7() }),
        )
        .await;
    assert_eq!(after["counted_qty"], json!(15), "прибавка — к итогу Айбека");
    let line = w.line(aziz, &rid, &item).await;
    assert_eq!(line["counted_qty"], json!(15));
    assert_eq!(
        log_of(&line),
        vec![
            ("Азиз".into(), "add".into(), 6),
            ("Алексей".into(), "add".into(), 4),
            ("Азиз".into(), "add".into(), 1),
            ("Айбек".into(), "set".into(), 12),
            ("Алексей".into(), "add".into(), 3),
        ]
    );
}

/// Общий планшет: сотрудники входят на нём по очереди. Старый экран (без `expected_qty`) другого
/// входа на том же устройстве не затирает свежие прибавки — 409 `client_outdated`, как с другого
/// устройства.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn other_login_on_same_tablet_does_not_wipe_fresh_adds(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item) = w.draft("Лампа H4").await;
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let tablet = Uuid::now_v7();
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.people[0].cookie),
        Some(
            json!({ "product_id": item, "counted_qty": 6, "mode": "add", "op_id": Uuid::now_v7() }),
        ),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.people[1].cookie),
        Some(json!({ "product_id": item, "counted_qty": 4 })),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(code(&v), "client_outdated", "{v}");
    assert_eq!(w.line(&w.owner, &rid, &item).await["counted_qty"], json!(6));
    // Тот же вход на том же планшете — свои прибавки не мешают (как раньше).
    let (s, v) = call(
        &w.app,
        "PUT",
        &uri,
        Some(&w.people[0].cookie),
        Some(json!({ "product_id": item, "counted_qty": 7 })),
        tablet,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["counted_qty"], json!(7));
}

/// «= моё» (задача 007): каждый видит только свой пересчёт и исправляет своё число строки. Азиз
/// пробил 5 вместо 3 — «= моё 3» убирает 2 из итога, а 4 Алексея целы (итог 7). Повтор того же
/// `op_id` не меняет итог; «= моё» больше своего — прибавка разницы; «= моё 0» в своей строке —
/// своё 0, чужое цело.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn mine_fixes_own_count_and_keeps_others(pool: PgPool) {
    let w = World::new(pool).await;
    let (rid, item) = w.draft("Трос буксировочный").await;
    let uri = format!("/api/v1/revisions/{rid}/lines");
    let (aziz, aleksey) = (&w.people[0], &w.people[1]);
    let put = |who: &'static str, qty: i64, mode: &'static str, op: Uuid| json!({ "product_id": item, "counted_qty": qty, "mode": mode, "op_id": op, "_who": who });
    w.ok(aziz, "PUT", &uri, put("a", 5, "add", Uuid::now_v7()))
        .await;
    w.ok(aleksey, "PUT", &uri, put("b", 4, "add", Uuid::now_v7()))
        .await;
    let op = Uuid::now_v7();
    let fixed = w.ok(aziz, "PUT", &uri, put("a", 3, "mine", op)).await;
    assert_eq!(fixed["counted_qty"], json!(7));
    // Ответ потерялся — тот же ввод ещё раз: итог тот же.
    let again = w.ok(aziz, "PUT", &uri, put("a", 3, "mine", op)).await;
    assert_eq!(again["counted_qty"], json!(7));
    let line = w.line(&w.owner, &rid, &item).await;
    assert_eq!(
        log_of(&line),
        vec![
            ("Азиз".into(), "add".into(), 5),
            ("Алексей".into(), "add".into(), 4),
            ("Азиз".into(), "add".into(), -2)
        ]
    );
    // Больше своего — прибавка разницы; у Алексея «= моё 0» — его 4 уходят, Азиза цело.
    let up = w
        .ok(aziz, "PUT", &uri, put("a", 6, "mine", Uuid::now_v7()))
        .await;
    assert_eq!(up["counted_qty"], json!(10));
    let zero = w
        .ok(aleksey, "PUT", &uri, put("b", 0, "mine", Uuid::now_v7()))
        .await;
    assert_eq!(zero["counted_qty"], json!(6));
    // «= итог» владельца перекрывает всех: своё число Азиза после него — с нуля.
    let seen = w.line(&w.owner, &rid, &item).await;
    w.ok(
        &w.owner,
        "PUT",
        &uri,
        json!({ "product_id": item, "counted_qty": 8, "mode": "set", "op_id": Uuid::now_v7(), "expected_qty": seen["counted_qty"] }),
    )
    .await;
    let after = w
        .ok(aziz, "PUT", &uri, put("a", 1, "mine", Uuid::now_v7()))
        .await;
    assert_eq!(after["counted_qty"], json!(9));
}
