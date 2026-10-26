//! Несколько филиалов (SPEC-33): администратор филиала Б не видит и не меняет ничего филиала А —
//! обход всех маршрутов API; владелец выбирает филиал; «Все филиалы» — сумма по филиалам.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, HashSet};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, bootstrap, build_state};

/// Метка данных филиала А: ни один ответ администратору Б её не содержит.
const SECRET: &str = "СЕКРЕТ-А";

struct Api {
    app: Router,
}

impl Api {
    async fn call(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        branch: Option<Uuid>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
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
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, json)
    }

    async fn ok(
        &self,
        method: &str,
        uri: &str,
        cookie: &str,
        branch: Option<Uuid>,
        body: Value,
    ) -> Value {
        let (s, v) = self.call(method, uri, cookie, branch, Some(body)).await;
        assert!(s.is_success(), "{method} {uri}: {s} {v}");
        v
    }

    async fn get(&self, uri: &str, cookie: &str, branch: Option<Uuid>) -> Value {
        let (s, v) = self.call("GET", uri, cookie, branch, None).await;
        assert!(s.is_success(), "GET {uri}: {s} {v}");
        v
    }

    async fn login(&self, login: &str, password: &str) -> String {
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/auth/login")
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
            .map(str::to_string)
            .unwrap()
    }
}

fn id(v: &Value) -> String {
    v["id"]
        .as_str()
        .unwrap_or_else(|| panic!("нет id: {v}"))
        .to_string()
}

fn op() -> Uuid {
    Uuid::now_v7()
}

/// Администратор филиала со всеми правами (SPEC-30 §5): обход проверяет филиал, а не права.
async fn admin_in(api: &Api, owner: &str, branch: Uuid, login: &str) -> String {
    let u = api
        .ok(
            "POST",
            "/api/v1/users",
            owner,
            Some(branch),
            json!({ "login": login, "password": "admin-pass-1", "full_name": format!("Админ {login}"), "role": "admin" }),
        )
        .await;
    let all: Vec<&str> = avtodom_server::auth::PERMISSIONS
        .iter()
        .map(|p| p.key)
        .collect();
    api.ok(
        "PUT",
        &format!("/api/v1/users/{}/permissions", id(&u)),
        owner,
        Some(branch),
        json!({ "op_id": op(), "permissions": all }),
    )
    .await;
    api.login(login, "admin-pass-1").await
}

struct World {
    api: Api,
    owner: String,
    a: Uuid,
    b: Uuid,
    admin_a: String,
    admin_b: String,
    item: String,
    client: String,
}

async fn world(pool: PgPool) -> World {
    bootstrap::ensure_owner(&pool, Some(("owner".into(), "owner-pass-1".into())))
        .await
        .unwrap();
    let api = Api {
        app: app(build_state(pool.clone(), false).await.unwrap(), None),
    };
    let owner = api.login("owner", "owner-pass-1").await;
    let a: Uuid = sqlx::query_scalar("select id from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    let nb = api
        .ok(
            "POST",
            "/api/v1/branches",
            &owner,
            None,
            json!({ "op_id": op(), "name": "Филиал Б", "address": "ул. Б", "phone": "0700 000 002" }),
        )
        .await;
    let b = Uuid::parse_str(&id(&nb)).unwrap();
    let admin_a = admin_in(&api, &owner, a, "admin_a").await;
    let admin_b = admin_in(&api, &owner, b, "admin_b").await;
    let cat = api
        .ok(
            "POST",
            "/api/v1/categories",
            &owner,
            Some(a),
            json!({ "name": "Фильтры", "kind": "filter" }),
        )
        .await;
    let item = id(&api
        .ok(
            "POST",
            "/api/v1/products",
            &owner,
            Some(a),
            json!({ "op_id": op(), "category_id": id(&cat), "name": "Фильтр общий", "sale_price_tyiyn": 50_000 }),
        )
        .await);
    let client = id(&api
        .ok(
            "POST",
            "/api/v1/parties",
            &owner,
            Some(a),
            json!({ "kind": "person", "first_name": "Бакыт", "last_name": "Общий", "phone": "0700 11-22-33", "inn": "21201199000123" }),
        )
        .await);
    World {
        api,
        owner,
        a,
        b,
        admin_a,
        admin_b,
        item,
        client,
    }
}

/// Документы филиала А — всех видов, что проводит точка. Возвращает поставщика.
async fn fill_branch(w: &World, branch: Uuid, admin: &str, tag: &str) -> String {
    let api = &w.api;
    let owner = &w.owner;
    let at = Some(branch);
    let sup = id(&api
        // Поставщики общие (SPEC-33): в названии нет метки филиала.
        .ok(
            "POST",
            "/api/v1/suppliers",
            owner,
            at,
            json!({ "name": format!("Поставщик {}", tag.len()) }),
        )
        .await);
    api.ok(
        "POST",
        "/api/v1/receipts",
        owner,
        at,
        json!({ "op_id": op(), "supplier_id": sup, "payment": "debt", "comment": tag,
                "lines": [{ "product_id": w.item, "qty": 50, "cost_tyiyn": 500_000 }] }),
    )
    .await;
    let cashier = id(&api
        .ok(
            "POST",
            "/api/v1/employees",
            owner,
            at,
            json!({ "full_name": format!("Кассир {tag}"), "is_cashier": true, "pin": "1234" }),
        )
        .await);
    let master = id(&api
        .ok(
            "POST",
            "/api/v1/employees",
            owner,
            at,
            json!({ "full_name": format!("Мастер {tag}"), "is_master": true }),
        )
        .await);
    api.ok(
        "POST",
        "/api/v1/payroll/accruals",
        owner,
        at,
        json!({ "op_id": op(), "employee_id": master, "kind": "bonus", "amount_tyiyn": 100_000, "comment": tag }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/shifts/open",
        admin,
        None,
        json!({ "op_id": op(), "cashier_employee_id": cashier, "cashier_pin": "1234" }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/cash/movements",
        admin,
        None,
        json!({ "op_id": op(), "kind": "cash_in", "amount_tyiyn": 1_000_000, "comment": tag }),
    )
    .await;
    let sale = id(&api
        .ok(
            "POST",
            "/api/v1/sales",
            admin,
            None,
            json!({ "op_id": op(), "sale_type": "takeaway", "cashier_id": cashier, "comment": tag,
                    "lines": [{ "kind": "piece", "product_id": w.item, "qty": 5, "unit_price_tyiyn": 50_000 }],
                    "payments": [{ "method": "cash", "amount_tyiyn": 250_000 }] }),
        )
        .await);
    api.ok(
        "POST",
        &format!("/api/v1/sales/{sale}/return"),
        admin,
        None,
        json!({ "op_id": op(), "lines": [{ "line_no": 1, "qty": 1 }],
                "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }] }),
    )
    .await;
    // Общий клиент берёт в долг в этом филиале и гасит часть.
    api.ok(
        "POST",
        "/api/v1/sales",
        admin,
        None,
        json!({ "op_id": op(), "sale_type": "takeaway", "cashier_id": cashier, "party_id": w.client,
                "comment": tag,
                "lines": [{ "kind": "piece", "product_id": w.item, "qty": 2, "unit_price_tyiyn": 50_000 }],
                "payments": [{ "method": "debt", "amount_tyiyn": 100_000 }] }),
    )
    .await;
    let articles = api.get("/api/v1/expense-articles", admin, None).await;
    let household = articles
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "Хозтовары")
        .map(id)
        .unwrap();
    api.ok(
        "POST",
        "/api/v1/expenses",
        admin,
        None,
        json!({ "op_id": op(), "article_id": household, "amount_tyiyn": 1_000, "comment": tag }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/payouts",
        admin,
        None,
        json!({ "op_id": op(), "employee_id": master, "amount_tyiyn": 1_000, "comment": tag }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/stock-ins",
        admin,
        None,
        json!({ "op_id": op(), "comment": tag,
                "lines": [{ "product_id": w.item, "qty": 1, "price_tyiyn": 10_000 }] }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/supplier-returns",
        admin,
        None,
        json!({ "op_id": op(), "supplier_id": sup, "settle": "debt", "comment": tag,
                "lines": [{ "product_id": w.item, "qty": 1 }] }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/defects",
        admin,
        None,
        json!({ "op_id": op(), "product_id": w.item, "qty": 1, "reason": tag, "no_supplier": true }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/revisions",
        admin,
        None,
        json!({ "category_id": null }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/batteries/intake",
        admin,
        None,
        json!({ "op_id": op(), "grams": 1_000, "price_per_kg_tyiyn": 10_000, "comment": tag }),
    )
    .await;
    let accounts = api.get("/api/v1/cash/accounts", owner, at).await;
    let find = |k: &str, owner_only: bool| {
        accounts
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["kind"] == k && a["owner_only"] == owner_only)
            .map(id)
            .unwrap()
    };
    api.ok(
        "POST",
        "/api/v1/cash/transfers",
        admin,
        None,
        json!({ "op_id": op(), "from_account_id": find("register", false),
                "to_account_id": find("safe", false), "amount_tyiyn": 1_000, "comment": tag }),
    )
    .await;
    api.ok(
        "POST",
        "/api/v1/expense-articles",
        owner,
        at,
        json!({ "name": format!("Статья {tag}") }),
    )
    .await;
    // Список Умага филиала (SPEC-35): строки с меткой, одна связана с общим товаром, одна пара —
    // «Не то». Коды у филиалов разные: товары общие, их коды — тоже.
    let code = |n: u32| format!("46{}{n:010}", if branch == w.a { 1 } else { 2 });
    api.ok(
        "PUT",
        "/api/v1/old-catalog",
        owner,
        at,
        json!([
            { "name": format!("Умаг {tag} связана"), "barcode": code(1), "extra_code": code(2) },
            { "name": format!("Умаг {tag} не то"), "barcode": code(3) },
            { "name": format!("Умаг {tag} свободна"), "barcode": code(4) }
        ]),
    )
    .await;
    let row = |c: String| async move {
        let hits = api
            .get(
                &format!("/api/v1/old-catalog/search?barcode={c}"),
                owner,
                at,
            )
            .await;
        id(&hits[0])
    };
    let linked = row(code(1)).await;
    api.ok(
        "POST",
        &format!("/api/v1/old-catalog/{linked}/link"),
        owner,
        at,
        json!({ "op_id": op(), "product_id": w.item }),
    )
    .await;
    let rejected = row(code(3)).await;
    api.ok(
        "POST",
        &format!("/api/v1/old-catalog/{rejected}/reject"),
        owner,
        at,
        json!({ "op_id": op(), "product_id": w.item }),
    )
    .await;
    sup
}

/// Таблицы с документами и справочниками филиала (`branch_id` и `id`); общие для филиалов —
/// клиенты, машины, книжка, баллы, услуги, подарки — не в списке.
async fn branch_tables(pool: &PgPool) -> Vec<String> {
    const SHARED: [&str; 14] = [
        "parties",
        "party_contacts",
        "party_vehicles",
        "oil_changes",
        "oil_change_items",
        "vehicle_notes",
        "vehicle_book_intervals",
        "loyalty_ledger",
        "services",
        "gift_rules",
        "users",
        "telegram_links",
        "telegram_employees",
        "client_messages",
    ];
    sqlx::query_scalar::<_, String>(
        "select c.table_name::text from information_schema.columns c
         where c.table_schema = 'public' and c.column_name = 'branch_id'
           and exists (select 1 from information_schema.columns i
                       where i.table_schema = 'public' and i.table_name = c.table_name and i.column_name = 'id')
         order by 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .filter(|t| !SHARED.contains(&t.as_str()))
    .collect()
}

/// Id всех строк филиала: по ним обход стучится в чужие документы.
async fn branch_ids(pool: &PgPool, branch: Uuid) -> BTreeMap<String, Vec<Uuid>> {
    let mut out = BTreeMap::new();
    for t in branch_tables(pool).await {
        let ids: Vec<Uuid> = sqlx::query_scalar(&format!(
            "select id from {t} where branch_id = $1 order by id"
        ))
        .bind(branch)
        .fetch_all(pool)
        .await
        .unwrap();
        if !ids.is_empty() {
            out.insert(t, ids);
        }
    }
    out
}

/// Отпечаток всех строк филиала: любая правка или новая строка его меняют.
async fn snapshot(pool: &PgPool, branch: Uuid) -> BTreeMap<String, String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "select table_name::text from information_schema.columns
         where table_schema = 'public' and column_name = 'branch_id' order by 1",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    let mut out = BTreeMap::new();
    for t in tables {
        let h: Option<String> = sqlx::query_scalar(&format!(
            "select md5(string_agg(x::text, '|' order by x::text)) from {t} x where x.branch_id = $1"
        ))
        .bind(branch)
        .fetch_one(pool)
        .await
        .unwrap();
        out.insert(t, h.unwrap_or_default());
    }
    out
}

/// Маршруты API из роутеров `src/api/*.rs`: путь и методы.
fn routes() -> Vec<(String, Vec<String>)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/api");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let mut rest = src.as_str();
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            let Some(q1) = rest.find('"') else { break };
            let after = &rest[q1 + 1..];
            let Some(q2) = after.find('"') else { break };
            let route = after[..q2].to_string();
            // Выражение обработчиков — до закрывающей скобки `.route(`.
            let mut depth = 1i32;
            let mut end = q1 + 1 + q2 + 1;
            for (i, ch) in rest[end..].char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end += i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let expr = &rest[q1 + 1 + q2 + 1..end];
            let methods: Vec<String> = ["get", "post", "put", "patch", "delete"]
                .iter()
                .filter(|m| {
                    expr.contains(&format!("routing::{m}(")) || expr.contains(&format!(".{m}("))
                })
                .map(|m| m.to_uppercase())
                .collect();
            assert!(!methods.is_empty(), "{route}: {expr}");
            out.push((route, methods));
            rest = &rest[end..];
        }
    }
    out.sort();
    out
}

/// Тело «на всё»: поля, которые принимают действия, — с id филиала А.
fn kitchen_sink(ids: &BTreeMap<String, Vec<Uuid>>, route: &str, item: &str) -> Value {
    let first = |t: &str| ids.get(t).and_then(|v| v.first()).map(Uuid::to_string);
    // Действия с документом по id — с телом, которое проходит проверку полей: тогда ответ
    // решает поиск документа, а не формат запроса.
    let special = match route {
        "/sales/{id}/return" => json!({
            "lines": [{ "line_no": 1, "qty": 1 }],
            "payments": [{ "method": "cash", "amount_tyiyn": 50_000 }],
        }),
        "/defects/{id}/resolve" => json!({ "kind": "write_off" }),
        "/revisions/{id}/lines" => json!({ "product_id": item, "counted_qty": 1 }),
        // Строка Умага по id: тело проходит проверку, ответ решает поиск строки (SPEC-35).
        "/old-catalog/{id}/link" | "/old-catalog/{id}/reject" => json!({ "product_id": item }),
        _ => json!({}),
    };
    let mut body = json!({
        "op_id": op(),
        "comment": "обход",
        "reason": "обход",
        "amount_tyiyn": 100,
        "account_id": first("cash_accounts"),
        "from_account_id": first("cash_accounts"),
        "to_account_id": ids.get("cash_accounts").and_then(|v| v.get(1)).map(Uuid::to_string),
        "employee_id": first("employees"),
        "cashier_id": first("employees"),
        "cashier_employee_id": first("employees"),
        "cashier_pin": "1234",
        "article_id": first("expense_articles"),
        "shift_id": first("shifts"),
        "receipt_id": first("receipts"),
        "sale_id": first("sales"),
        "lines": [],
        "active": false,
        "name": "обход",
        "counted_tyiyn": 0,
    });
    if let (Some(b), Some(extra)) = (body.as_object_mut(), special.as_object()) {
        for (k, v) in extra {
            b.insert(k.clone(), v.clone());
        }
    }
    body
}

/// Строки ответа, которые выдают данные филиала А.
fn leaks(body: &str, a_ids: &HashSet<String>, own: Option<&str>) -> Vec<String> {
    let mut found: Vec<String> = a_ids
        .iter()
        .filter(|x| Some(x.as_str()) != own && body.contains(x.as_str()))
        .cloned()
        .collect();
    if body.contains(SECRET) {
        found.push(SECRET.into());
    }
    found
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn admin_of_branch_b_sees_and_changes_nothing_of_branch_a(pool: PgPool) {
    let w = world(pool.clone()).await;
    fill_branch(&w, w.a, &w.admin_a, SECRET).await;
    fill_branch(&w, w.b, &w.admin_b, "Б").await;
    let ids = branch_ids(&pool, w.a).await;
    // Обход видит документы всех видов, что провели в А.
    for t in [
        "sales",
        "receipts",
        "shifts",
        "cash_movements",
        "cash_transfers",
        "cash_accounts",
        "expenses",
        "payroll_accruals",
        "payouts",
        "revisions",
        "defects",
        "stock_ins",
        "supplier_returns",
        "battery_intakes",
        "employees",
        "expense_articles",
        "party_ledger",
        "old_catalog",
    ] {
        assert!(
            ids.contains_key(t),
            "в филиале А нет строк {t}: {:?}",
            ids.keys()
        );
    }
    let a_ids: HashSet<String> = ids.values().flatten().map(Uuid::to_string).collect();
    let before = snapshot(&pool, w.a).await;
    // По одному id каждого вида и все id документов — в параметры пути.
    let mut probe: Vec<Uuid> = ids.values().filter_map(|v| v.first().copied()).collect();
    probe.sort();
    probe.dedup();
    let routes = routes();
    assert!(routes.len() > 150, "маршрутов мало: {}", routes.len());
    let mut problems = Vec::new();
    let mut requests = 0usize;
    // Вход и выход — не про данные филиала; выход закрыл бы сессию обхода.
    let skip = [
        "/auth/login",
        "/auth/logout",
        "/client/login",
        "/client/logout",
    ];
    for (route, methods) in &routes {
        if skip.contains(&route.as_str()) {
            continue;
        }
        let params = route.matches('{').count();
        for method in methods {
            let fill = |x: &str| {
                let mut uri = route.clone();
                while let Some(s) = uri.find('{') {
                    let e = uri[s..].find('}').unwrap() + s;
                    uri.replace_range(s..=e, x);
                }
                format!("/api/v1{uri}")
            };
            let uris: Vec<(String, Option<String>)> = if params == 0 {
                vec![(format!("/api/v1{route}"), None)]
            } else {
                probe
                    .iter()
                    .map(|x| (fill(&x.to_string()), Some(x.to_string())))
                    .collect()
            };
            for (who, cookie, branch) in [
                ("администратор Б", &w.admin_b, None),
                // Администратор Б шлёт заголовок филиала А — сервер его не слушает.
                ("администратор Б с заголовком А", &w.admin_b, Some(w.a)),
                ("владелец в филиале Б", &w.owner, Some(w.b)),
            ] {
                // Владелец в филиале Б: действия с документами А по id — нельзя, он проводит их,
                // выбрав филиал А; действия без id создали бы документы Б — их не шлём.
                if who.starts_with("владелец") && method != "GET" && params == 0 {
                    continue;
                }
                // Чужой документ отвечает так же, как несуществующий: тот же код ответа.
                let baseline = if params > 0 {
                    let body = (method != "GET").then(|| kitchen_sink(&ids, route, &w.item));
                    let uri = fill(&Uuid::now_v7().to_string());
                    requests += 1;
                    Some(w.api.call(method, &uri, cookie, branch, body).await.0)
                } else {
                    None
                };
                for (uri, own) in &uris {
                    let body = (method != "GET").then(|| kitchen_sink(&ids, route, &w.item));
                    let (s, v) = w.api.call(method, uri, cookie, branch, body).await;
                    requests += 1;
                    let text = v.to_string();
                    let leak = leaks(&text, &a_ids, own.as_deref());
                    if s.is_success() && !leak.is_empty() {
                        problems.push(format!("{who}: {method} {uri} → {s}, видно А: {leak:?}"));
                    }
                    if s.is_success() && method != "GET" && own.is_some() {
                        problems.push(format!(
                            "{who}: {method} {uri} → {s}: действие с документом А"
                        ));
                    }
                    if std::env::var("BRANCH_WALK_LOG").is_ok() && own.is_some() && method != "GET"
                    {
                        eprintln!(
                            "WALK {who} {method} {route} {s} {}",
                            text.chars().take(120).collect::<String>()
                        );
                    }
                    if let Some(base) = baseline
                        && base != s
                        && !(base.is_success() && s.is_success())
                    {
                        problems.push(format!(
                            "{who}: {method} {uri} → {s}, а несуществующий id → {base}: видно, что документ есть"
                        ));
                    }
                }
            }
        }
    }
    let after = snapshot(&pool, w.a).await;
    for (t, h) in &before {
        if after.get(t) != Some(h) {
            problems.push(format!("изменились строки филиала А в {t}"));
        }
    }
    assert!(
        problems.is_empty(),
        "{} нарушений из {requests} запросов:\n{}",
        problems.len(),
        problems.join("\n")
    );
    eprintln!("обход: {} маршрутов, {requests} запросов", routes.len());
}

fn num(v: &Value, path: &[&str]) -> i64 {
    // «rows.поле» — сумма поля по строкам списка.
    if let [list, field] = path
        && let Some(f) = field.strip_prefix("*")
    {
        return v[*list]
            .as_array()
            .unwrap_or_else(|| panic!("нет списка {list}: {v}"))
            .iter()
            .map(|r| r[f].as_i64().unwrap_or(0))
            .sum();
    }
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_i64()
        .unwrap_or_else(|| panic!("нет числа {path:?}: {v}"))
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_reports_all_branches_as_the_sum_of_branches(pool: PgPool) {
    let w = world(pool.clone()).await;
    fill_branch(&w, w.a, &w.admin_a, SECRET).await;
    fill_branch(&w, w.b, &w.admin_b, "Б").await;
    let api = &w.api;
    for (uri, path) in [
        ("/api/v1/owner/dashboard", vec!["totals", "goods_tyiyn"]),
        ("/api/v1/reports/profit", vec!["totals", "net_tyiyn"]),
        ("/api/v1/reports/income", vec!["goods_tyiyn"]),
        ("/api/v1/payroll/month", vec!["rows", "*accrued_tyiyn"]),
        ("/api/v1/products/list", vec!["totals", "sale_tyiyn"]),
    ] {
        let a = api.get(uri, &w.owner, Some(w.a)).await;
        let b = api.get(uri, &w.owner, Some(w.b)).await;
        let sep = if uri.contains('?') { '&' } else { '?' };
        let all = api
            .get(&format!("{uri}{sep}branch=all"), &w.owner, Some(w.b))
            .await;
        assert_eq!(
            num(&all, &path),
            num(&a, &path) + num(&b, &path),
            "{uri}: {all}"
        );
        assert_eq!(all["branches"].as_array().unwrap().len(), 2, "{uri}");
        assert_eq!(all["all_branches"], true);
        // Администратору «Все филиалы» нет — как чужих данных.
        let (s, _) = api
            .call(
                "GET",
                &format!("{uri}{sep}branch=all"),
                &w.admin_b,
                None,
                None,
            )
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
    }
    // Остаток товара во всех филиалах — сумма остатков филиалов.
    let stock = |v: &Value| {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == w.item.as_str())
            .map(|r| r["stock_qty"].as_i64().unwrap())
            .unwrap()
    };
    let a = api.get("/api/v1/products/list", &w.owner, Some(w.a)).await;
    let b = api.get("/api/v1/products/list", &w.owner, Some(w.b)).await;
    let all = api
        .get("/api/v1/products/list?branch=all", &w.owner, None)
        .await;
    assert_eq!(stock(&all), stock(&a) + stock(&b));
    // Долги клиента: в каждом филиале — свой, у клиента — общий.
    let debts = api.get("/api/v1/debts/list", &w.admin_b, None).await;
    let row = debts
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == w.client.as_str())
        .unwrap();
    assert_eq!(row["branch_balance_tyiyn"], 100_000);
    assert_eq!(row["balance_tyiyn"], 200_000);
    assert_eq!(row["branch_balances"].as_array().unwrap().len(), 2);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shared_price_and_own_branch_price(pool: PgPool) {
    let w = world(pool.clone()).await;
    let api = &w.api;
    let price = |v: &Value| {
        (
            v["sale_price_tyiyn"].as_i64().unwrap(),
            v["own_price"].as_bool().unwrap(),
        )
    };
    let uri = format!("/api/v1/products/{}", w.item);
    let prices = format!("/api/v1/products/{}/prices", w.item);
    assert_eq!(
        price(&api.get(&uri, &w.admin_b, None).await),
        (50_000, false)
    );
    // Своя цена филиала Б.
    api.ok(
        "PATCH",
        &prices,
        &w.owner,
        Some(w.b),
        json!({ "sale_price_tyiyn": 60_000, "own_price": true }),
    )
    .await;
    // Общая цена меняется в А и во всех без своей; своя цена Б остаётся.
    api.ok(
        "PATCH",
        &prices,
        &w.owner,
        Some(w.a),
        json!({ "sale_price_tyiyn": 55_000 }),
    )
    .await;
    let c = Uuid::parse_str(&id(&api
        .ok(
            "POST",
            "/api/v1/branches",
            &w.owner,
            None,
            json!({ "op_id": op(), "name": "Филиал В" }),
        )
        .await))
    .unwrap();
    assert_eq!(
        price(&api.get(&uri, &w.owner, Some(w.a)).await),
        (55_000, false)
    );
    assert_eq!(
        price(&api.get(&uri, &w.owner, Some(w.b)).await),
        (60_000, true)
    );
    assert_eq!(
        price(&api.get(&uri, &w.owner, Some(c)).await),
        (55_000, false)
    );
    // Своя цена правится там, где она есть: правка в Б не трогает общую.
    api.ok(
        "PATCH",
        &prices,
        &w.owner,
        Some(w.b),
        json!({ "sale_price_tyiyn": 61_000 }),
    )
    .await;
    assert_eq!(
        price(&api.get(&uri, &w.owner, Some(w.a)).await),
        (55_000, false)
    );
    // «Вернуть общую».
    api.ok(
        "PATCH",
        &prices,
        &w.owner,
        Some(w.b),
        json!({ "own_price": false }),
    )
    .await;
    let back = api.get(&uri, &w.owner, Some(w.b)).await;
    assert_eq!(price(&back), (55_000, false));
    assert_eq!(back["shared_sale_price_tyiyn"], 55_000);
    // Свою цену ставит только владелец.
    let (s, _) = api
        .call(
            "PATCH",
            &prices,
            &w.admin_b,
            None,
            Some(json!({ "own_price": true })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn client_debt_repaid_in_another_branch_is_settled_between_branches(pool: PgPool) {
    let w = world(pool.clone()).await;
    fill_branch(&w, w.a, &w.admin_a, SECRET).await;
    fill_branch(&w, w.b, &w.admin_b, "Б").await;
    let api = &w.api;
    let card = |v: &Value| {
        let mut parts: Vec<(String, i64)> = v["branch_balances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["name"].as_str().unwrap().to_string(),
                    b["balance_tyiyn"].as_i64().unwrap(),
                )
            })
            .collect();
        parts.sort();
        (v["balance_tyiyn"].as_i64().unwrap(), parts)
    };
    let party = format!("/api/v1/parties/{}", w.client);
    assert_eq!(
        card(&api.get(&party, &w.admin_b, None).await),
        (
            200_000,
            vec![("Основной".into(), 100_000), ("Филиал Б".into(), 100_000)]
        )
    );
    // Клиент гасит 150 000 в Б: свой долг Б и 50 000 долга А — деньги в кассе Б.
    let r = api
        .ok(
            "POST",
            "/api/v1/debts/repayments",
            &w.admin_b,
            None,
            json!({ "op_id": op(), "party_id": w.client, "amount_tyiyn": 150_000, "method": "cash" }),
        )
        .await;
    assert_eq!(r["balance_tyiyn"], 50_000);
    assert_eq!(
        card(&api.get(&party, &w.admin_a, None).await),
        (50_000, vec![("Основной".into(), 50_000)])
    );
    let owed: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from branch_settlements
         where branch_id = $1 and creditor_branch_id = $2",
    )
    .bind(w.b)
    .bind(w.a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(owed, 50_000);
    let s = api
        .get("/api/v1/branches/settlements", &w.owner, None)
        .await;
    assert_eq!(s["items"][0]["amount_tyiyn"], 50_000, "{s}");
    // Администратору расчёты между филиалами не показываются.
    let (st, _) = api
        .call(
            "GET",
            "/api/v1/branches/settlements",
            &w.admin_b,
            None,
            None,
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    // Сторно погашения владельцем в Б снимает обе части и расчёт.
    let ledger = r["ledger_id"].as_str().unwrap();
    api.ok(
        "POST",
        &format!("/api/v1/debts/ledger/{ledger}/reverse"),
        &w.owner,
        Some(w.b),
        json!({ "op_id": op(), "comment": "ошибка" }),
    )
    .await;
    assert_eq!(
        card(&api.get(&party, &w.admin_b, None).await),
        (
            200_000,
            vec![("Основной".into(), 100_000), ("Филиал Б".into(), 100_000)]
        )
    );
    let owed: i64 =
        sqlx::query_scalar("select coalesce(sum(amount_tyiyn), 0)::bigint from branch_settlements")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(owed, 0);
    // Остатки балансов по филиалам — сумма записей долга (инвариант 3).
    let bad: i64 = sqlx::query_scalar(
        "select count(*) from party_balances b
         where b.balance_tyiyn <> (select coalesce(sum(l.amount_tyiyn), 0) from party_ledger l
                                   where l.party_id = b.party_id and l.branch_id = b.branch_id)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(bad, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn new_branch_gets_own_accounts_settings_numbers_and_closed_branch_does_not_sell(
    pool: PgPool,
) {
    let w = world(pool.clone()).await;
    let api = &w.api;
    api.ok(
        "PUT",
        "/api/v1/settings/stock",
        &w.owner,
        Some(w.a),
        json!({ "stale_days": 45 }),
    )
    .await;
    let c = Uuid::parse_str(&id(&api
        .ok(
            "POST",
            "/api/v1/branches",
            &w.owner,
            None,
            json!({ "op_id": op(), "name": "Филиал В", "address": "ул. В" }),
        )
        .await))
    .unwrap();
    // Свои кассы и счета.
    let accounts = api.get("/api/v1/cash/accounts", &w.owner, Some(c)).await;
    let names: Vec<&str> = accounts
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"Касса") && names.contains(&"Счёт"),
        "{names:?}"
    );
    let a_accounts = api.get("/api/v1/cash/accounts", &w.owner, Some(w.a)).await;
    assert!(accounts.as_array().unwrap().iter().all(|x| {
        a_accounts
            .as_array()
            .unwrap()
            .iter()
            .all(|y| y["id"] != x["id"])
    }));
    // Настройки — копия главного.
    let st = api.get("/api/v1/settings/stock", &w.owner, Some(c)).await;
    assert_eq!(st["stale_days"], 45);
    // Своя нумерация: первый чек филиала — № 1.
    let cashier = id(&api
        .ok(
            "POST",
            "/api/v1/employees",
            &w.owner,
            Some(c),
            json!({ "full_name": "Кассир В", "is_cashier": true, "pin": "1234" }),
        )
        .await);
    let sale = |branch: Uuid| {
        json!({ "op_id": op(), "sale_type": "takeaway", "cashier_id": cashier, "branch": branch,
                "lines": [{ "kind": "piece", "product_id": w.item, "qty": 1, "unit_price_tyiyn": 50_000 }],
                "payments": [{ "method": "card", "amount_tyiyn": 50_000 }] })
    };
    let first = api
        .ok("POST", "/api/v1/sales", &w.owner, Some(c), sale(c))
        .await;
    assert_eq!(first["number"], 1);
    // Список филиалов: администратор видит только свой.
    let mine = api.get("/api/v1/branches", &w.admin_b, None).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
    assert_eq!(mine[0]["id"], w.b.to_string());
    let all = api.get("/api/v1/branches", &w.owner, None).await;
    assert_eq!(all.as_array().unwrap().len(), 3);
    // Закрытый филиал не продаёт, история остаётся; главный не закрывается.
    api.ok(
        "PATCH",
        &format!("/api/v1/branches/{c}"),
        &w.owner,
        None,
        json!({ "archived": true }),
    )
    .await;
    let (s, _) = api
        .call("POST", "/api/v1/sales", &w.owner, Some(c), Some(sale(c)))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let doc = api
        .get(&format!("/api/v1/sales/{}", id(&first)), &w.owner, Some(c))
        .await;
    assert_eq!(doc["number"], 1);
    let (s, _) = api
        .call(
            "PATCH",
            &format!("/api/v1/branches/{}", w.a),
            &w.owner,
            None,
            Some(json!({ "archived": true })),
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    // Неизвестный филиал в заголовке — не молча в другой филиал.
    let (s, _) = api
        .call(
            "GET",
            "/api/v1/cash/accounts",
            &w.owner,
            Some(Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[sqlx::test(migrations = false)]
async fn migration_makes_one_main_branch_and_keeps_everything(pool: PgPool) {
    // Миграция 0062 на базе с данными: один главный филиал, общая цена — цена филиала, долг
    // по филиалам — сумма записей долга, ничего не теряется.
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 62) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let (branch, user, cat, item, party) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "insert into branches (id, name) values ('{branch}', 'Основной');
         insert into users (id, branch_id, login, password_hash, role, full_name)
           values ('{user}', '{branch}', 'owner', 'x', 'owner', 'Владелец');
         insert into categories (id, name, kind) values ('{cat}', 'Фильтры', 'filter');
         insert into products (id, category_id, name, unit) values ('{item}', '{cat}', 'Фильтр', 'piece');
         insert into branch_products (branch_id, product_id, sale_price_tyiyn, stock_qty, stock_value_tyiyn)
           values ('{branch}', '{item}', 45000, 3, 60000);
         insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
           values (gen_random_uuid(), '{branch}', '{item}', 3, 60000, 'stock_in', gen_random_uuid());
         insert into parties (id, branch_id, role, kind, name, balance_tyiyn)
           values ('{party}', '{branch}', 'customer', 'person', 'Клиент', 70000);
         insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date, user_id, device_id)
           values (gen_random_uuid(), '{branch}', '{party}', 'debt', 70000, current_date, '{user}', gen_random_uuid());"
    ))
    .execute(&pool)
    .await
    .unwrap();
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version >= 62) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let main: (bool, bool) = sqlx::query_as("select is_main, archived from branches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(main, (true, false));
    let price: (i64, i64, bool) = sqlx::query_as(
        "select p.sale_price_tyiyn, bp.sale_price_tyiyn, bp.own_price
         from products p join branch_products bp on bp.product_id = p.id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(price, (45_000, 45_000, false));
    let stock: (i64, i64) = sqlx::query_as(
        "select bp.stock_qty, (select sum(qty_delta)::bigint from stock_movements) from branch_products bp",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stock, (3, 3));
    let bal: (Uuid, i64) = sqlx::query_as("select branch_id, balance_tyiyn from party_balances")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(bal, (branch, 70_000));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn owner_bot_feed_and_reports_name_the_branch(pool: PgPool) {
    use avtodom_server::api::telegram::{handle_message, pending};
    let w = world(pool.clone()).await;
    let owner_id: Uuid = sqlx::query_scalar("select id from users where login = 'owner'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let chat = 770_001i64;
    sqlx::query(
        "insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $1)",
    )
    .bind(chat)
    .bind(owner_id)
    .bind(w.a)
    .execute(&pool)
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let mut say = async |text: &str| {
        handle_message(&mut conn, chat, chat, Some(text), None)
            .await
            .unwrap()
            .text
    };
    // Лента: первый проход ставит курсор.
    assert!(say("/филиал").await.contains("/филиал 2 — Филиал Б"));
    let today = say("📊 Сегодня").await;
    assert!(today.starts_with("🏬 Основной"), "{today}");
    assert_eq!(say("/филиал 2").await, "🏬 Филиал: Филиал Б");
    assert!(say("📊 Сегодня").await.starts_with("🏬 Филиал Б"));
    say("/филиал все").await;
    let all = say("📊 Сегодня").await;
    assert!(
        all.contains("🏬 Основной") && all.contains("🏬 Филиал Б"),
        "{all}"
    );
    drop(say);
    let mut c2 = pool.acquire().await.unwrap();
    assert!(pending(&mut c2).await.unwrap().is_empty());
    // Событие филиала Б уходит владельцу с названием филиала.
    sqlx::query(
        "insert into audit_log (id, branch_id, user_id, action, entity, data) values ($1, $2, $3, 'receipt.reverse', 'receipt', '{\"number\": 7}')",
    )
    .bind(Uuid::now_v7())
    .bind(w.b)
    .bind(owner_id)
    .execute(&pool)
    .await
    .unwrap();
    let msgs = pending(&mut c2).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].text.starts_with("🏬 Филиал Б"), "{}", msgs[0].text);
}
