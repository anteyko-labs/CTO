//! Задача 002, волна 2: баллы по товару, категории и общей ставке (П1, ADR-{B1.2}); единица
//! и объём канистры товара (П4; задача 003, Н10; ADR-{B1.1}).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::api::loyalty::balance;
use avtodom_server::api::product_units::{UnitReq, change_unit_tx};
use avtodom_server::api::receipts::{
    ReceiptLineReq, ReceiptReq, ReverseReq, post_receipt_tx, reverse_receipt_tx, verify_stock_tx,
};
use avtodom_server::api::sales::{
    PaymentReq, ReturnLineReq, ReturnReq, SaleLineReq, SaleReq, post_sale_tx, return_sale_tx,
};
use avtodom_server::auth::{Ctx, CurrentUser, Role};
use avtodom_server::error::AppError;
use avtodom_server::{app, auth::hash_password, bootstrap, build_state};

struct World {
    owner: Ctx,
    admin: Ctx,
    cashier: Uuid,
    master: Uuid,
    service: Uuid,
    cat_filter: Uuid,
    filter: Uuid,
    oil: Uuid,
}

async fn seed(pool: &PgPool) -> World {
    let branch = Uuid::now_v7();
    sqlx::query("insert into branches (id, name, is_main) values ($1, 'Тест', true)")
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    let mut users = vec![];
    for (login, role) in [("owner", "owner"), ("admin", "admin")] {
        let id = Uuid::now_v7();
        sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, 'x', $4, $3)")
            .bind(id)
            .bind(branch)
            .bind(login)
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
        if role == "admin" {
            let keys: Vec<String> = avtodom_server::auth::default_permissions()
                .into_iter()
                .map(String::from)
                .collect();
            sqlx::query(
                "insert into user_permissions (user_id, branch_id, permission) select $1, $2, unnest($3::text[])",
            )
            .bind(id)
            .bind(branch)
            .bind(keys)
            .execute(pool)
            .await
            .unwrap();
        }
        users.push(Ctx {
            user: CurrentUser {
                id,
                branch_id: branch,
                login: login.into(),
                full_name: login.into(),
                role: Role::parse(role).unwrap(),
            },
            device_id: Uuid::now_v7(),
            bot_chat: None,
        });
    }
    let (cashier, master, service) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into employees (id, branch_id, full_name, is_cashier) values ($1, $2, 'Кассир', true)")
        .bind(cashier)
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master) values ($1, $2, 'Мастер', true)")
        .bind(master)
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into services (id, branch_id, name, price_tyiyn, free_with_oil, book_slot) values ($1, $2, 'Замена масла', 20000, true, 'oil')")
        .bind(service)
        .bind(branch)
        .execute(pool)
        .await
        .unwrap();
    let (cat_f, cat_o, filter, oil) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("insert into categories (id, name, kind) values ($1, 'Фильтры', 'filter'), ($2, 'Масла', 'oil')")
        .bind(cat_f)
        .bind(cat_o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("update services set oil_category_id = $2 where id = $1")
        .bind(service)
        .bind(cat_o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit) values ($1, $2, 'Фильтр W712', 'piece')")
        .bind(filter)
        .bind(cat_f)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit, container_ml, sale_price_tyiyn) values ($1, $2, 'Масло 5W-30 4л', 'ml', 4000, 200000)")
        .bind(oil)
        .bind(cat_o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into branch_products (branch_id, product_id, sale_price_tyiyn) values ($1, $2, 50000), ($1, $3, 200000)",
    )
    .bind(branch)
    .bind(filter)
    .bind(oil)
    .execute(pool)
    .await
    .unwrap();
    let mut it = users.into_iter();
    World {
        owner: it.next().unwrap(),
        admin: it.next().unwrap(),
        cashier,
        master,
        service,
        cat_filter: cat_f,
        filter,
        oil,
    }
}

async fn owner_safe(pool: &PgPool, branch: Uuid) -> Uuid {
    let mut conn = pool.acquire().await.unwrap();
    avtodom_server::api::cash::ensure_accounts(&mut conn, branch)
        .await
        .unwrap();
    sqlx::query_scalar(
        "select id from cash_accounts where branch_id = $1 and name = 'Сейф владельца'",
    )
    .bind(branch)
    .fetch_one(&mut *conn)
    .await
    .unwrap()
}

async fn receive(pool: &PgPool, w: &World, product: Uuid, qty: i64, cost: i64) -> Uuid {
    let safe = owner_safe(pool, w.owner.user.branch_id).await;
    let mut tx = pool.begin().await.unwrap();
    let r = post_receipt_tx(
        &mut tx,
        &w.owner,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            payment: None,
            paid_from: Some(safe.to_string()),
            paid_now_tyiyn: None,
            supplier_id: None,
            supplier_doc: String::new(),
            comment: String::new(),
            lines: vec![ReceiptLineReq {
                product_id: product,
                qty,
                cost_tyiyn: cost,
                sale_price_tyiyn: None,
            }],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    r.id
}

async fn open_till(pool: &PgPool, w: &World) {
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn)
         values ($1, $2, 1, (now() at time zone 'Asia/Bishkek')::date, $3, $4, $5, $1, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(till)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&mut *conn)
    .await
    .unwrap();
}

fn line(kind: &str, product: Uuid, qty: i64, price: i64) -> SaleLineReq {
    SaleLineReq {
        kind: kind.into(),
        gift: false,
        product_id: Some(product),
        service_id: None,
        qty,
        unit_price_tyiyn: price,
        seen_list_price_tyiyn: None,
    }
}

fn service_line(service: Uuid) -> SaleLineReq {
    SaleLineReq {
        kind: "service".into(),
        gift: false,
        product_id: None,
        service_id: Some(service),
        qty: 1,
        unit_price_tyiyn: 0,
        seen_list_price_tyiyn: None,
    }
}

fn cash(amount: i64) -> Vec<PaymentReq> {
    vec![PaymentReq {
        account_id: None,
        bank_qr_id: None,
        price_per_kg_tyiyn: None,
        method: "cash".into(),
        amount_tyiyn: amount,
        grams: None,
    }]
}

fn takeaway(w: &World, party: Option<Uuid>, lines: Vec<SaleLineReq>, total: i64) -> SaleReq {
    SaleReq {
        op_id: Uuid::now_v7(),
        client_time: None,
        sale_type: "takeaway".into(),
        cashier_id: w.cashier,
        master_id: None,
        party_id: party,
        contact_id: None,
        vehicle_id: None,
        comment: String::new(),
        lines,
        payments: cash(total),
        offline: false,
        claimed_user_id: None,
        mileage_km: None,
        mileage: None,
        mileage_unit: None,
        book_intervals: Vec::new(),
        next_changes: Vec::new(),
        delivery_address: String::new(),
        declined: String::new(),
        removed_lines: serde_json::Value::Null,
        correction: None,
    }
}

async fn sell(
    pool: &PgPool,
    ctx: &Ctx,
    req: SaleReq,
) -> Result<avtodom_server::api::sales::SaleOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = post_sale_tx(&mut tx, ctx, req).await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

async fn give_back(
    pool: &PgPool,
    w: &World,
    sale: Uuid,
    line_no: i32,
    qty: i64,
    money: i64,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = return_sale_tx(
        &mut tx,
        &w.owner,
        sale,
        ReturnReq {
            op_id: Uuid::now_v7(),
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no, qty }],
            payments: cash(money),
            business_date: None,
        },
    )
    .await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r.map(|_| ())
}

/// Клиент из бота с машиной: чек «В сервис» с заменой масла даёт баллы.
async fn member(pool: &PgPool, w: &World) -> (Uuid, Uuid) {
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name, first_name, last_name, phone) values ($1, $2, 'customer', 'person', 'Асанов Бакыт', 'Бакыт', 'Асанов', '0700112233')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) values ($1, $2, $3, '01KG900XYZ', '01KG900XYZ')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into telegram_customers (chat_id, party_id, phone) values (555, $1, '0700112233')",
    )
    .bind(party)
    .execute(pool)
    .await
    .unwrap();
    (party, vehicle)
}

fn oil_change(w: &World, (party, vehicle): (Uuid, Uuid), filters: i64) -> SaleReq {
    let mut lines = vec![];
    if filters > 0 {
        lines.push(line("piece", w.filter, filters, 50_000));
    }
    lines.push(line("container", w.oil, 1, 200_000));
    lines.push(service_line(w.service));
    let mut req = takeaway(w, Some(party), lines, 50_000 * filters + 200_000);
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.vehicle_id = Some(vehicle);
    req.mileage_km = Some(85_000);
    req
}

/// Строки начисления по чеку: (строка, баллы, откуда правило).
async fn accrued_lines(pool: &PgPool, sale: Uuid) -> Vec<(i32, i64, String)> {
    sqlx::query_as(
        "select line_no, amount_tyiyn, source from loyalty_lines where sale_id = $1 and amount_tyiyn > 0 order by line_no",
    )
    .bind(sale)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn points_of(pool: &PgPool, party: Uuid) -> i64 {
    let mut conn = pool.acquire().await.unwrap();
    balance(&mut conn, party).await.unwrap()
}

async fn loyalty_settings(pool: &PgPool, w: &World, oil_change_only: bool) {
    sqlx::query("insert into settings (branch_id, key, value) values ($1, 'loyalty', $2) on conflict (branch_id, key) do update set value = excluded.value")
        .bind(w.owner.user.branch_id)
        .bind(json!({ "rate_bp": 50, "oil_change_only": oil_change_only }))
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn points_follow_product_then_category_then_common(pool: PgPool) {
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    receive(&pool, &w, w.filter, 30, 30_000).await;
    receive(&pool, &w, w.oil, 40_000, 1_000_000).await;
    let client = member(&pool, &w).await;
    let party = client.0;

    // По умолчанию — как раньше: 0,5 % с 2500 с — 12,50, по строкам общей ставкой.
    let first = sell(&pool, &w.owner, oil_change(&w, client, 1))
        .await
        .unwrap();
    assert_eq!(points_of(&pool, party).await, 1_250);
    assert_eq!(
        accrued_lines(&pool, first.id).await,
        vec![(1, 250, "common".into()), (2, 1_000, "common".into())]
    );

    // Категория фильтров — 2 %, масло — 10 с за литр: 10 с + 40 с.
    sqlx::query("update categories set points_mode = 'percent', points_value = 200 where id = $1")
        .bind(w.cat_filter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update products set points_mode = 'fixed', points_value = 1000 where id = $1")
        .bind(w.oil)
        .execute(&pool)
        .await
        .unwrap();
    let second = sell(&pool, &w.owner, oil_change(&w, client, 1))
        .await
        .unwrap();
    assert_eq!(
        accrued_lines(&pool, second.id).await,
        vec![(1, 1_000, "category".into()), (2, 4_000, "product".into())]
    );
    assert_eq!(points_of(&pool, party).await, 6_250);

    // Товар «нет» — сильнее категории.
    sqlx::query("update products set points_mode = 'none' where id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    let third = sell(&pool, &w.owner, oil_change(&w, client, 1))
        .await
        .unwrap();
    assert_eq!(
        accrued_lines(&pool, third.id).await,
        vec![(2, 4_000, "product".into())]
    );
    sqlx::query("update products set points_mode = 'inherit' where id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(points_of(&pool, party).await, 10_250);

    // «Только в чеке с заменой масла» выключен — покупка на вынос тоже даёт баллы; включён — нет.
    let to_go = || {
        takeaway(
            &w,
            Some(party),
            vec![line("piece", w.filter, 2, 50_000)],
            100_000,
        )
    };
    sell(&pool, &w.owner, to_go()).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 10_250);
    loyalty_settings(&pool, &w, false).await;
    let bought = sell(&pool, &w.owner, to_go()).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 12_250);
    loyalty_settings(&pool, &w, true).await;
    sell(&pool, &w.owner, to_go()).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 12_250);

    // Возврат строки снимает её баллы: масло второго чека — 40 с, фильтр — 10 с. Замена без
    // нашего масла стала платной — её цена удерживается из возврата (ADR-104).
    give_back(&pool, &w, second.id, 2, 1, 180_000)
        .await
        .unwrap();
    assert_eq!(points_of(&pool, party).await, 8_250);
    give_back(&pool, &w, second.id, 1, 1, 50_000).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 7_250);
    // По частям: 2 фильтра по 2 % — 20 с; одна штука — 10 с, вторая — остаток.
    give_back(&pool, &w, bought.id, 1, 1, 50_000).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 6_250);
    give_back(&pool, &w, bought.id, 1, 1, 50_000).await.unwrap();
    assert_eq!(points_of(&pool, party).await, 5_250);
    let taken: i64 = sqlx::query_scalar(
        "select coalesce(-sum(amount_tyiyn), 0)::bigint from loyalty_lines where sale_id = $1 and amount_tyiyn < 0",
    )
    .bind(second.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(taken, 5_000, "снято ровно начисленное по строкам");

    // Клиенту в бот — за что начислено.
    let mut conn = pool.acquire().await.unwrap();
    let notices = avtodom_server::api::telegram::bonus_notices(&mut conn)
        .await
        .unwrap();
    let texts: Vec<&str> = notices.iter().map(|n| n.text.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("начислено 50 за Фильтр W712, Масло 5W-30 4л")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("начислено 12,50 за Фильтр W712, Масло 5W-30 4л")),
        "{texts:?}"
    );
    // Выключатель — и в тексте бота.
    loyalty_settings(&pool, &w, false).await;
    assert_eq!(
        avtodom_server::api::telegram::bonus_rule(&mut conn)
            .await
            .unwrap(),
        "За покупки — бонусные баллы."
    );
}

fn unit_req(unit: &str, container_ml: Option<i64>, piece_ml: Option<i64>) -> UnitReq {
    UnitReq {
        op_id: Uuid::now_v7(),
        unit: unit.into(),
        container_ml,
        piece_ml,
        keep: None,
    }
}

async fn change(
    pool: &PgPool,
    ctx: &Ctx,
    id: Uuid,
    req: UnitReq,
) -> Result<avtodom_server::api::product_units::UnitOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = change_unit_tx(&mut tx, ctx, id, req).await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

async fn stock(pool: &PgPool, product: Uuid) -> (i64, i64, i64) {
    sqlx::query_as("select stock_qty, stock_value_tyiyn, sale_price_tyiyn from branch_products where product_id = $1")
        .bind(product)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn assert_consistent(pool: &PgPool, w: &World) {
    let mut conn = pool.acquire().await.unwrap();
    let diff = verify_stock_tx(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    assert!(diff.is_empty(), "остатки расходятся с движениями");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn unit_and_canister_change_recount_stock_in_one_operation(pool: PgPool) {
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    // Три канистры по 4 л за 6000 с.
    let receipt = receive(&pool, &w, w.oil, 12_000, 600_000).await;

    // Администратору — нельзя.
    assert!(matches!(
        change(&pool, &w.admin, w.oil, unit_req("ml", Some(5_000), None)).await,
        Err(AppError::Forbidden)
    ));
    // Идёт ревизия — сначала её закончить.
    let rev = Uuid::now_v7();
    sqlx::query("insert into revisions (id, branch_id, number, status, user_id, device_id) values ($1, $2, 1, 'draft', $3, $1)")
        .bind(rev)
        .bind(w.owner.user.branch_id)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        change(&pool, &w.owner, w.oil, unit_req("ml", Some(5_000), None)).await,
        Err(AppError::Conflict(_))
    ));
    sqlx::query("update revisions set status = 'cancelled' where id = $1")
        .bind(rev)
        .execute(&pool)
        .await
        .unwrap();

    // Объём канистры 4 л → 5 л, канистр столько же: 12 л → 15 л той же стоимостью.
    let req = unit_req("ml", Some(5_000), None);
    let out = change(&pool, &w.owner, w.oil, req.clone()).await.unwrap();
    assert_eq!(out.branches[0].from_qty, 12_000);
    assert_eq!(out.branches[0].to_qty, 15_000);
    assert_eq!(out.product.container_ml, Some(5_000));
    assert_eq!(stock(&pool, w.oil).await, (15_000, 600_000, 200_000));
    // Повтор того же op_id — тот же ответ, без второго пересчёта (I8).
    let again = change(&pool, &w.owner, w.oil, req).await.unwrap();
    assert_eq!(again.branches[0].to_qty, 15_000);
    let changes: i64 =
        sqlx::query_scalar("select count(*) from product_unit_changes where product_id = $1")
            .bind(w.oil)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(changes, 1);
    let moves: i64 = sqlx::query_scalar(
        "select count(*) from stock_movements where product_id = $1 and doc_type = 'unit_change'",
    )
    .bind(w.oil)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(moves, 2, "списание и приход");
    assert_consistent(&pool, &w).await;

    // Продали 0,5 л розливом — остаток 14,5 л не делится на штуки по 5 л: 422 с подсказкой.
    let poured = sell(
        &pool,
        &w.owner,
        takeaway(&w, None, vec![line("pour", w.oil, 500, 40_000)], 20_000),
    )
    .await
    .unwrap();
    match change(&pool, &w.owner, w.oil, unit_req("piece", None, Some(5_000))).await {
        Err(AppError::Validation(m)) => assert!(m.contains("14,5 л"), "{m}"),
        other => panic!("{:?}", other.map(|o| o.branches)),
    }
    // Штуки по 0,5 л: 29 шт той же стоимостью, цена — за штуку (2000 с за 5 л → 200 с).
    let out = change(&pool, &w.owner, w.oil, unit_req("piece", None, Some(500)))
        .await
        .unwrap();
    assert_eq!(out.product.unit, "piece");
    assert_eq!(out.product.container_ml, None);
    let (qty, value, price) = stock(&pool, w.oil).await;
    assert_eq!((qty, price), (29, 20_000));
    assert!(value > 0);
    // История продаж — в прежних единицах; возврат старого розлива ложится на склад штукой.
    let unit_of_line: i64 = sqlx::query_scalar("select units from sale_lines where sale_id = $1")
        .bind(poured.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(unit_of_line, 500);
    give_back(&pool, &w, poured.id, 1, 500, 20_000)
        .await
        .unwrap();
    assert_eq!(stock(&pool, w.oil).await.0, 30);
    assert_consistent(&pool, &w).await;
    // Приход до пересчёта не сторнируется: его количества в прежних единицах.
    let mut tx = pool.begin().await.unwrap();
    match reverse_receipt_tx(
        &mut tx,
        &w.owner,
        receipt,
        ReverseReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            in_correction: false,
        },
    )
    .await
    {
        Err(AppError::Conflict(m)) => assert!(m.contains("изменилась единица"), "{m}"),
        other => panic!("{:?}", other.map(|r| r.number)),
    }
    drop(tx);

    // Обратно в литры: штука — 0,5 л, канистра 4 л: 30 шт → 15 л, цена канистры — 8 штук.
    let out = change(
        &pool,
        &w.owner,
        w.oil,
        unit_req("ml", Some(4_000), Some(500)),
    )
    .await
    .unwrap();
    assert_eq!(out.branches[0].to_qty, 15_000);
    assert_eq!(stock(&pool, w.oil).await.2, 160_000);
    assert_consistent(&pool, &w).await;

    // Товар без движений меняется сразу: фильтр в литрах.
    let out = change(&pool, &w.owner, w.filter, unit_req("ml", Some(1_000), None))
        .await
        .unwrap();
    assert_eq!(out.product.unit, "ml");
    let moves: i64 =
        sqlx::query_scalar("select count(*) from stock_movements where product_id = $1")
            .bind(w.filter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(moves, 0);
    // Журнал — что было и что стало.
    let logged: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'product.unit_change'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(logged, 4);
}

// ---------- По HTTP: права и настройка ----------

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
    (
        status,
        cookie,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn points_and_units_are_owner_only(pool: PgPool) {
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
    let app = app(build_state(pool.clone(), false).await.unwrap(), None);
    let owner = login(&app, "owner", "owner-pass-1").await;
    let admin = login(&app, "admin", "admin-pass-1").await;
    let (s, _, cat) = call(
        &app,
        "POST",
        "/api/v1/categories",
        Some(&owner),
        Some(json!({ "name": "Антифриз", "kind": "other" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{cat}");
    assert_eq!(cat["points_mode"], "inherit");
    let cat_id = cat["id"].as_str().unwrap().to_string();
    // Новый товар в литрах в категории «прочее» (П4: единица — у товара).
    let (s, _, p) = call(
        &app,
        "POST",
        "/api/v1/products",
        Some(&owner),
        Some(json!({ "op_id": Uuid::now_v7(), "category_id": cat_id, "name": "Антифриз G12", "unit": "ml", "container_ml": 5000, "sale_price_tyiyn": 90000 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["unit"], "ml");
    let pid = p["id"].as_str().unwrap().to_string();
    let points = json!({ "points": { "mode": "fixed", "value": 500 } });
    // Баллы товара и категории — только владелец.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{pid}"),
        Some(&admin),
        Some(points.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/categories/{cat_id}"),
        Some(&admin),
        Some(json!({ "points": { "mode": "none" } })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, out) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{pid}"),
        Some(&owner),
        Some(points),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(
        (out["points_mode"].clone(), out["points_value"].clone()),
        (json!("fixed"), json!(500))
    );
    let (s, _, out) = call(
        &app,
        "PATCH",
        &format!("/api/v1/categories/{cat_id}"),
        Some(&owner),
        Some(json!({ "points": { "mode": "percent", "value": 100 } })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["points_value"], 100);
    // Неверное правило — 422; без правки баллов администратор правит карточку как раньше.
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{pid}"),
        Some(&owner),
        Some(json!({ "points": { "mode": "percent", "value": 20000 } })),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _, _) = call(
        &app,
        "PATCH",
        &format!("/api/v1/products/{pid}"),
        Some(&admin),
        Some(json!({ "name": "Антифриз G12+", "points": { "mode": "fixed", "value": 500 } })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let logged: i64 = sqlx::query_scalar("select count(*) from audit_log where action = 'product.update' and data -> 'changes' ? 'points_mode'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(logged, 1, "изменение баллов — в журнале");
    // Единица — только владелец.
    let body = json!({ "op_id": Uuid::now_v7(), "unit": "piece", "piece_ml": 5000 });
    let (s, _, _) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/unit"),
        Some(&admin),
        Some(body.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, out) = call(
        &app,
        "POST",
        &format!("/api/v1/products/{pid}/unit"),
        Some(&owner),
        Some(body),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{out}");
    assert_eq!(out["product"]["unit"], "piece");
    // Настройка «только в чеке с заменой масла»: по умолчанию да, меняет владелец.
    let (_, _, st) = call(&app, "GET", "/api/v1/settings/loyalty", Some(&admin), None).await;
    assert_eq!(st, json!({ "rate_bp": 50, "oil_change_only": true }));
    let off = json!({ "rate_bp": 70, "oil_change_only": false });
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/loyalty",
        Some(&admin),
        Some(off.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _, _) = call(
        &app,
        "PUT",
        "/api/v1/settings/loyalty",
        Some(&owner),
        Some(off.clone()),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (_, _, st) = call(&app, "GET", "/api/v1/settings/loyalty", Some(&owner), None).await;
    assert_eq!(st, off);
    // Только процент — выключатель не сбрасывается.
    let (s, _, st) = call(
        &app,
        "PUT",
        "/api/v1/settings/loyalty",
        Some(&owner),
        Some(json!({ "rate_bp": 60 })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["oil_change_only"], false);
}
