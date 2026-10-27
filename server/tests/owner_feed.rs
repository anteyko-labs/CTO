//! Лента владельцу (SPEC-36 §7, ADR-121): отчёт смены при закрытии, «Итог дня» после последней
//! смены дня, продажа в долг и возврат с долгом клиента.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::telegram::{Outgoing, mark_sent, pending};
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const OWNER_CHAT: i64 = 700_001;
const ADMIN_CHAT: i64 = 700_002;

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

async fn ok(app: &Router, who: &str, uri: &str, body: Value) -> Value {
    let (s, _, out) = call(app, "POST", uri, Some(who), Some(body)).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

async fn get(app: &Router, who: &str, uri: &str) -> Value {
    let (s, _, out) = call(app, "GET", uri, Some(who), None).await;
    assert_eq!(s, StatusCode::OK, "{uri}: {out}");
    out
}

fn id_of(v: &Value) -> String {
    v["id"].as_str().unwrap().to_string()
}

/// Мир теста: владелец и администратор с привязанными чатами, кассир, мастер, клиент со сроком
/// долга 10 дней, товар, открытая смена с разменом.
struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    cashier: String,
    master: String,
    client: String,
    item: String,
}

async fn world(pool: PgPool) -> W {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let hash = hash_password("admin-pass-1".into()).await.unwrap();
    let admin_id = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'admin', $3, 'admin', 'Админ')")
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
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
        .bind(admin_id)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
    // Чат владельца и чат администратора (ленту получает только владелец, ADR-063).
    for (chat, login) in [(OWNER_CHAT, "owner"), (ADMIN_CHAT, "admin")] {
        sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, linked_at, tg_user_id) select $1, id, branch_id, now() - interval '1 hour', $1 from users where login = $2")
            .bind(chat)
            .bind(login)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "insert into bot_state (key, value) values
           ('cursor', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))),
           ('feed_since', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')))
         on conflict (key) do update set value = excluded.value",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let cashier = id_of(
        &ok(
            &app,
            &owner,
            "/api/v1/employees",
            json!({ "full_name": "Кассир Айбек", "is_cashier": true, "pin": "1234" }),
        )
        .await,
    );
    let master = id_of(
        &ok(
            &app,
            &owner,
            "/api/v1/employees",
            json!({ "full_name": "Мастер Нурлан", "is_master": true }),
        )
        .await,
    );
    let client = id_of(
        &ok(
            &app,
            &admin,
            "/api/v1/parties",
            json!({ "name": "Бакыт", "kind": "person", "phone": "+996 555 111 222", "inn": "21234567890123" }),
        )
        .await,
    );
    sqlx::query("update parties set due_days = 10 where id = $1::uuid")
        .bind(&client)
        .execute(&pool)
        .await
        .unwrap();
    let (_, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Фильтры", "kind": "filter" })),
    )
    .await;
    let item = id_of(
        &ok(
            &app,
            &owner,
            "/api/v1/products",
            json!({ "op_id": Uuid::now_v7(), "category_id": cat["id"], "name": "Фильтр", "sale_price_tyiyn": 100_000 }),
        )
        .await,
    );
    let w = W {
        app,
        pool,
        owner,
        admin,
        cashier,
        master,
        client,
        item,
    };
    w.open_shift().await;
    ok(
        &w.app,
        &w.admin,
        "/api/v1/cash/movements",
        json!({ "op_id": Uuid::now_v7(), "kind": "cash_in", "amount_tyiyn": 1_000_000, "comment": "размен" }),
    )
    .await;
    w
}

impl W {
    async fn open_shift(&self) -> String {
        id_of(
            &ok(
                &self.app,
                &self.admin,
                "/api/v1/shifts/open",
                json!({ "op_id": Uuid::now_v7(), "cashier_employee_id": self.cashier, "cashier_pin": "1234" }),
            )
            .await,
        )
    }

    /// Чек на 1 000 с: способы оплаты — как переданы.
    async fn sale(&self, payments: Value, party: bool) -> Value {
        let mut body = json!({
            "op_id": Uuid::now_v7(), "sale_type": "takeaway", "cashier_id": self.cashier,
            "lines": [{ "kind": "piece", "product_id": self.item, "qty": 1, "unit_price_tyiyn": 100_000 }],
            "payments": payments
        });
        if party {
            body["party_id"] = json!(self.client);
        }
        ok(&self.app, &self.admin, "/api/v1/sales", body).await
    }

    /// Закрыть текущую смену с недостачей `short`.
    async fn close(&self, short: i64) {
        let shift = get(&self.app, &self.owner, "/api/v1/shifts/current").await;
        let id = id_of(&shift);
        let expected = shift["expected_tyiyn"].as_i64().unwrap();
        ok(
            &self.app,
            &self.admin,
            &format!("/api/v1/shifts/{id}/close"),
            json!({ "op_id": Uuid::now_v7(), "counted_tyiyn": expected - short,
                    "comment": if short != 0 { "не хватает" } else { "" }, "payouts": [] }),
        )
        .await;
    }

    async fn accrue(&self, emp: &str, kind: &str, amount: i64) {
        sqlx::query(
            "insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn, user_id, device_id)
             select $1, e.branch_id, e.id, (now() at time zone 'Asia/Bishkek')::date, $3, $4, (select id from users where login = 'owner'), $1
             from employees e where e.id = $2::uuid",
        )
        .bind(Uuid::now_v7())
        .bind(emp)
        .bind(kind)
        .bind(amount)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    /// Что сейчас уйдёт в чаты; всё отмечается доставленным. Чату администратора — ничего.
    async fn feed(&self) -> Vec<Outgoing> {
        let mut conn = self.pool.acquire().await.unwrap();
        let out = pending(&mut conn).await.unwrap();
        for m in &out {
            mark_sent(&mut conn, m).await.unwrap();
        }
        assert!(
            out.iter().all(|m| m.chat_id == OWNER_CHAT),
            "только владельцу"
        );
        out
    }

    async fn date(&self, days: i32) -> String {
        sqlx::query_scalar(
            "select to_char((now() at time zone 'Asia/Bishkek')::date + $1, 'DD.MM.YYYY')",
        )
        .bind(days)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

fn find<'a>(msgs: &'a [Outgoing], start: &str) -> &'a str {
    msgs.iter()
        .find(|m| m.text.starts_with(start))
        .map(|m| m.text.as_str())
        .unwrap_or_else(|| {
            panic!(
                "нет «{start}»:\n{}",
                msgs.iter()
                    .map(|m| m.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            )
        })
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_sale_and_return_show_client_debt(pool: PgPool) {
    let w = world(pool).await;
    w.feed().await;
    // Продажа: 600 наличными + 400 в долг.
    let sale = w
        .sale(
            json!([{ "method": "cash", "amount_tyiyn": 60_000 }, { "method": "debt", "amount_tyiyn": 40_000 }]),
            true,
        )
        .await;
    let number = sale["number"].as_i64().unwrap();
    let msgs = w.feed().await;
    let text = find(
        &msgs,
        &format!("🧾 Чек № {number} — 1 000 с (1 поз.) · 📒 В долг"),
    );
    let due = w.date(9).await;
    assert!(
        text.contains(&format!("📒 Долг клиента теперь 400 с, срок до {due}")),
        "{text}"
    );
    assert!(text.contains("клиент Бакыт"), "{text}");
    // Чек без долга — без пометки.
    w.sale(
        json!([{ "method": "cash", "amount_tyiyn": 100_000 }]),
        false,
    )
    .await;
    let msgs = w.feed().await;
    let plain = find(&msgs, "🧾 Чек №");
    assert!(
        !plain.contains("В долг") && !plain.contains("Долг клиента"),
        "{plain}"
    );

    // Возврат: 400 с долга, 600 наличными — долг клиента после возврата 0.
    let sid = id_of(&sale);
    let ret = ok(
        &w.app,
        &w.admin,
        &format!("/api/v1/sales/{sid}/return"),
        json!({ "op_id": Uuid::now_v7(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "debt", "amount_tyiyn": 40_000 }, { "method": "cash", "amount_tyiyn": 60_000 }] }),
    )
    .await;
    let msgs = w.feed().await;
    let text = find(
        &msgs,
        &format!(
            "↩️ Возврат № {} по чеку № {number} — 1 000 с",
            ret["number"].as_i64().unwrap()
        ),
    );
    for part in [
        "клиент Бакыт",
        "вернули: наличными 600 с, с долга 400 с",
        "📒 Долг клиента после возврата 0 с",
    ] {
        assert!(text.contains(part), "{part}\n{text}");
    }
    assert!(!text.contains("срок до"), "{text}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shift_close_sends_shift_report_and_day_total_after_last_shift(pool: PgPool) {
    let w = world(pool).await;
    // Смена № 1: чек наличными, чек в долг, выплаты мастеру по видам и аванс, недостача 100 с.
    w.sale(
        json!([{ "method": "cash", "amount_tyiyn": 100_000 }]),
        false,
    )
    .await;
    let debt = w
        .sale(
            json!([{ "method": "card", "amount_tyiyn": 60_000 }, { "method": "debt", "amount_tyiyn": 40_000 }]),
            true,
        )
        .await;
    w.accrue(&w.master, "revenue_percent", 50_000).await;
    w.accrue(&w.master, "bonus", 100_000).await;
    ok(
        &w.app,
        &w.owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.master,
                "lines": [{ "purpose": "percent", "amount_tyiyn": 50_000 },
                          { "purpose": "bonus", "amount_tyiyn": 100_000 }] }),
    )
    .await;
    ok(
        &w.app,
        &w.owner,
        "/api/v1/payouts",
        json!({ "op_id": Uuid::now_v7(), "employee_id": w.master,
                "lines": [{ "purpose": "advance", "amount_tyiyn": 20_000 }] }),
    )
    .await;
    w.feed().await;
    w.close(10_000).await;
    let msgs = w.feed().await;
    assert_eq!(msgs.len(), 1, "только сообщение о закрытии");
    let text = find(&msgs, "🔴 Смена № 1 закрыта");
    for part in [
        "💰 Деньги в кассе",
        "недостача 100 с",
        "🧾 Выручка: 2 000 с",
        "наличные 1 000 с · карта / QR банка 600 с · в долг 400 с",
        "безнал «Счёт»: 600 с",
        "валовая прибыль:",
        "чистая прибыль:",
        "👥 Выплачено 1 500 с: Мастер Нурлан — проценты 500 с, бонус 1 000 с",
        "💸 Авансы 200 с: Мастер Нурлан 200 с",
        &format!(
            "📒 В долг: Бакыт 400 с (чек № {})",
            debt["number"].as_i64().unwrap()
        ),
        "⚠️ Недостача 100 с — удержано с кассира Кассир Айбек 100 с",
    ] {
        assert!(text.contains(part), "{part}\n{text}");
    }
    assert!(!text.contains("Итог дня"), "{text}");
    let today = w.date(0).await;
    let hour: i32 =
        sqlx::query_scalar("select extract(hour from now() at time zone 'Asia/Bishkek')::int")
            .fetch_one(&w.pool)
            .await
            .unwrap();
    let msgs = w.feed().await;
    if hour < 20 {
        // До 20:00 день ещё идёт: может открыться следующая смена.
        assert!(msgs.is_empty(), "{}", msgs.len());
    } else {
        // После 20:00 без открытой смены день закончен.
        find(&msgs, &format!("📊 Итог дня, {today}"));
    }

    // Вторая смена того же дня, а другая касса уже открыла смену следующего дня: пока вторая
    // смена открыта, день не закончен.
    w.open_shift().await;
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn, opened_at)
         select $1, u.branch_id, 99, (now() at time zone 'Asia/Bishkek')::date + 1,
                (select a.id from cash_accounts a where a.branch_id = u.branch_id and a.name <> 'Касса' limit 1),
                $2::uuid, u.id, $1, 0, now() + interval '1 day'
         from users u where u.login = 'owner'",
    )
    .bind(Uuid::now_v7())
    .bind(&w.cashier)
    .execute(&w.pool)
    .await
    .unwrap();
    assert!(w.feed().await.iter().all(|m| !m.text.contains("Итог дня")));

    // Закрыли вторую смену: отчёт смены; итог дня — следующим проходом, после него.
    w.close(0).await;
    let msgs = w.feed().await;
    let closed = find(&msgs, "🔴 Смена № 2 закрыта");
    assert!(
        !closed.contains("Итог дня") && !closed.contains("Недостача"),
        "{closed}"
    );
    let msgs = w.feed().await;
    assert_eq!(msgs.len(), 1, "один итог дня");
    let day = find(&msgs, &format!("📊 Итог дня, {today}"));
    assert!(day.contains("ЧИСТАЯ ПРИБЫЛЬ"), "{day}");
    // Повторно не приходит.
    assert!(w.feed().await.is_empty());
}
