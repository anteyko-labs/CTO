//! Сквозные проверки критериев приёмки SPEC-01…04 на реальной базе.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use avtodom_server::api::batteries::{IntakeReq, intake_tx};
use avtodom_server::api::oil::{TransferReq, post_transfer_tx, reverse_transfer_tx};
use avtodom_server::api::receipts::{
    ReceiptLineReq, ReceiptReq, ReverseReq, post_receipt_tx, reverse_receipt_tx, verify_stock_tx,
};
use avtodom_server::api::revisions::{PostReq, post_revision_tx};
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
    // Приход для проверок склада: оплачен «из своих, мимо касс», кассы не трогает (ADR-062).
    // Так платит только владелец, поэтому документ проводится от того же пользователя с ролью владельца.
    let mut payer = ctx.clone();
    payer.user.role = Role::Owner;
    let mut tx = pool.begin().await.unwrap();
    let r = post_receipt_tx(
        &mut tx,
        &payer,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            payment: None,
            paid_from: Some("outside".into()),
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
        gift: false,
        product_id: Some(product),
        service_id: None,
        qty,
        unit_price_tyiyn: price,
        seen_list_price_tyiyn: None,
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
        party_id: None,
        contact_id: None,
        vehicle_id: None,
        comment: String::new(),
        lines,
        payments,
        offline: false,
        mileage_km: None,
        delivery_address: String::new(),
        declined: String::new(),
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
    // 2 канистры по 4 л за 1200 с каждая: литр закупки 300 с, розлив по 333,33 — не дешевле.
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    // Последняя закупочная цена — за канистру, видна и администратору.
    let mut conn = pool.acquire().await.unwrap();
    let p = avtodom_server::api::catalog::product_by_id(&mut conn, &w.admin.user, w.oil)
        .await
        .unwrap();
    assert_eq!(p.last_purchase_price_tyiyn, Some(120_000));
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
    assert_eq!(value, 240_000 - 120_000 - 45_000);
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
async fn oil_change_is_a_mark_not_a_service_line(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100).await;

    // «В сервис» без мастера не проводится.
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    req.sale_type = "service".into();
    assert!(matches!(
        sell(&pool, &w.owner, req).await,
        Err(AppError::Validation(_))
    ));

    // Мастер на вынос тоже не проводится.
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    req.master_id = Some(w.master);
    assert!(matches!(
        sell(&pool, &w.owner, req).await,
        Err(AppError::Validation(_))
    ));

    // Чек с заменой: ставка одна на чек, строк услуг в чеке нет (ADR-027).
    let mut req = takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000));
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    let service_sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(service_sale.master_fee_tyiyn, 3000);
    assert!(service_sale.lines.iter().all(|l| l.kind != "service"));

    // Тот же товар на вынос стоит столько же, но мастеру не идёт ничего.
    let takeaway_sale = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    )
    .await
    .unwrap();
    assert_eq!(takeaway_sale.total_tyiyn, service_sale.total_tyiyn);
    assert_eq!(takeaway_sale.master_fee_tyiyn, 0);

    // Частичный возврат ставку не снимает, полный — снимает.
    let ret = |qty| ReturnReq {
        op_id: Uuid::now_v7(),
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty }],
        payments: cash(50_000 * qty),
    };
    let mut tx = pool.begin().await.unwrap();
    let partial = return_sale_tx(&mut tx, &w.owner, service_sale.id, ret(1))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(partial.master_fee_tyiyn, 0);
    let mut tx = pool.begin().await.unwrap();
    let full = return_sale_tx(&mut tx, &w.owner, service_sale.id, ret(1))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(full.master_fee_tyiyn, -3000);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn service_line_pays_master_its_own_rate(pool: PgPool) {
    // Клиент со своим маслом: услуга 400 с, мастеру 200 с, ставки замены нет (ADR-043).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 1_000).await;
    sqlx::query("update services set price_tyiyn = 40000, master_fee_tyiyn = 20000 where id = $1")
        .bind(w.service)
        .execute(&pool)
        .await
        .unwrap();
    let service_line = SaleLineReq {
        kind: "service".into(),
        gift: false,
        product_id: None,
        service_id: Some(w.service),
        qty: 1,
        unit_price_tyiyn: 40_000,
        seen_list_price_tyiyn: None,
    };
    let mut req = takeaway(&w, vec![service_line.clone()], cash(40_000));
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(sale.lines[0].list_price_tyiyn, 40_000);
    assert_eq!(sale.lines[0].master_fee_tyiyn, 20_000);
    assert_eq!(sale.master_fee_tyiyn, 20_000);
    let accrued: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1",
    )
    .bind(w.master)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accrued, 20_000);

    // С товаром в чеке: ставка замены за чек плюс ставка услуги.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 500), service_line],
        cash(40_500),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(sale.master_fee_tyiyn, 3_000 + 20_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_sale_moves_customer_balance(pool: PgPool) {
    // Продажа в долг ложится на клиента, возврат долг уменьшает (SPEC-10).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100).await;
    let party = Uuid::now_v7();
    sqlx::query(
        "insert into parties (id, branch_id, role, kind, name, inn) values ($1, $2, 'customer', 'company', 'ОсОО Тест', '12345')",
    )
    .bind(party)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();

    let pay = |method: &str, amount: i64| PaymentReq {
        method: method.into(),
        amount_tyiyn: amount,
    };
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 3, 1000)],
        vec![pay("cash", 1000), pay("debt", 2000)],
    );
    req.party_id = Some(party);
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(sale.party_balance_tyiyn, Some(2000));
    assert_eq!(sale.party_name.as_deref(), Some("ОсОО Тест"));

    // Долг без клиента не проводится.
    let bad = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 1000)],
        vec![pay("debt", 1000)],
    );
    assert!(matches!(
        sell(&pool, &w.owner, bad).await,
        Err(AppError::Validation(_))
    ));

    // Возврат одной штуки гасит часть долга.
    let mut tx = pool.begin().await.unwrap();
    let back = return_sale_tx(
        &mut tx,
        &w.owner,
        sale.id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: "вернули".into(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: vec![pay("debt", 1000)],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(back.party_id, Some(party));
    let balance: i64 = sqlx::query_scalar("select balance_tyiyn from parties where id = $1")
        .bind(party)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, 1000);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cash_follows_payments(pool: PgPool) {
    // Наличные идут в кассу, безнал — на счёт за вычетом комиссии банка (SPEC-05, ADR-024).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100).await;
    let balance = |kind: &'static str| {
        let pool = pool.clone();
        async move {
            // Кассы заводятся при первой операции, до неё их просто нет.
            sqlx::query_scalar::<_, i64>(
                "select balance_tyiyn from cash_accounts where branch_id = $1 and kind = $2",
            )
            .bind(w.owner.user.branch_id)
            .bind(kind)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .unwrap_or(0)
        }
    };
    let before_cash = balance("register").await;
    let before_bank = balance("bank").await;

    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 100_000)], cash(100_000)),
    )
    .await
    .unwrap();
    assert_eq!(balance("register").await, before_cash + 100_000);

    let mut by_card = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 100_000)],
        vec![PaymentReq {
            method: "card".into(),
            amount_tyiyn: 100_000,
        }],
    );
    by_card.op_id = Uuid::now_v7();
    sell(&pool, &w.owner, by_card).await.unwrap();
    // 0,5 % комиссии: на счёт пришло 99 500 тыйын.
    assert_eq!(balance("bank").await, before_bank + 99_500);
    assert_eq!(balance("register").await, before_cash + 100_000);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cashier_percent_waits_for_the_debt(pool: PgPool) {
    // 2 % с валовой прибыли: с оплаченной части сразу, с долговой — при погашении (ADR-035, ADR-036).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 30_000).await;
    sqlx::query(
        "insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base, rate_bp, user_id)
         values ($1, $2, $3, 'revenue_percent', 'cashier', 'gross', 200, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let party = Uuid::now_v7();
    sqlx::query(
        "insert into parties (id, branch_id, role, kind, name, inn) values ($1, $2, 'customer', 'company', 'ОсОО Процент', '01204201910123')",
    )
    .bind(party)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();

    // Приход 10 шт на 30 000 → 3 000 за штуку. Чек на 100 000 за 2 шт: валовая 94 000,
    // процент 2 % = 1 880, половина чека оплачена наличными.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 2, 50_000)],
        vec![
            PaymentReq {
                method: "cash".into(),
                amount_tyiyn: 50_000,
            },
            PaymentReq {
                method: "debt".into(),
                amount_tyiyn: 50_000,
            },
        ],
    );
    req.party_id = Some(party);
    sell(&pool, &w.owner, req).await.unwrap();

    let accrued = |employee: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1",
            )
            .bind(employee)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // Половина чека оплачена — начислена половина процента.
    assert_eq!(accrued(w.cashier).await, 940);

    let remaining: i64 =
        sqlx::query_scalar("select debt_remaining_tyiyn from payroll_pending where party_id = $1")
            .bind(party)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 50_000);

    // Вернули одну штуку: половина денег наличными, половина списана с долга.
    // Процент снимается только с оплаченной части возврата: 940 × 23 500 / 47 000 = 470,
    // а ожидание по долгу уменьшается на возвращённый долг (SPEC-07).
    let sale_id: Uuid =
        sqlx::query_scalar("select id from sales where kind = 'sale' and party_id = $1")
            .bind(party)
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        sale_id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: vec![
                PaymentReq {
                    method: "cash".into(),
                    amount_tyiyn: 25_000,
                },
                PaymentReq {
                    method: "debt".into(),
                    amount_tyiyn: 25_000,
                },
            ],
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(accrued(w.cashier).await, 470);
    let (gross, left): (i64, i64) = sqlx::query_as(
        "select gross_tyiyn, debt_remaining_tyiyn from payroll_pending where party_id = $1",
    )
    .bind(party)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((gross, left), (23_500, 25_000));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn full_return_takes_back_master_fee(pool: PgPool) {
    // Мастер получил ставку за замену; чек вернули целиком — ставка снимается (ADR-027, SPEC-07).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 2, 1_000).await;
    let master_total = |pool: PgPool, m: Uuid| async move {
        sqlx::query_scalar::<_, i64>(
            "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1",
        )
        .bind(m)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let mut req = takeaway(&w, vec![line("piece", w.filter, 2, 5_000)], cash(10_000));
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    let paid = master_total(pool.clone(), w.master).await;
    assert_eq!(paid, 3_000);
    for _ in 0..2 {
        let mut tx = pool.begin().await.unwrap();
        return_sale_tx(
            &mut tx,
            &w.owner,
            sale.id,
            ReturnReq {
                op_id: Uuid::now_v7(),
                comment: String::new(),
                lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
                payments: cash(5_000),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(master_total(pool.clone(), w.master).await, 0);
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn price_below_cost_is_refused_except_gifts(pool: PgPool) {
    // Дешевле закупки не продаём; подарок можно; чек без сети принимается и уходит владельцу (ADR-042).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 30_000).await;
    let below = takeaway(&w, vec![line("piece", w.filter, 1, 2_000)], cash(2_000));
    assert!(matches!(
        sell(&pool, &w.admin, below).await,
        Err(AppError::Validation(_))
    ));
    // Ровно по закупке — можно.
    sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("piece", w.filter, 1, 3_000)], cash(3_000)),
    )
    .await
    .unwrap();
    let mut offline = takeaway(&w, vec![line("piece", w.filter, 1, 2_000)], cash(2_000));
    offline.offline = true;
    // Признак без времени из прошлого не помогает: это онлайн-чек.
    assert!(matches!(
        sell(&pool, &w.admin, offline).await,
        Err(AppError::Validation(_))
    ));
    let mut offline = takeaway(&w, vec![line("piece", w.filter, 1, 2_000)], cash(2_000));
    offline.offline = true;
    offline.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
    sell(&pool, &w.admin, offline).await.unwrap();
    let flagged: i64 =
        sqlx::query_scalar("select count(*) from audit_log where action = 'sale.below_cost'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(flagged, 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn gift_needs_threshold(pool: PgPool) {
    // Подарок к маслу от 3 л: литр — нельзя, канистра 4 л — можно, но не больше одного (SPEC-11).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let rule = Uuid::now_v7();
    sqlx::query(
        "insert into gift_rules (id, branch_id, trigger_product_id, user_id, min_units) values ($1, $2, $3, $4, 3000)",
    )
    .bind(rule)
    .bind(w.owner.user.branch_id)
    .bind(w.oil)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into gift_rule_items (rule_id, gift_product_id, gift_qty) values ($1, $2, 1)",
    )
    .bind(rule)
    .bind(w.filter)
    .execute(&pool)
    .await
    .unwrap();
    let gift = |qty| {
        let mut l = line("piece", w.filter, qty, 0);
        l.gift = true;
        l
    };
    let small = takeaway(
        &w,
        vec![line("pour", w.oil, 1000, 33_333), gift(1)],
        cash(33_333),
    );
    assert!(matches!(
        sell(&pool, &w.admin, small).await,
        Err(AppError::Validation(_))
    ));
    let too_many = takeaway(
        &w,
        vec![line("container", w.oil, 1, 200_000), gift(2)],
        cash(200_000),
    );
    assert!(matches!(
        sell(&pool, &w.admin, too_many).await,
        Err(AppError::Validation(_))
    ));
    let ok = takeaway(
        &w,
        vec![line("container", w.oil, 1, 200_000), gift(1)],
        cash(200_000),
    );
    let sale = sell(&pool, &w.admin, ok).await.unwrap();
    assert!(sale.lines.iter().any(|l| l.gift && l.amount_tyiyn == 0));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_transfer_averages_cost(pool: PgPool) {
    // Пример заказчика: в Hi-Tec 5 л по 100 с, доливаем 50 л Totachi по 200 с → 190,91 с за литр (SPEC-14).
    let w = seed(&pool).await;
    let hitec = Uuid::now_v7();
    let cat: Uuid = sqlx::query_scalar("select category_id from products where id = $1")
        .bind(w.oil)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit, container_ml) values ($1, $2, 'Hi-Tec 5W-30', 'ml', 4000)")
        .bind(hitec)
        .bind(cat)
        .execute(&pool)
        .await
        .unwrap();
    receive(&pool, &w.admin, hitec, 5_000, 50_000).await;
    receive(&pool, &w.admin, w.oil, 50_000, 1_000_000).await;
    let transfer = |ctx, qty| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let r = post_transfer_tx(
                &mut tx,
                ctx,
                TransferReq {
                    op_id: Uuid::now_v7(),
                    from_product_id: w.oil,
                    to_product_id: hitec,
                    qty_ml: qty,
                    comment: String::new(),
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.unwrap();
            }
            r
        }
    };
    // Администратору перелив недоступен; больше, чем есть, не перелить.
    assert!(matches!(
        transfer(&w.admin, 1_000).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        transfer(&w.owner, 60_000).await,
        Err(AppError::Validation(_))
    ));
    let out = transfer(&w.owner, 50_000).await.unwrap();
    assert_eq!(out.value_tyiyn, 1_000_000);
    assert_eq!(out.to.stock_ml, 55_000);
    assert_eq!(out.to.avg_per_l_tyiyn, Some(19_091));
    assert_eq!(out.from.stock_ml, 0);
    assert_eq!(pool_of(&pool, hitec).await.1, 1_050_000);
    assert_stock_consistent(&pool, &w).await;

    // Отмена: масло возвращается в источник той же стоимостью, получатель — как до перелива (ADR-052).
    let reverse = |ctx, id, comment: &str| {
        let pool = pool.clone();
        let comment = comment.to_string();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let r = reverse_transfer_tx(
                &mut tx,
                ctx,
                id,
                avtodom_server::api::oil::ReverseReq {
                    op_id: Uuid::now_v7(),
                    comment,
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.unwrap();
            }
            r
        }
    };
    assert!(matches!(
        reverse(&w.admin, out.id, "ошиблись").await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        reverse(&w.owner, out.id, " ").await,
        Err(AppError::Validation(_))
    ));
    let back = reverse(&w.owner, out.id, "перелили не то масло")
        .await
        .unwrap();
    assert_eq!(
        (back.from.stock_ml, back.from.value_tyiyn),
        (50_000, 1_000_000)
    );
    assert_eq!((back.to.stock_ml, back.to.value_tyiyn), (5_000, 50_000));
    assert!(matches!(
        reverse(&w.owner, out.id, "ещё раз").await,
        Err(AppError::Conflict(_))
    ));
    // Перелитое уже ушло дальше — отменить нельзя.
    let again = transfer(&w.owner, 50_000).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    post_transfer_tx(
        &mut tx,
        &w.owner,
        TransferReq {
            op_id: Uuid::now_v7(),
            from_product_id: hitec,
            to_product_id: w.oil,
            qty_ml: 20_000,
            comment: String::new(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        reverse(&w.owner, again.id, "поздно").await,
        Err(AppError::Validation(_))
    ));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn empty_stock_keeps_no_value(pool: PgPool) {
    // Продали больше, чем было, потом приняли ровно недостающее: товара 0 — и стоимости 0 (ADR-044).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 1_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 15, 200)], cash(3_000)),
    )
    .await
    .unwrap();
    assert_eq!(pool_of(&pool, w.filter).await.0, -5);
    receive(&pool, &w.admin, w.filter, 5, 1_000).await;
    let (qty, value, _) = pool_of(&pool, w.filter).await;
    assert_eq!((qty, value), (0, 0));
    let revalued: i64 = sqlx::query_scalar(
        "select coalesce(sum(value_delta_tyiyn), 0)::bigint from stock_movements where doc_type = 'revaluation'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revalued, -500);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_needs_pin_or_inn(pool: PgPool) {
    // Долг без ПИН или ИНН покупателя не проводится: расписка без него недействительна (SPEC-10).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 500).await;
    let party = Uuid::now_v7();
    sqlx::query(
        "insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'customer', 'person', 'Без ПИН')",
    )
    .bind(party)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();
    let debt = || {
        let mut req = takeaway(
            &w,
            vec![line("piece", w.filter, 1, 1_000)],
            vec![PaymentReq {
                method: "debt".into(),
                amount_tyiyn: 1_000,
            }],
        );
        req.party_id = Some(party);
        req
    };
    assert!(matches!(
        sell(&pool, &w.admin, debt()).await,
        Err(AppError::Validation(_))
    ));
    sqlx::query("update parties set inn = '21201199000123' where id = $1")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    sell(&pool, &w.admin, debt()).await.unwrap();
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_after_negative_stock_values_rest_at_its_cost(pool: PgPool) {
    // Продали 3 при остатке 1, потом приняли 3 по 500 с: на складе 1 шт — и стоит она 500 с,
    // а не 1300; разница — в себестоимость (ADR-044).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 1, 10_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 3, 100_000)], cash(300_000)),
    )
    .await
    .unwrap();
    receive(&pool, &w.admin, w.filter, 3, 150_000).await;
    assert_eq!(&pool_of(&pool, w.filter).await, &(1, 50_000, true));
    // Цена 1000 с выше закупки 500 с — продаётся.
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 100_000)], cash(100_000)),
    )
    .await
    .unwrap();
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn return_after_merge_lands_on_main_card(pool: PgPool) {
    // Возврат по карточке, которую потом влили в основную, ложится на основную (ADR-044).
    let w = seed(&pool).await;
    let main = Uuid::now_v7();
    let cat: Uuid = sqlx::query_scalar("select category_id from products where id = $1")
        .bind(w.filter)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit) values ($1, $2, 'Фильтр основной', 'piece')")
        .bind(main)
        .bind(cat)
        .execute(&pool)
        .await
        .unwrap();
    receive(&pool, &w.admin, w.filter, 2, 20_000).await;
    let sale = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    // Объединение так же, как делает сервер: остаток движениями, дубль в архив со ссылкой.
    sqlx::query("update products set archived = true, merged_into = $2 where id = $1")
        .bind(w.filter)
        .bind(main)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        sale.id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: cash(50_000),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(pool_of(&pool, main).await.0, 1);
    // Архивную карточку больше не продать.
    assert!(
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
        )
        .await
        .is_ok_and(|s| s.lines[0].product_id == Some(main))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_reversal_revalues_rest_at_restored_cost(pool: PgPool) {
    // Приняли 10 по 100 и 10 по 1000 (ошибка), продали 5, сторно ошибочного прихода:
    // остаток 5 шт стоит по восстановленной цене 100, а не по ошибочной (ADR-044).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100_000).await;
    let wrong = receive(&pool, &w.admin, w.filter, 10, 1_000_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("piece", w.filter, 5, 300_000)],
            cash(1_500_000),
        ),
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    reverse_receipt_tx(
        &mut tx,
        &w.owner,
        wrong.id,
        ReverseReq {
            op_id: Uuid::now_v7(),
            comment: "ошибка цены".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (qty, value, _) = pool_of(&pool, w.filter).await;
    assert_eq!((qty, value), (5, 50_000));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_survives_changed_references(pool: PgPool) {
    // Пока чек лежал на устройстве, кассира отключили и подняли цену: продажа уже была —
    // чек принимаем, владелец видит «по старой цене», а не «кассир поменял цену» (SPEC-09).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    sqlx::query("update employees set active = false where id = $1")
        .bind(w.cashier)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update branch_products set sale_price_tyiyn = 60000 where product_id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    let make = |offline: bool| {
        let mut l = line("piece", w.filter, 1, 50_000);
        l.seen_list_price_tyiyn = Some(50_000);
        let mut req = takeaway(&w, vec![l], cash(50_000));
        req.offline = offline;
        req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(20));
        req
    };
    // Онлайн отключённым кассиром не продать.
    assert!(matches!(
        sell(&pool, &w.owner, make(false)).await,
        Err(AppError::Validation(_))
    ));
    let sale = sell(&pool, &w.owner, make(true)).await.unwrap();
    let count = |action: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select count(*) from audit_log where action = $1 and entity_id = $2",
            )
            .bind(action)
            .bind(sale.id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(count("sale.stale_price").await, 1);
    assert_eq!(count("sale.price_override").await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn stale_stock_after_sixty_days(pool: PgPool) {
    // Остаток есть, а продаж нет дольше 60 дней — товар залежался; свежий приход — нет (ADR-039).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let mut conn = pool.acquire().await.unwrap();
    let (days, items) =
        avtodom_server::api::receipts::stale_list(&mut conn, w.owner.user.branch_id)
            .await
            .unwrap();
    assert_eq!(days, 60);
    assert!(items.is_empty());
    // Тот же товар, будто первый приход был 90 дней назад.
    sqlx::query(
        "insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id, created_at)
         values ($1, $2, $3, 0, 0, 'receipt', $1, now() - interval '90 days')",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(w.filter)
    .execute(&pool)
    .await
    .unwrap();
    let (_, items) = avtodom_server::api::receipts::stale_list(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert!(!items[0].sold_ever);
    assert!(items[0].days >= 89);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_aligns_stock_and_charges_shortage(pool: PgPool) {
    // Приняли 10 по 100 с, пересчитали 8: недостача 2 по средней, остаток 8 (SPEC-15).
    use avtodom_server::api::revisions::over_norm;
    // Норма по маслу 0,5 %: из 100 л расхождение 0,5 л — в норме, 0,501 л — сверх; штучное не считается.
    assert!(!over_norm("ml", 100_000, Some(99_500), 50));
    assert!(over_norm("ml", 100_000, Some(99_499), 50));
    assert!(over_norm("ml", 100_000, Some(100_501), 50));
    assert!(over_norm("ml", 0, Some(1), 50));
    assert!(!over_norm("ml", 100_000, None, 50));
    assert!(!over_norm("piece", 10, Some(1), 50));
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 10_000).await;
    let new_revision = |counted: i64| {
        let pool = pool.clone();
        let user = w.owner.user.clone();
        let product = w.filter;
        async move {
            let id = Uuid::now_v7();
            sqlx::query("insert into revisions (id, branch_id, number, user_id, device_id) values ($1, $2, (select coalesce(max(number), 0) + 1 from revisions), $3, $1)")
                .bind(id)
                .bind(user.branch_id)
                .bind(user.id)
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("insert into revision_lines (revision_id, product_id, counted_qty, user_id) values ($1, $2, $3, $4)")
                .bind(id)
                .bind(product)
                .bind(counted)
                .bind(user.id)
                .execute(&pool)
                .await
                .unwrap();
            id
        }
    };
    let post = |ctx: &Ctx, id: Uuid| {
        let pool = pool.clone();
        let ctx = ctx.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let r = post_revision_tx(
                &mut tx,
                &ctx,
                id,
                PostReq {
                    op_id: Uuid::now_v7(),
                    comment: "ревизия за месяц".into(),
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.unwrap();
            }
            r
        }
    };
    let id = new_revision(8).await;
    assert!(matches!(post(&w.admin, id).await, Err(AppError::Forbidden)));
    let out = post(&w.owner, id).await.unwrap();
    assert_eq!(out.lines[0].expected_qty, 10);
    assert_eq!(out.lines[0].value_delta_tyiyn, Some(-2_000));
    let (qty, value, _) = pool_of(&pool, w.filter).await;
    assert_eq!((qty, value), (8, 8_000));
    assert!(matches!(
        post(&w.owner, id).await,
        Err(AppError::Conflict(_))
    ));
    // Нашли ещё одну: излишек по средней 100 с.
    let id = new_revision(9).await;
    let out = post(&w.owner, id).await.unwrap();
    assert_eq!(out.lines[0].value_delta_tyiyn, Some(1_000));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn batteries_by_weight_average_and_no_loss(pool: PgPool) {
    // Четыре приёма по разным ценам: средняя за кг по всем; продать дешевле средней нельзя (ADR-048).
    let w = seed(&pool).await;
    let till: Uuid = sqlx::query_scalar(
        "insert into cash_accounts (id, branch_id, name, kind, is_default) values ($1, $2, 'Касса', 'register', true) returning id",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into cash_movements (id, branch_id, account_id, kind, amount_tyiyn, user_id, device_id) values ($1, $2, $3, 'cash_in', 10000000, $4, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(till)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update cash_accounts set balance_tyiyn = 10000000 where id = $1")
        .bind(till)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn)
         values ($1, $2, 1, (now() at time zone 'Asia/Bishkek')::date, $3, $4, $5, $1, 10000000)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(till)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    // 10 кг по 100, 15 кг по 80, 5 кг по 120, 20 кг по 90 → 50 кг за 4 600 с, средняя 92 с/кг.
    let mut last = None;
    for (kg, price) in [(10, 10_000), (15, 8_000), (5, 12_000), (20, 9_000)] {
        let mut tx = pool.begin().await.unwrap();
        let out = intake_tx(
            &mut tx,
            &w.admin,
            IntakeReq {
                op_id: Uuid::now_v7(),
                grams: kg * 1000,
                price_per_kg_tyiyn: price,
                comment: String::new(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        last = Some(out);
    }
    let info = last.unwrap().info;
    assert_eq!(info.stock_g, 50_000);
    assert_eq!(info.avg_per_kg_tyiyn, Some(9_200));
    let balance: i64 = sqlx::query_scalar("select balance_tyiyn from cash_accounts where id = $1")
        .bind(till)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, 10_000_000 - 460_000);
    let weight = |grams: i64, per_kg: i64| SaleLineReq {
        kind: "weight".into(),
        gift: false,
        product_id: Some(info.product_id),
        service_id: None,
        qty: grams,
        unit_price_tyiyn: per_kg,
        seen_list_price_tyiyn: None,
    };
    // 12,5 кг по 85 с/кг — дешевле средней 92: отказ.
    assert!(matches!(
        sell(
            &pool,
            &w.admin,
            takeaway(&w, vec![weight(12_500, 8_500)], cash(106_250))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    // 12,5 кг по 110 с/кг = 1 375 с — проходит, на складе 37,5 кг.
    let sale = sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![weight(12_500, 11_000)], cash(137_500)),
    )
    .await
    .unwrap();
    assert_eq!(sale.total_tyiyn, 137_500);
    let (qty, value, _) = pool_of(&pool, info.product_id).await;
    assert_eq!((qty, value), (37_500, 345_000));
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_book_records_service_change(pool: PgPool) {
    // Замена в сервисе на машине клиента пишет книжку; следующая — пробег + интервал (SPEC-16).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'customer', 'person', 'Эрлан')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) values ($1, $2, $3, '01KG123ABC', '01KG123ABC')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let service = |lines: Vec<SaleLineReq>, total: i64| {
        let mut req = takeaway(&w, lines, cash(total));
        req.sale_type = "service".into();
        req.master_id = Some(w.master);
        req.party_id = Some(party);
        req.vehicle_id = Some(vehicle);
        req.mileage_km = Some(85_000);
        req
    };
    // Канистра масла и фильтр, пробег 85 000.
    let sale = sell(
        &pool,
        &w.owner,
        service(
            vec![
                line("container", w.oil, 1, 200_000),
                line("piece", w.filter, 1, 50_000),
            ],
            250_000,
        ),
    )
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let books =
        avtodom_server::api::oil_book::books(&mut conn, w.owner.user.branch_id, Some(party), None)
            .await
            .unwrap();
    let b = &books[0];
    assert_eq!(b.records.len(), 1);
    assert!(
        b.records[0]
            .oil_text
            .contains("Масло 5W-30 4л · 1 кан. × 4 л")
    );
    assert!(b.records[0].filter_text.contains("Фильтр W712"));
    assert_eq!(b.next_km, Some(93_000));
    assert_eq!(
        b.next_date,
        b.records[0]
            .change_date
            .checked_add_days(chrono::Days::new(31))
    );
    drop(conn);
    // Полный возврат — запись из книжки пропадает.
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        sale.id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            lines: vec![
                ReturnLineReq { line_no: 1, qty: 1 },
                ReturnLineReq { line_no: 2, qty: 1 },
            ],
            payments: cash(250_000),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let books =
        avtodom_server::api::oil_book::books(&mut conn, w.owner.user.branch_id, Some(party), None)
            .await
            .unwrap();
    assert!(books[0].records.is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn own_oil_service_writes_oil_book_and_staff_pay(pool: PgPool) {
    // Масло клиента: в чеке только услуга, но это замена — запись в книжке с пробегом (SPEC-16);
    // сводка «кто сколько заработал» видит ставку мастера.
    let w = seed(&pool).await;
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'customer', 'person', 'Нурлан')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) values ($1, $2, $3, '01KG777AAA', '01KG777AAA')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(
        &w,
        vec![SaleLineReq {
            kind: "service".into(),
            gift: false,
            product_id: None,
            service_id: Some(w.service),
            qty: 1,
            unit_price_tyiyn: 20_000,
            seen_list_price_tyiyn: None,
        }],
        cash(20_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(party);
    req.vehicle_id = Some(vehicle);
    req.mileage_km = Some(120_500);
    req.declined = "  воздушный фильтр ".into();
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let books =
        avtodom_server::api::oil_book::books(&mut conn, w.owner.user.branch_id, Some(party), None)
            .await
            .unwrap();
    let r = &books[0].records[0];
    assert_eq!(r.sale_id, Some(sale.id));
    assert_eq!(r.oil_text, "Замена масла");
    assert_eq!(r.mileage_km, Some(120_500));
    // Отказанные работы — в книжке, чтобы напомнить в следующий раз (ADR-055).
    assert_eq!(r.declined, "воздушный фильтр");
    let day: chrono::NaiveDate =
        sqlx::query_scalar("select business_date from sales where id = $1")
            .bind(sale.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let staff =
        avtodom_server::api::payroll::staff_pay(&mut conn, w.owner.user.branch_id, day, day)
            .await
            .unwrap();
    let master = staff.iter().find(|p| p.employee_id == w.master).unwrap();
    assert!(master.total_tyiyn > 0);
    assert!(
        master
            .items
            .iter()
            .any(|i| i.kind == "service_fee" && i.count == 1)
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn delivery_address_stays_on_check(pool: PgPool) {
    // Доставка бесплатная: в чеке только адрес (BLUEPRINT §6, вопросы 16 и 21).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 2, 1_000).await;
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    req.delivery_address = "  г. Бишкек, ул. Ахунбаева 98, гараж 4  ".into();
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(
        sale.delivery_address,
        "г. Бишкек, ул. Ахунбаева 98, гараж 4"
    );
    assert_eq!(sale.total_tyiyn, 50_000);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn loyalty_points_accrue_redeem_and_return(pool: PgPool) {
    // Баллы 0,5 % с оплаченного деньгами, только клиенту из бота; списание — оплата «bonus» (SPEC-19).
    use avtodom_server::api::loyalty::balance;
    use avtodom_server::api::telegram::bonus_notices;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 10_000).await;
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Эрлан', '0555123456')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let pay = |list: &[(&str, i64)]| -> Vec<PaymentReq> {
        list.iter()
            .map(|(m, a)| PaymentReq {
                method: (*m).into(),
                amount_tyiyn: *a,
            })
            .collect()
    };
    let sale = |qty: i64, payments: Vec<PaymentReq>| {
        let mut req = takeaway(&w, vec![line("piece", w.filter, qty, 50_000)], payments);
        req.party_id = Some(party);
        req
    };
    // Не подключён к боту — баллов нет, оплатить ими нельзя.
    sell(&pool, &w.owner, sale(1, cash(50_000))).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(balance(&mut conn, party).await.unwrap(), 0);
    assert!(matches!(
        sell(
            &pool,
            &w.owner,
            sale(1, pay(&[("bonus", 100), ("cash", 49_900)]))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values (555, $1, '+996555123456')")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    // 1000 с наличными → 5 баллов.
    sell(&pool, &w.owner, sale(2, cash(100_000))).await.unwrap();
    assert_eq!(balance(&mut conn, party).await.unwrap(), 500);
    // Больше, чем есть, не списать.
    assert!(matches!(
        sell(
            &pool,
            &w.owner,
            sale(1, pay(&[("bonus", 600), ("cash", 49_400)]))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    // Списали 5, остальное картой: начисление только с 495 с → 2,48.
    let paid = sell(
        &pool,
        &w.owner,
        sale(1, pay(&[("bonus", 500), ("card", 49_500)])),
    )
    .await
    .unwrap();
    assert_eq!(balance(&mut conn, party).await.unwrap(), 248);
    drop(conn);
    // Возврат: баллы обратно, начисленное с этого чека снимается.
    let ret = |payments: Vec<PaymentReq>| ReturnReq {
        op_id: Uuid::now_v7(),
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
        payments,
    };
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(
        return_sale_tx(
            &mut tx,
            &w.owner,
            paid.id,
            ret(pay(&[("bonus", 600), ("cash", 49_400)]))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    drop(tx);
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        paid.id,
        ret(pay(&[("bonus", 500), ("cash", 49_500)])),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(balance(&mut conn, party).await.unwrap(), 500);
    // Клиенту в бот — по сообщению на чек.
    let notices = bonus_notices(&mut conn).await.unwrap();
    assert_eq!(notices.len(), 3);
    assert!(
        notices[0].text.contains("начислено 5."),
        "{}",
        notices[0].text
    );
    assert!(notices[1].text.contains("списано 5,") && notices[1].text.contains("начислено 2,48"));
    let book = say(&mut conn, 555, "/баллы").await;
    assert!(book.contains("Баллов: 5 "), "{book}");
}

fn pay(list: &[(&str, i64)]) -> Vec<PaymentReq> {
    list.iter()
        .map(|(m, a)| PaymentReq {
            method: (*m).into(),
            amount_tyiyn: *a,
        })
        .collect()
}

async fn return_one(
    pool: &PgPool,
    ctx: &Ctx,
    sale: Uuid,
    payments: Vec<PaymentReq>,
) -> Result<avtodom_server::api::sales::SaleOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = return_sale_tx(
        &mut tx,
        ctx,
        sale,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments,
        },
    )
    .await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn returns_respect_debt_points_and_bank_fee(pool: PgPool) {
    // Ревью денежных цепочек: возврат не уменьшает погашенный долг, баллы не обналичиваются,
    // комиссия возвращается только та, что была у исходного чека (ADR-054).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 10_000).await;
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone, inn) values ($1, $2, 'customer', 'person', 'Эрлан', '0555123456', '12345678901234')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    // Долг погасили — вернуть «с долга» нельзя, только деньгами.
    // Наличные в кассе, чтобы было что выдать при возврате.
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    )
    .await
    .unwrap();
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        pay(&[("debt", 50_000)]),
    );
    req.party_id = Some(party);
    let debt_sale = sell(&pool, &w.owner, req).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    avtodom_server::api::parties::add_ledger(
        &mut tx,
        &w.owner,
        avtodom_server::api::parties::LedgerEntry {
            party_id: party,
            kind: "repayment",
            amount: -50_000,
            doc_type: "repayment",
            doc_id: Some(Uuid::now_v7()),
            comment: "",
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        return_one(&pool, &w.owner, debt_sale.id, pay(&[("debt", 50_000)])).await,
        Err(AppError::Validation(_))
    ));
    return_one(&pool, &w.owner, debt_sale.id, cash(50_000))
        .await
        .unwrap();

    // Чек баллами: деньгами не вернуть, баллами — можно.
    sqlx::query(
        "insert into telegram_customers (chat_id, party_id, phone) values (77, $1, '0555123456')",
    )
    .bind(party)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, doc_type, doc_id) values ($1, $2, $3, 'accrual', 50000, 'adjust', $1)")
        .bind(Uuid::now_v7())
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        pay(&[("bonus", 50_000)]),
    );
    req.party_id = Some(party);
    let bonus_sale = sell(&pool, &w.owner, req).await.unwrap();
    assert!(matches!(
        return_one(&pool, &w.owner, bonus_sale.id, cash(50_000)).await,
        Err(AppError::Validation(_))
    ));
    return_one(&pool, &w.owner, bonus_sale.id, pay(&[("bonus", 50_000)]))
        .await
        .unwrap();

    // Наличный чек, возвращённый на карту, комиссии «назад» не даёт; карточный — даёт свою.
    let cash_sale = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    let card_sale = sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("piece", w.filter, 4, 50_000)],
            pay(&[("card", 200_000)]),
        ),
    )
    .await
    .unwrap();
    let back = return_one(&pool, &w.owner, cash_sale.id, pay(&[("card", 50_000)]))
        .await
        .unwrap();
    let fee = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "select coalesce(sum(fee_tyiyn), 0)::bigint from sale_payments where sale_id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(fee(back.id).await, 0);
    let back = return_one(&pool, &w.owner, card_sale.id, pay(&[("card", 50_000)]))
        .await
        .unwrap();
    assert_eq!(fee(back.id).await, -250);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn revision_counts_from_stock_at_count_and_skips_merged(pool: PgPool) {
    // Пересчитали 10, потом продали 2, провели — остаток 8, а не «излишек 2» (ADR-054).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 10_000).await;
    let rev = Uuid::now_v7();
    sqlx::query("insert into revisions (id, branch_id, number, user_id, device_id) values ($1, $2, 1, $3, $1)")
        .bind(rev)
        .bind(w.owner.user.branch_id)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count) values ($1, $2, 10, $3, 10)")
        .bind(rev)
        .bind(w.filter)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let out = post_revision_tx(
        &mut tx,
        &w.owner,
        rev,
        PostReq {
            op_id: Uuid::now_v7(),
            comment: "пересчёт".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(out.lines[0].expected_qty, 10);
    assert_eq!(pool_of(&pool, w.filter).await.0, 8);

    // Карточку объединили после пересчёта — ревизия не проводится.
    let rev2 = Uuid::now_v7();
    sqlx::query("insert into revisions (id, branch_id, number, user_id, device_id) values ($1, $2, 2, $3, $1)")
        .bind(rev2)
        .bind(w.owner.user.branch_id)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count) values ($1, $2, 8, $3, 8)")
        .bind(rev2)
        .bind(w.filter)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update products set archived = true where id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(
        post_revision_tx(
            &mut tx,
            &w.owner,
            rev2,
            PostReq {
                op_id: Uuid::now_v7(),
                comment: "пересчёт".into(),
            },
        )
        .await,
        Err(AppError::Conflict(_))
    ));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn forgiven_debt_does_not_pay_cashier_later(pool: PgPool) {
    // Прощённый долг: процент кассира по нему больше не ждёт (ADR-054).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, inn) values ($1, $2, 'customer', 'person', 'Долгов', '12345678901234')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        pay(&[("debt", 50_000)]),
    );
    req.party_id = Some(party);
    let mut tx = pool.begin().await.unwrap();
    avtodom_server::api::payroll::ensure_cashier_rules(&mut tx, &w.owner, w.cashier)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    sell(&pool, &w.owner, req).await.unwrap();
    let pending = || {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select coalesce(sum(debt_remaining_tyiyn), 0)::bigint from payroll_pending where party_id = $1")
                .bind(party)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(pending().await, 50_000);
    let mut tx = pool.begin().await.unwrap();
    avtodom_server::api::payroll::forgive_pending(&mut tx, &w.owner, party, 50_000)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(pending().await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn loss_making_check_does_not_charge_cashier(pool: PgPool) {
    // Чек в минус (подарок без покупки) процента не даёт и с кассира не вычитается (ADR-058).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 50_000).await;
    let mut tx = pool.begin().await.unwrap();
    avtodom_server::api::payroll::ensure_cashier_rules(&mut tx, &w.owner, w.cashier)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // Продаём дешевле закупки «без сети» — валовая отрицательная.
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 5_000)], cash(5_000));
    req.offline = true;
    req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
    sell(&pool, &w.owner, req).await.unwrap();
    let percent: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1 and kind = 'revenue_percent'",
    )
    .bind(w.cashier)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(percent, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_check_spends_points_only_if_balance_allows(pool: PgPool) {
    // Чек с баллами из очереди (онлайн-отправка оборвалась без ответа) проводится, если остатка
    // хватает на момент проведения; не хватает — отказ (ADR-059).
    use avtodom_server::api::loyalty::balance;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Баллов', '0555000111')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into telegram_customers (chat_id, party_id, phone) values (91, $1, '0555000111')",
    )
    .bind(party)
    .execute(&pool)
    .await
    .unwrap();
    let offline_sale = |payments: Vec<PaymentReq>| {
        let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], payments);
        req.party_id = Some(party);
        req.offline = true;
        req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
        req
    };
    // Баллов нет — офлайн-чек с баллами отклоняется.
    assert!(matches!(
        sell(
            &pool,
            &w.owner,
            offline_sale(pay(&[("bonus", 10_000), ("cash", 40_000)]))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    // 20 000 с наличными → 100 баллов.
    let mut big = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 2_000_000)],
        cash(2_000_000),
    );
    big.party_id = Some(party);
    sell(&pool, &w.owner, big).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(balance(&mut conn, party).await.unwrap(), 10_000);
    // Хватает — чек из очереди проводится, баллы списаны, с денег начислено 0,5 %.
    let posted = sell(
        &pool,
        &w.owner,
        offline_sale(pay(&[("bonus", 10_000), ("cash", 40_000)])),
    )
    .await
    .unwrap();
    assert!(posted.number > 0);
    assert_eq!(balance(&mut conn, party).await.unwrap(), 200);
    // Остатка уже мало — следующий такой чек отклоняется, в минус баллы не уходят.
    assert!(matches!(
        sell(
            &pool,
            &w.owner,
            offline_sale(pay(&[("bonus", 10_000), ("cash", 40_000)]))
        )
        .await,
        Err(AppError::Validation(_))
    ));
    assert_eq!(balance(&mut conn, party).await.unwrap(), 200);
}

/// Текст ответа бота на сообщение из чата `chat`.
async fn say(conn: &mut sqlx::PgConnection, chat: i64, text: &str) -> String {
    avtodom_server::api::telegram::handle_message(conn, chat, chat, Some(text), None)
        .await
        .unwrap()
        .text
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn telegram_customer_sees_oil_book_and_gets_reminders(pool: PgPool) {
    // Клиент подключается своим номером, видит книжку, напоминания уходят один раз (SPEC-18, ADR-051).
    use avtodom_server::api::telegram::{
        Keys, SharedContact, due_reminders, handle_message, mark_reminded,
    };
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Эрлан', '0555 12-34-56')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, brand) values ($1, $2, $3, '01KG123ABC', '01KG123ABC', 'Toyota')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(
        &w,
        vec![line("container", w.oil, 1, 200_000)],
        cash(200_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(party);
    req.vehicle_id = Some(vehicle);
    req.mileage_km = Some(85_000);
    sell(&pool, &w.owner, req).await.unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let chat = 555_000_222i64;
    let first = handle_message(&mut conn, chat, chat, Some("/start"), None)
        .await
        .unwrap();
    assert_eq!(first.keys, Keys::Contact);
    // Чужой контакт не подключает.
    let foreign = SharedContact {
        phone: "+996555123456".into(),
        user_id: Some(42),
    };
    assert!(
        handle_message(&mut conn, chat, chat, None, Some(foreign))
            .await
            .unwrap()
            .keys
            == Keys::Contact
    );
    let unknown = SharedContact {
        phone: "+996700000000".into(),
        user_id: Some(chat),
    };
    assert!(
        handle_message(&mut conn, chat, chat, None, Some(unknown))
            .await
            .unwrap()
            .text
            .contains("нет среди")
    );
    let own = SharedContact {
        phone: "+996555123456".into(),
        user_id: Some(chat),
    };
    let linked = handle_message(&mut conn, chat, chat, None, Some(own))
        .await
        .unwrap();
    assert_eq!(linked.keys, Keys::Customer);
    assert!(
        linked.text.contains("01KG123ABC (Toyota)"),
        "{}",
        linked.text
    );
    assert!(linked.text.contains("85 000 км") && linked.text.contains("на 93 000 км"));
    // Команды владельца клиенту не отвечают — только его книжка.
    let book = say(&mut conn, chat, "/сегодня").await;
    assert!(book.contains("Последняя замена") && !book.contains("Выручка"));

    // Через 31 день — не сегодня; ставим интервал 7 дней: напоминание «за неделю».
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
    sqlx::query("update party_vehicles set interval_days = 7 where id = $1")
        .bind(vehicle)
        .execute(&pool)
        .await
        .unwrap();
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!((due[0].chat_id, due[0].days_before), (chat, 7));
    assert!(due[0].text.contains("через 7 дней"));
    mark_reminded(&mut conn, &due[0]).await.unwrap();
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
    sqlx::query("update party_vehicles set interval_days = 1 where id = $1")
        .bind(vehicle)
        .execute(&pool)
        .await
        .unwrap();
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert!(due[0].text.contains("завтра"));

    // Кнопки клиента: «Баллы», «Отключиться» с подтверждением, «Назад» — к книжке.
    assert!(say(&mut conn, chat, "⭐ Баллы").await.contains("Баллов:"));
    assert!(
        say(&mut conn, chat, "🚪 Отключиться")
            .await
            .contains("Отключиться от бота?")
    );
    assert!(say(&mut conn, chat, "Назад").await.contains("01KG123ABC"));
    let left = handle_message(&mut conn, chat, chat, Some("Да, отключиться"), None)
        .await
        .unwrap();
    assert!(left.text.contains("вы отключены"));
    assert_eq!(left.keys, Keys::Contact);
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn telegram_bot_links_owner_and_forwards_events(pool: PgPool) {
    // Бот: привязка по коду, команды владельца, события журнала в чат (SPEC-18).
    use avtodom_server::api::telegram::{pending, save_cursor};
    let w = seed(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let chat = 777_000_111i64;
    let reply = say(&mut conn, chat, "/сегодня").await;
    assert!(reply.contains("Поделиться номером"));
    assert!(
        say(&mut conn, chat, "/start 000000")
            .await
            .contains("не подошёл")
    );
    sqlx::query("insert into telegram_codes (code, user_id, expires_at) values ('123456', $1, now() + interval '15 minutes')")
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        say(&mut conn, chat, "/start 123456")
            .await
            .starts_with("Готово")
    );
    // Код одноразовый.
    assert!(say(&mut conn, 1, "123456").await.contains("не подошёл"));
    // Кнопка «📊 Сегодня» — то же, что команда.
    let today = say(&mut conn, chat, "📊 Сегодня").await;
    assert!(
        today.contains("Выручка")
            && today.contains("ЧИСТАЯ ПРИБЫЛЬ")
            && today.contains("Заработали")
    );
    assert!(
        say(&mut conn, chat, "/долги")
            .await
            .contains("никто не должен")
    );

    // Первый проход ставит курсор на «сейчас» и ничего не шлёт.
    let (first, at) = pending(&mut conn).await.unwrap();
    assert!(first.is_empty());
    save_cursor(&mut conn, at.unwrap()).await.unwrap();
    sqlx::query(
        "insert into audit_log (id, branch_id, user_id, action, entity, data) values ($1, $2, $3, 'receipt.reverse', 'receipt', '{\"number\": 7}')",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(w.admin.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let (msgs, next) = pending(&mut conn).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].chat_id, chat);
    assert!(msgs[0].text.contains("Сторно прихода") && msgs[0].text.contains("№ 7"));
    save_cursor(&mut conn, next.unwrap()).await.unwrap();
    assert!(pending(&mut conn).await.unwrap().0.is_empty());
    // Владельцу в бот — и обычный чек: номер, сумма, как платили, кассир.
    receive(&pool, &w.admin, w.filter, 2, 2_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    let (msgs, _) = pending(&mut conn).await.unwrap();
    let sale_msg = msgs
        .iter()
        .find(|m| m.text.contains("🧾 Чек №"))
        .expect("сообщение о чеке");
    assert!(
        sale_msg.text.contains("наличными 500 с") && sale_msg.text.contains("кассир"),
        "{}",
        sale_msg.text
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_after_shift_close_notifies_owner(pool: PgPool) {
    // Чек без сети с наличными пришёл, когда смена его дня уже закрыта: деньги легли в кассу
    // после пересчёта — владелец получает уведомление с суммой (ADR-059).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let branch = w.owner.user.branch_id;
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, branch)
        .await
        .unwrap();
    drop(conn);
    let shift = Uuid::now_v7();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn)
         values ($1, $2, 7, (now() at time zone 'Asia/Bishkek')::date, $3, $4, $5, $1, 0)",
    )
    .bind(shift)
    .bind(branch)
    .bind(till)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let events = || {
        sqlx::query_as::<_, (serde_json::Value,)>(
            "select data from audit_log where action = 'sale.offline_after_close' order by created_at",
        )
        .fetch_all(&pool)
    };
    let offline = |amount: i64| {
        let mut req = takeaway(&w, vec![line("piece", w.filter, 1, amount)], cash(amount));
        req.offline = true;
        req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
        req
    };
    // Смена открыта — чек без сети уведомления не даёт.
    sell(&pool, &w.admin, offline(20_000)).await.unwrap();
    assert!(events().await.unwrap().is_empty());
    sqlx::query(
        "insert into shift_closes (id, shift_id, seq, expected_tyiyn, counted_tyiyn, diff_tyiyn, user_id, device_id)
         values ($1, $2, 1, 20000, 20000, 0, $3, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(shift)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    // Смена закрыта: онлайн-чек — обычная продажа без смены, уведомления нет.
    sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("piece", w.filter, 1, 15_000)], cash(15_000)),
    )
    .await
    .unwrap();
    assert!(events().await.unwrap().is_empty());
    // Чек без сети того же дня после закрытия — уведомление с суммой наличных.
    let late = sell(&pool, &w.admin, offline(30_000)).await.unwrap();
    let got = events().await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0["cash"], 30_000);
    assert_eq!(got[0].0["shift_number"], 7);
    assert_eq!(got[0].0["number"], late.number);
}

// ---------- Машины клиента в боте (SPEC-22) ----------

/// Нажатие кнопки под сообщением в чате `chat`.
async fn press(
    conn: &mut sqlx::PgConnection,
    chat: i64,
    data: &str,
) -> avtodom_server::api::telegram::Reply {
    avtodom_server::api::telegram::handle_callback(conn, chat, data)
        .await
        .unwrap()
}

/// Клиент-физлицо (или фирма), подключённый к боту из чата `chat`.
async fn bot_customer(pool: &PgPool, w: &World, name: &str, kind: &str, chat: i64) -> Uuid {
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', $3, $4, '')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .bind(kind)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values ($1, $2, '')")
        .bind(chat)
        .bind(party)
        .execute(pool)
        .await
        .unwrap();
    party
}

async fn staff_car(pool: &PgPool, w: &World, party: Uuid, plate: &str, active: bool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, active) values ($1, $2, $3, $4, plate_norm($4), $5)")
        .bind(id)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .bind(plate)
        .bind(active)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Замена в сервисе: канистра масла и фильтр на 2 500 с, пробег 85 000.
async fn service_change(pool: &PgPool, w: &World, party: Uuid, vehicle: Uuid) -> Uuid {
    let mut req = takeaway(
        w,
        vec![
            line("container", w.oil, 1, 200_000),
            line("piece", w.filter, 1, 50_000),
        ],
        cash(250_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(party);
    req.vehicle_id = Some(vehicle);
    req.mileage_km = Some(85_000);
    sell(pool, &w.owner, req).await.unwrap().id
}

async fn car_of(
    pool: &PgPool,
    party: Uuid,
    plate: &str,
) -> Option<(Uuid, bool, String, String, String)> {
    sqlx::query_as("select id, active, plate, brand, added_by from party_vehicles where party_id = $1 and plate_norm = $2")
        .bind(party)
        .bind(plate)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn plate_norm_in_database_matches_rust(pool: PgPool) {
    // Миграция заполнила номера функцией базы — правило то же, что в боте.
    use avtodom_server::domain::plate::{SAMPLES, normalize};
    for s in SAMPLES {
        let db: String = sqlx::query_scalar("select plate_norm($1)")
            .bind(s)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(db, normalize(s), "{s:?}");
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_customer_adds_finds_and_removes_cars(pool: PgPool) {
    use avtodom_server::api::bot_cars::added_text;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let chat = 700_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let other = bot_customer(&pool, &w, "Чужой Бакыт", "person", 700_002).await;
    // У другого клиента — машина с историей.
    let foreign = staff_car(&pool, &w, other, "01KG123ABC", true).await;
    service_change(&pool, &w, other, foreign).await;
    let mut conn = pool.acquire().await.unwrap();

    // Нет машин — бот сразу просит номер; кривой номер — подсказка, ожидание остаётся.
    let r = say(&mut conn, chat, "🚗 Мои машины").await;
    assert!(r.contains("Напишите госномер"), "{r}");
    assert!(
        say(&mut conn, chat, "01-k")
            .await
            .contains("Не похоже на госномер")
    );
    // «01 kg 777 аВс»: пробелы убраны, кириллица стала латиницей; затем марка и модель.
    let r = say(&mut conn, chat, "01 kg 777 аВс").await;
    assert!(r.contains("Марка и модель"), "{r}");
    let added = say(&mut conn, chat, "Toyota Camry").await;
    assert!(added.starts_with(&added_text("01KG777ABC")), "{added}");
    let (_, active, plate, brand, by) = car_of(&pool, me, "01KG777ABC").await.unwrap();
    assert!(active);
    assert_eq!(
        (plate.as_str(), brand.as_str(), by.as_str()),
        ("01KG777ABC", "Toyota", "client")
    );

    // Номер другого клиента: новая пустая машина, ответ как у нового номера, владельцу уведомление.
    press(&mut conn, chat, "add").await;
    say(&mut conn, chat, "01 КG 123 АВС").await;
    let conflict = press(&mut conn, chat, "skip").await;
    press(&mut conn, chat, "add").await;
    say(&mut conn, chat, "B 555 XY").await;
    let fresh = press(&mut conn, chat, "skip").await;
    assert_eq!(
        conflict.text.replace("01KG123ABC", "#"),
        fresh.text.replace("B555XY", "#")
    );
    let labels = |r: &avtodom_server::api::telegram::Reply| -> Vec<String> {
        r.inline.iter().flatten().map(|b| b.text.clone()).collect()
    };
    assert_eq!(labels(&conflict), labels(&fresh));
    assert!(!conflict.text.contains("85 000") && !conflict.text.contains("Бакыт"));
    let (mine, ..) = car_of(&pool, me, "01KG123ABC").await.unwrap();
    assert_ne!(mine, foreign);
    let mut c2 = pool.acquire().await.unwrap();
    let book =
        avtodom_server::api::oil_book::books(&mut c2, w.owner.user.branch_id, Some(me), Some(mine))
            .await
            .unwrap();
    assert!(book[0].records.is_empty());
    let alerts: Vec<(serde_json::Value,)> =
        sqlx::query_as("select data from audit_log where action = 'vehicle.plate_conflict'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].0["plate"], "01KG123ABC");
    assert_eq!(alerts[0].0["other"], "Чужой Бакыт");
    assert_eq!(alerts[0].0["client"], "Эрлан");

    // Своя убранная машина по номеру возвращается с историей, дубля нет.
    // Замену пишут на машину в списке; потом клиент её убирает (на убранную чек не пробить, ADR-062).
    let old = staff_car(&pool, &w, me, "A 555 BB", true).await;
    service_change(&pool, &w, me, old).await;
    sqlx::query("update party_vehicles set active = false where id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    press(&mut conn, chat, "add").await;
    let found = say(&mut conn, chat, "a555bb").await;
    assert!(found.starts_with("Нашли вашу машину"), "{found}");
    assert!(
        found.contains("85 000 км") && found.contains("Замены:"),
        "{found}"
    );
    let (id, active, ..) = car_of(&pool, me, "A555BB").await.unwrap();
    assert!(active && id == old);
    let n: i64 = sqlx::query_scalar(
        "select count(*) from party_vehicles where party_id = $1 and plate_norm = 'A555BB'",
    )
    .bind(me)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);

    // Убрать: подтверждение, машина пропала из списка, история и чек на месте.
    let ask = press(&mut conn, chat, &format!("rm:{old}")).await;
    assert!(ask.text.contains("Убрать A 555 BB"), "{}", ask.text);
    let list = press(&mut conn, chat, &format!("rmy:{old}")).await;
    assert!(list.text.contains("убрана"));
    assert!(!labels(&list).iter().any(|l| l.contains("A 555 BB")));
    assert!(!car_of(&pool, me, "A555BB").await.unwrap().1);
    let kept: i64 = sqlx::query_scalar("select count(*) from oil_changes where vehicle_id = $1")
        .bind(old)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 1);
    let removed: i64 = sqlx::query_scalar(
        "select count(*) from audit_log where action = 'vehicle.client_removed' and entity_id = $1",
    )
    .bind(old)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(removed, 1);
    // Снова тот же номер — машина с историей.
    press(&mut conn, chat, "add").await;
    let back = say(&mut conn, chat, "A-555-BB").await;
    assert!(
        back.starts_with("Нашли вашу машину") && back.contains("85 000 км"),
        "{back}"
    );

    // Кнопка меню отменяет ожидание: текст после неё — не номер.
    press(&mut conn, chat, "add").await;
    say(&mut conn, chat, "🚗 Мои машины").await;
    assert!(say(&mut conn, chat, "X 999 ZZ").await.contains("Баллов"));
    assert!(car_of(&pool, me, "X999ZZ").await.is_none());

    // 11-я машина не добавляется (сейчас 4 активные).
    for i in 0..6 {
        staff_car(&pool, &w, me, &format!("Z{i}000ZZ"), true).await;
    }
    assert!(
        press(&mut conn, chat, "add")
            .await
            .text
            .contains("уже 10 машин")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_notes_limits_and_foreign_chat(pool: PgPool) {
    let w = seed(&pool).await;
    let chat = 710_001i64;
    let stranger_chat = 710_002i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    bot_customer(&pool, &w, "Другой", "person", stranger_chat).await;
    let car = staff_car(&pool, &w, me, "01KG123ABC", true).await;
    let mut conn = pool.acquire().await.unwrap();

    // Пометка: длинная не принимается, обычная сохраняется и видна в карточке и в программе.
    assert!(
        press(&mut conn, chat, &format!("note:{car}"))
            .await
            .text
            .contains("до 500")
    );
    assert!(
        say(&mut conn, chat, &"а".repeat(501))
            .await
            .contains("Слишком длинно")
    );
    let saved = say(&mut conn, chat, "стук справа при повороте").await;
    assert!(
        saved.starts_with("Сохранено") && saved.contains("— стук справа"),
        "{saved}"
    );
    let note: Uuid = sqlx::query_scalar("select id from vehicle_notes where vehicle_id = $1")
        .bind(car)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut c2 = pool.acquire().await.unwrap();
    let books =
        avtodom_server::api::oil_book::books(&mut c2, w.owner.user.branch_id, Some(me), None)
            .await
            .unwrap();
    assert_eq!(books[0].client_notes.len(), 1);
    assert_eq!(books[0].client_notes[0].text, "стук справа при повороте");

    // Чужой чат: кнопки с номером чужой машины и пометки ничего не открывают и не меняют.
    for data in [
        format!("car:{car}"),
        format!("more:{car}:10"),
        format!("note:{car}"),
        format!("rm:{car}"),
        format!("rmy:{car}"),
        format!("dn:{note}"),
    ] {
        let r = press(&mut conn, stranger_chat, &data).await;
        assert!(r.text.contains("не найдена"), "{data}: {}", r.text);
        assert!(!r.text.contains("стук"));
    }
    assert!(car_of(&pool, me, "01KG123ABC").await.unwrap().1);
    let pending: i64 = sqlx::query_scalar("select count(*) from bot_input where chat_id = $1")
        .bind(stranger_chat)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(pending, 0);
    let deleted: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("select deleted_at from vehicle_notes where id = $1")
            .bind(note)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(deleted.is_none());

    // Владелец и неподключённый чат кнопок не получают.
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id) values (710009, $1, $2)")
        .bind(w.owner.user.id)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let owner = press(&mut conn, 710_009, &format!("car:{car}")).await;
    assert_eq!(owner.keys, avtodom_server::api::telegram::Keys::Owner);
    assert!(owner.inline.is_empty() && !owner.text.contains("01KG123ABC"));
    assert!(
        !say(&mut conn, 710_009, "🚗 Мои машины")
            .await
            .contains("01KG123ABC")
    );
    let nobody = press(&mut conn, 710_010, &format!("car:{car}")).await;
    assert_eq!(nobody.keys, avtodom_server::api::telegram::Keys::Contact);
    assert!(nobody.inline.is_empty());

    // Клиент удаляет свою пометку: из бота и с кассы пропадает, владельцу видна удалённой.
    let r = press(&mut conn, chat, &format!("dn:{note}")).await;
    assert!(
        r.text.starts_with("Пометка удалена") && !r.text.contains("стук"),
        "{}",
        r.text
    );
    let mut books =
        avtodom_server::api::oil_book::books(&mut c2, w.owner.user.branch_id, Some(me), None)
            .await
            .unwrap();
    assert!(books[0].client_notes[0].deleted);
    avtodom_server::api::oil_book::hide_deleted_notes(&mut books);
    assert!(books[0].client_notes.is_empty());

    // 31-я пометка не добавляется.
    for i in 0..30 {
        sqlx::query("insert into vehicle_notes (id, branch_id, vehicle_id, party_id, text) values ($1, $2, $3, $4, $5)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(car)
            .bind(me)
            .bind(format!("пометка {i}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    assert!(
        press(&mut conn, chat, &format!("note:{car}"))
            .await
            .text
            .contains("уже 30 пометок")
    );

    // Не больше 30 действий в минуту.
    for _ in 0..30 {
        sqlx::query("insert into audit_log (id, branch_id, action, entity, data) values ($1, $2, 'vehicle.client_note', 'vehicle', $3)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(serde_json::json!({ "chat": chat }))
            .execute(&pool)
            .await
            .unwrap();
    }
    assert!(
        press(&mut conn, chat, &format!("rmy:{car}"))
            .await
            .text
            .contains("Слишком часто")
    );
    assert!(car_of(&pool, me, "01KG123ABC").await.unwrap().1);

    // Фирма: машины смотрят, но не правят.
    let firm_chat = 710_003i64;
    let firm = bot_customer(&pool, &w, "ОсОО Фирма", "company", firm_chat).await;
    let firm_car = staff_car(&pool, &w, firm, "B777CC", true).await;
    assert!(
        press(&mut conn, firm_chat, "add")
            .await
            .text
            .contains("Машины фирмы ведёт точка")
    );
    assert!(
        press(&mut conn, firm_chat, &format!("rmy:{firm_car}"))
            .await
            .text
            .contains("ведёт точка")
    );
    let card = press(&mut conn, firm_chat, &format!("car:{firm_car}")).await;
    assert!(card.text.contains("B777CC"));
    assert!(
        !card
            .inline
            .iter()
            .flatten()
            .any(|b| b.data.starts_with("rm:"))
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_car_card_shows_check_total_minus_returns(pool: PgPool) {
    use avtodom_server::domain::money::format_som;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let chat = 720_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let car = staff_car(&pool, &w, me, "01KG123ABC", true).await;
    let sale = service_change(&pool, &w, me, car).await;
    // Фильтр вернули: в карточке итог чека минус возврат.
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        sale,
        ReturnReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 2, qty: 1 }],
            payments: cash(50_000),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // Запись из тетради — без суммы.
    sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km, oil_text, user_id) values ($1, $2, $3, $4, '2026-01-10', 70000, 'Тетрадное масло', $5)")
        .bind(Uuid::now_v7())
        .bind(w.owner.user.branch_id)
        .bind(car)
        .bind(me)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let books =
        avtodom_server::api::oil_book::books(&mut conn, w.owner.user.branch_id, Some(me), None)
            .await
            .unwrap();
    assert_eq!(books[0].records[0].sale_total_tyiyn, Some(200_000));
    assert_eq!(books[0].records[1].sale_total_tyiyn, None);
    let card = press(&mut conn, chat, &format!("car:{car}")).await.text;
    assert!(
        card.contains(&format!("85 000 км · {}", format_som(200_000))),
        "{card}"
    );
    assert!(card.contains("10.01.2026 · 70 000 км · —"), "{card}");
    assert!(card.contains("Масло: Масло 5W-30 4л") && card.contains("Фильтр: Фильтр W712"));
    assert!(card.contains("Следующая замена: на 93 000 км или до") && card.contains("что раньше"));
    assert!(!card.contains(&format_som(250_000)));

    // Больше 10 замен — кнопка «Ещё» со следующими.
    for d in 1..=10 {
        sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, oil_text, user_id) values ($1, $2, $3, $4, $5, 'старое', $6)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(car)
            .bind(me)
            .bind(chrono::NaiveDate::from_ymd_opt(2025, 1, d).unwrap())
            .bind(w.owner.user.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let card = press(&mut conn, chat, &format!("car:{car}")).await;
    let more = card
        .inline
        .iter()
        .flatten()
        .find(|b| b.text == "Ещё")
        .unwrap()
        .data
        .clone();
    let next = press(&mut conn, chat, &more).await.text;
    assert!(
        next.contains("продолжение") && next.contains("01.01.2025"),
        "{next}"
    );
}

// ---------- Вопросы владельца боту текстом (SPEC-21) ----------

/// Шаг поддельного сервиса разбора вопросов.
#[derive(Clone)]
enum Step {
    /// Ответить этим кодом и телом.
    Reply(u16, serde_json::Value),
    /// Вернуть текстом то, что пришло в результатах отчётов последнего сообщения.
    Echo,
}

/// Поддельный сервис разбора вопросов на 127.0.0.1: отдаёт заготовленные ответы по очереди
/// и запоминает каждый запрос (заголовки и тело).
#[derive(Clone, Default)]
struct FakeService {
    steps: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<Step>>>,
    seen: std::sync::Arc<std::sync::Mutex<Vec<(axum::http::HeaderMap, serde_json::Value)>>>,
}

impl FakeService {
    fn push(&self, steps: Vec<Step>) {
        self.steps.lock().unwrap().extend(steps);
    }
    fn requests(&self) -> Vec<serde_json::Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, b)| b.clone())
            .collect()
    }
    fn header(&self, i: usize, name: &str) -> String {
        self.seen.lock().unwrap()[i]
            .0
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    }
}

async fn fake_messages(
    axum::extract::State(f): axum::extract::State<FakeService>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    f.seen.lock().unwrap().push((headers, body.clone()));
    let step = f.steps.lock().unwrap().pop_front();
    match step {
        Some(Step::Reply(code, v)) => (
            axum::http::StatusCode::from_u16(code).unwrap(),
            axum::Json(v),
        ),
        Some(Step::Echo) => {
            let last = body["messages"].as_array().unwrap().last().unwrap().clone();
            let text = last["content"]
                .as_array()
                .unwrap()
                .iter()
                .map(|b| b["content"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            (axum::http::StatusCode::OK, axum::Json(final_text(&text)))
        }
        None => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "type": "error", "error": { "type": "api_error" } })),
        ),
    }
}

async fn fake_service() -> (String, FakeService) {
    let fake = FakeService::default();
    let app = axum::Router::new()
        .route("/v1/messages", axum::routing::post(fake_messages))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

const SERVICE_KEY: &str = "test-secret-key-7f3a9c";

/// Ответ модели: размышление (пустое, с подписью), текст и вызов отчёта.
fn tool_use(name: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "thinking", "thinking": "", "signature": "sig-abc-123" },
            { "type": "text", "text": "Смотрю отчёт." },
            { "type": "tool_use", "id": "tu_1", "name": name, "input": input }
        ],
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 50 }
    })
}

fn final_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "msg_2",
        "type": "message",
        "role": "assistant",
        "content": [
            { "type": "thinking", "thinking": "", "signature": "sig-final" },
            { "type": "text", "text": text }
        ],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 150, "output_tokens": 30 }
    })
}

async fn ask(
    conn: &mut sqlx::PgConnection,
    a: &avtodom_server::api::assistant::Assistant,
    chat: i64,
    text: &str,
) -> String {
    avtodom_server::api::telegram::handle_message_with(conn, Some(a), chat, chat, Some(text), None)
        .await
        .unwrap()
        .text
}

async fn link_owner(pool: &PgPool, w: &World, chat: i64) {
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id) values ($1, $2, $3)")
        .bind(chat)
        .bind(w.owner.user.id)
        .bind(w.owner.user.branch_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn bishkek_today(pool: &PgPool) -> String {
    sqlx::query_scalar::<_, String>(
        "select to_char((now() at time zone 'Asia/Bishkek')::date, 'YYYY-MM-DD')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Ни в журнале вопросов, ни в памяти разговора нет ключа сервиса.
async fn assert_no_key_stored(pool: &PgPool) {
    let n: i64 = sqlx::query_scalar(
        "select (select count(*) from bot_questions where bot_questions::text like '%' || $1 || '%')
              + (select count(*) from bot_dialog where bot_dialog::text like '%' || $1 || '%')",
    )
    .bind(SERVICE_KEY)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(n, 0, "ключ попал в базу");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_answers_from_profit_report(pool: PgPool) {
    // Вопрос → вызов отчёта → результат → ответ с теми же цифрами, что «📊 Сегодня» (SPEC-21).
    use avtodom_server::api::assistant::Assistant;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 30_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    let chat = 880_001i64;
    link_owner(&pool, &w, chat).await;
    let (url, fake) = fake_service().await;
    let a = Assistant::new(SERVICE_KEY.into(), Some(url), None, 100);
    let today = bishkek_today(&pool).await;
    fake.push(vec![
        Step::Reply(
            200,
            tool_use(
                "profit_totals",
                serde_json::json!({ "from": today, "to": today }),
            ),
        ),
        Step::Echo,
    ]);
    let mut conn = pool.acquire().await.unwrap();
    let reply = ask(&mut conn, &a, chat, "сколько заработали сегодня?").await;

    // Чистая прибыль до тыйына — как в отчёте «📊 Сегодня».
    let button = say(&mut conn, chat, "📊 Сегодня").await;
    let net = button
        .lines()
        .find_map(|l| l.trim().strip_prefix("= ЧИСТАЯ ПРИБЫЛЬ: "))
        .unwrap()
        .to_string();
    assert_ne!(net, "0 с");
    assert!(reply.contains(&format!("Чистая прибыль: {net}")), "{reply}");
    // Кнопки отвечают сами, без обращения к сервису.
    assert_eq!(fake.requests().len(), 2);

    let reqs = fake.requests();
    // Ключ — в заголовке; без temperature; инструкция кешируется и без даты.
    assert_eq!(fake.header(0, "x-api-key"), SERVICE_KEY);
    assert!(reqs[0].get("temperature").is_none());
    assert_eq!(reqs[0]["max_tokens"], 4096);
    assert_eq!(reqs[0]["tools"].as_array().unwrap().len(), 11);
    let system = reqs[0]["system"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(system["cache_control"]["type"], "ephemeral");
    let dmy = format!("{}.{}.{}", &today[8..10], &today[5..7], &today[0..4]);
    assert!(!system["text"].as_str().unwrap().contains(&dmy));
    let first = reqs[0]["messages"].as_array().unwrap();
    assert_eq!(first.len(), 1);
    let q = first[0]["content"].as_str().unwrap();
    assert!(q.contains(&dmy) && q.contains("сколько заработали сегодня?"));
    // Второй запрос: ответ модели целиком, с размышлением и подписью, затем только результаты.
    let second = reqs[1]["messages"].as_array().unwrap();
    assert_eq!(second.len(), 3);
    assert_eq!(second[1]["role"], "assistant");
    assert_eq!(
        second[1]["content"],
        tool_use(
            "profit_totals",
            serde_json::json!({ "from": today, "to": today })
        )["content"]
    );
    assert_eq!(second[2]["role"], "user");
    let results = second[2]["content"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["type"], "tool_result");
    assert_eq!(results[0]["tool_use_id"], "tu_1");
    assert!(results[0].get("is_error").is_none());

    // Журнал: отчёт, токены, ответ.
    let (tools, input, output, answer, error): (
        serde_json::Value,
        i64,
        i64,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "select tools, input_tokens, output_tokens, answer, error from bot_questions where chat_id = $1",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tools[0]["name"], "profit_totals");
    assert_eq!((input, output), (300, 50));
    assert_eq!(answer.as_deref(), Some(reply.as_str()));
    assert!(error.is_none());

    // Уточнение: прошлый вопрос и ответ уходят простым текстом, без размышлений.
    fake.push(vec![Step::Reply(200, final_text("Пожалуйста."))]);
    assert_eq!(
        ask(&mut conn, &a, chat, "а спасибо за ответ").await,
        "Пожалуйста."
    );
    let third = fake.requests()[2]["messages"].as_array().unwrap().clone();
    assert_eq!(third.len(), 3);
    assert_eq!(third[0]["content"], "сколько заработали сегодня?");
    assert_eq!(third[1]["role"], "assistant");
    assert_eq!(third[1]["content"].as_str(), Some(reply.as_str()));
    assert!(third[2]["content"].as_str().unwrap().contains("а спасибо"));
    assert_no_key_stored(&pool).await;

    // «Сколько заработали вчера» — та же чистая прибыль, что отчёт о прибыли за вчера.
    let mut late = takeaway(&w, vec![line("piece", w.filter, 2, 47_550)], cash(95_100));
    late.offline = true;
    late.client_time = Some(chrono::Utc::now() - chrono::Duration::hours(24));
    sell(&pool, &w.owner, late).await.unwrap();
    let yesterday: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date - 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    let report = avtodom_server::api::reports::totals_for(
        &mut conn,
        w.owner.user.branch_id,
        yesterday,
        yesterday,
    )
    .await
    .unwrap();
    assert!(report.sales_count == 1 && report.net_tyiyn != 0);
    let day = yesterday.format("%Y-%m-%d").to_string();
    fake.push(vec![
        Step::Reply(
            200,
            tool_use(
                "profit_totals",
                serde_json::json!({ "from": day, "to": day }),
            ),
        ),
        Step::Echo,
    ]);
    let reply = ask(&mut conn, &a, chat, "сколько заработали вчера?").await;
    assert!(
        reply.contains(&format!(
            "Чистая прибыль: {}",
            avtodom_server::domain::money::format_som(report.net_tyiyn)
        )) && reply.contains(&yesterday.format("за %d.%m.%Y").to_string()),
        "{reply}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_debts_match_buttons(pool: PgPool) {
    // «Кто должен больше всех» — первая строка «📒 Долги»; два клиента «Ромашка» — список и
    // просьба уточнить (SPEC-21).
    use avtodom_server::api::assistant::Assistant;
    let w = seed(&pool).await;
    for (name, debt) in [
        ("ООО Ромашка", 1_500_050i64),
        ("Ромашка Плюс", 300_000),
        ("Иванов", 2_000_000),
    ] {
        sqlx::query("insert into parties (id, branch_id, role, kind, name, balance_tyiyn) values ($1, $2, 'customer', 'company', $3, $4)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(name)
            .bind(debt)
            .execute(&pool)
            .await
            .unwrap();
    }
    let chat = 880_002i64;
    link_owner(&pool, &w, chat).await;
    let (url, fake) = fake_service().await;
    let a = Assistant::new(SERVICE_KEY.into(), Some(url), None, 100);
    let mut conn = pool.acquire().await.unwrap();
    fake.push(vec![
        Step::Reply(200, tool_use("debtors", serde_json::json!({}))),
        Step::Echo,
    ]);
    let reply = ask(&mut conn, &a, chat, "кто должен больше всех?").await;
    let button = say(&mut conn, chat, "📒 Долги").await;
    let top = button.lines().nth(1).unwrap().trim().to_string();
    assert_eq!(top, "Иванов — 20 000 с");
    let first = reply.lines().nth(1).unwrap();
    assert!(first.starts_with("1. ") && first.contains(&top), "{reply}");

    fake.push(vec![
        Step::Reply(
            200,
            tool_use("client_debt", serde_json::json!({ "query": "Ромашка" })),
        ),
        Step::Echo,
    ]);
    let reply = ask(&mut conn, &a, chat, "долг Ромашка").await;
    assert!(
        reply.contains("ООО Ромашка: долг 15 000,50 с")
            && reply.contains("Ромашка Плюс: долг 3 000 с")
            && reply.contains("Уточните"),
        "{reply}"
    );
    // Неверные параметры — результат с пометкой ошибки, а не сбой.
    fake.push(vec![
        Step::Reply(
            200,
            tool_use(
                "profit_by_day",
                serde_json::json!({ "from": "2026-01-01", "to": "2026-10-01" }),
            ),
        ),
        Step::Reply(200, final_text("Период слишком длинный, уточните.")),
    ]);
    ask(&mut conn, &a, chat, "прибыль по дням за год").await;
    let reqs = fake.requests();
    let last = reqs.last().unwrap()["messages"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(last["content"][0]["is_error"], true);
    assert!(
        last["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("не больше 62 дней")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_guards_errors_and_customers(pool: PgPool) {
    // Без ключа — как SPEC-18; сумма без отчёта не проходит; ошибки сервиса — понятный ответ и
    // запись с ошибкой; клиентский чат сервис не вызывает; журнал не правится (SPEC-21).
    use avtodom_server::api::assistant::Assistant;
    // Журнал сервера этого теста — в память: ключа в нём быть не должно.
    let logs = LogBuf::default();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish(),
    );
    let w = seed(&pool).await;
    let chat = 880_003i64;
    link_owner(&pool, &w, chat).await;
    let (url, fake) = fake_service().await;
    let a = Assistant::new(SERVICE_KEY.into(), Some(url), None, 100);
    let mut conn = pool.acquire().await.unwrap();

    // Без ключа — подсказка с кнопками, как раньше.
    let old = say(&mut conn, chat, "сколько заработали вчера?").await;
    assert!(old.starts_with("Нажмите кнопку внизу"), "{old}");
    // С ключом кнопки и команды работают сами.
    assert!(
        ask(&mut conn, &a, chat, "📊 Сегодня")
            .await
            .contains("ЧИСТАЯ ПРИБЫЛЬ")
    );
    assert!(!ask(&mut conn, &a, chat, "/долги").await.is_empty());
    assert!(
        ask(&mut conn, &a, chat, "/непонятно что")
            .await
            .starts_with("Нажмите кнопку внизу")
    );
    assert_eq!(fake.requests().len(), 0);

    // Сумма в ответе без вызова отчёта — заменяется.
    fake.push(vec![Step::Reply(
        200,
        final_text("Выручка за вчера 5 000 с."),
    )]);
    assert_eq!(
        ask(&mut conn, &a, chat, "сколько выручки вчера?").await,
        "Не нашёл данных, уточните вопрос."
    );
    // Отказ без цифр проходит как есть.
    fake.push(vec![Step::Reply(
        200,
        final_text("Этого я посмотреть не могу."),
    )]);
    assert_eq!(
        ask(&mut conn, &a, chat, "какая погода завтра?").await,
        "Этого я посмотреть не могу."
    );

    // Сбой сервиса, отказ по ключу, нет соединения — понятный ответ.
    fake.push(vec![
        Step::Reply(500, serde_json::json!({ "type": "error", "error": { "type": "api_error" } })),
        Step::Reply(
            401,
            serde_json::json!({ "type": "error", "error": { "type": "authentication_error", "message": format!("invalid x-api-key {SERVICE_KEY}") } }),
        ),
    ]);
    for q in ["сколько в кассе?", "что кончается?"] {
        assert!(
            ask(&mut conn, &a, chat, q)
                .await
                .starts_with("Нет связи с сервисом разбора вопросов")
        );
    }
    let offline = Assistant::new(
        SERVICE_KEY.into(),
        Some("http://127.0.0.1:1".into()),
        None,
        100,
    );
    assert!(
        ask(&mut conn, &offline, chat, "кто должен?")
            .await
            .starts_with("Нет связи с сервисом разбора вопросов")
    );
    let errors: Vec<Option<String>> = sqlx::query_scalar(
        "select error from bot_questions where chat_id = $1 order by asked_at, id",
    )
    .bind(chat)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(errors.len(), 5);
    assert_eq!(errors[0].as_deref(), Some("сумма в ответе без отчёта"));
    assert!(errors[1].is_none());
    assert_eq!(errors[2].as_deref(), Some("сервис ответил 500 api_error"));
    assert_eq!(
        errors[3].as_deref(),
        Some("сервис ответил 401 authentication_error")
    );
    assert_eq!(errors[4].as_deref(), Some("нет связи: нет соединения"));

    // Клиентский чат на тот же вопрос получает свою книжку; сервис не вызывается.
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Клиент', '0555000222')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values (880004, $1, '0555000222')")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let before = fake.requests().len();
    let book = ask(&mut conn, &a, 880_004, "сколько заработали сегодня?").await;
    assert!(
        book.contains("Баллов") && !book.contains("ПРИБЫЛЬ"),
        "{book}"
    );
    // Непривязанный чат — кнопка номера.
    assert!(
        ask(&mut conn, &a, 880_005, "сколько заработали сегодня?")
            .await
            .contains("Поделиться номером")
    );
    assert_eq!(fake.requests().len(), before);

    // Журнал вопросов не правится и не удаляется.
    assert!(
        sqlx::query("update bot_questions set answer = 'x'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("delete from bot_questions")
            .execute(&pool)
            .await
            .is_err()
    );
    assert_no_key_stored(&pool).await;
    let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("разбор вопросов"), "{logged}");
    assert!(!logged.contains(SERVICE_KEY));
}

/// Журнал сервера в память — для проверки, что ключ туда не пишется.
#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_limit_makes_no_call(pool: PgPool) {
    // 101-й вопрос за сутки — сообщение о лимите, обращения к сервису нет (SPEC-21).
    use avtodom_server::api::assistant::Assistant;
    let w = seed(&pool).await;
    let chat = 880_006i64;
    link_owner(&pool, &w, chat).await;
    sqlx::query(
        "insert into bot_questions (id, chat_id, user_id, branch_id, asked_at, question, answer)
         select gen_random_uuid(), $1, $2, $3,
                greatest(((now() at time zone 'Asia/Bishkek')::date)::timestamp at time zone 'Asia/Bishkek',
                         now() - interval '2 hours'),
                'вопрос ' || n, 'ответ'
         from generate_series(1, 100) n",
    )
    .bind(chat)
    .bind(w.owner.user.id)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();
    let (url, fake) = fake_service().await;
    let a = Assistant::new(SERVICE_KEY.into(), Some(url), None, 100);
    let mut conn = pool.acquire().await.unwrap();
    fake.push(vec![Step::Reply(200, final_text("не должно прийти"))]);
    let reply = ask(&mut conn, &a, chat, "сколько заработали сегодня?").await;
    assert!(
        reply.starts_with("Лимит вопросов на сегодня исчерпан"),
        "{reply}"
    );
    assert_eq!(fake.requests().len(), 0);
    // Вопрос сверх предела тоже в журнале.
    let n: i64 =
        sqlx::query_scalar("select count(*) from bot_questions where error = 'лимит вопросов'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 1);
    // Кнопки работают.
    assert!(
        ask(&mut conn, &a, chat, "📊 Сегодня")
            .await
            .contains("ЧИСТАЯ ПРИБЫЛЬ")
    );
}

async fn audit_data(pool: &PgPool, action: &str) -> Vec<serde_json::Value> {
    sqlx::query_scalar("select data from audit_log where action = $1 order by created_at")
        .bind(action)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_bypassing_checks_is_reported(pool: PgPool) {
    // Чек без сети принимается, но всё, что онлайн отклонили бы, уходит владельцу (ADR-062).
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    receive(&pool, &w.admin, w.filter, 10, 100_000).await;
    let client = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, inn, active) values ($1, $2, 'customer', 'person', 'Отключённый', '123', false)")
        .bind(client)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update employees set active = false where id = $1")
        .bind(w.cashier)
        .execute(&pool)
        .await
        .unwrap();
    // Онлайн такой чек не проходит.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        vec![PaymentReq {
            method: "debt".into(),
            amount_tyiyn: 50_000,
        }],
    );
    req.party_id = Some(client);
    assert!(sell(&pool, &w.admin, req).await.is_err());

    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        vec![PaymentReq {
            method: "debt".into(),
            amount_tyiyn: 50_000,
        }],
    );
    req.party_id = Some(client);
    req.offline = true;
    req.client_time = Some(chrono::Utc::now() - chrono::Duration::days(1));
    sell(&pool, &w.admin, req).await.unwrap();
    let alerts = audit_data(&pool, "sale.offline_bypass").await;
    assert_eq!(alerts.len(), 1);
    let reasons: Vec<String> = alerts[0]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons[0].starts_with("кассир «Кассир» отключён"));
    assert!(reasons[1].contains("в долг отключённому клиенту"));
    assert!(reasons[2].starts_with("проведён задним числом"));

    // Обычный чек без сети, без обходов — без уведомления.
    sqlx::query("update employees set active = true where id = $1")
        .bind(w.cashier)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    req.offline = true;
    req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
    sell(&pool, &w.admin, req).await.unwrap();
    assert_eq!(audit_data(&pool, "sale.offline_bypass").await.len(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn seen_price_is_trusted_only_offline(pool: PgPool) {
    // Прайс «который видела касса» онлайн не принимается: правка цены видна по прайсу сервера.
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 100_000).await;
    let mut l = line("piece", w.filter, 1, 40_000);
    l.seen_list_price_tyiyn = Some(40_000);
    sell(&pool, &w.admin, takeaway(&w, vec![l.clone()], cash(40_000)))
        .await
        .unwrap();
    assert_eq!(audit_data(&pool, "sale.price_override").await.len(), 1);
    // Без сети прайс мог смениться, пока чек лежал: это не правка кассира.
    let mut req = takeaway(&w, vec![l], cash(40_000));
    req.offline = true;
    req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(5));
    sell(&pool, &w.admin, req).await.unwrap();
    assert_eq!(audit_data(&pool, "sale.price_override").await.len(), 1);
    assert_eq!(audit_data(&pool, "sale.stale_price").await.len(), 1);
}
