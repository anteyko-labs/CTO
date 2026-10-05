//! Сквозные проверки критериев приёмки SPEC-01…04 на реальной базе.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use avtodom_server::api::receipts::{
    ReceiptLineReq, ReceiptReq, ReverseReq, post_receipt_tx, reverse_receipt_tx, verify_stock_tx,
};
use avtodom_server::api::sales::{
    PaymentReq, ReturnLineReq, ReturnReq, SaleLineReq, SaleReq, post_sale_tx, return_sale_tx,
};
use avtodom_server::auth::{Ctx, CurrentUser, Role};
use avtodom_server::error::AppError;
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    owner: Ctx,
    admin: Ctx,
    cashier: Uuid,
    master: Uuid,
    service: Uuid,
    filter: Uuid,
    oil: Uuid,
}

async fn seed(pool: &PgPool) -> World {
    let branch = Uuid::now_v7();
    sqlx::query("insert into branches (id, name) values ($1, 'Тест')")
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
        users.push(Ctx {
            user: CurrentUser {
                id,
                branch_id: branch,
                login: login.into(),
                full_name: login.into(),
                role: Role::parse(role).unwrap(),
            },
            device_id: Uuid::now_v7(),
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
    sqlx::query("insert into services (id, branch_id, name, price_tyiyn) values ($1, $2, 'Замена масла', 20000)")
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
    sqlx::query("insert into products (id, category_id, name, unit) values ($1, $2, 'Фильтр W712', 'piece')")
        .bind(filter)
        .bind(cat_f)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit, container_ml) values ($1, $2, 'Масло 5W-30 4л', 'ml', 4000)")
        .bind(oil)
        .bind(cat_o)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into branch_products (branch_id, product_id, sale_price_tyiyn, pour_price_per_l_tyiyn) values ($1, $2, 50000, null), ($1, $3, 200000, 33333)",
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
        filter,
        oil,
    }
}

async fn receive(
    pool: &PgPool,
    ctx: &Ctx,
    product: Uuid,
    qty: i64,
    cost: i64,
) -> avtodom_server::api::receipts::ReceiptOut {
    let mut tx = pool.begin().await.unwrap();
    let r = post_receipt_tx(
        &mut tx,
        ctx,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            supplier_id: None,
            supplier_doc: String::new(),
            comment: String::new(),
            lines: vec![ReceiptLineReq {
                product_id: product,
                qty,
                cost_tyiyn: cost,
            }],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    r
}

async fn pool_of(pool: &PgPool, product: Uuid) -> (i64, i64, bool) {
    sqlx::query_as("select stock_qty, stock_value_tyiyn, needs_review from branch_products where product_id = $1")
        .bind(product)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn line(kind: &str, product: Uuid, qty: i64, price: i64) -> SaleLineReq {
    SaleLineReq {
        kind: kind.into(),
        product_id: Some(product),
        service_id: None,
        qty,
        unit_price_tyiyn: price,
    }
}

fn cash(amount: i64) -> Vec<PaymentReq> {
    vec![PaymentReq {
        method: "cash".into(),
        amount_tyiyn: amount,
    }]
}

fn takeaway(w: &World, lines: Vec<SaleLineReq>, payments: Vec<PaymentReq>) -> SaleReq {
    SaleReq {
        op_id: Uuid::now_v7(),
        client_time: None,
        sale_type: "takeaway".into(),
        cashier_id: w.cashier,
        master_id: None,
        comment: String::new(),
        lines,
        payments,
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

async fn assert_stock_consistent(pool: &PgPool, w: &World) {
    let mut conn = pool.acquire().await.unwrap();
    let diff = verify_stock_tx(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    assert!(diff.is_empty(), "остатки расходятся с движениями");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn weighted_average_and_full_sellout(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100_000).await;
    receive(&pool, &w.admin, w.filter, 10, 200_000).await;
    assert_eq!(pool_of(&pool, w.filter).await, (20, 300_000, false));

    let sale = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    assert_eq!(sale.lines[0].cost_tyiyn, Some(15_000));

    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 19, 50_000)], cash(950_000)),
    )
    .await
    .unwrap();
    assert_eq!(pool_of(&pool, w.filter).await, (0, 0, false));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_container_and_pour(pool: PgPool) {
    let w = seed(&pool).await;
    // 2 канистры по 4 л за 1500 с каждая.
    receive(&pool, &w.admin, w.oil, 8000, 300_000).await;
    // Последняя закупочная цена — за канистру, видна и администратору.
    let mut conn = pool.acquire().await.unwrap();
    let p = avtodom_server::api::catalog::product_by_id(&mut conn, &w.admin.user, w.oil)
        .await
        .unwrap();
    assert_eq!(p.last_purchase_price_tyiyn, Some(150_000));
    assert!(p.avg_cost_tyiyn.is_none());
    drop(conn);
    let sale = sell(
        &pool,
        &w.admin,
        takeaway(
            &w,
            vec![
                line("container", w.oil, 1, 200_000),
                line("pour", w.oil, 1500, 33_333),
            ],
            cash(250_000),
        ),
    )
    .await
    .unwrap();
    assert_eq!(sale.total_tyiyn, 250_000);
    assert_eq!(sale.lines[1].amount_tyiyn, 50_000);
    assert_eq!(sale.lines[0].units, 4000);
    // Администратор не получает себестоимость.
    assert!(sale.lines.iter().all(|l| l.cost_tyiyn.is_none()));
    let (qty, value, _) = pool_of(&pool, w.oil).await;
    assert_eq!(qty, 2500);
    assert_eq!(value, 300_000 - 150_000 - 56_250);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn payments_must_match_total(pool: PgPool) {
    let w = seed(&pool).await;
    let err = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(40_000)),
    )
    .await;
    assert!(matches!(err, Err(AppError::Validation(_))));
    let n: i64 = sqlx::query_scalar("select count(*) from sales")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn service_sale_requires_master(pool: PgPool) {
    let w = seed(&pool).await;
    let svc = SaleLineReq {
        kind: "service".into(),
        product_id: None,
        service_id: Some(w.service),
        qty: 1,
        unit_price_tyiyn: 25_000,
    };
    let mut req = takeaway(&w, vec![svc.clone()], cash(25_000));
    req.sale_type = "service".into();
    assert!(matches!(
        sell(&pool, &w.owner, req).await,
        Err(AppError::Validation(_))
    ));

    let mut req = takeaway(
        &w,
        vec![svc, line("piece", w.filter, 1, 50_000)],
        cash(75_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(sale.lines[0].master_fee_tyiyn, 3000);
    assert_eq!(sale.lines[0].list_price_tyiyn, 20_000);
    // Цена работы изменена в чеке — запись в журнале.
    let overrides: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'sale.price_override'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(overrides, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn idempotent_sale(pool: PgPool) {
    let w = seed(&pool).await;
    let op = Uuid::now_v7();
    let mut first = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    first.op_id = op;
    let mut second = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    second.op_id = op;
    let a = sell(&pool, &w.owner, first).await.unwrap();
    let b = sell(&pool, &w.owner, second).await.unwrap();
    assert_eq!(a.id, b.id);
    let n: i64 = sqlx::query_scalar("select count(*) from sales")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn negative_stock_is_flagged(pool: PgPool) {
    let w = seed(&pool).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    )
    .await
    .unwrap();
    let (qty, _, review) = pool_of(&pool, w.filter).await;
    assert_eq!(qty, -2);
    assert!(review);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn partial_returns_restore_exact_cost(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 3, 100).await;
    let sale = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 3, 100)], cash(300)),
    )
    .await
    .unwrap();
    let ret = |qty| ReturnReq {
        op_id: Uuid::now_v7(),
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty }],
        payments: cash(100 * qty),
    };
    for _ in 0..3 {
        let mut tx = pool.begin().await.unwrap();
        return_sale_tx(&mut tx, &w.owner, sale.id, ret(1))
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(pool_of(&pool, w.filter).await, (3, 100, false));
    let mut tx = pool.begin().await.unwrap();
    let over = return_sale_tx(&mut tx, &w.owner, sale.id, ret(1)).await;
    assert!(matches!(over, Err(AppError::Validation(_))));
    drop(tx);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_reversal(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 1000).await;
    let r = receive(&pool, &w.admin, w.filter, 5, 3000).await;
    let rev = |op| ReverseReq {
        op_id: op,
        comment: "ошибка".into(),
    };
    let mut tx = pool.begin().await.unwrap();
    let out = reverse_receipt_tx(&mut tx, &w.admin, r.id, rev(Uuid::now_v7()))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(out.total_tyiyn, -3000);
    assert_eq!(pool_of(&pool, w.filter).await, (5, 1000, false));
    let mut tx = pool.begin().await.unwrap();
    let again = reverse_receipt_tx(&mut tx, &w.admin, r.id, rev(Uuid::now_v7())).await;
    assert!(matches!(again, Err(AppError::Conflict(_))));
    drop(tx);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn posted_documents_are_immutable(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 1, 100).await;
    for sql in [
        "update receipts set total_tyiyn = 0",
        "delete from stock_movements",
        "delete from audit_log",
    ] {
        assert!(
            sqlx::query(sql).execute(&pool).await.is_err(),
            "{sql} должно отклоняться"
        );
    }
}
