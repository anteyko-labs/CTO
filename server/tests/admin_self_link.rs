//! «Мой Телеграм» администратора (SPEC-40 §10, ADR-{h18}): администратор сам берёт код привязки
//! на сайте — код привязывает только его самого; владелец выключает это настройкой; отключённый
//! администратор кода не получает; владельцу — уведомления о привязке и отвязке; повтор `op_id`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::my_telegram::SELF_OFF;
use avtodom_server::api::telegram::{Keys, WRONG_CODE, handle_update};
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

const CHAT: i64 = 730_001;

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

async fn ok(app: &Router, who: &str, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (s, _, out) = call(app, method, uri, Some(who), body).await;
    assert_eq!(s, StatusCode::OK, "{method} {uri}: {out}");
    out
}

struct W {
    app: Router,
    pool: PgPool,
    owner: String,
    admin: String,
    admin_id: Uuid,
    owner_id: Uuid,
}

/// Владелец и администратор «Эрбол» с правами по умолчанию; чатов нет.
async fn world(pool: PgPool) -> W {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
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
    sqlx::query("insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])")
        .bind(admin_id)
        .bind(branch)
        .bind(keys)
        .execute(&pool)
        .await
        .unwrap();
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    W {
        app,
        pool,
        owner,
        admin,
        admin_id,
        owner_id,
    }
}

impl W {
    async fn audit(&self, action: &str) -> i64 {
        sqlx::query_scalar("select count(*) from audit_log where action = $1")
            .bind(action)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn linked_user(&self, chat: i64) -> Option<Uuid> {
        sqlx::query_scalar("select user_id from telegram_links where chat_id = $1")
            .bind(chat)
            .fetch_optional(&self.pool)
            .await
            .unwrap()
    }

    async fn feed(&self) -> String {
        ok(
            &self.app,
            &self.owner,
            "GET",
            "/api/v1/notifications?limit=50",
            None,
        )
        .await
        .to_string()
    }
}

/// `/start <код>` из личного чата с Телеграм-именем @erbol_tg.
async fn start(pool: &PgPool, chat: i64, code: &str) -> avtodom_server::api::telegram::Reply {
    let mut conn = pool.acquire().await.unwrap();
    handle_update(
        &mut conn,
        None,
        json!({
            "update_id": 1,
            "message": {
                "chat": { "id": chat, "type": "private" },
                "from": { "id": chat, "username": "erbol_tg", "first_name": "Эрбол" },
                "text": format!("/start {code}")
            }
        }),
    )
    .await
    .unwrap()
    .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_links_own_chat_from_site(pool: PgPool) {
    let w = world(pool).await;
    // До привязки: не привязан, привязывать сам можно, что увидит в боте — по его правам.
    let me = ok(&w.app, &w.admin, "GET", "/api/v1/me/telegram", None).await;
    assert_eq!(me["linked"], false, "{me}");
    assert_eq!(me["self_link"], true);
    let items = me["items"].as_array().unwrap();
    let item = |key: &str| items.iter().find(|i| i["key"] == key).unwrap().clone();
    assert_eq!(item("shift")["on"], true);
    assert_eq!(item("add_debt")["on"], false);
    assert_eq!(
        item("add_debt")["need"],
        "Нет права: Старые долги из тетради"
    );
    assert_eq!(me["events"], json!([]));
    assert!(!me.to_string().contains("прибыл"), "{me}");

    // «Мой Телеграм» — только администратору; чужой код и привязка чужого id — нельзя.
    let (s, _, _) = call(&w.app, "GET", "/api/v1/me/telegram", Some(&w.owner), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &w.app,
        "POST",
        "/api/v1/me/telegram/code",
        Some(&w.owner),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &w.app,
        "POST",
        &format!("/api/v1/users/{}/telegram/code", w.owner_id),
        Some(&w.admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &w.app,
        "GET",
        "/api/v1/settings/telegram/admins",
        Some(&w.admin),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Без op_id — не принимается (I8).
    let (s, _, _) = call(
        &w.app,
        "POST",
        "/api/v1/me/telegram/code",
        Some(&w.admin),
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Свой код: в теле — чужой user_id, код всё равно привязывает только его самого.
    let op = Uuid::now_v7();
    let body = json!({ "op_id": op, "user_id": w.owner_id });
    let first = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(body.clone()),
    )
    .await;
    let code = first["code"].as_str().unwrap().to_string();
    assert_eq!(code.len(), 6);
    // Повтор op_id — тот же код, вторая запись журнала не появляется.
    let again = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(body),
    )
    .await;
    assert_eq!(again, first);
    assert_eq!(w.audit("telegram.admin_code").await, 1);
    let owner_of_code: Uuid =
        sqlx::query_scalar("select user_id from telegram_codes where code = $1 and by_self")
            .bind(&code)
            .fetch_one(&w.pool)
            .await
            .unwrap();
    assert_eq!(owner_of_code, w.admin_id);

    // /start <код> — привязан именно он; бот сразу показывает меню по его правам.
    let r = start(&w.pool, CHAT, &code).await;
    assert!(
        r.text.starts_with("Готово, Эрбол: бот подключён."),
        "{}",
        r.text
    );
    let Keys::Admin(menu) = r.keys else {
        panic!("не кнопки администратора: {:?}", r.keys)
    };
    assert!(
        menu.has("shift") && menu.has("invoice") && !menu.has("add_debt"),
        "{menu:?}"
    );
    assert_eq!(w.linked_user(CHAT).await, Some(w.admin_id));
    // Код одноразовый.
    let r = start(&w.pool, CHAT + 1, &code).await;
    assert_eq!(r.text, WRONG_CODE);

    // Состояние: привязан, к какому Телеграм-имени; у владельца в окне — то же имя.
    let me = ok(&w.app, &w.admin, "GET", "/api/v1/me/telegram", None).await;
    assert_eq!(me["linked"], true, "{me}");
    assert!(me["linked_at"].is_string());
    assert_eq!(me["tg_name"], "@erbol_tg");
    let bot = ok(
        &w.app,
        &w.owner,
        "GET",
        &format!("/api/v1/users/{}/bot", w.admin_id),
        None,
    )
    .await;
    assert_eq!(bot["tg_name"], "@erbol_tg");
    // Включённое владельцем событие видно и администратору.
    ok(
        &w.app,
        &w.owner,
        "PUT",
        &format!("/api/v1/users/{}/bot-feed", w.admin_id),
        Some(json!({ "op_id": Uuid::now_v7(), "on": ["shift.open"] })),
    )
    .await;
    let me = ok(&w.app, &w.admin, "GET", "/api/v1/me/telegram", None).await;
    assert_eq!(me["events"], json!(["Смена открыта"]));

    // Владельцу — уведомление о привязке: сам, в «Мой Телеграм».
    let feed = w.feed().await;
    assert!(
        feed.contains("Телеграм привязан: администратор Эрбол"),
        "{feed}"
    );
    assert!(feed.contains("привязал сам в «Мой Телеграм»"), "{feed}");

    // Отвязать себя; повтор op_id — тот же ответ, одна запись журнала и одно уведомление.
    let op = Uuid::now_v7();
    let uri = format!("/api/v1/me/telegram?op_id={op}");
    let out = ok(&w.app, &w.admin, "DELETE", &uri, None).await;
    assert_eq!(out["unlinked"], true);
    let out = ok(&w.app, &w.admin, "DELETE", &uri, None).await;
    assert_eq!(out["unlinked"], true, "повтор — прежний ответ");
    assert_eq!(w.audit("telegram.admin_unlink").await, 1);
    assert_eq!(w.linked_user(CHAT).await, None);
    let feed = w.feed().await;
    assert!(
        feed.contains("Телеграм отвязан: администратор Эрбол"),
        "{feed}"
    );
    // Новая отвязка без привязки — без записи журнала.
    let out = ok(&w.app, &w.admin, "DELETE", "/api/v1/me/telegram", None).await;
    assert_eq!(out["unlinked"], false);
    assert_eq!(w.audit("telegram.admin_unlink").await, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_switches_self_link_off(pool: PgPool) {
    let w = world(pool).await;
    let s = ok(
        &w.app,
        &w.owner,
        "GET",
        "/api/v1/settings/telegram/admins",
        None,
    )
    .await;
    assert_eq!(s, json!({ "self_link": true }), "по умолчанию — да");
    let (st, _, _) = call(
        &w.app,
        "PUT",
        "/api/v1/settings/telegram/admins",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7(), "self_link": false })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // Администратор взял код, владелец выключил — код пропал той же транзакцией.
    let code = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let off = json!({ "op_id": Uuid::now_v7(), "self_link": false });
    for _ in 0..2 {
        let s = ok(
            &w.app,
            &w.owner,
            "PUT",
            "/api/v1/settings/telegram/admins",
            Some(off.clone()),
        )
        .await;
        assert_eq!(s["self_link"], false);
    }
    assert_eq!(
        w.audit("settings.telegram_admins").await,
        1,
        "повтор op_id — одна запись"
    );
    assert_eq!(start(&w.pool, CHAT, &code).await.text, WRONG_CODE);
    assert_eq!(w.linked_user(CHAT).await, None);
    // Выключено: кода нет — 403 со словами, у администратора на экране — «делает владелец».
    let (st, _, v) = call(
        &w.app,
        "POST",
        "/api/v1/me/telegram/code",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(v["error"]["message"], SELF_OFF);
    let me = ok(&w.app, &w.admin, "GET", "/api/v1/me/telegram", None).await;
    assert_eq!(me["self_link"], false);
    // Свой код, оставшийся после выключения (гонка запросов), тоже не привязывает.
    sqlx::query("insert into telegram_codes (code, user_id, expires_at, by_self) values ('424242', $1, now() + interval '15 minutes', true)")
        .bind(w.admin_id)
        .execute(&w.pool)
        .await
        .unwrap();
    assert_eq!(start(&w.pool, CHAT, "424242").await.text, WRONG_CODE);
    // Код от владельца работает по-прежнему; владелец отвязывает любого.
    let code = ok(
        &w.app,
        &w.owner,
        "POST",
        &format!("/api/v1/users/{}/telegram/code", w.admin_id),
        Some(json!({})),
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    start(&w.pool, CHAT, &code).await;
    assert_eq!(w.linked_user(CHAT).await, Some(w.admin_id));
    assert!(w.feed().await.contains("по коду от владельца"));
    ok(
        &w.app,
        &w.owner,
        "DELETE",
        &format!("/api/v1/users/{}/telegram", w.admin_id),
        None,
    )
    .await;
    assert_eq!(w.linked_user(CHAT).await, None);
    // Включил обратно — администратор снова берёт код сам.
    ok(
        &w.app,
        &w.owner,
        "PUT",
        "/api/v1/settings/telegram/admins",
        Some(json!({ "op_id": Uuid::now_v7(), "self_link": true })),
    )
    .await;
    ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn disabled_admin_gets_no_code_and_old_code_does_not_link(pool: PgPool) {
    let w = world(pool).await;
    let code = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &w.app,
        &w.owner,
        "PATCH",
        &format!("/api/v1/users/{}", w.admin_id),
        Some(json!({ "full_name": "Эрбол", "role": "admin", "active": false })),
    )
    .await;
    // Вход отключён: сессия не пускает ни к коду, ни к состоянию.
    let (st, _, _) = call(
        &w.app,
        "POST",
        "/api/v1/me/telegram/code",
        Some(&w.admin),
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await;
    assert!(
        st == StatusCode::UNAUTHORIZED || st == StatusCode::FORBIDDEN,
        "{st}"
    );
    let (st, _, _) = call(&w.app, "GET", "/api/v1/me/telegram", Some(&w.admin), None).await;
    assert!(
        st == StatusCode::UNAUTHORIZED || st == StatusCode::FORBIDDEN,
        "{st}"
    );
    // Код, взятый до отключения, не привязывает.
    assert_eq!(start(&w.pool, CHAT, &code).await.text, WRONG_CODE);
    assert_eq!(w.linked_user(CHAT).await, None);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn code_retry_after_switch_off_returns_same_code(pool: PgPool) {
    // I8: повтор того же op_id — прежний ответ, даже если владелец уже выключил настройку.
    let w = world(pool).await;
    let body = json!({ "op_id": Uuid::now_v7() });
    let first = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(body.clone()),
    )
    .await;
    ok(
        &w.app,
        &w.owner,
        "PUT",
        "/api/v1/settings/telegram/admins",
        Some(json!({ "op_id": Uuid::now_v7(), "self_link": false })),
    )
    .await;
    let again = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(body),
    )
    .await;
    assert_eq!(again, first);
    // Но сам код уже снят выключением — чат он не привяжет.
    let code = first["code"].as_str().unwrap();
    assert_eq!(start(&w.pool, CHAT, code).await.text, WRONG_CODE);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn relink_forgets_previous_owner_dialog_and_draft(pool: PgPool) {
    // I13: чат владельца перепривязали к администратору — память разговора помощника (ответы
    // владельцу с прибылью), режим вопросов и открытый черновик владельца ему не достаются.
    let w = world(pool).await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $1)",
    )
    .bind(CHAT)
    .bind(w.owner_id)
    .bind(branch)
    .execute(&w.pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into bot_dialog (id, chat_id, branch_id, role, content) values ($1, $2, $3, 'assistant', 'Прибыль за неделю: 1 000 000 с')",
    )
    .bind(Uuid::now_v7())
    .bind(CHAT)
    .bind(branch)
    .execute(&w.pool)
    .await
    .unwrap();
    sqlx::query("insert into bot_question_mode (chat_id, user_id, branch_id) values ($1, $2, $3)")
        .bind(CHAT)
        .bind(w.owner_id)
        .bind(branch)
        .execute(&w.pool)
        .await
        .unwrap();
    let draft = Uuid::now_v7();
    sqlx::query(
        "insert into bot_drafts (id, chat_id, user_id, branch_id, kind, payload, summary, op_id, expires_at) values ($1, $2, $3, $4, 'expense', '{}', 'Расход 500', $5, now() + interval '10 minutes')",
    )
    .bind(draft)
    .bind(CHAT)
    .bind(w.owner_id)
    .bind(branch)
    .bind(Uuid::now_v7())
    .execute(&w.pool)
    .await
    .unwrap();

    let code = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    start(&w.pool, CHAT, &code).await;
    assert_eq!(w.linked_user(CHAT).await, Some(w.admin_id));
    let count = |sql: &'static str| {
        let pool = w.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(CHAT)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count("select count(*) from bot_dialog where chat_id = $1").await,
        0,
        "память разговора владельца стёрта"
    );
    assert_eq!(
        count("select count(*) from bot_question_mode where chat_id = $1").await,
        0,
        "режим вопросов владельца закрыт"
    );
    let status: String = sqlx::query_scalar("select status from bot_drafts where id = $1")
        .bind(draft)
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(status, "cancelled", "открытый черновик владельца снят");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn employee_cabinet_and_login_share_one_chat(pool: PgPool) {
    // Задача 005, ответ 8а: работник уже смотрел заработок в боте по телефону, теперь у него вход —
    // в том же чате и заработок, и кнопки по его правам.
    let w = world(pool).await;
    let branch: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&w.pool)
        .await
        .unwrap();
    let employee = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master, phone) values ($1, $2, 'Эрбол', true, '+996700123456')")
        .bind(employee)
        .bind(branch)
        .execute(&w.pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_employees (chat_id, employee_id, branch_id, phone) values ($1, $2, $3, '+996700123456')")
        .bind(CHAT)
        .bind(employee)
        .bind(branch)
        .execute(&w.pool)
        .await
        .unwrap();
    let code = ok(
        &w.app,
        &w.admin,
        "POST",
        "/api/v1/me/telegram/code",
        Some(json!({ "op_id": Uuid::now_v7() })),
    )
    .await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let r = start(&w.pool, CHAT, &code).await;
    assert!(r.text.starts_with("Готово, Эрбол"), "{}", r.text);
    assert_eq!(w.linked_user(CHAT).await, Some(w.admin_id));
    let say = |text: &'static str| {
        let pool = w.pool.clone();
        async move {
            let mut conn = pool.acquire().await.unwrap();
            avtodom_server::api::telegram::handle_message(&mut conn, CHAT, CHAT, Some(text), None)
                .await
                .unwrap()
        }
    };
    // Свободный текст — меню администратора, и в нём «💼 Мой заработок».
    let r = say("привет").await;
    let Keys::Admin(menu) = r.keys else {
        panic!("не кнопки администратора: {:?} {}", r.keys, r.text)
    };
    assert!(menu.has("shift"), "{menu:?}");
    assert_eq!(menu.rows()[0], vec!["💼 Мой заработок"]);
    // «💼 Мой заработок» — заработок работника, кнопки остаются администраторские.
    let r = say("💼 Мой заработок").await;
    assert!(r.text.starts_with("💼 Сегодня"), "{}", r.text);
    assert!(matches!(r.keys, Keys::Admin(_)), "{:?}", r.keys);
    // Кнопка меню по правам работает.
    let r = say("🧾 Смена сейчас").await;
    assert!(r.text.starts_with("🧾 Смена"), "{}", r.text);
}
