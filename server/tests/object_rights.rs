//! Задача 003 Н10: права кассира без входа (ADR-{B8.1}) и характеристики объектов
//! (ADR-{B8.2}): «Скрыть категорию», реквизиты поставщика, перенос машины к другому клиенту.
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

async fn setup(pool: PgPool) -> (Router, Uuid) {
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
    (app(build_state(pool, false).await.unwrap(), None), admin)
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

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    admin_id: Uuid,
}

impl W {
    async fn new(pool: PgPool) -> Self {
        let (app, admin_id) = setup(pool.clone()).await;
        let owner = login(&app, "owner", "owner-pass-1").await;
        let admin = login(&app, "admin", "admin-pass-1").await;
        W {
            app,
            pool,
            owner,
            admin,
            admin_id,
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

    async fn cashier(&self, name: &str, pin: &str) -> String {
        let e = self
            .ok(
                "POST",
                &self.owner,
                "/api/v1/employees",
                json!({ "full_name": name, "is_cashier": true, "pin": pin }),
            )
            .await;
        e["id"].as_str().unwrap().to_string()
    }

    async fn open_shift(&self, cashier: &str, pin: &str) -> String {
        let s = self
            .ok(
                "POST",
                &self.owner,
                "/api/v1/shifts/open",
                json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": cashier, "cashier_pin": pin }),
            )
            .await;
        s["id"].as_str().unwrap().to_string()
    }

    async fn close_shift(&self, id: &str) {
        let cur = self.get(&self.owner, "/api/v1/shifts/current").await;
        let expected = cur["expected_tyiyn"].as_i64().unwrap();
        self.ok(
            "POST",
            &self.owner,
            &format!("/api/v1/shifts/{id}/close"),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected, "comment": "" }),
        )
        .await;
        self.ok(
            "POST",
            &self.owner,
            &format!("/api/v1/shifts/{id}/handover"),
            json!({ "op_id": Uuid::now_v7(), "amount_tyiyn": expected }),
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

fn sale_body(cashier: &str, item: &str, price: i64, method: &str) -> Value {
    json!({ "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": cashier,
            "lines": [{ "kind": "piece", "product_id": item, "qty": 1, "unit_price_tyiyn": price }],
            "payments": [{ "method": method, "amount_tyiyn": price }] })
}

#[sqlx::test(migrations = "./migrations")]
async fn cashier_limit_intersects_login_rights(pool: PgPool) {
    let w = W::new(pool).await;
    let item = stocked_item(&w).await;
    // Входу администратора выдана скидка; Нурлану владелец её снимает, Айбек — «как у входа».
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, branch_id, 'discount' from users where id = $1")
        .bind(w.admin_id)
        .execute(&w.pool)
        .await
        .unwrap();
    let nurlan = w.cashier("Нурлан", "1111").await;
    let aibek = w.cashier("Айбек", "2222").await;
    let uri = format!("/api/v1/employees/{nurlan}/permissions");
    let rights = w.get(&w.owner, &uri).await;
    assert_eq!(rights["denied"], json!([]));
    assert!(
        rights["keys"]
            .as_array()
            .unwrap()
            .contains(&json!("discount"))
    );
    assert!(
        !rights["keys"]
            .as_array()
            .unwrap()
            .contains(&json!("receipts"))
    );

    // Администратору права кассира не видны и не меняются.
    let (s, _) = w.req("GET", &w.admin, &uri, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w
        .req(
            "PUT",
            &w.admin,
            &uri,
            Some(json!({ "op_id": Uuid::now_v7(), "denied": [] })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Снять можно только права кассы и денег.
    let (s, _) = w
        .req(
            "PUT",
            &w.owner,
            &uri,
            Some(json!({ "op_id": Uuid::now_v7(), "denied": ["receipts"] })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "denied": ["expenses", "discount", "sale_debt"] });
    let out = w.ok("PUT", &w.owner, &uri, body.clone()).await;
    assert_eq!(out["denied"], json!(["sale_debt", "discount", "expenses"]));
    // Повтор того же op_id — тот же ответ, без второй записи в журнале.
    assert_eq!(w.ok("PUT", &w.owner, &uri, body).await, out);
    assert_eq!(
        w.scalar("select count(*) from audit_log where action = 'employee.permissions'")
            .await,
        1
    );

    // Смена Нурлана: у администратора скидка, долг и расход — «Нет права у кассира Нурлан».
    let shift = w.open_shift(&nurlan, "1111").await;
    let cur = w.get(&w.admin, "/api/v1/shifts/current").await;
    assert_eq!(
        cur["cashier_denied"],
        json!(["sale_debt", "discount", "expenses"])
    );
    let (s, e) = w
        .req(
            "POST",
            &w.admin,
            "/api/v1/sales",
            Some(sale_body(&nurlan, &item, 90_000, "cash")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(
        e["error"]["message"],
        json!("Нет права у кассира Нурлан: Скидка в чеке")
    );
    let (s, e) = w
        .req(
            "POST",
            &w.admin,
            "/api/v1/sales",
            Some(sale_body(&nurlan, &item, 100_000, "debt")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(
        e["error"]["message"],
        json!("Нет права у кассира Нурлан: Продажа в долг")
    );
    let (s, e) = w
        .req(
            "POST",
            &w.admin,
            "/api/v1/expenses",
            Some(json!({ "op_id": Uuid::now_v7(), "article_id": Uuid::now_v7(), "amount_tyiyn": 1000 })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(
        e["error"]["message"],
        json!("Нет права у кассира Нурлан: Расходы")
    );
    // Права входа, которых нет в ограничении, работают; обычный чек проходит.
    w.ok(
        "POST",
        &w.admin,
        "/api/v1/sales",
        sale_body(&nurlan, &item, 100_000, "cash"),
    )
    .await;
    // Владелец правами кассира не ограничен.
    w.ok(
        "POST",
        &w.owner,
        "/api/v1/sales",
        sale_body(&nurlan, &item, 90_000, "cash"),
    )
    .await;
    w.close_shift(&shift).await;

    // Смена Айбека («как у входа»): скидка администратора проходит, долг — уже не 403.
    w.open_shift(&aibek, "2222").await;
    let cur = w.get(&w.admin, "/api/v1/shifts/current").await;
    assert_eq!(cur["cashier_denied"], json!([]));
    w.ok(
        "POST",
        &w.admin,
        "/api/v1/sales",
        sale_body(&aibek, &item, 90_000, "cash"),
    )
    .await;
    let (s, e) = w
        .req(
            "POST",
            &w.admin,
            "/api/v1/sales",
            Some(sale_body(&aibek, &item, 100_000, "debt")),
        )
        .await;
    assert_ne!(s, StatusCode::FORBIDDEN, "{e}");
}

#[sqlx::test(migrations = "./migrations")]
async fn cashier_limit_does_not_extend_login_rights(pool: PgPool) {
    let w = W::new(pool).await;
    let item = stocked_item(&w).await;
    // У входа скидки нет, у кассира ничего не снято — скидки по-прежнему нет.
    let cashier = w.cashier("Айбек", "2222").await;
    w.open_shift(&cashier, "2222").await;
    let (s, e) = w
        .req(
            "POST",
            &w.admin,
            "/api/v1/sales",
            Some(sale_body(&cashier, &item, 90_000, "cash")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{e}");
    assert_eq!(e["error"]["message"], json!("Скидку ставит владелец"));
    // Не кассиру права на кассе не задаются.
    let master = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/employees",
            json!({ "full_name": "Мастер", "is_master": true }),
        )
        .await;
    let (s, _) = w
        .req(
            "PUT",
            &w.owner,
            &format!(
                "/api/v1/employees/{}/permissions",
                master["id"].as_str().unwrap()
            ),
            Some(json!({ "op_id": Uuid::now_v7(), "denied": ["discount"] })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = "./migrations")]
async fn hidden_category_leaves_the_pickers(pool: PgPool) {
    let w = W::new(pool).await;
    let cat = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/categories",
            json!({ "name": "Щётки", "kind": "other" }),
        )
        .await;
    let cid = cat["id"].as_str().unwrap().to_string();
    assert_eq!(cat["archived"], json!(false));
    let p = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cid, "name": "Щётка", "sale_price_tyiyn": 30_000 }),
        )
        .await;
    let pid = p["id"].as_str().unwrap().to_string();
    let uri = format!("/api/v1/categories/{cid}");
    // Товар в продаже — скрыть нельзя.
    let (s, e) = w
        .req("PATCH", &w.owner, &uri, Some(json!({ "archived": true })))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("в продаже: 1")
    );
    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/products/{pid}"),
        json!({ "archived": true }),
    )
    .await;
    let out = w
        .ok("PATCH", &w.owner, &uri, json!({ "archived": true }))
        .await;
    assert_eq!(out["archived"], json!(true));
    let has = |list: &Value| {
        list.as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == json!(cid))
    };
    assert!(!has(&w.get(&w.admin, "/api/v1/categories").await));
    assert!(has(&w.get(&w.owner, "/api/v1/categories?all=true").await));
    // Новый товар в скрытую категорию и возврат товара из архива — 422.
    let (s, _) = w
        .req(
            "POST",
            &w.owner,
            "/api/v1/products",
            Some(json!({ "op_id": Uuid::now_v7(), "category_id": cid, "name": "Щётка 2", "sale_price_tyiyn": 30_000 })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/products/{pid}"),
            Some(json!({ "archived": false })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Журнал: скрыта — было → стало.
    let logged: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'category.update' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(logged["archived"]["archived"]["new"], json!(true));
    // Показать снова — товар возвращается из архива.
    w.ok("PATCH", &w.owner, &uri, json!({ "archived": false }))
        .await;
    w.ok(
        "PATCH",
        &w.owner,
        &format!("/api/v1/products/{pid}"),
        json!({ "archived": false }),
    )
    .await;
    assert!(has(&w.get(&w.admin, "/api/v1/categories").await));
}

#[sqlx::test(migrations = "./migrations")]
async fn supplier_requisites_are_editable(pool: PgPool) {
    let w = W::new(pool).await;
    let s = w
        .ok(
            "POST",
            &w.admin,
            "/api/v1/suppliers",
            json!({ "name": "ОсОО Масла", "inn": "01234567890123", "address": "Бишкек, Жибек Жолу 1",
                    "bank": "Оптима Банк, БИК 109018", "bank_account": "1090000000000001", "due_days": 14 }),
        )
        .await;
    let id = s["id"].as_str().unwrap().to_string();
    assert_eq!(s["kind"], json!("company"));
    assert_eq!(s["inn"], json!("01234567890123"));
    assert_eq!(s["due_days"], json!(14));
    // Правит и администратор с «Приходом товара»: вид, адрес, снять срок.
    let out = w
        .ok(
            "PATCH",
            &w.admin,
            &format!("/api/v1/suppliers/{id}"),
            json!({ "kind": "person", "address": "Ош, Ленина 5", "clear_due_days": true }),
        )
        .await;
    assert_eq!(out["kind"], json!("person"));
    assert_eq!(out["address"], json!("Ош, Ленина 5"));
    assert_eq!(out["due_days"], Value::Null);
    assert_eq!(out["bank"], json!("Оптима Банк, БИК 109018"));
    let got = w.get(&w.owner, &format!("/api/v1/suppliers/{id}")).await;
    assert_eq!(got, out);
    // ИНН у поставщика — в акте сверки.
    let recon = w
        .get(&w.owner, &format!("/api/v1/parties/{id}/reconciliation"))
        .await;
    assert_eq!(recon["inn"], json!("01234567890123"));
    // Журнал — было → стало.
    let logged: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'supplier.update' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(
        logged["changes"]["address"]["old"],
        json!("Бишкек, Жибек Жолу 1")
    );
    assert_eq!(logged["changes"]["due_days"]["new"], Value::Null);
    // Неверные данные — 422; без права — 403.
    let (st, _) = w
        .req(
            "PATCH",
            &w.owner,
            &format!("/api/v1/suppliers/{id}"),
            Some(json!({ "inn": "12ab" })),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    sqlx::query("delete from user_permissions where permission = 'receipts'")
        .execute(&w.pool)
        .await
        .unwrap();
    let (st, _) = w
        .req(
            "PATCH",
            &w.admin,
            &format!("/api/v1/suppliers/{id}"),
            Some(json!({ "inn": "1" })),
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn vehicle_moves_with_its_book(pool: PgPool) {
    let w = W::new(pool).await;
    let party = |first: &str, phone: &str| json!({ "role": "customer", "kind": "person", "first_name": first, "last_name": "Асанов", "phone": phone });
    let old = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/parties",
            party("Бакыт", "+996555000111"),
        )
        .await;
    let new = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/parties",
            party("Нурбек", "+996555000222"),
        )
        .await;
    let (old_id, new_id) = (
        old["id"].as_str().unwrap().to_string(),
        new["id"].as_str().unwrap().to_string(),
    );
    let v = w
        .ok(
            "POST",
            &w.owner,
            &format!("/api/v1/parties/{old_id}/vehicles"),
            json!({ "plate": "01KG777ABC", "brand": "Toyota", "model": "Camry" }),
        )
        .await;
    let vid = v["id"].as_str().unwrap().to_string();
    w.ok(
        "POST",
        &w.owner,
        &format!("/api/v1/vehicles/{vid}/oil-changes"),
        json!({ "change_date": "2026-10-01", "mileage_km": 120_000, "oil_text": "Mobil 5W-30" }),
    )
    .await;
    let uri = format!("/api/v1/parties/{old_id}/vehicles/{vid}/move");
    // Без права «Клиенты и машины» — 403; к поставщику — 422.
    sqlx::query("delete from user_permissions where permission = 'clients'")
        .execute(&w.pool)
        .await
        .unwrap();
    let (s, _) = w
        .req(
            "POST",
            &w.admin,
            &uri,
            Some(json!({ "op_id": Uuid::now_v7(), "to_party_id": new_id })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let sup = w
        .ok(
            "POST",
            &w.owner,
            "/api/v1/suppliers",
            json!({ "name": "Поставщик" }),
        )
        .await;
    let (s, _) = w
        .req(
            "POST",
            &w.owner,
            &uri,
            Some(json!({ "op_id": Uuid::now_v7(), "to_party_id": sup["id"] })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let body = json!({ "op_id": Uuid::now_v7(), "to_party_id": new_id });
    let out = w.ok("POST", &w.owner, &uri, body.clone()).await;
    assert_eq!(out["oil_changes"], json!(1));
    // Повтор op_id — тот же ответ, одна запись в журнале.
    assert_eq!(w.ok("POST", &w.owner, &uri, body).await, out);
    assert_eq!(
        w.scalar("select count(*) from audit_log where action = 'vehicle.move'")
            .await,
        1
    );
    // Книжка — у нового клиента, у прежнего машины больше нет.
    let book = w
        .get(&w.owner, &format!("/api/v1/parties/{new_id}/oil-book"))
        .await;
    let text = book.to_string();
    assert!(
        text.contains("01KG777ABC") && text.contains("Mobil 5W-30"),
        "{book}"
    );
    let old_book = w
        .get(&w.owner, &format!("/api/v1/parties/{old_id}/oil-book"))
        .await;
    assert!(!old_book.to_string().contains("01KG777ABC"), "{old_book}");
    // Машина с тем же номером у получателя — 409; обратно к прежнему — можно.
    let back = format!("/api/v1/parties/{new_id}/vehicles/{vid}/move");
    let twin = w
        .ok(
            "POST",
            &w.owner,
            &format!("/api/v1/parties/{old_id}/vehicles"),
            json!({ "plate": "01 kg 777 abc" }),
        )
        .await;
    let (s, _) = w
        .req(
            "POST",
            &w.owner,
            &back,
            Some(json!({ "op_id": Uuid::now_v7(), "to_party_id": old_id })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_ne!(twin["id"], json!(vid));
}

#[sqlx::test(migrations = "./migrations")]
async fn owner_changes_user_login(pool: PgPool) {
    let w = W::new(pool).await;
    let uri = format!("/api/v1/users/{}", w.admin_id);
    // Администратор логин не меняет; занятый логин — 409; пустой — 422.
    let (s, _) = w
        .req("PATCH", &w.admin, &uri, Some(json!({ "login": "kassa" })))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = w
        .req("PATCH", &w.owner, &uri, Some(json!({ "login": "owner" })))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = w
        .req("PATCH", &w.owner, &uri, Some(json!({ "login": "  " })))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let out = w
        .ok("PATCH", &w.owner, &uri, json!({ "login": " kassa " }))
        .await;
    assert_eq!(out["login"], json!("kassa"));
    // Новым логином входят с тем же паролем; журнал помнит прежний.
    login(&w.app, "kassa", "admin-pass-1").await;
    let logged: Value = sqlx::query_scalar(
        "select data from audit_log where action = 'user.update' order by created_at desc limit 1",
    )
    .fetch_one(&w.pool)
    .await
    .unwrap();
    assert_eq!(logged["old_login"], json!("admin"));
}
