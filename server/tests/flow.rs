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
        // Администратор — с правами по умолчанию, как новый в программе (SPEC-30 §5).
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
    sqlx::query("insert into services (id, branch_id, name, price_tyiyn, free_with_oil) values ($1, $2, 'Замена масла', 20000, true)")
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
        "insert into branch_products (branch_id, product_id, sale_price_tyiyn, pour_price_per_l_tyiyn) values ($1, $2, 50000, null), ($1, $3, 200000, null)",
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
    // Приход для проверок склада: оплачен из «Сейфа владельца» — касса смены не трогается.
    // «Из своих» больше нет (ADR-079); сейф платит только владелец, поэтому документ проводится
    // от того же пользователя с ролью владельца.
    let mut payer = ctx.clone();
    payer.user.role = Role::Owner;
    let safe = owner_safe(pool, ctx.user.branch_id).await;
    let mut tx = pool.begin().await.unwrap();
    let r = post_receipt_tx(
        &mut tx,
        &payer,
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
    r
}

/// Приход в долг новому поставщику — без денег, его сторнирует и администратор.
async fn receive_on_debt(
    pool: &PgPool,
    ctx: &Ctx,
    product: Uuid,
    qty: i64,
    cost: i64,
) -> avtodom_server::api::receipts::ReceiptOut {
    let sid = Uuid::now_v7();
    sqlx::query("insert into suppliers (id, name) values ($1, 'Поставщик в долг')")
        .bind(sid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'supplier', 'company', 'Поставщик в долг')")
        .bind(sid)
        .bind(ctx.user.branch_id)
        .execute(pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let r = post_receipt_tx(
        &mut tx,
        ctx,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            payment: Some("debt".into()),
            paid_from: None,
            paid_now_tyiyn: None,
            supplier_id: Some(sid),
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
    r
}

/// «Сейф владельца» филиала: из него тесты оплачивают приходы для склада.
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

/// Открытая смена на «Кассе»: возврат наличными без неё не проводится (ADR-065).
async fn open_till(pool: &PgPool, w: &World) -> Uuid {
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn)
         values ($1, $2, (select coalesce(max(number), 0) + 1 from shifts where branch_id = $2),
                 (now() at time zone 'Asia/Bishkek')::date, $3, $4, $5, $1, 0)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(till)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&mut *conn)
    .await
    .unwrap();
    till
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
        account_id: None,
        price_per_kg_tyiyn: None,
        method: "cash".into(),
        amount_tyiyn: amount,
        grams: None,
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
        claimed_user_id: None,
        mileage_km: None,
        mileage: None,
        mileage_unit: None,
        book_intervals: Vec::new(),
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

/// Право администратора сверх роли (ADR-082, ADR-097), например «Скидка в чеке». Ограничение
/// списка прав в базе снимается: тест не зависит от того, какие права в нём уже перечислены.
async fn grant(pool: &PgPool, w: &World, permission: &str) {
    sqlx::query(
        "alter table user_permissions drop constraint if exists user_permissions_permission_check",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into user_permissions (user_id, branch_id, permission, granted_by) values ($1, $2, $3, $4) on conflict do nothing",
    )
    .bind(w.admin.user.id)
    .bind(w.admin.user.branch_id)
    .bind(permission)
    .bind(w.owner.user.id)
    .execute(pool)
    .await
    .unwrap();
}

/// Данные клиента для чека «В сервис» (ADR-100): имя и фамилия, телефон. Телефон — и в привязке
/// бота клиента, чтобы подключение осталось действующим.
async fn ready_for_service(pool: &PgPool, party: Uuid) {
    sqlx::query(
        "update parties set first_name = case when kind = 'person' and first_name = '' then 'Имя' else first_name end,
                            last_name = case when kind = 'person' and last_name = '' then 'Фамилия' else last_name end,
                            phone = case when phone = '' then '0555000111' else phone end
         where id = $1",
    )
    .bind(party)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "update telegram_customers set phone = '0555000111' where party_id = $1 and phone = ''",
    )
    .bind(party)
    .execute(pool)
    .await
    .unwrap();
}

/// Клиент для чека «В сервис»: человек с именем, фамилией и телефоном и его машина.
async fn service_client(pool: &PgPool, w: &World) -> (Uuid, Uuid) {
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name, first_name, last_name, phone) values ($1, $2, 'customer', 'person', 'Асанов Бакыт', 'Бакыт', 'Асанов', '0700 11-22-33')")
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
    (party, vehicle)
}

/// Вид замены из справочника строкой чека (ADR-096).
fn kind_line(service: Uuid, qty: i64, price: i64) -> SaleLineReq {
    SaleLineReq {
        kind: "service".into(),
        gift: false,
        product_id: None,
        service_id: Some(service),
        qty,
        unit_price_tyiyn: price,
        seen_list_price_tyiyn: None,
    }
}

/// Чек «В сервис»: мастер, клиент с машиной и пробег (ADR-100).
fn in_service(w: &World, (party, vehicle): (Uuid, Uuid), mut req: SaleReq) -> SaleReq {
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(party);
    req.vehicle_id = Some(vehicle);
    req.mileage_km = Some(85_000);
    req
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
    // Розлив дешевле прайса — скидка: у администратора есть право (ADR-097).
    grant(&pool, &w, "discount").await;
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

/// Одна цена масла (ADR-074): канистра 4 л за 2 000 с — розлив 500 с за литр, 4 л розливом
/// стоят ровно как канистра, 2,5 л — 1 250 с; прайс строки розлива — от цены канистры.
#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn pour_price_comes_from_canister_price(pool: PgPool) {
    let w = seed(&pool).await;
    receive(&pool, &w.owner, w.oil, 12_000, 300_000).await;
    let mut conn = pool.acquire().await.unwrap();
    let p = avtodom_server::api::catalog::product_by_id(&mut conn, &w.admin.user, w.oil)
        .await
        .unwrap();
    assert_eq!(p.sale_price_tyiyn, 200_000);
    assert_eq!(p.pour_price_per_l_tyiyn, Some(50_000));
    drop(conn);
    let sale = sell(
        &pool,
        &w.admin,
        takeaway(
            &w,
            vec![
                line("pour", w.oil, 4000, 50_000),
                line("pour", w.oil, 2500, 50_000),
            ],
            cash(325_000),
        ),
    )
    .await
    .unwrap();
    assert_eq!(sale.lines[0].amount_tyiyn, 200_000);
    assert_eq!(sale.lines[1].amount_tyiyn, 125_000);
    assert!(sale.lines.iter().all(|l| l.list_price_tyiyn == 50_000));
    // Цена розлива совпала с прайсом — это не правка цены кассиром.
    assert!(audit_data(&pool, "sale.price_override").await.is_empty());
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
async fn service_needs_kind_and_pays_its_rate(pool: PgPool) {
    // «В сервис» — только с видом замены (ADR-096): мастеру — ставка услуги, ставка за чек из
    // настроек новым чекам не начисляется; возврат товара ставку не снимает, возврат услуги — снимает.
    let w = seed(&pool).await;
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
    receive(&pool, &w.admin, w.filter, 10, 100).await;
    let client = service_client(&pool, &w).await;
    sqlx::query("insert into settings (branch_id, key, value) values ($1, 'sales', '{\"oil_change_master_fee_tyiyn\": 5000}')")
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();

    // «В сервис» без мастера не проводится.
    let mut req = in_service(
        &w,
        client,
        takeaway(
            &w,
            vec![
                line("piece", w.filter, 1, 50_000),
                kind_line(w.service, 1, 20_000),
            ],
            cash(70_000),
        ),
    );
    req.master_id = None;
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

    // Без вида замены — 422 «Выберите вид замены».
    let req = in_service(
        &w,
        client,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    );
    match sell(&pool, &w.owner, req).await {
        Err(AppError::Validation(m)) => assert_eq!(m, "Выберите вид замены"),
        other => panic!("ждали 422, получили {:?}", other.map(|s| s.number)),
    }

    // С видом замены: фильтр — не масло, услуга по цене клиенту; мастеру — ставка услуги (30 с),
    // а не 50 с за чек из настроек.
    let req = in_service(
        &w,
        client,
        takeaway(
            &w,
            vec![
                line("piece", w.filter, 2, 50_000),
                kind_line(w.service, 1, 20_000),
            ],
            cash(120_000),
        ),
    );
    let service_sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(service_sale.master_fee_tyiyn, 3000);
    assert_eq!(service_sale.lines[1].list_price_tyiyn, 20_000);
    assert_eq!(service_sale.lines[1].master_fee_tyiyn, 3000);

    // Возврат фильтра ставку не снимает, возврат услуги — снимает.
    let ret = |line_no, amount| ReturnReq {
        op_id: Uuid::now_v7(),
        battery_cash: false,
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no, qty: 1 }],
        payments: cash(amount),
        business_date: None,
    };
    let mut tx = pool.begin().await.unwrap();
    let partial = return_sale_tx(&mut tx, &w.owner, service_sale.id, ret(1, 50_000))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(partial.master_fee_tyiyn, 0);
    let mut tx = pool.begin().await.unwrap();
    let back = return_sale_tx(&mut tx, &w.owner, service_sale.id, ret(2, 20_000))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(back.master_fee_tyiyn, -3000);
    let accrued: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where employee_id = $1",
    )
    .bind(w.master)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(accrued, 0);
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
    let client = service_client(&pool, &w).await;
    let req = in_service(
        &w,
        client,
        takeaway(&w, vec![service_line.clone()], cash(40_000)),
    );
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

    // С товаром в чеке: ставки за чек больше нет (ADR-096) — мастеру только ставка услуги.
    let req = in_service(
        &w,
        client,
        takeaway(
            &w,
            vec![line("piece", w.filter, 1, 500), service_line],
            cash(40_500),
        ),
    );
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(sale.master_fee_tyiyn, 20_000);
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
        account_id: None,
        price_per_kg_tyiyn: None,
        method: method.into(),
        amount_tyiyn: amount,
        grams: None,
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
            battery_cash: false,
            comment: "вернули".into(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: vec![pay("debt", 1000)],
            business_date: None,
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
            account_id: None,
            price_per_kg_tyiyn: None,
            method: "card".into(),
            amount_tyiyn: 100_000,
            grams: None,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
                account_id: None,
                price_per_kg_tyiyn: None,
                method: "cash".into(),
                amount_tyiyn: 50_000,
                grams: None,
            },
            PaymentReq {
                account_id: None,
                price_per_kg_tyiyn: None,
                method: "debt".into(),
                amount_tyiyn: 50_000,
                grams: None,
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
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: vec![
                PaymentReq {
                    account_id: None,
                    price_per_kg_tyiyn: None,
                    method: "cash".into(),
                    amount_tyiyn: 25_000,
                    grams: None,
                },
                PaymentReq {
                    account_id: None,
                    price_per_kg_tyiyn: None,
                    method: "debt".into(),
                    amount_tyiyn: 25_000,
                    grams: None,
                },
            ],
            business_date: None,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
    let client = service_client(&pool, &w).await;
    let req = in_service(
        &w,
        client,
        takeaway(
            &w,
            vec![
                line("piece", w.filter, 2, 5_000),
                kind_line(w.service, 1, 20_000),
            ],
            cash(30_000),
        ),
    );
    let sale = sell(&pool, &w.owner, req).await.unwrap();
    let paid = master_total(pool.clone(), w.master).await;
    assert_eq!(paid, 3_000);
    // Вид замены возвращают отдельно: ставка уходит вместе с ним.
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        sale.id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 2, qty: 1 }],
            payments: cash(20_000),
            business_date: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    for _ in 0..2 {
        let mut tx = pool.begin().await.unwrap();
        return_sale_tx(
            &mut tx,
            &w.owner,
            sale.id,
            ReturnReq {
                op_id: Uuid::now_v7(),
                battery_cash: false,
                comment: String::new(),
                lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
                payments: cash(5_000),
                business_date: None,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
        battery_cash: false,
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty }],
        payments: cash(100 * qty),
        business_date: None,
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
    let r = receive_on_debt(&pool, &w.admin, w.filter, 5, 3000).await;
    let rev = |op| ReverseReq {
        op_id: op,
        comment: "ошибка".into(),
        in_correction: false,
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
    // Цены ниже прайса — скидка: право у администратора есть (ADR-097).
    grant(&pool, &w, "discount").await;
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
    // Розлив ниже прайса — скидка: право у администратора есть (ADR-097).
    grant(&pool, &w, "discount").await;
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
        Err(AppError::ForbiddenWith(_))
    ));
    assert!(matches!(
        transfer(&w.owner, 60_000).await,
        Err(AppError::Validation(_))
    ));
    let out = transfer(&w.owner, 50_000).await.unwrap();
    assert_eq!(out.value_tyiyn, Some(1_000_000));
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
        (50_000, Some(1_000_000))
    );
    assert_eq!(
        (back.to.stock_ml, back.to.value_tyiyn),
        (5_000, Some(50_000))
    );
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
    grant(&pool, &w, "discount").await;
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
                account_id: None,
                price_per_kg_tyiyn: None,
                method: "debt".into(),
                amount_tyiyn: 1_000,
                grams: None,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments: cash(50_000),
            business_date: None,
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
            in_correction: false,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let (party, vehicle) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'customer', 'person', 'Эрлан')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    ready_for_service(&pool, party).await;
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm) values ($1, $2, $3, '01KG123ABC', '01KG123ABC')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let service = |mut lines: Vec<SaleLineReq>, total: i64| {
        lines.push(kind_line(w.service, 1, 0));
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
            battery_cash: false,
            comment: String::new(),
            lines: vec![
                ReturnLineReq { line_no: 1, qty: 1 },
                ReturnLineReq { line_no: 2, qty: 1 },
                ReturnLineReq { line_no: 3, qty: 1 },
            ],
            payments: cash(250_000),
            business_date: None,
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
    ready_for_service(&pool, party).await;
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
                account_id: None,
                price_per_kg_tyiyn: None,
                method: (*m).into(),
                amount_tyiyn: *a,
                grams: None,
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
        battery_cash: false,
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
        payments,
        business_date: None,
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
            account_id: None,
            price_per_kg_tyiyn: None,
            method: (*m).into(),
            amount_tyiyn: *a,
            grams: None,
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
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
            payments,
            business_date: None,
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
    avtodom_server::api::parties::post_adjust_tx(
        &mut tx,
        &w.owner,
        avtodom_server::api::parties::AdjustReq {
            op_id: Uuid::now_v7(),
            party_id: party,
            amount_tyiyn: -50_000,
            comment: "прощаем".into(),
        },
    )
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
    ready_for_service(&pool, party).await;
    sqlx::query("insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, brand) values ($1, $2, $3, '01KG123ABC', '01KG123ABC', 'Toyota')")
        .bind(vehicle)
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let mut req = takeaway(
        &w,
        vec![
            line("container", w.oil, 1, 200_000),
            kind_line(w.service, 1, 0),
        ],
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
    // Одна машина — книжка целиком (SPEC-30 §10).
    assert!(book.contains("Замены:") && !book.contains("Выручка"));

    // Через 31 день — не сегодня; ставим интервал 7 дней: напоминание «за неделю».
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
    sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) select id, branch_id, 'oil', 8000, 7 from party_vehicles where id = $1 on conflict (vehicle_id, slot) do update set interval_days = excluded.interval_days")
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
    sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) select id, branch_id, 'oil', 8000, 1 from party_vehicles where id = $1 on conflict (vehicle_id, slot) do update set interval_days = excluded.interval_days")
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
    use avtodom_server::api::telegram::{advance_cursor, mark_sent, pending};
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
    let first = pending(&mut conn).await.unwrap();
    assert!(first.is_empty());
    sqlx::query(
        "insert into audit_log (id, branch_id, user_id, action, entity, data) values ($1, $2, $3, 'receipt.reverse', 'receipt', '{\"number\": 7}')",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(w.admin.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let msgs = pending(&mut conn).await.unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].chat_id, chat);
    assert!(msgs[0].text.contains("Сторно прихода") && msgs[0].text.contains("№ 7"));
    mark_sent(&mut conn, &msgs[0]).await.unwrap();
    advance_cursor(&mut conn).await.unwrap();
    assert!(pending(&mut conn).await.unwrap().is_empty());
    // Владельцу в бот — и обычный чек: номер, сумма, как платили, кассир.
    receive(&pool, &w.admin, w.filter, 2, 2_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
    )
    .await
    .unwrap();
    let msgs = pending(&mut conn).await.unwrap();
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
    // Чек без сети с наличными пришёл, когда его смена уже закрыта: деньги легли в кассу
    // после пересчёта — владелец получает уведомление с суммой (ADR-059).
    let w = seed(&pool).await;
    // Цены ниже прайса — скидка: право у администратора есть (ADR-097).
    grant(&pool, &w, "discount").await;
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn offline_sale_finds_its_shift_by_time(pool: PgPool) {
    // Смен в день несколько (ADR-071): чек без сети относится к смене, открытой в момент чека.
    // Её кассир — кассир чека; смена с тех пор закрыта — владельцу уведомление. Чек между
    // сменами остаётся за кассиром с устройства и, пока новую смену не открыли, сходится.
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let branch = w.owner.user.branch_id;
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, branch)
        .await
        .unwrap();
    drop(conn);
    let on_shift = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_cashier) values ($1, $2, 'Бермет', true)")
        .bind(on_shift)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    let shift = Uuid::now_v7();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn, opened_at)
         values ($1, $2, 3, (now() at time zone 'Asia/Bishkek')::date, $3, $4, $5, $1, 0, now() - interval '60 minutes')",
    )
    .bind(shift)
    .bind(branch)
    .bind(till)
    .bind(on_shift)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into shift_closes (id, shift_id, seq, expected_tyiyn, counted_tyiyn, diff_tyiyn, user_id, device_id, closed_at)
         values ($1, $2, 1, 0, 0, 0, $3, $1, now() - interval '30 minutes')",
    )
    .bind(Uuid::now_v7())
    .bind(shift)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let offline = |minutes: i64| {
        let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 10_000)], cash(10_000));
        req.offline = true;
        req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(minutes));
        req
    };
    let events = || {
        sqlx::query_as::<_, (serde_json::Value,)>(
            "select data from audit_log where action = 'sale.offline_after_close' order by created_at",
        )
        .fetch_all(&pool)
    };
    // Пробит в смене № 3 (кассир Бермет), пришёл после её закрытия.
    let during = sell(&pool, &w.admin, offline(45)).await.unwrap();
    assert_eq!(during.cashier_id, on_shift);
    let got = events().await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0["shift_number"], 3);
    // Пробит после закрытия, новой смены нет — кассир с устройства, уведомления нет.
    let between = sell(&pool, &w.admin, offline(10)).await.unwrap();
    assert_eq!(between.cashier_id, w.cashier);
    assert_eq!(events().await.unwrap().len(), 1);
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

/// Замена в сервисе: канистра масла и фильтр на 2 500 с, вид замены (бесплатно с нашим маслом),
/// пробег 85 000.
async fn service_change(pool: &PgPool, w: &World, party: Uuid, vehicle: Uuid) -> Uuid {
    ready_for_service(pool, party).await;
    let mut req = takeaway(
        w,
        vec![
            line("container", w.oil, 1, 200_000),
            line("piece", w.filter, 1, 50_000),
            kind_line(w.service, 1, 0),
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
    // Возврат наличными — только в открытую смену (ADR-065).
    open_till(&pool, &w).await;
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
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 2, qty: 1 }],
            payments: cash(50_000),
            business_date: None,
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
    // Строки книжки по видам и следующая замена по каждой (SPEC-27).
    assert!(
        card.contains("Масло двигателя: Масло 5W-30 4л")
            && card.contains("Фильтр масляный: Фильтр W712"),
        "{card}"
    );
    assert!(
        card.contains("что наступит раньше")
            && card.contains("Масло двигателя: на 93 000 км или до")
            && card.contains("Фильтр масляный: на 93 000 км или до"),
        "{card}"
    );
    assert!(!card.contains(&format_som(250_000)));

    // Мастер чека — в записи (SPEC-30 §10).
    assert!(card.contains("  Мастер: Мастер"), "{card}");

    // Книжка целиком, от новой к старой (SPEC-30 §10): «Ещё» больше нет, длинная книжка уходит
    // несколькими сообщениями, запись не рвётся между ними.
    for d in 1..=28 {
        sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km, oil_text, filter_text, declined, user_id) values ($1, $2, $3, $4, $5, $6, $7, 'Фильтр Sakura C-1821', 'воздушный фильтр', $8)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(car)
            .bind(me)
            .bind(chrono::NaiveDate::from_ymd_opt(2025, 1, d).unwrap())
            .bind(10_000 + i32::try_from(d).unwrap() * 1000)
            .bind("Масло Totachi Niro LV 0W-20, 4 литра, синтетика ".repeat(3))
            .bind(w.owner.user.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let card = press(&mut conn, chat, &format!("car:{car}")).await;
    assert!(
        !card.inline.iter().flatten().any(|b| b.text == "Ещё"),
        "{:?}",
        card.inline
    );
    let text = &card.text;
    assert!(
        text.contains("01.01.2025 · 11 000 км · —") && text.contains("28.01.2025 · 38 000 км"),
        "{text}"
    );
    assert_eq!(
        text.matches("Мастер рекомендовал: воздушный фильтр")
            .count(),
        28
    );
    // Новые сверху: замена по чеку раньше тетрадной 10.01.2026, та — раньше январских 2025.
    let pos = |s: &str| text.find(s).unwrap();
    assert!(pos("85 000 км") < pos("10.01.2026") && pos("10.01.2026") < pos("28.01.2025"));
    assert!(pos("28.01.2025") < pos("01.01.2025"));
    let parts = avtodom_server::api::telegram::split_message(text);
    assert!(parts.len() > 1, "{}", parts.len());
    for d in 1..=28 {
        let head = format!("{d:02}.01.2025 · ");
        let part = parts.iter().find(|p| p.contains(&head)).unwrap();
        // Вся запись — в одном сообщении: дата и рекомендация мастера рядом.
        let rest = &part[part.find(&head).unwrap()..];
        assert!(
            rest.split("\n\n")
                .next()
                .unwrap()
                .contains("Мастер рекомендовал"),
            "{rest}"
        );
    }
    // Старая кнопка «Ещё» открывает ту же карточку целиком.
    let old = press(&mut conn, chat, &format!("more:{car}:10")).await.text;
    assert!(
        old.contains("01.01.2025") && old.contains("85 000 км"),
        "{old}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_book_shows_master_and_services(pool: PgPool) {
    // В записи книжки: масло литрами или канистрами, фильтры, услуги чека, мастер и что он
    // рекомендовал (SPEC-30 §10).
    use avtodom_server::api::telegram::customer_book;
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let (cat, washer) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into categories (id, name, kind) values ($1, 'Автохимия', 'other')")
        .bind(cat)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into products (id, category_id, name, unit) values ($1, $2, 'Омыватель', 'piece')",
    )
    .bind(washer)
    .bind(cat)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into branch_products (branch_id, product_id, sale_price_tyiyn) values ($1, $2, 30000)",
    )
    .bind(w.owner.user.branch_id)
    .bind(washer)
    .execute(&pool)
    .await
    .unwrap();
    receive(&pool, &w.admin, washer, 3, 15_000).await;
    let chat = 720_101i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let car = staff_car(&pool, &w, me, "01KG555XYZ", true).await;
    let mut service = line("piece", washer, 1, 30_000);
    service.kind = "service".into();
    service.product_id = None;
    service.service_id = Some(w.service);
    service.unit_price_tyiyn = 20_000;
    let mut req = takeaway(
        &w,
        vec![
            line("container", w.oil, 1, 200_000),
            line("piece", w.filter, 1, 50_000),
            line("piece", washer, 1, 30_000),
            service,
        ],
        cash(300_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(me);
    req.vehicle_id = Some(car);
    req.mileage_km = Some(91_000);
    req.declined = "салонный фильтр".into();
    sell(&pool, &w.owner, req).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let book = customer_book(&mut conn, me).await.unwrap();
    assert!(
        book.contains("01KG555XYZ") && book.contains("Замены:"),
        "{book}"
    );
    assert!(book.contains("91 000 км"), "{book}");
    assert!(
        book.contains("  Масло двигателя: Масло 5W-30 4л · 1 кан. × 4 л")
            && book.contains("  Фильтр масляный: Фильтр W712 · 1 шт"),
        "{book}"
    );
    // Услуги чека — да; товар категории «не в книжке» — нет (решение владельца, SPEC-27).
    assert!(book.contains("  Услуги: Замена масла"), "{book}");
    assert!(!book.contains("Омыватель"), "{book}");
    assert!(book.contains("  Мастер: Мастер"), "{book}");
    assert!(
        book.contains("  Мастер рекомендовал: салонный фильтр"),
        "{book}"
    );
    // Больше трёх машин — по последней замене, вся история — в карточке машины.
    for i in 0..3 {
        staff_car(&pool, &w, me, &format!("02KG{i:03}AAA"), true).await;
    }
    let book = customer_book(&mut conn, me).await.unwrap();
    assert!(
        book.contains("Последняя замена: ") && book.contains("Вся история машины"),
        "{book}"
    );
    assert!(book.contains("  Мастер: Мастер"), "{book}");
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
    paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
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
    fn path(&self, i: usize) -> String {
        self.paths.lock().unwrap()[i].clone()
    }
}

/// Результаты функций последнего сообщения запроса — текстом.
fn function_results(body: &serde_json::Value) -> String {
    let last = body["contents"].as_array().unwrap().last().unwrap().clone();
    last["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let r = &p["functionResponse"]["response"];
            r["result"]
                .as_str()
                .or_else(|| r["error"].as_str())
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn fake_messages(
    axum::extract::State(f): axum::extract::State<FakeService>,
    axum::extract::Path(path): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    f.paths.lock().unwrap().push(path);
    f.seen.lock().unwrap().push((headers, body.clone()));
    let step = f.steps.lock().unwrap().pop_front();
    match step {
        Some(Step::Reply(code, v)) => (
            axum::http::StatusCode::from_u16(code).unwrap(),
            axum::Json(v),
        ),
        Some(Step::Echo) => (
            axum::http::StatusCode::OK,
            axum::Json(final_text(&function_results(&body))),
        ),
        None => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(service_error(500, "INTERNAL", "")),
        ),
    }
}

async fn fake_service() -> (String, FakeService) {
    let fake = FakeService::default();
    let app = axum::Router::new()
        .route("/v1beta/models/{*rest}", axum::routing::post(fake_messages))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}"), fake)
}

const SERVICE_KEY: &str = "test-secret-key-7f3a9c";

/// Ошибка сервиса: `{"error": {"code", "message", "status"}}`.
fn service_error(code: u16, status: &str, message: &str) -> serde_json::Value {
    serde_json::json!({ "error": { "code": code, "message": message, "status": status } })
}

/// Подключение к подставному сервису: без повторов, как одна попытка.
fn fake_assistant(url: String) -> avtodom_server::api::assistant::Assistant {
    avtodom_server::api::assistant::Assistant::new(SERVICE_KEY.into(), Some(url), None, 100)
        .with_retry_pauses(Vec::new())
}

/// Ответ модели: размышление, вызов отчёта с подписью размышления.
fn tool_use(name: &str, input: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [
                    { "text": "Смотрю отчёт.", "thought": true },
                    { "functionCall": { "id": "fc_1", "name": name, "args": input }, "thoughtSignature": "sig-abc-123" }
                ]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 150, "cachedContentTokenCount": 50, "candidatesTokenCount": 20 }
    })
}

fn final_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [
                    { "text": "Пишу ответ.", "thought": true },
                    { "text": text, "thoughtSignature": "sig-final" }
                ]
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": { "promptTokenCount": 150, "candidatesTokenCount": 25, "thoughtsTokenCount": 5 }
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

/// Владелец нажимает «💬 Спросить»: только после этого текст уходит в разбор (ADR-067).
async fn start_asking(
    conn: &mut sqlx::PgConnection,
    a: &avtodom_server::api::assistant::Assistant,
    chat: i64,
) {
    assert!(
        ask(conn, a, chat, "💬 Спросить")
            .await
            .starts_with("Задайте вопрос о делах точки")
    );
}

async fn link_owner(pool: &PgPool, w: &World, chat: i64) {
    // Личный чат: номер чата — номер владельца в Телеграме (ADR-070).
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, tg_user_id) values ($1, $2, $3, $1)")
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
    let a = fake_assistant(url);
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
    start_asking(&mut conn, &a, chat).await;
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
    // Ключ — в заголовке, модель — в адресе; без temperature; инструкция без даты.
    assert_eq!(fake.header(0, "x-goog-api-key"), SERVICE_KEY);
    assert_eq!(
        fake.path(0),
        format!(
            "{}:generateContent",
            avtodom_server::api::assistant::DEFAULT_MODEL
        )
    );
    assert!(reqs[0]["generationConfig"].get("temperature").is_none());
    assert_eq!(
        reqs[0]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
        "low"
    );
    assert_eq!(
        reqs[0]["tools"][0]["functionDeclarations"]
            .as_array()
            .unwrap()
            .len(),
        21
    );
    assert_eq!(
        reqs[0]["toolConfig"]["functionCallingConfig"]["mode"],
        "AUTO"
    );
    let system = reqs[0]["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    let dmy = format!("{}.{}.{}", &today[8..10], &today[5..7], &today[0..4]);
    assert!(!system.contains(&dmy));
    let first = reqs[0]["contents"].as_array().unwrap();
    assert_eq!(first.len(), 1);
    let q = first[0]["parts"][0]["text"].as_str().unwrap();
    assert!(q.contains(&dmy) && q.contains("сколько заработали сегодня?"));
    // Второй запрос: ответ модели целиком, с размышлением и подписью, затем ответ функции.
    let second = reqs[1]["contents"].as_array().unwrap();
    assert_eq!(second.len(), 3);
    assert_eq!(
        second[1],
        tool_use(
            "profit_totals",
            serde_json::json!({ "from": today, "to": today })
        )["candidates"][0]["content"]
    );
    assert_eq!(second[2]["role"], "user");
    let results = second[2]["parts"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let fr = &results[0]["functionResponse"];
    assert_eq!(fr["name"], "profit_totals");
    assert_eq!(fr["id"], "fc_1");
    assert!(fr["response"]["result"].is_string() && fr["response"].get("error").is_none());

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
    let third = fake.requests()[2]["contents"].as_array().unwrap().clone();
    assert_eq!(third.len(), 3);
    assert_eq!(third[0]["parts"][0]["text"], "сколько заработали сегодня?");
    assert_eq!(third[1]["role"], "model");
    assert_eq!(third[1]["parts"][0]["text"].as_str(), Some(reply.as_str()));
    assert!(
        third[2]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("а спасибо")
    );
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
    let a = fake_assistant(url);
    let mut conn = pool.acquire().await.unwrap();
    start_asking(&mut conn, &a, chat).await;
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
    let last = reqs.last().unwrap()["contents"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let response = &last["parts"][0]["functionResponse"]["response"];
    assert!(response.get("result").is_none());
    assert!(
        response["error"]
            .as_str()
            .unwrap()
            .contains("не больше 62 дней")
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_guards_errors_and_customers(pool: PgPool) {
    // Без ключа — как SPEC-18; сумма без отчёта не проходит; ошибки сервиса — понятный ответ и
    // запись с ошибкой; клиентский чат сервис не вызывает; журнал не правится (SPEC-21).
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
    let a = fake_assistant(url);
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
    start_asking(&mut conn, &a, chat).await;

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
    let off_topic = avtodom_server::api::assistant::OFF_TOPIC;
    fake.push(vec![Step::Reply(200, final_text(off_topic))]);
    assert_eq!(
        ask(&mut conn, &a, chat, "какая погода завтра?").await,
        off_topic
    );

    // Сбой сервиса, отказ по ключу, нет соединения — понятный ответ.
    fake.push(vec![
        Step::Reply(500, service_error(500, "INTERNAL", "")),
        Step::Reply(
            401,
            service_error(
                401,
                "UNAUTHENTICATED",
                &format!("API key not valid {SERVICE_KEY}"),
            ),
        ),
    ]);
    assert!(
        ask(&mut conn, &a, chat, "сколько в кассе?")
            .await
            .starts_with("Нет связи с сервисом разбора вопросов")
    );
    // Отказ по ключу — не «нет связи», а подсказка проверить настройки (ADR-063).
    assert!(
        ask(&mut conn, &a, chat, "что кончается?")
            .await
            .starts_with("Ключ сервиса разбора вопросов не подходит")
    );
    let offline = fake_assistant("http://127.0.0.1:1".into());
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
    assert_eq!(errors[2].as_deref(), Some("сервис ответил 500 INTERNAL"));
    assert_eq!(
        errors[3].as_deref(),
        Some("сервис ответил 401 UNAUTHENTICATED")
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
    // Кнопки «💬 Спросить» у клиента нет, и её текст режим не включает.
    let book = ask(&mut conn, &a, 880_004, "💬 Спросить").await;
    assert!(book.contains("Баллов"), "{book}");
    let book = ask(&mut conn, &a, 880_004, "сколько заработали сегодня?").await;
    assert!(
        book.contains("Баллов") && !book.contains("ПРИБЫЛЬ"),
        "{book}"
    );
    // Непривязанный чат — кнопка номера.
    for t in ["💬 Спросить", "сколько заработали сегодня?"] {
        assert!(
            ask(&mut conn, &a, 880_005, t)
                .await
                .contains("Поделиться номером")
        );
    }
    assert_eq!(fake.requests().len(), before);
    let modes: Vec<i64> = sqlx::query_scalar("select chat_id from bot_question_mode")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(modes, vec![chat]);

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
    let a = fake_assistant(url);
    let mut conn = pool.acquire().await.unwrap();
    start_asking(&mut conn, &a, chat).await;
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

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_questions_only_in_ask_mode(pool: PgPool) {
    // Вопросы — только после «💬 Спросить»: вне режима текст — подсказка без обращения к
    // сервису; команды работают в режиме и его не закрывают; «✖ Закончить» и 10 минут без
    // сообщений закрывают режим; без ключа кнопки и режима нет (ADR-067).
    use avtodom_server::api::assistant::OFF_TOPIC;
    use avtodom_server::api::telegram::{Keys, handle_message, handle_message_with};
    let w = seed(&pool).await;
    let chat = 880_010i64;
    link_owner(&pool, &w, chat).await;
    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    let mut conn = pool.acquire().await.unwrap();
    let mode_rows = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("select count(*) from bot_question_mode")
            .fetch_one(&pool)
            .await
            .unwrap()
    };

    // Без ключа: кнопки нет, её текст режим не включает.
    let r = handle_message(&mut conn, chat, chat, Some("💬 Спросить"), None)
        .await
        .unwrap();
    assert!(r.text.starts_with("Нажмите кнопку внизу"), "{}", r.text);
    assert!(!r.text.contains("Спросить"));
    assert_eq!(r.keys, Keys::Owner);
    assert_eq!(mode_rows(pool.clone()).await, 0);

    // С ключом, но без режима: подсказка с кнопкой «💬 Спросить», сервис не вызывается.
    let r = handle_message_with(
        &mut conn,
        Some(&a),
        chat,
        chat,
        Some("сколько заработали сегодня?"),
        None,
    )
    .await
    .unwrap();
    assert!(
        r.text.starts_with("Нажмите кнопку внизу") && r.text.contains("💬 Спросить"),
        "{}",
        r.text
    );
    assert_eq!(r.keys, Keys::OwnerAsk);
    // Несколько слов с командой в начале вне режима — команда, как в SPEC-18.
    assert!(
        ask(&mut conn, &a, chat, "сегодня сколько масла продали")
            .await
            .contains("ЧИСТАЯ ПРИБЫЛЬ")
    );
    assert_eq!(fake.requests().len(), 0);

    // «💬 Спросить» — режим включён, кнопки: «✖ Закончить» и кнопки владельца.
    let r = handle_message_with(&mut conn, Some(&a), chat, chat, Some("💬 Спросить"), None)
        .await
        .unwrap();
    assert_eq!(
        r.text,
        "Задайте вопрос о делах точки. Закончить — «✖ Закончить»"
    );
    assert_eq!(r.keys, Keys::OwnerAsking);
    assert_eq!(audit_data(&pool, "telegram.ask_on").await.len(), 1);

    // Вопрос не о делах точки — отказ из инструкции; сама инструкция содержит отказ дословно.
    fake.push(vec![Step::Reply(200, final_text(OFF_TOPIC))]);
    assert_eq!(
        ask(&mut conn, &a, chat, "какая погода завтра?").await,
        OFF_TOPIC
    );
    let reqs = fake.requests();
    assert_eq!(reqs.len(), 1);
    let system = reqs[0]["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(system.contains(&format!("«{OFF_TOPIC}»")));

    // Кнопки команд в режиме работают сами и режим не закрывают.
    let r = handle_message_with(&mut conn, Some(&a), chat, chat, Some("📊 Сегодня"), None)
        .await
        .unwrap();
    assert!(r.text.contains("ЧИСТАЯ ПРИБЫЛЬ"));
    assert_eq!(r.keys, Keys::OwnerAsking);
    assert!(!ask(&mut conn, &a, chat, "/долги").await.is_empty());
    assert_eq!(fake.requests().len(), 1);
    fake.push(vec![Step::Reply(200, final_text("Готово."))]);
    assert_eq!(
        ask(&mut conn, &a, chat, "а что кончается?").await,
        "Готово."
    );
    assert_eq!(fake.requests().len(), 2);

    // «✖ Закончить» — режим закрыт, память разговора очищена, текст снова — подсказка.
    let r = handle_message_with(&mut conn, Some(&a), chat, chat, Some("✖ Закончить"), None)
        .await
        .unwrap();
    assert!(r.text.starts_with("Режим вопросов закрыт"), "{}", r.text);
    assert_eq!(r.keys, Keys::OwnerAsk);
    assert_eq!(mode_rows(pool.clone()).await, 0);
    let dialog: i64 = sqlx::query_scalar("select count(*) from bot_dialog where chat_id = $1")
        .bind(chat)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(dialog, 0);
    assert!(
        ask(&mut conn, &a, chat, "сколько денег в кассе?")
            .await
            .starts_with("Нажмите кнопку внизу")
    );
    assert_eq!(fake.requests().len(), 2);
    assert_eq!(audit_data(&pool, "telegram.ask_off").await.len(), 1);

    // 10 минут без сообщений — режим закрыт: кнопки команд работают, первый текст —
    // сообщение о закрытии без обращения к сервису, следующий — подсказка.
    start_asking(&mut conn, &a, chat).await;
    sqlx::query("update bot_question_mode set last_at = now() - interval '11 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    let r = handle_message_with(&mut conn, Some(&a), chat, chat, Some("💵 Смена"), None)
        .await
        .unwrap();
    assert_eq!(r.keys, Keys::OwnerAsk);
    let r = handle_message_with(
        &mut conn,
        Some(&a),
        chat,
        chat,
        Some("сколько заработали сегодня?"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(r.text, "Режим вопросов закрыт — нажмите «💬 Спросить»");
    assert_eq!(r.keys, Keys::OwnerAsk);
    assert!(
        ask(&mut conn, &a, chat, "сколько заработали сегодня?")
            .await
            .starts_with("Нажмите кнопку внизу")
    );
    assert_eq!(fake.requests().len(), 2);

    // Сообщение в режиме продлевает его: 9 минут назад + вопрос — режим ещё открыт.
    start_asking(&mut conn, &a, chat).await;
    sqlx::query("update bot_question_mode set last_at = now() - interval '9 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    fake.push(vec![Step::Reply(200, final_text("Готово."))]);
    assert_eq!(ask(&mut conn, &a, chat, "что кончается?").await, "Готово.");
    let fresh: bool = sqlx::query_scalar(
        "select last_at > now() - interval '1 minute' from bot_question_mode where chat_id = $1",
    )
    .bind(chat)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(fresh);
    assert_eq!(fake.requests().len(), 3);
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
            account_id: None,
            price_per_kg_tyiyn: None,
            method: "debt".into(),
            amount_tyiyn: 50_000,
            grams: None,
        }],
    );
    req.party_id = Some(client);
    assert!(sell(&pool, &w.admin, req).await.is_err());

    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        vec![PaymentReq {
            account_id: None,
            price_per_kg_tyiyn: None,
            method: "debt".into(),
            amount_tyiyn: 50_000,
            grams: None,
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
    grant(&pool, &w, "discount").await;
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

// ---------- Доставка в Телеграм, привязка и рассылки (ADR-063) ----------

/// Заготовленные ответы по чатам: код и тело.
type Queued = std::collections::HashMap<i64, std::collections::VecDeque<(u16, serde_json::Value)>>;

/// Поддельный Телеграм на 127.0.0.1: принимает sendMessage, отвечает по заготовкам для чата и
/// запоминает доставленное.
#[derive(Clone, Default)]
struct FakeTelegram {
    /// Ответы по очереди для чата; кончились — 200.
    queued: std::sync::Arc<std::sync::Mutex<Queued>>,
    /// Чаты, которым всегда отвечаем этим кодом.
    always: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<i64, u16>>>,
    got: std::sync::Arc<std::sync::Mutex<Vec<(i64, String)>>>,
}

impl FakeTelegram {
    fn queue(&self, chat: i64, code: u16, body: serde_json::Value) {
        self.queued
            .lock()
            .unwrap()
            .entry(chat)
            .or_default()
            .push_back((code, body));
    }
    fn fail(&self, chat: i64, code: Option<u16>) {
        let mut a = self.always.lock().unwrap();
        match code {
            Some(c) => a.insert(chat, c),
            None => a.remove(&chat),
        };
    }
    fn texts(&self, chat: i64) -> Vec<String> {
        self.got
            .lock()
            .unwrap()
            .iter()
            .filter(|(c, _)| *c == chat)
            .map(|(_, t)| t.clone())
            .collect()
    }
}

async fn fake_send(
    axum::extract::State(f): axum::extract::State<FakeTelegram>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    let chat = body["chat_id"].as_i64().unwrap();
    let fail = f.always.lock().unwrap().get(&chat).copied();
    let queued = f
        .queued
        .lock()
        .unwrap()
        .get_mut(&chat)
        .and_then(|q| q.pop_front());
    let (code, reply) = match (fail, queued) {
        (Some(c), _) => (c, serde_json::json!({ "ok": false, "description": "fail" })),
        (None, Some(r)) => r,
        (None, None) => {
            f.got
                .lock()
                .unwrap()
                .push((chat, body["text"].as_str().unwrap().to_string()));
            (200, serde_json::json!({ "ok": true, "result": {} }))
        }
    };
    (
        axum::http::StatusCode::from_u16(code).unwrap(),
        axum::Json(reply),
    )
}

const TG_TOKEN: &str = "123456:tg-secret-token-q9z";

async fn fake_telegram() -> (avtodom_server::api::telegram::Api, FakeTelegram) {
    let fake = FakeTelegram::default();
    let app = axum::Router::new()
        .route("/{bot}/sendMessage", axum::routing::post(fake_send))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let api = avtodom_server::api::telegram::Api::new(&format!("http://{addr}"), TG_TOKEN)
        .with_pace(std::time::Duration::ZERO, std::time::Duration::ZERO);
    (api, fake)
}

/// Привязать чат к пользователю: `linked_at` на час назад, чтобы видеть события «из прошлого».
async fn link_user_chat(pool: &PgPool, user: Uuid, branch: Uuid, chat: i64, hours_ago: i32) {
    sqlx::query("insert into telegram_links (chat_id, user_id, branch_id, linked_at, tg_user_id) values ($1, $2, $3, now() - make_interval(hours => $4), $1)")
        .bind(chat)
        .bind(user)
        .bind(branch)
        .bind(hours_ago)
        .execute(pool)
        .await
        .unwrap();
}

/// Рассылка уже идёт час: курсор и начало — час назад.
async fn feed_started_hour_ago(pool: &PgPool) {
    sqlx::query(
        "insert into bot_state (key, value) values
           ('cursor', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'))),
           ('feed_since', to_jsonb(to_char((now() - interval '1 hour') at time zone 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')))
         on conflict (key) do update set value = excluded.value",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn journal_event(
    pool: &PgPool,
    branch: Uuid,
    action: &str,
    data: serde_json::Value,
    minutes_ago: i32,
) {
    sqlx::query("insert into audit_log (id, branch_id, action, entity, data, created_at) values ($1, $2, $3, 'test', $4, now() - make_interval(mins => $5))")
        .bind(Uuid::now_v7())
        .bind(branch)
        .bind(action)
        .bind(data)
        .bind(minutes_ago)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_feed_reaches_each_owner_chat_exactly_once(pool: PgPool) {
    // События — только чатам активных владельцев; одинаковое время у 150 событий, запись старше
    // уже отправленной, сбой одного из двух чатов — ничего не теряется и не повторяется (ADR-063).
    use avtodom_server::api::telegram::feed_pass;
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    let (owner_chat, admin_chat, second_chat) = (900_001i64, 900_002i64, 900_003i64);
    link_user_chat(&pool, w.owner.user.id, branch, owner_chat, 1).await;
    // Чат администратора (например, бывшего владельца) событий не получает.
    link_user_chat(&pool, w.admin.user.id, branch, admin_chat, 1).await;
    feed_started_hour_ago(&pool).await;
    sqlx::query(
        "insert into audit_log (id, branch_id, action, entity, data, created_at)
         select gen_random_uuid(), $1, 'receipt.reverse', 'receipt', jsonb_build_object('number', n),
                date_trunc('second', now()) - interval '10 minutes'
         from generate_series(1, 150) n",
    )
    .bind(branch)
    .execute(&pool)
    .await
    .unwrap();
    let (api, fake) = fake_telegram().await;
    let mut conn = pool.acquire().await.unwrap();

    // 429: проход остановлен, ничего не отмечено, пауза — сколько попросил Телеграм.
    fake.queue(
        owner_chat,
        429,
        serde_json::json!({ "ok": false, "parameters": { "retry_after": 3 } }),
    );
    let first = feed_pass(&mut conn, &api).await.unwrap();
    assert!(first.stopped);
    assert_eq!(first.pause, Some(std::time::Duration::from_secs(3)));
    assert!(fake.texts(owner_chat).is_empty());
    // Дальше — по 100 за проход, все 150 по одному разу.
    feed_pass(&mut conn, &api).await.unwrap();
    assert_eq!(fake.texts(owner_chat).len(), 100);
    feed_pass(&mut conn, &api).await.unwrap();
    feed_pass(&mut conn, &api).await.unwrap();
    let mut numbers: Vec<i64> = fake
        .texts(owner_chat)
        .iter()
        .map(|t| {
            t.split("№ ")
                .nth(1)
                .unwrap()
                .split(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (1..=150).collect::<Vec<_>>());
    assert!(fake.texts(admin_chat).is_empty());

    // Запись журнала стала видна позже уже отправленных, хотя её время раньше.
    journal_event(
        &pool,
        branch,
        "receipt.reverse",
        serde_json::json!({ "number": 777 }),
        12,
    )
    .await;
    feed_pass(&mut conn, &api).await.unwrap();
    let texts = fake.texts(owner_chat);
    assert_eq!(texts.len(), 151);
    assert!(texts[150].contains("№ 777"));

    // Второй владелец, его чат сбоит: первый получает всё сразу, второй — после починки, один раз.
    let second = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'owner2', 'x', 'owner', 'Второй')")
        .bind(second)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    link_user_chat(&pool, second, branch, second_chat, 0).await;
    for n in 801..=803 {
        journal_event(
            &pool,
            branch,
            "receipt.reverse",
            serde_json::json!({ "number": n }),
            0,
        )
        .await;
    }
    fake.fail(second_chat, Some(500));
    let failing = feed_pass(&mut conn, &api).await.unwrap();
    assert!(!failing.stopped && failing.pause.is_some());
    feed_pass(&mut conn, &api).await.unwrap();
    assert_eq!(fake.texts(owner_chat).len(), 154);
    assert!(fake.texts(second_chat).is_empty());
    fake.fail(second_chat, None);
    feed_pass(&mut conn, &api).await.unwrap();
    feed_pass(&mut conn, &api).await.unwrap();
    let got = fake.texts(second_chat);
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(fake.texts(owner_chat).len(), 154);

    // Отключённый владелец больше ничего не получает.
    sqlx::query("update users set active = false where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    journal_event(
        &pool,
        branch,
        "receipt.reverse",
        serde_json::json!({ "number": 900 }),
        0,
    )
    .await;
    feed_pass(&mut conn, &api).await.unwrap();
    assert_eq!(fake.texts(second_chat).len(), 3);
    assert_eq!(fake.texts(owner_chat).len(), 155);
}

#[tokio::test]
async fn bot_api_splits_long_text_and_keeps_token_out_of_logs() {
    use avtodom_server::api::telegram::{Api, Delivery, MAX_MESSAGE};
    let logs = LogBuf::default();
    let _logs = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish(),
    );
    let (api, fake) = fake_telegram().await;
    // Длинный текст — частями по строкам, каждая не длиннее предела.
    let text = (1..=300)
        .map(|i| format!("строка {i}: {}", "масло ".repeat(5)))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(api.send(1, &text).await, Delivery::Sent);
    let parts = fake.texts(1);
    assert!(parts.len() > 1);
    assert!(parts.iter().all(|p| p.chars().count() <= MAX_MESSAGE));
    assert_eq!(parts.join("\n"), text);
    // Бот заблокирован, чат не найден — пропуск, а не повтор.
    fake.queue(
        2,
        403,
        serde_json::json!({ "ok": false, "description": "Forbidden: bot was blocked by the user" }),
    );
    fake.queue(
        2,
        400,
        serde_json::json!({ "ok": false, "description": "Bad Request: chat not found" }),
    );
    assert_eq!(api.send(2, "a").await, Delivery::Skip);
    assert_eq!(api.send(2, "b").await, Delivery::Skip);
    // Нет связи: повтор всей пачкой, в журнале сервера ни ключа бота, ни адреса с ним.
    let offline = Api::new("http://127.0.0.1:1", TG_TOKEN);
    assert!(matches!(
        offline.send(3, "c").await,
        Delivery::Retry { all: true, .. }
    ));
    let logged = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("телеграм: сообщение не отправлено"),
        "{logged}"
    );
    assert!(
        !logged.contains(TG_TOKEN) && !logged.contains("tg-secret"),
        "{logged}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_shift_close_reports_its_own_day_once(pool: PgPool) {
    // Закрытие смены — одно сообщение «закрыта» с номером и итог её учётного дня, а не
    // календарного; сдача кассы — отдельным сообщением без «закрыта» (ADR-063).
    use avtodom_server::api::telegram::pending;
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    link_user_chat(&pool, w.owner.user.id, branch, 910_001, 1).await;
    feed_started_hour_ago(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, branch)
        .await
        .unwrap();
    let shift = Uuid::now_v7();
    sqlx::query(
        "insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id, opened_by, opened_device, opening_expected_tyiyn)
         values ($1, $2, 7, (now() at time zone 'Asia/Bishkek')::date - 1, $3, $4, $5, $1, 0)",
    )
    .bind(shift)
    .bind(branch)
    .bind(till)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    for (action, data) in [
        (
            "shift.close",
            serde_json::json!({ "expected": 100_000, "counted": 90_000, "diff": -10_000 }),
        ),
        (
            "shift.handover",
            serde_json::json!({ "number": 7, "expected": 100_000, "counted": 90_000, "diff": -10_000, "to_safe": 80_000, "left": 10_000 }),
        ),
    ] {
        sqlx::query("insert into audit_log (id, branch_id, user_id, action, entity, entity_id, data) values ($1, $2, $3, $4, 'shift', $5, $6)")
            .bind(Uuid::now_v7())
            .bind(branch)
            .bind(w.admin.user.id)
            .bind(action)
            .bind(shift)
            .bind(data)
            .execute(&pool)
            .await
            .unwrap();
    }
    let yesterday: String = sqlx::query_scalar(
        "select to_char((now() at time zone 'Asia/Bishkek')::date - 1, 'DD.MM.YYYY')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let msgs = pending(&mut conn).await.unwrap();
    assert_eq!(msgs.len(), 2);
    let closed: Vec<_> = msgs.iter().filter(|m| m.text.contains("закрыта")).collect();
    assert_eq!(closed.len(), 1);
    assert!(
        closed[0].text.starts_with("🔴 Смена № 7 закрыта"),
        "{}",
        closed[0].text
    );
    assert!(closed[0].text.contains("недостача 100 с"));
    assert!(
        closed[0]
            .text
            .contains(&format!("📊 Итог дня, {yesterday}")),
        "{}",
        closed[0].text
    );
    let handover = msgs.iter().find(|m| !m.text.contains("закрыта")).unwrap();
    assert!(
        handover.text.starts_with("📥 Касса смены № 7 сдана")
            && handover.text.contains("в сейф 800 с"),
        "{}",
        handover.text
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_feed_names_parties_registers_and_new_events(pool: PgPool) {
    // Погашение: кто платил и кому; касса по названию; новые события — понятным текстом.
    use avtodom_server::api::telegram::pending;
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    link_user_chat(&pool, w.owner.user.id, branch, 920_001, 1).await;
    feed_started_hour_ago(&pool).await;
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, branch)
        .await
        .unwrap();
    let (supplier, client) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, role, kind, name) in [
        (supplier, "supplier", "company", "ОсОО Масла"),
        (client, "customer", "person", "Бакыт"),
    ] {
        sqlx::query(
            "insert into parties (id, branch_id, role, kind, name) values ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(branch)
        .bind(role)
        .bind(kind)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    let event = |action: &'static str, entity: Option<Uuid>, data: serde_json::Value| {
        let pool = pool.clone();
        async move {
            sqlx::query("insert into audit_log (id, branch_id, action, entity, entity_id, data) values ($1, $2, $3, 'test', $4, $5)")
                .bind(Uuid::now_v7())
                .bind(branch)
                .bind(action)
                .bind(entity)
                .bind(data)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    event("debt.repayment", Some(supplier), serde_json::json!({ "amount_tyiyn": 500_000, "method": "outside", "balance_tyiyn": -100_000 })).await;
    event(
        "debt.repayment",
        Some(client),
        serde_json::json!({ "amount_tyiyn": 30_000, "method": "card", "balance_tyiyn": 0 }),
    )
    .await;
    event("cash.movement", Some(till), serde_json::json!({ "kind": "cash_out", "amount_tyiyn": -20_000, "comment": "на хозтовары" })).await;
    event(
        "backup.failed",
        None,
        serde_json::json!({ "reason": "нет места на диске" }),
    )
    .await;
    event(
        "product.merge",
        None,
        serde_json::json!({ "from_name": "Фильтр дубль", "into_name": "Фильтр W712" }),
    )
    .await;
    event(
        "payout.reverse",
        Some(w.cashier),
        serde_json::json!({ "amount_tyiyn": 10_000, "comment": "ошибся" }),
    )
    .await;
    event(
        "settings.loyalty",
        None,
        serde_json::json!({ "rate_bp": 75 }),
    )
    .await;
    event(
        "sale.credit_limit_exceeded",
        None,
        serde_json::json!({ "party_id": client, "debt": 5_000 }),
    )
    .await;
    let texts: Vec<String> = pending(&mut conn)
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.text)
        .collect();
    let has = |needle: &str| texts.iter().any(|t| t.contains(needle));
    assert!(
        has("💸 Оплатили поставщику ОсОО Масла: 5 000 с"),
        "{texts:?}"
    );
    assert!(
        has("мимо касс (из своих) · ещё должны 1 000 с"),
        "{texts:?}"
    );
    assert!(has("💰 Клиент Бакыт погасил 300 с"), "{texts:?}");
    assert!(has("➖ Изъяли из «Касса»"), "{texts:?}");
    assert!(
        has("⚠️ Копия базы не создана\nнет места на диске"),
        "{texts:?}"
    );
    assert!(has("«Фильтр дубль» влита в «Фильтр W712»"), "{texts:?}");
    assert!(has("↩️ Сторно выплаты: Кассир"), "{texts:?}");
    assert!(has("начисление 0,75 %"), "{texts:?}");
    assert!(has("Долг клиента вышел за лимит: Бакыт"), "{texts:?}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_debts_match_the_program(pool: PgPool) {
    // «📒 Долги»: те же контрагенты, что «Нам должны» в программе (не только покупатели), раздел
    // «Мы должны» и «… и ещё N» при длинном списке.
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    for i in 0..16 {
        sqlx::query("insert into parties (id, branch_id, role, kind, name, balance_tyiyn, active) values ($1, $2, 'customer', 'person', $3, $4, $5)")
            .bind(Uuid::now_v7())
            .bind(branch)
            .bind(format!("Клиент {i:02}"))
            .bind(10_000 + i64::from(i) * 100)
            .bind(i != 3)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (name, balance) in [
        ("Поставщик с переплатой", 70_000i64),
        ("ОсОО Масла", -250_000),
    ] {
        sqlx::query("insert into parties (id, branch_id, role, kind, name, balance_tyiyn) values ($1, $2, 'supplier', 'company', $3, $4)")
            .bind(Uuid::now_v7())
            .bind(branch)
            .bind(name)
            .bind(balance)
            .execute(&pool)
            .await
            .unwrap();
    }
    let chat = 930_001i64;
    link_user_chat(&pool, w.owner.user.id, branch, chat, 1).await;
    let mut conn = pool.acquire().await.unwrap();
    let text = say(&mut conn, chat, "📒 Долги").await;
    // Как сводка программы: сумма всех положительных и всех отрицательных балансов.
    let (owe_us, we_owe): (i64, i64) = sqlx::query_as(
        "select coalesce(sum(balance_tyiyn) filter (where balance_tyiyn > 0), 0)::bigint,
                coalesce(sum(-balance_tyiyn) filter (where balance_tyiyn < 0), 0)::bigint
         from parties where branch_id = $1",
    )
    .bind(branch)
    .fetch_one(&pool)
    .await
    .unwrap();
    use avtodom_server::domain::money::format_som;
    assert!(
        text.starts_with(&format!("Должны нам всего: {}", format_som(owe_us))),
        "{text}"
    );
    assert_eq!(
        text.lines().nth(1).unwrap().trim(),
        "Поставщик с переплатой — 700 с"
    );
    assert!(text.contains("  … и ещё 2"), "{text}");
    assert!(
        text.contains(&format!(
            "Мы должны всего: {}\n  ОсОО Масла — 2 500 с",
            format_som(we_owe)
        )),
        "{text}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_customer_link_is_unambiguous_and_rechecked(pool: PgPool) {
    // Номер в двух карточках — не угадываем; карточку отключили или сменили номер — чат
    // отвязывается; «да…» и «стоп…» в начале фразы — обычный текст; владелец не клиент (ADR-063).
    use avtodom_server::api::telegram::{
        Keys, RESHARE, SharedContact, drop_stale_customers, handle_message,
    };
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, name) in [(first, "Эрлан"), (second, "Эрлан старая карточка")]
    {
        sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', $3, '0555 12-34-56')")
            .bind(id)
            .bind(branch)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
    }
    let chat = 940_001i64;
    let share = |phone: &str| SharedContact {
        phone: phone.into(),
        user_id: Some(chat),
    };
    let mut conn = pool.acquire().await.unwrap();
    let r = handle_message(&mut conn, chat, chat, None, Some(share("+996555123456")))
        .await
        .unwrap();
    assert!(
        r.text.contains("несколько карточек") && r.keys == Keys::Contact,
        "{}",
        r.text
    );
    handle_message(&mut conn, chat, chat, None, Some(share("+996555123456")))
        .await
        .unwrap();
    let ambiguous = audit_data(&pool, "telegram.customer_ambiguous").await;
    assert_eq!(ambiguous.len(), 1);
    assert_eq!(ambiguous[0]["phone"], "…3456");
    let linked: i64 = sqlx::query_scalar("select count(*) from telegram_customers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(linked, 0);

    // Вторую карточку отключили — подключается к единственной.
    sqlx::query("update parties set active = false where id = $1")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();
    let r = handle_message(&mut conn, chat, chat, None, Some(share("+996555123456")))
        .await
        .unwrap();
    assert_eq!(r.keys, Keys::Customer);
    assert_eq!(audit_data(&pool, "telegram.customer_link").await.len(), 1);
    // Фразы на «да» и «стоп» — обычный текст, клиент остаётся подключён.
    for t in ["да, спасибо", "Стоп, а когда замена?", "стоп"] {
        assert!(say(&mut conn, chat, t).await.contains("Баллов"), "{t}");
    }
    // Номер в карточке сменили — следующий же ответ просит поделиться номером снова.
    sqlx::query("update parties set phone = '0700 99 88 77' where id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    let r = handle_message(&mut conn, chat, chat, Some("⭐ Баллы"), None)
        .await
        .unwrap();
    assert_eq!((r.text.as_str(), r.keys), (RESHARE, Keys::Contact));
    assert_eq!(audit_data(&pool, "telegram.customer_unlink").await.len(), 1);
    // С новым номером — снова подключён; «/стоп» отключает.
    let r = handle_message(&mut conn, chat, chat, None, Some(share("+996700998877")))
        .await
        .unwrap();
    assert_eq!(r.keys, Keys::Customer);
    assert!(say(&mut conn, chat, "/стоп").await.contains("вы отключены"));
    // Рассылка перед напоминаниями отвязывает отключённые карточки и называет их чаты.
    handle_message(&mut conn, chat, chat, None, Some(share("+996700998877")))
        .await
        .unwrap();
    sqlx::query("update parties set active = false where id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(drop_stale_customers(&mut conn).await.unwrap(), vec![chat]);
    assert_eq!(audit_data(&pool, "telegram.customer_unlink").await.len(), 3);

    // Владелец делится контактом — не становится клиентом; кнопки клиента не вопрос.
    let owner_chat = 940_009i64;
    link_owner(&pool, &w, owner_chat).await;
    sqlx::query("update parties set active = true, phone = '0555 77 77 77' where id = $1")
        .bind(first)
        .execute(&pool)
        .await
        .unwrap();
    let r = handle_message(
        &mut conn,
        owner_chat,
        owner_chat,
        None,
        Some(SharedContact {
            phone: "+996555777777".into(),
            user_id: Some(owner_chat),
        }),
    )
    .await
    .unwrap();
    assert_eq!(r.keys, Keys::Owner);
    let n: i64 = sqlx::query_scalar("select count(*) from telegram_customers where chat_id = $1")
        .bind(owner_chat)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let (url, service) = fake_service().await;
    let a = fake_assistant(url);
    for t in ["🚗 Мои машины", "⭐ Баллы", "Да, отключиться"] {
        assert!(
            ask(&mut conn, &a, owner_chat, t)
                .await
                .starts_with("Нажмите кнопку внизу")
        );
    }
    assert_eq!(service.requests().len(), 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_waits_for_brand_text_not_sticker(pool: PgPool) {
    // Наклейка или фото, пока бот ждёт марку, машину молча не заводит (ADR-063).
    use avtodom_server::api::telegram::handle_message;
    let w = seed(&pool).await;
    let chat = 950_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let mut conn = pool.acquire().await.unwrap();
    say(&mut conn, chat, "🚗 Мои машины").await;
    assert!(
        say(&mut conn, chat, "01KG555AAA")
            .await
            .contains("Марка и модель")
    );
    let r = handle_message(&mut conn, chat, chat, None, None)
        .await
        .unwrap();
    assert!(r.text.contains("марку и модель текстом"), "{}", r.text);
    assert!(car_of(&pool, me, "01KG555AAA").await.is_none());
    say(&mut conn, chat, "Honda Fit").await;
    let (_, active, _, brand, _) = car_of(&pool, me, "01KG555AAA").await.unwrap();
    assert!(active && brand == "Honda");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_owner_code_attempts_are_limited(pool: PgPool) {
    // 5 неверных кодов с чата и 20 всего за 15 минут; на пределе — одно уведомление владельцу;
    // шесть цифр от подключённого клиента — не попытка кода (ADR-063).
    let w = seed(&pool).await;
    sqlx::query("insert into telegram_codes (code, user_id, expires_at) values ('424242', $1, now() + interval '15 minutes')")
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let chat = 960_001i64;
    for _ in 0..5 {
        assert!(
            say(&mut conn, chat, "/start 000000")
                .await
                .contains("не подошёл")
        );
    }
    assert!(
        say(&mut conn, chat, "/start 424242")
            .await
            .contains("Слишком много неверных кодов")
    );
    // Клиент пишет шесть цифр — это не код.
    let customer_chat = 960_100i64;
    bot_customer(&pool, &w, "Клиент", "person", customer_chat).await;
    assert!(
        say(&mut conn, customer_chat, "123456")
            .await
            .contains("Баллов")
    );
    let failures = || async {
        sqlx::query_scalar::<_, i64>("select count(*) from telegram_code_failures")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(failures().await, 5);
    // Ещё 15 неверных с разных чатов — привязка закрыта для всех, владельцу одно уведомление.
    for i in 0..15 {
        say(&mut conn, 960_010 + i, "000001").await;
    }
    assert!(
        say(&mut conn, 960_050, "/start 424242")
            .await
            .contains("временно закрыта")
    );
    say(&mut conn, 960_051, "000002").await;
    assert_eq!(audit_data(&pool, "telegram.code_flood").await.len(), 1);
    // Через 15 минут верный код снова работает, привязка — в журнале.
    sqlx::query("update telegram_code_failures set at = at - interval '16 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        say(&mut conn, chat, "/start 424242")
            .await
            .starts_with("Готово")
    );
    assert_eq!(audit_data(&pool, "telegram.link").await.len(), 1);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_reminders_catch_up_inside_the_window(pool: PgPool) {
    // Пропущенное «за неделю» догоняется, «за день» отменяет неотправленное «за неделю»; ночью
    // напоминания и баллы не уходят (ADR-063).
    use avtodom_server::api::telegram::{due_reminders, mark_reminded, notify_pass};
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let chat = 970_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let car = staff_car(&pool, &w, me, "01KG123ABC", true).await;
    service_change(&pool, &w, me, car).await;
    let mut conn = pool.acquire().await.unwrap();
    let interval = |days: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) select id, branch_id, 'oil', 8000, $1 from party_vehicles where id = $2 on conflict (vehicle_id, slot) do update set interval_days = excluded.interval_days")
                .bind(days)
                .bind(car)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    // До срока 5 дней — «за неделю» ещё нужно.
    interval(5).await;
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].days_before, 7);
    assert!(due[0].text.contains("через 5 дней"), "{}", due[0].text);
    // До срока день, «за неделю» не уходило — уходит одно «за день», «за неделю» уже не нужно.
    interval(1).await;
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].days_before, 1);
    assert!(due[0].text.contains("завтра"));
    // Ночью ничего не уходит, днём — одно сообщение.
    let (api, fake) = fake_telegram().await;
    notify_pass(&mut conn, &api, 22, true).await.unwrap();
    assert!(fake.texts(chat).is_empty());
    notify_pass(&mut conn, &api, 10, true).await.unwrap();
    let texts = fake.texts(chat);
    assert!(texts.iter().any(|t| t.contains("завтра")), "{texts:?}");
    assert!(texts.iter().any(|t| t.starts_with("Баллы")), "{texts:?}");
    let marks: Vec<i32> = sqlx::query_scalar(
        "select days_before from oil_reminders where vehicle_id = $1 order by days_before",
    )
    .bind(car)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(marks, vec![1, 7]);
    // В сам день срока «за день» ещё уходит, если не ушло.
    sqlx::query("delete from oil_reminders")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update oil_changes set change_date = change_date - 1 where vehicle_id = $1")
        .bind(car)
        .execute(&pool)
        .await
        .unwrap();
    let due = due_reminders(&mut conn).await.unwrap();
    assert!(
        due.len() == 1 && due[0].text.contains("сегодня"),
        "{}",
        due.len()
    );
    mark_reminded(&mut conn, &due[0]).await.unwrap();
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_long_car_lists_are_capped(pool: PgPool) {
    // 40 машин фирмы: книжка — 20 машин и «… и ещё 20», каждая часть сообщения в пределе;
    // список — 20 кнопок, остальные открываются номером (ADR-063).
    use avtodom_server::api::telegram::{MAX_MESSAGE, customer_book, split_message};
    let w = seed(&pool).await;
    let chat = 980_001i64;
    let firm = bot_customer(&pool, &w, "ОсОО Таксопарк", "company", chat).await;
    for i in 0..40 {
        let id = staff_car(&pool, &w, firm, &format!("01KG{i:03}AAA"), true).await;
        sqlx::query("insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km, oil_text, filter_text, user_id) values ($1, $2, $3, $4, current_date, 100000, $5, 'Фильтр Sakura C-1821', $6)")
            .bind(Uuid::now_v7())
            .bind(w.owner.user.branch_id)
            .bind(id)
            .bind(firm)
            .bind("Масло Totachi Niro LV 0W-20, 4 литра, синтетика ".repeat(2))
            .bind(w.owner.user.id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    let book = customer_book(&mut conn, firm).await.unwrap();
    assert!(book.contains("… и ещё 20"), "{book}");
    assert_eq!(book.matches("Последняя замена").count(), 20);
    assert!(
        split_message(&book)
            .iter()
            .all(|p| p.chars().count() <= MAX_MESSAGE)
    );
    let list = avtodom_server::api::telegram::handle_message(
        &mut conn,
        chat,
        chat,
        Some("🚗 Мои машины"),
        None,
    )
    .await
    .unwrap();
    let cars = list
        .inline
        .iter()
        .flatten()
        .filter(|b| b.data.starts_with("car:"))
        .count();
    assert_eq!(cars, 20);
    assert!(list.text.contains("… и ещё 20"), "{}", list.text);
    let card = say(&mut conn, chat, "01kg035aaa").await;
    assert!(card.starts_with("🚗 01KG035AAA"), "{card}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_plate_conflicts_and_car_count_are_capped(pool: PgPool) {
    // Не больше трёх уведомлений «номер у другого клиента» от клиента за сутки и не больше
    // 50 машин в истории клиента (ADR-063).
    let w = seed(&pool).await;
    let chat = 990_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let other = bot_customer(&pool, &w, "Бакыт", "person", 990_002).await;
    for i in 0..5 {
        staff_car(&pool, &w, other, &format!("B{i}00XY"), true).await;
    }
    let mut conn = pool.acquire().await.unwrap();
    for i in 0..5 {
        press(&mut conn, chat, "add").await;
        say(&mut conn, chat, &format!("B{i}00XY")).await;
        press(&mut conn, chat, "skip").await;
    }
    assert_eq!(audit_data(&pool, "vehicle.plate_conflict").await.len(), 3);
    assert_eq!(
        audit_data(&pool, "vehicle.plate_conflict_quiet")
            .await
            .len(),
        2
    );
    for i in 0..45 {
        staff_car(&pool, &w, me, &format!("Z{i:02}OLD"), false).await;
    }
    press(&mut conn, chat, "add").await;
    let r = say(&mut conn, chat, "01KG999NEW").await;
    assert!(r.contains("Обратитесь на точку"), "{r}");
    assert!(car_of(&pool, me, "01KG999NEW").await.is_none());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_question_counts_report_calls_and_hourly_limit(pool: PgPool) {
    // Не больше 4 вызовов отчётов на вопрос (вызовов, а не запросов); 21-й вопрос за час —
    // «лимит на этот час» (ADR-063).
    let w = seed(&pool).await;
    let chat = 995_001i64;
    link_owner(&pool, &w, chat).await;
    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    let mut conn = pool.acquire().await.unwrap();
    start_asking(&mut conn, &a, chat).await;
    let calls = |n: usize| {
        let parts: Vec<serde_json::Value> = (0..n)
            .map(|i| serde_json::json!({ "functionCall": { "id": format!("fc_{i}"), "name": "money_balances", "args": {} } }))
            .collect();
        serde_json::json!({
            "candidates": [{ "content": { "role": "model", "parts": parts }, "finishReason": "STOP" }],
            "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 }
        })
    };
    fake.push(vec![Step::Reply(200, calls(5))]);
    assert!(
        ask(&mut conn, &a, chat, "сколько денег во всех кассах?")
            .await
            .starts_with("Не получилось разобрать вопрос")
    );
    assert_eq!(fake.requests().len(), 1);
    // Два запроса по два вызова — четыре, это можно.
    fake.push(vec![
        Step::Reply(200, calls(2)),
        Step::Reply(200, calls(2)),
        Step::Reply(200, final_text("Готово.")),
    ]);
    assert_eq!(
        ask(&mut conn, &a, chat, "а теперь ещё раз деньги?").await,
        "Готово."
    );
    sqlx::query(
        "insert into bot_questions (id, chat_id, user_id, branch_id, asked_at, question, answer)
         select gen_random_uuid(), $1, $2, $3, now() - interval '5 minutes', 'вопрос ' || n, 'ответ'
         from generate_series(1, 20) n",
    )
    .bind(chat)
    .bind(w.owner.user.id)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();
    let before = fake.requests().len();
    assert!(
        ask(&mut conn, &a, chat, "сколько заработали сегодня?")
            .await
            .starts_with("Лимит вопросов на этот час исчерпан")
    );
    assert_eq!(fake.requests().len(), before);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_update_offset_survives_restart(pool: PgPool) {
    use avtodom_server::api::telegram::{load_offset, save_offset};
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(load_offset(&mut conn).await.unwrap(), 0);
    save_offset(&mut conn, 912_345_678).await.unwrap();
    assert_eq!(load_offset(&mut conn).await.unwrap(), 912_345_678);
}

/// Наличные в кассе для проверок: внесение мимо смены, остаток сходится с движениями.
async fn fund_till(pool: &PgPool, w: &World, till: Uuid, amount: i64) {
    sqlx::query(
        "insert into cash_movements (id, branch_id, account_id, kind, amount_tyiyn, user_id, device_id) values ($1, $2, $3, 'cash_in', $4, $5, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(till)
    .bind(amount)
    .bind(w.owner.user.id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("update cash_accounts set balance_tyiyn = balance_tyiyn + $2 where id = $1")
        .bind(till)
        .bind(amount)
        .execute(pool)
        .await
        .unwrap();
}

async fn till_balance(pool: &PgPool, till: Uuid) -> i64 {
    sqlx::query_scalar("select balance_tyiyn from cash_accounts where id = $1")
        .bind(till)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn cashier_percent_is_only_from_money(pool: PgPool) {
    // 2 % кассира — с валовой за вычетом скидки баллами; возврат снимает ровно начисленное (ADR-065).
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
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
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Эрлан', '0555123456')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into telegram_customers (chat_id, party_id, phone) values (777, $1, '+996555123456')")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    // 1 000 баллов на счету клиента.
    sqlx::query(
        "insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, doc_type, doc_id) values ($1, $2, $3, 'accrual', 100000, 'test', $1)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(party)
    .execute(&pool)
    .await
    .unwrap();
    let percent = |sale: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (i64, i64)>(
                "select coalesce(sum(amount_tyiyn), 0)::bigint, coalesce(sum(base_tyiyn), 0)::bigint
                 from payroll_accruals where kind = 'revenue_percent'
                   and (doc_id = $1 or doc_id in (select id from sales where reversal_of = $1))",
            )
            .bind(sale)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let ret = |qty: i64, payments: Vec<PaymentReq>| ReturnReq {
        op_id: Uuid::now_v7(),
        battery_cash: false,
        comment: String::new(),
        lines: vec![ReturnLineReq { line_no: 1, qty }],
        payments,
        business_date: None,
    };

    // Чек 1 000 с за штуку с закупкой 30 с: валовая 970 с, из них 200 с баллами.
    // База 770 с, 2 % = 15,40 с.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 100_000)],
        pay(&[("bonus", 20_000), ("cash", 80_000)]),
    );
    req.party_id = Some(party);
    let one = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(percent(one.id).await, (1_540, 77_000));
    // Тот же чек без баллов дал бы 19,40 с: база меньше ровно на 200 с.
    let plain = sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 1, 100_000)], cash(100_000)),
    )
    .await
    .unwrap();
    assert_eq!(percent(plain.id).await, (1_940, 97_000));
    // Полный возврат снимает ровно начисленное.
    let mut tx = pool.begin().await.unwrap();
    return_sale_tx(
        &mut tx,
        &w.owner,
        one.id,
        ret(1, pay(&[("bonus", 20_000), ("cash", 80_000)])),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(percent(one.id).await, (0, 0));

    // Две штуки по 500 с, 200 с баллами: база 940 − 200 = 740 с, 2 % = 14,80 с.
    // Возврат по одной штуке снимает по половине, в сумме — всё.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 2, 50_000)],
        pay(&[("bonus", 20_000), ("cash", 80_000)]),
    );
    req.party_id = Some(party);
    let two = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(percent(two.id).await, (1_480, 74_000));
    for _ in 0..2 {
        let mut tx = pool.begin().await.unwrap();
        return_sale_tx(
            &mut tx,
            &w.owner,
            two.id,
            ret(1, pay(&[("bonus", 10_000), ("cash", 40_000)])),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(percent(two.id).await, (0, 0));

    // Баллы больше валовой (100 − 30 = 70 с, баллами 90 с) — процента нет, база не уходит в минус.
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 10_000)],
        pay(&[("bonus", 9_000), ("cash", 1_000)]),
    );
    req.party_id = Some(party);
    let cheap = sell(&pool, &w.owner, req).await.unwrap();
    assert_eq!(percent(cheap.id).await, (0, 0));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_intake_can_be_reversed_once(pool: PgPool) {
    // Отмена приёма: вес со склада, деньги в ту же кассу; проданный вес не отменить (ADR-065).
    use avtodom_server::api::batteries::{IntakeReverseReq, reverse_intake_tx};
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 1_000_000).await;
    let intake = |kg: i64| {
        let pool = pool.clone();
        let admin = w.admin.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let out = intake_tx(
                &mut tx,
                &admin,
                IntakeReq {
                    op_id: Uuid::now_v7(),
                    grams: kg * 1000,
                    price_per_kg_tyiyn: 10_000,
                    comment: String::new(),
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            out
        }
    };
    let reverse = |ctx: Ctx, id: Uuid| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let r = reverse_intake_tx(
                &mut tx,
                &ctx,
                id,
                IntakeReverseReq {
                    op_id: Uuid::now_v7(),
                    comment: "ошиблись весом".into(),
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.unwrap();
            }
            r
        }
    };

    // 10 кг по 100 с: из кассы 1 000 с. Отмена возвращает деньги и снимает вес.
    let first = intake(10).await;
    assert_eq!(till_balance(&pool, till).await, 900_000);
    let back = reverse(w.admin.clone(), first.id).await.unwrap();
    assert_eq!(back.info.stock_g, 0);
    assert_eq!(till_balance(&pool, till).await, 1_000_000);
    // Второй раз — 409.
    assert!(matches!(
        reverse(w.owner.clone(), first.id).await,
        Err(AppError::Conflict(_))
    ));

    // Приняли 10 кг, продали 5 — отменять нечего: остаток ушёл бы в минус.
    let second = intake(10).await;
    let product = second.info.product_id;
    sell(
        &pool,
        &w.admin,
        takeaway(
            &w,
            vec![SaleLineReq {
                kind: "weight".into(),
                gift: false,
                product_id: Some(product),
                service_id: None,
                qty: 5_000,
                unit_price_tyiyn: 15_000,
                seen_list_price_tyiyn: None,
            }],
            cash(75_000),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(
        reverse(w.owner.clone(), second.id).await,
        Err(AppError::Conflict(_))
    ));

    // Смена закрыта: администратор приём не отменяет; владелец — когда смена снова открыта.
    let third = intake(4).await;
    let shift: Uuid = sqlx::query_scalar("select id from shifts where account_id = $1")
        .bind(till)
        .fetch_one(&pool)
        .await
        .unwrap();
    let close = Uuid::now_v7();
    sqlx::query(
        "insert into shift_closes (id, shift_id, seq, expected_tyiyn, counted_tyiyn, diff_tyiyn, user_id, device_id)
         values ($1, $2, 1, 0, 0, 0, $3, $1)",
    )
    .bind(close)
    .bind(shift)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        reverse(w.admin.clone(), third.id).await,
        Err(AppError::Forbidden)
    ));
    assert!(matches!(
        reverse(w.owner.clone(), third.id).await,
        Err(AppError::Conflict(_))
    ));
    sqlx::query(
        "insert into shift_reopens (id, shift_id, close_id, reason, user_id, device_id) values ($1, $2, $3, 'продолжаем', $4, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(shift)
    .bind(close)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let before = till_balance(&pool, till).await;
    let out = reverse(w.owner.clone(), third.id).await.unwrap();
    assert_eq!(out.info.stock_g, 5_000);
    assert_eq!(till_balance(&pool, till).await, before + 40_000);
    assert_eq!(audit_data(&pool, "battery.intake_reverse").await.len(), 2);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipt_reversal_of_sold_goods_is_owner_only(pool: PgPool) {
    // Сторно прихода, товар которого уже продан, — только владелец, и ему уведомление (ADR-065).
    let w = seed(&pool).await;
    grant(&pool, &w, "discount").await;
    let sold = receive_on_debt(&pool, &w.admin, w.filter, 5, 5_000).await;
    let intact = receive_on_debt(&pool, &w.admin, w.oil, 4000, 100_000).await;
    sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("piece", w.filter, 3, 2_000)], cash(6_000)),
    )
    .await
    .unwrap();
    let reverse = |ctx: Ctx, id: Uuid| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let r = reverse_receipt_tx(
                &mut tx,
                &ctx,
                id,
                ReverseReq {
                    op_id: Uuid::now_v7(),
                    comment: "ошибка".into(),
                    in_correction: false,
                },
            )
            .await;
            if r.is_ok() {
                tx.commit().await.unwrap();
            }
            r
        }
    };
    // Непроданный товар администратор сторнирует, как прежде, без уведомления о минусе.
    reverse(w.admin.clone(), intact.id).await.unwrap();
    assert!(
        audit_data(&pool, "receipt.reverse_negative")
            .await
            .is_empty()
    );
    // Проданный — 403 администратору.
    assert!(matches!(
        reverse(w.admin.clone(), sold.id).await,
        Err(AppError::Forbidden)
    ));
    assert_eq!(pool_of(&pool, w.filter).await.0, 2);
    // Владелец проводит, остаток −3, уведомление «проведите верный приход».
    reverse(w.owner.clone(), sold.id).await.unwrap();
    assert_eq!(pool_of(&pool, w.filter).await.0, -3);
    let notes = audit_data(&pool, "receipt.reverse_negative").await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["lines"][0]["after"], -3);
}

// ---------- Кабинет работника в боте (SPEC-23) ----------

async fn set_phone(pool: &PgPool, employee: Uuid, phone: &str) {
    sqlx::query("update employees set phone = $2 where id = $1")
        .bind(employee)
        .bind(phone)
        .execute(pool)
        .await
        .unwrap();
}

/// Контакт из чата `chat`: свой (`own`) или пересланный чужой.
async fn share(
    conn: &mut sqlx::PgConnection,
    chat: i64,
    phone: &str,
    own: bool,
) -> avtodom_server::api::telegram::Reply {
    let c = avtodom_server::api::telegram::SharedContact {
        phone: phone.into(),
        user_id: Some(if own { chat } else { chat + 1 }),
    };
    avtodom_server::api::telegram::handle_message(conn, chat, chat, None, Some(c))
        .await
        .unwrap()
}

async fn employee_chats(pool: &PgPool) -> Vec<(i64, Uuid)> {
    sqlx::query_as("select chat_id, employee_id from telegram_employees order by chat_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_employee_links_only_by_own_unambiguous_number(pool: PgPool) {
    // Свой контакт — подключает, чужой — нет; номер у двух сотрудников — привязки нет; владельцу
    // уведомление о подключении; чат владельца контакт не принимает (SPEC-23).
    use avtodom_server::api::telegram::{Keys, pending};
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    set_phone(&pool, w.master, "0700 11-22-33").await;
    let owner_chat = 951_000i64;
    link_user_chat(&pool, w.owner.user.id, branch, owner_chat, 1).await;
    feed_started_hour_ago(&pool).await;
    let mut conn = pool.acquire().await.unwrap();

    let r = share(&mut conn, owner_chat, "+996700112233", true).await;
    assert_eq!(r.keys, Keys::Owner);
    let chat = 951_001i64;
    let r = share(&mut conn, chat, "+996700112233", false).await;
    assert_eq!(r.keys, Keys::Contact);
    assert!(r.text.contains("собственный номер"), "{}", r.text);
    assert!(employee_chats(&pool).await.is_empty());

    let r = share(&mut conn, chat, "+996700112233", true).await;
    assert_eq!(r.keys, Keys::Employee, "{}", r.text);
    assert!(
        r.text.contains("Мастер, вы подключены") && r.text.contains("💼 Сегодня"),
        "{}",
        r.text
    );
    assert_eq!(employee_chats(&pool).await, vec![(chat, w.master)]);
    // Повторный контакт того же работника — без второго уведомления.
    share(&mut conn, chat, "+996700112233", true).await;
    assert_eq!(audit_data(&pool, "telegram.employee_link").await.len(), 1);
    let feed = pending(&mut conn).await.unwrap();
    assert!(
        feed.iter()
            .any(|m| m.chat_id == owner_chat
                && m.text.contains("Работник Мастер подключился к боту")),
        "{:?}",
        feed.iter().map(|m| m.text.clone()).collect::<Vec<_>>()
    );

    // Тот же номер у двух активных сотрудников — не угадываем.
    let twin = Uuid::now_v7();
    sqlx::query("insert into employees (id, branch_id, full_name, is_master, phone) values ($1, $2, 'Двойник', true, '+996 555 44 33 22')")
        .bind(twin)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    set_phone(&pool, w.cashier, "0555 443 322").await;
    let other = 951_002i64;
    let r = share(&mut conn, other, "+996555443322", true).await;
    assert_eq!(r.keys, Keys::Contact);
    assert!(
        r.text.to_lowercase().contains("обратитесь к владельцу"),
        "{}",
        r.text
    );
    assert_eq!(employee_chats(&pool).await, vec![(chat, w.master)]);
    // Второго отключили — номер снова однозначный.
    sqlx::query("update employees set active = false where id = $1")
        .bind(twin)
        .execute(&pool)
        .await
        .unwrap();
    let r = share(&mut conn, other, "+996555443322", true).await;
    assert_eq!(r.keys, Keys::Employee, "{}", r.text);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_employee_and_customer_on_one_number(pool: PgPool) {
    // Номер у сотрудника и у клиента — оба раздела; поддельная кнопка с чужим сотрудником или
    // месяцем вне пределов ничего не показывает; отключение — с подтверждением, от всего (SPEC-23).
    use avtodom_server::api::telegram::{Keys, handle_callback};
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    set_phone(&pool, w.master, "0555 12-34-56").await;
    set_phone(&pool, w.cashier, "0700 99-88-77").await;
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Мастер как клиент', '+996555123456')")
        .bind(party)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 30_000).await;
    let car = staff_car(&pool, &w, party, "01KG777AAA", true).await;
    service_change(&pool, &w, party, car).await;
    let chat = 952_001i64;
    let mut conn = pool.acquire().await.unwrap();
    let r = share(&mut conn, chat, "+996555123456", true).await;
    assert_eq!(r.keys, Keys::Both, "{}", r.text);
    assert!(r.text.contains("Как работник") && r.text.contains("01KG777AAA"));

    // Клиентская кнопка — кнопки обоих разделов остаются.
    let r = handle_message(&mut conn, chat, "⭐ Баллы").await;
    assert!(
        r.text.contains("Баллов:") && r.keys == Keys::Both,
        "{}",
        r.text
    );
    let r = handle_message(&mut conn, chat, "💼 Мой заработок").await;
    assert!(r.text.contains("за замены: 1 × 30 с = 30 с"), "{}", r.text);
    let data: Vec<String> = r.inline.iter().flatten().map(|b| b.data.clone()).collect();
    assert!(data.contains(&format!("e:m:{}", w.master)), "{data:?}");
    // «Мои работы» мастера: госномер и масло, без суммы чека.
    let works = handle_callback(&mut conn, chat, &format!("e:w:{}", w.master))
        .await
        .unwrap();
    assert!(works.text.contains("01KG777AAA"), "{}", works.text);
    assert!(!works.text.contains("2 500 с"), "{}", works.text);
    let r = handle_callback(&mut conn, chat, &format!("e:m:{}", w.master))
        .await
        .unwrap();
    assert!(r.text.contains("Итого за месяц: 30 с"), "{}", r.text);

    // Поддельные кнопки: чужой сотрудник, месяц вне пределов, мусор.
    sqlx::query("update employees set full_name = 'Айгуль' where id = $1")
        .bind(w.cashier)
        .execute(&pool)
        .await
        .unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();
    let next = today
        .checked_add_months(chrono::Months::new(1))
        .unwrap()
        .format("%Y-%m");
    let old = today
        .checked_sub_months(chrono::Months::new(13))
        .unwrap()
        .format("%Y-%m");
    let last = today
        .checked_sub_months(chrono::Months::new(12))
        .unwrap()
        .format("%Y-%m");
    for forged in [
        format!("e:t:{}", w.cashier),
        format!("e:m:{}:{}", w.cashier, today.format("%Y-%m")),
        format!("e:a:{}", w.cashier),
        format!("e:w:{}", w.cashier),
        format!("e:m:{}:{next}", w.master),
        format!("e:m:{}:{old}", w.master),
        format!("e:m:{}:2026-1", w.master),
        format!("e:t:{}:x", w.master),
        "e:t".to_string(),
    ] {
        let r = handle_callback(&mut conn, chat, &forged).await.unwrap();
        assert!(r.text.contains("Кнопка устарела"), "{forged}: {}", r.text);
        assert!(!r.text.contains("Айгуль") && !r.text.contains("Итого"));
    }
    // Двенадцать месяцев назад — можно, дальше назад кнопки нет.
    let r = handle_callback(&mut conn, chat, &format!("e:m:{}:{last}", w.master))
        .await
        .unwrap();
    assert!(r.text.contains("Итого за месяц: 0 с"), "{}", r.text);
    assert!(!r.inline.iter().flatten().any(|b| b.text.starts_with('←')));
    // Чужой чат кнопками работника ничего не видит.
    let r = handle_callback(&mut conn, 952_999, &format!("e:t:{}", w.master))
        .await
        .unwrap();
    assert_eq!(r.keys, Keys::Contact);
    assert!(!r.text.contains("Итого"));

    let r = handle_message(&mut conn, chat, "🚪 Отключиться").await;
    assert_eq!(r.keys, Keys::ConfirmLeave);
    assert!(
        r.text.contains("баллы") && r.text.contains("Заработок"),
        "{}",
        r.text
    );
    let r = handle_message(&mut conn, chat, "Да, отключиться").await;
    assert_eq!(r.keys, Keys::Contact);
    assert!(employee_chats(&pool).await.is_empty());
    let customers: i64 = sqlx::query_scalar("select count(*) from telegram_customers")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(customers, 0);
}

/// Ответ бота целиком на текст из чата `chat`.
async fn handle_message(
    conn: &mut sqlx::PgConnection,
    chat: i64,
    text: &str,
) -> avtodom_server::api::telegram::Reply {
    avtodom_server::api::telegram::handle_message(conn, chat, chat, Some(text), None)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_employee_is_rechecked_limited_and_never_asked(pool: PgPool) {
    // Свободный текст работника не уходит в разбор вопросов; 30 действий в минуту; сменили
    // телефон или отключили сотрудника — при следующем обращении чат отвязан (SPEC-23).
    use avtodom_server::api::bot_staff::RESHARE;
    use avtodom_server::api::telegram::{Keys, handle_callback, handle_message_with};
    let w = seed(&pool).await;
    set_phone(&pool, w.master, "0700 11-22-33").await;
    let chat = 953_001i64;
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(
        share(&mut conn, chat, "+996700112233", true).await.keys,
        Keys::Employee
    );
    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    for q in [
        "💬 Спросить",
        "сколько я заработал за неделю?",
        "когда зарплата",
        "123456",
    ] {
        let r = handle_message_with(&mut conn, Some(&a), chat, chat, Some(q), None)
            .await
            .unwrap();
        assert!(
            r.text.starts_with("Нажмите кнопку внизу"),
            "{q}: {}",
            r.text
        );
        assert_eq!(r.keys, Keys::Employee);
    }
    assert_eq!(fake.requests().len(), 0);
    let modes: i64 = sqlx::query_scalar("select count(*) from bot_question_mode")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(modes, 0);
    let asked: i64 = sqlx::query_scalar("select count(*) from bot_questions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(asked, 0);

    // Не больше 30 действий в минуту: 31-е — отказ, и кнопки под сообщением тоже.
    sqlx::query("delete from telegram_hits")
        .execute(&pool)
        .await
        .unwrap();
    for i in 0..30 {
        let r = handle_message(&mut conn, chat, "💼 Сегодня").await;
        assert!(r.text.contains("💼 Сегодня,"), "{i}: {}", r.text);
    }
    assert!(
        handle_message(&mut conn, chat, "📅 Месяц")
            .await
            .text
            .contains("Слишком часто")
    );
    assert!(
        handle_callback(&mut conn, chat, &format!("e:t:{}", w.master))
            .await
            .unwrap()
            .text
            .contains("Слишком часто")
    );
    sqlx::query("delete from telegram_hits")
        .execute(&pool)
        .await
        .unwrap();

    // Телефон в карточке сменили мимо программы — чат отвязан при следующем обращении.
    set_phone(&pool, w.master, "0700 00-00-01").await;
    let r = handle_message(&mut conn, chat, "💼 Сегодня").await;
    assert_eq!(r.text, RESHARE);
    assert_eq!(r.keys, Keys::Contact);
    assert!(employee_chats(&pool).await.is_empty());
    let gone = audit_data(&pool, "telegram.employee_unlink").await;
    assert_eq!(gone.len(), 1);
    assert_eq!(gone[0]["chat"], chat);
    // Новый номер — подключается снова; сотрудника отключили — кнопка ничего не показывает.
    assert_eq!(
        share(&mut conn, chat, "+996700000001", true).await.keys,
        Keys::Employee
    );
    sqlx::query("update employees set active = false where id = $1")
        .bind(w.master)
        .execute(&pool)
        .await
        .unwrap();
    let r = handle_callback(&mut conn, chat, &format!("e:m:{}", w.master))
        .await
        .unwrap();
    assert_eq!(r.text, RESHARE);
    assert!(employee_chats(&pool).await.is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_shows_cash_diff_like_profit_report(pool: PgPool) {
    // Строка «Недостача / излишек кассы» в «📊 Сегодня» и в отчётах для вопросов — та же, что в
    // отчёте о прибыли (ADR-065).
    use avtodom_server::domain::money::format_som_signed;
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    let till = open_till(&pool, &w).await;
    let shift: Uuid = sqlx::query_scalar("select id from shifts where branch_id = $1")
        .bind(branch)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into cash_movements (id, branch_id, account_id, shift_id, kind, amount_tyiyn, doc_type, doc_id, user_id, device_id)
         values ($1, $2, $3, $4, 'count_diff', -100000, 'shift', $4, $5, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(branch)
    .bind(till)
    .bind(shift)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();
    let t = avtodom_server::api::reports::totals_for(&mut conn, branch, today, today)
        .await
        .unwrap();
    assert_eq!(t.cash_diff_tyiyn, -100_000);
    let line = format_som_signed(t.cash_diff_tyiyn);
    let chat = 954_001i64;
    link_owner(&pool, &w, chat).await;
    let button = say(&mut conn, chat, "📊 Сегодня").await;
    assert!(
        button.contains(&format!("± недостача / излишек кассы: {line}")),
        "{button}"
    );
    assert!(button.contains(&format!(
        "ЧИСТАЯ ПРИБЫЛЬ: {}",
        avtodom_server::domain::money::format_som(t.net_tyiyn)
    )));

    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    let day = bishkek_today(&pool).await;
    fake.push(vec![
        Step::Reply(
            200,
            tool_use(
                "profit_totals",
                serde_json::json!({ "from": day, "to": day }),
            ),
        ),
        Step::Echo,
        Step::Reply(
            200,
            tool_use(
                "profit_by_day",
                serde_json::json!({ "from": day, "to": day }),
            ),
        ),
        Step::Echo,
    ]);
    start_asking(&mut conn, &a, chat).await;
    let reply = ask(&mut conn, &a, chat, "сколько прибыли сегодня?").await;
    assert!(
        reply.contains(&format!("Недостача / излишек кассы: {line}")),
        "{reply}"
    );
    let reply = ask(&mut conn, &a, chat, "а по дням?").await;
    assert!(
        reply.contains(&format!("недостача / излишек кассы {line}")),
        "{reply}"
    );
}

// ---------- Операции владельца через бота (SPEC-24) ----------

const DRAFT_SAID: &str = "Проверьте черновик и нажмите «✅ Провести».";

/// Ответ бота целиком (с черновиком) на сообщение владельца в режиме вопросов.
async fn ask_reply(
    conn: &mut sqlx::PgConnection,
    a: &avtodom_server::api::assistant::Assistant,
    chat: i64,
    text: &str,
) -> avtodom_server::api::telegram::Reply {
    avtodom_server::api::telegram::handle_message_with(conn, Some(a), chat, chat, Some(text), None)
        .await
        .unwrap()
}

/// Модель вызывает черновик `tool` и отвечает просьбой проверить его.
async fn draft_turn(
    conn: &mut sqlx::PgConnection,
    a: &avtodom_server::api::assistant::Assistant,
    fake: &FakeService,
    chat: i64,
    tool: &str,
    input: serde_json::Value,
) -> avtodom_server::api::telegram::Reply {
    fake.push(vec![
        Step::Reply(200, tool_use(tool, input)),
        Step::Reply(200, final_text(DRAFT_SAID)),
    ]);
    ask_reply(conn, a, chat, "внеси, пожалуйста").await
}

/// Данные кнопок «✅ Провести» и «✖ Отмена» под черновиком.
fn draft_buttons(r: &avtodom_server::api::telegram::Reply) -> (String, String) {
    let f = r.follow.as_ref().expect("черновик не показан");
    assert_eq!(f.inline[0][0].text, "✅ Провести");
    assert_eq!(f.inline[0][1].text, "✖ Отмена");
    (f.inline[0][0].data.clone(), f.inline[0][1].data.clone())
}

/// Что сервер вернул модели последним результатом инструмента.
fn last_tool_result(fake: &FakeService) -> String {
    let reqs = fake.requests();
    let last = reqs.last().unwrap()["contents"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let r = &last["parts"][0]["functionResponse"]["response"];
    r["result"]
        .as_str()
        .or_else(|| r["error"].as_str())
        .unwrap()
        .to_string()
}

/// Черновик, подготовленный и проведённый кнопкой; ответ на нажатие.
async fn post_via_bot(
    conn: &mut sqlx::PgConnection,
    a: &avtodom_server::api::assistant::Assistant,
    fake: &FakeService,
    chat: i64,
    tool: &str,
    input: serde_json::Value,
) -> String {
    let r = draft_turn(conn, a, fake, chat, tool, input).await;
    assert!(
        r.follow.is_some(),
        "нет черновика: {}",
        last_tool_result(fake)
    );
    let (ok, _) = draft_buttons(&r);
    let text = press(conn, chat, &ok).await.text;
    assert!(text.starts_with("Проведено: "), "{text}");
    text
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// Владелец в режиме «💬 Спросить» с поддельным сервисом разбора.
async fn owner_asking(
    pool: &PgPool,
    w: &World,
    chat: i64,
) -> (avtodom_server::api::assistant::Assistant, FakeService) {
    link_owner(pool, w, chat).await;
    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    let mut conn = pool.acquire().await.unwrap();
    start_asking(&mut conn, &a, chat).await;
    (a, fake)
}

async fn add_party(pool: &PgPool, w: &World, name: &str, role: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, $3, 'person', $4, '')")
        .bind(id)
        .bind(w.owner.user.branch_id)
        .bind(role)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    if role == "supplier" {
        sqlx::query("insert into suppliers (id, name) values ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_expense_posts_only_by_button_like_the_program(pool: PgPool) {
    // Модель готовит черновик — документа нет; «✅ Провести» проводит той же функцией, что
    // программа, один раз; журнал помечает источник «бот» (SPEC-24).
    use avtodom_server::api::expenses::{ExpenseReq, post_expense_tx};
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 2_000_000).await;
    let chat = 970_001i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();

    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_expense",
        serde_json::json!({ "article": "хозтовары", "amount": "1 500", "source": "из кассы", "comment": "ведро и тряпки" }),
    )
    .await;
    assert_eq!(r.text, DRAFT_SAID);
    let f = r.follow.as_ref().unwrap();
    assert_eq!(
        f.text,
        format!(
            "Расход\nСтатья: Хозтовары\nСумма: 1 500 с\nОткуда: Касса (сейчас 20 000 с)\nДата: {}\nКомментарий: «ведро и тряпки»",
            today.format("%d.%m.%Y")
        )
    );
    let (ok, no) = draft_buttons(&r);
    assert!(ok.starts_with("d:ok:") && no.starts_with("d:no:"));
    // Черновик — ещё не документ.
    assert_eq!(count(&pool, "select count(*) from expenses").await, 0);
    assert_eq!(till_balance(&pool, till).await, 2_000_000);
    let told = last_tool_result(&fake);
    assert!(told.contains("НЕ проведён"), "{told}");
    // Модели не дан инструмент, который проводит: только отчёты и черновики.
    let names: Vec<String> = fake.requests()[0]["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("draft_") || !n.contains("post"))
    );
    assert!(
        fake.requests()[0]["systemInstruction"]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Никогда не пиши, что операция проведена")
    );

    let done = press(&mut conn, chat, &ok).await.text;
    assert_eq!(done, "Проведено: расход № 1 — Хозтовары, 1 500 с.");
    // Двойное нажатие — тот же документ, второго нет.
    assert_eq!(
        press(&mut conn, chat, &ok).await.text,
        "Уже проведено: расход № 1 — Хозтовары, 1 500 с."
    );
    assert_eq!(
        press(&mut conn, chat, &no).await.text,
        "Уже проведено: расход № 1 — Хозтовары, 1 500 с."
    );
    assert_eq!(count(&pool, "select count(*) from expenses").await, 1);
    assert_eq!(till_balance(&pool, till).await, 1_850_000);

    // Тот же расход из программы — те же поля.
    let article: Uuid = sqlx::query_scalar("select article_id from expenses")
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    post_expense_tx(
        &mut tx,
        &w.owner,
        ExpenseReq {
            op_id: Uuid::now_v7(),
            article_id: article,
            amount_tyiyn: 150_000,
            source: Some("account".into()),
            account_id: Some(till),
            expense_date: Some(today),
            comment: "ведро и тряпки".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    type Row = (
        Uuid,
        i64,
        String,
        Option<Uuid>,
        chrono::NaiveDate,
        String,
        Uuid,
        Uuid,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select article_id, amount_tyiyn, source, account_id, expense_date, comment, user_id, branch_id from expenses order by number",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows[0], rows[1]);
    let devices: Vec<Uuid> = sqlx::query_scalar("select device_id from expenses order by number")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(devices[0], avtodom_server::auth::TELEGRAM_DEVICE);
    assert_eq!(devices[1], w.owner.device_id);
    // Журнал: источник «бот» и чат — только у проведённого через бот.
    let journal = audit_data(&pool, "expense.post").await;
    assert_eq!(journal[0]["source"], "bot");
    assert_eq!(journal[0]["chat"], chat);
    assert_eq!(journal[0]["from"], "account");
    assert!(journal[1].get("source").is_none());
    assert_eq!(journal[1]["from"], "account");
    // Черновик: проведён, номер документа в итоге; изменить или удалить нельзя.
    let (status, number): (String, Option<i64>) =
        sqlx::query_as("select status, (result->>'number')::bigint from bot_drafts")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), number), ("posted", Some(1)));
    assert!(
        sqlx::query("update bot_drafts set status = 'cancelled'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update bot_drafts set summary = 'x'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("delete from bot_drafts")
            .execute(&pool)
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_draft_cancel_expiry_and_foreign_buttons(pool: PgPool) {
    // «✖ Отмена», 10 минут, кнопка из другого чата или чужой черновик — ничего не проводят;
    // новый черновик закрывает прежний (SPEC-24).
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 100_000).await;
    let chat = 970_101i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();
    let bot_moves = "select count(*) from cash_movements where device_id = '7e1e6a3d-0000-4000-8000-000000000b07'";
    let deposit =
        serde_json::json!({ "operation": "внесение", "amount": "500", "comment": "размен" });

    let r = draft_turn(&mut conn, &a, &fake, chat, "draft_money", deposit.clone()).await;
    assert_eq!(
        r.follow.as_ref().unwrap().text,
        "Внесение денег\nКасса: Касса (сейчас 1 000 с)\nСумма: 500 с\nКомментарий: «размен»"
    );
    let (ok, no) = draft_buttons(&r);
    // Чат другого владельца того же филиала и непривязанный чат.
    let other_owner = Uuid::now_v7();
    sqlx::query("insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, 'owner2', 'x', 'owner', 'owner2')")
        .bind(other_owner)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let other_owner_chat = 970_102i64;
    link_user_chat(
        &pool,
        other_owner,
        w.owner.user.branch_id,
        other_owner_chat,
        0,
    )
    .await;
    assert_eq!(
        press(&mut conn, other_owner_chat, &ok).await.text,
        "Черновик не найден — ничего не проведено."
    );
    press(&mut conn, 970_103, &ok).await;
    // Чат работника.
    set_phone(&pool, w.master, "0700 55-66-77").await;
    let staff_chat = 970_104i64;
    share(&mut conn, staff_chat, "+996 700 55 66 77", true).await;
    assert_eq!(employee_chats(&pool).await.len(), 1);
    press(&mut conn, staff_chat, &ok).await;
    // Подделанные данные кнопки.
    assert_eq!(
        press(&mut conn, chat, "d:ok:не-id").await.text,
        "Кнопка устарела."
    );
    assert_eq!(
        press(&mut conn, chat, &format!("d:ok:{}", Uuid::now_v7()))
            .await
            .text,
        "Черновик не найден — ничего не проведено."
    );
    assert_eq!(count(&pool, bot_moves).await, 0);

    // Отмена — ничего не проведено, после неё «Провести» не работает.
    assert_eq!(
        press(&mut conn, chat, &no).await.text,
        "Черновик отменён — ничего не проведено."
    );
    assert_eq!(
        press(&mut conn, chat, &ok).await.text,
        "Черновик уже снят — ничего не проведено."
    );
    assert_eq!(count(&pool, bot_moves).await, 0);

    // Через 10 минут черновик не проводится.
    let r = draft_turn(&mut conn, &a, &fake, chat, "draft_money", deposit.clone()).await;
    let (ok, _) = draft_buttons(&r);
    sqlx::query("alter table bot_drafts disable trigger bot_drafts_immutable")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "update bot_drafts set expires_at = now() - interval '1 second' where status = 'open'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("alter table bot_drafts enable trigger bot_drafts_immutable")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        press(&mut conn, chat, &ok)
            .await
            .text
            .starts_with("Черновик устарел")
    );
    assert_eq!(count(&pool, bot_moves).await, 0);

    // Новый черновик закрывает прежний: проводится только последний.
    let first = draft_turn(&mut conn, &a, &fake, chat, "draft_money", deposit.clone()).await;
    let second = draft_turn(&mut conn, &a, &fake, chat, "draft_money", deposit).await;
    let (ok1, _) = draft_buttons(&first);
    let (ok2, _) = draft_buttons(&second);
    assert_eq!(
        press(&mut conn, chat, &ok1).await.text,
        "Черновик уже снят — ничего не проведено."
    );
    assert_eq!(
        press(&mut conn, chat, &ok2).await.text,
        "Проведено: внесение 500 с, в кассе теперь 1 500 с."
    );
    assert_eq!(count(&pool, bot_moves).await, 1);
    let statuses: Vec<String> =
        sqlx::query_scalar("select status from bot_drafts order by created_at")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(statuses, ["cancelled", "expired", "cancelled", "posted"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_draft_refuses_ambiguous_and_impossible(pool: PgPool) {
    // Два «Азамата» — список, черновика нет; расход больше остатка — отказ с остатком; без
    // черновика «записал» не проходит; вне режима и не владельцу черновик не готовится (SPEC-24).
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 10_000).await;
    // Минус по факту разрешён у всех счетов (ADR-077); владелец выключил его у кассы.
    sqlx::query("update cash_accounts set allow_negative = false where id = $1")
        .bind(till)
        .execute(&pool)
        .await
        .unwrap();
    add_party(&pool, &w, "Азамат Осмонов", "customer").await;
    add_party(&pool, &w, "Азамат Бекович", "customer").await;
    let chat = 970_201i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();

    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_debt",
        serde_json::json!({ "operation": "погашение долга клиентом", "party": "Азамат", "amount": "5 000" }),
    )
    .await;
    assert!(r.follow.is_none());
    let told = last_tool_result(&fake);
    assert!(
        told.contains("Азамат Осмонов")
            && told.contains("Азамат Бекович")
            && told.contains("уточните"),
        "{told}"
    );

    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_expense",
        serde_json::json!({ "article": "Хозтовары", "amount": "1 500 сом" }),
    )
    .await;
    assert!(r.follow.is_none());
    let told = last_tool_result(&fake);
    assert!(
        told.contains("на счёте «Касса» только 100 с") && told.contains("В «Касса» сейчас 100 с"),
        "{told}"
    );

    for (tool, input, why) in [
        (
            "draft_product",
            serde_json::json!({ "operation": "цена продажи", "product": "Свечи NGK", "value": "300" }),
            "не найден",
        ),
        (
            "draft_expense",
            serde_json::json!({ "article": "Хозтовары", "amount": "полторы тысячи" }),
            "не разобрать",
        ),
        (
            "draft_staff",
            serde_json::json!({ "operation": "премия", "employee": "Тимур", "amount": "100" }),
            "не найден",
        ),
        (
            "draft_money",
            serde_json::json!({ "operation": "перевод", "amount": "50", "to": "сейф" }),
            "несколько касс",
        ),
    ] {
        let r = draft_turn(&mut conn, &a, &fake, chat, tool, input).await;
        assert!(r.follow.is_none());
        let told = last_tool_result(&fake);
        assert!(told.contains(why), "{tool}: {told}");
    }
    assert_eq!(count(&pool, "select count(*) from bot_drafts").await, 0);

    // Модель «записала» без черновика — сервер не верит, документа нет.
    fake.push(vec![Step::Reply(
        200,
        final_text("Записал расход 1 500 на хозтовары."),
    )]);
    let r = ask_reply(&mut conn, &a, chat, "запиши расход 1500 на хозтовары").await;
    assert!(r.text.starts_with("Ничего не проведено"), "{}", r.text);
    assert!(r.follow.is_none());
    assert_eq!(count(&pool, "select count(*) from expenses").await, 0);

    // Вне режима «Спросить», в чужом чате и не владельцу черновик не готовится.
    use avtodom_server::api::bot_drafts::{Prepared, prepare};
    let input = serde_json::json!({ "article": "Хозтовары", "amount": "10" });
    for (user, chat_id) in [(&w.owner.user, 970_202i64), (&w.admin.user, chat)] {
        let p = prepare(&mut conn, user, chat_id, "draft_expense", &input)
            .await
            .unwrap();
        assert!(matches!(p, Prepared::Refused(_)));
    }
    say(&mut conn, chat, "✖ Закончить").await;
    let p = prepare(&mut conn, &w.owner.user, chat, "draft_expense", &input)
        .await
        .unwrap();
    assert!(matches!(p, Prepared::Refused(_)));
    assert_eq!(count(&pool, "select count(*) from bot_drafts").await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_receipt_totals_by_server_and_daily_cap(pool: PgPool) {
    // Сумму строк прихода считает сервер; приход через бот — как из программы; 31-я операция
    // за сутки не проводится (SPEC-24).
    use avtodom_server::api::receipts::{ReceiptLineReq, ReceiptReq, post_receipt_tx};
    let w = seed(&pool).await;
    let supplier = add_party(&pool, &w, "Автоснаб", "supplier").await;
    let chat = 970_301i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();

    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({
            "supplier": "Автоснаб", "payment": "в долг", "supplier_doc": "Н-77",
            "lines": [
                { "product": "Фильтр W712", "quantity": "10", "price": "350" },
                { "product": "масло 5w-30", "quantity": "2 канистры", "price": "1 800,50" }
            ]
        }),
    )
    .await;
    let text = r.follow.as_ref().unwrap().text.clone();
    assert_eq!(
        text,
        "Приход товара\nПоставщик: Автоснаб\n1. Фильтр W712 — 10 шт × 350 с = 3 500 с\n2. Масло 5W-30 4л — 2 кан. по 4 л × 1 800,50 с = 3 601 с\nИтого: 7 101 с\nРасчёт: в долг поставщику\nДокумент поставщика: «Н-77»\nКомментарий: —"
    );
    assert_eq!(count(&pool, "select count(*) from receipts").await, 0);
    let (ok, _) = draft_buttons(&r);
    assert_eq!(
        press(&mut conn, chat, &ok).await.text,
        "Проведено: приход № 1 — 7 101 с."
    );
    let balance: i64 = sqlx::query_scalar("select balance_tyiyn from parties where id = $1")
        .bind(supplier)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(balance, -710_100);
    // Тот же приход из программы.
    let mut tx = pool.begin().await.unwrap();
    post_receipt_tx(
        &mut tx,
        &w.owner,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            supplier_id: Some(supplier),
            payment: Some("debt".into()),
            supplier_doc: "Н-77".into(),
            paid_now_tyiyn: None,
            comment: String::new(),
            lines: vec![
                ReceiptLineReq {
                    product_id: w.filter,
                    qty: 10,
                    cost_tyiyn: 350_000,
                    sale_price_tyiyn: None,
                },
                ReceiptLineReq {
                    product_id: w.oil,
                    qty: 8000,
                    cost_tyiyn: 360_100,
                    sale_price_tyiyn: None,
                },
            ],
            paid_from: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    type Line = (i32, Uuid, i64, i64);
    let lines = |n: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, Line>(
                "select l.line_no, l.product_id, l.qty, l.cost_tyiyn from receipt_lines l join receipts r on r.id = l.receipt_id where r.number = $1 order by l.line_no",
            )
            .bind(n)
            .fetch_all(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(lines(1).await, lines(2).await);
    let heads: Vec<(i64, Option<Uuid>, String, Option<String>)> = sqlx::query_as(
        "select total_tyiyn, supplier_id, supplier_doc, paid_from from receipts order by number",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(heads[0], heads[1]);
    assert_eq!(heads[0].0, 710_100);
    let journal = audit_data(&pool, "receipt.post").await;
    assert_eq!(journal[0]["source"], "bot");
    assert!(journal[1].get("source").is_none());

    // 30 операций уже проведено через бот: черновик, созданный раньше, не проводится.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_product",
        serde_json::json!({ "operation": "минимальный остаток", "product": "Фильтр W712", "value": "5" }),
    )
    .await;
    let (ok, _) = draft_buttons(&r);
    sqlx::query(
        "insert into bot_drafts (id, chat_id, user_id, branch_id, kind, payload, summary, op_id, status, finished_at, expires_at)
         select gen_random_uuid(), 1, $1, $2, 'expense', '{}', '', gen_random_uuid(), 'posted', now(), now()
         from generate_series(1, 29)",
    )
    .bind(w.owner.user.id)
    .bind(w.owner.user.branch_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        press(&mut conn, chat, &ok).await.text,
        avtodom_server::api::bot_drafts::LIMIT_TEXT
    );
    let min: i64 =
        sqlx::query_scalar("select min_stock from branch_products where product_id = $1")
            .bind(w.filter)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(min, 0);
    assert_eq!(
        count(
            &pool,
            "select count(*) from bot_drafts where status = 'rejected'"
        )
        .await,
        1
    );
    // И новый черновик уже не готовится.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_product",
        serde_json::json!({ "operation": "минимальный остаток", "product": "Фильтр W712", "value": "5" }),
    )
    .await;
    assert!(r.follow.is_none());
    assert!(last_tool_result(&fake).contains("30 операций"));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_drafts_cover_every_operation_group(pool: PgPool) {
    // Деньги, долги, сотрудники, товар: каждая операция проводится той же функцией, что
    // программа, с источником «бот» в журнале (SPEC-24).
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 2_000_000).await;
    let client = add_party(&pool, &w, "Азамат Осмонов", "customer").await;
    let supplier = add_party(&pool, &w, "Автоснаб", "supplier").await;
    let chat = 970_401i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();
    let party_balance = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("select balance_tyiyn from parties where id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };

    // Сдать в сейф.
    post_via_bot(&mut conn, &a, &fake, chat, "draft_money",
        serde_json::json!({ "operation": "перевод", "amount": "5 000", "from": "Касса", "to": "сейф владельца" })).await;
    let safe: i64 =
        sqlx::query_scalar("select balance_tyiyn from cash_accounts where name = 'Сейф владельца'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (till_balance(&pool, till).await, safe),
        (1_500_000, 500_000)
    );

    // Долг из тетради, погашение наличными, оплата поставщику.
    post_via_bot(&mut conn, &a, &fake, chat, "draft_debt",
        serde_json::json!({ "operation": "перенос долга из тетради", "party": "Азамат", "amount": "5 000", "comment": "старая тетрадь" })).await;
    assert_eq!(party_balance(client).await, 500_000);
    let text = post_via_bot(&mut conn, &a, &fake, chat, "draft_debt",
        serde_json::json!({ "operation": "погашение долга клиентом", "party": "Азамат Осмонов", "amount": "2 000", "method": "наличные" })).await;
    assert_eq!(
        text,
        "Проведено: оплата 2 000 с учтена, баланс теперь 3 000 с — должны нам."
    );
    assert_eq!(party_balance(client).await, 300_000);
    post_via_bot(&mut conn, &a, &fake, chat, "draft_debt",
        serde_json::json!({ "operation": "оплата поставщику", "party": "Автоснаб", "amount": "1 000" })).await;
    assert_eq!(party_balance(supplier).await, 100_000);
    assert_eq!(till_balance(&pool, till).await, 1_600_000);

    // Премия, удержание, аванс сверх заработанного, выплата сверх — отказ, оклад.
    post_via_bot(&mut conn, &a, &fake, chat, "draft_staff",
        serde_json::json!({ "operation": "премия", "employee": "мастер", "amount": "2 000", "comment": "за чистоту" })).await;
    post_via_bot(&mut conn, &a, &fake, chat, "draft_staff",
        serde_json::json!({ "operation": "удержание", "employee": "Мастер", "amount": "500", "comment": "опоздание" })).await;
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_staff",
        serde_json::json!({ "operation": "выплата", "employee": "Мастер", "amount": "3 000" }),
    )
    .await;
    assert!(r.follow.is_none());
    assert!(
        last_tool_result(&fake).contains("заработано 1 500 с"),
        "{}",
        last_tool_result(&fake)
    );
    let text = post_via_bot(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_staff",
        serde_json::json!({ "operation": "аванс", "employee": "Мастер", "amount": "3 000" }),
    )
    .await;
    assert_eq!(text, "Проведено: аванс № 1 — 3 000 с.");
    assert_eq!(till_balance(&pool, till).await, 1_300_000);
    post_via_bot(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_staff",
        serde_json::json!({ "operation": "оклад", "employee": "Кассир", "amount": "30 000" }),
    )
    .await;
    let accruals: Vec<(String, i64, String)> = sqlx::query_as(
        "select kind, amount_tyiyn, comment from payroll_accruals order by created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        accruals,
        [
            ("bonus".to_string(), 200_000, "за чистоту".to_string()),
            ("penalty".to_string(), -50_000, "опоздание".to_string()),
            ("monthly_salary".to_string(), 3_000_000, String::new()),
        ]
    );

    // Цена продажи, минимальный остаток (масло — литрами), цена закупки без прихода.
    post_via_bot(&mut conn, &a, &fake, chat, "draft_product",
        serde_json::json!({ "operation": "цена продажи", "product": "Фильтр W712", "value": "550" })).await;
    post_via_bot(&mut conn, &a, &fake, chat, "draft_product",
        serde_json::json!({ "operation": "минимальный остаток", "product": "Масло 5W-30 4л", "value": "12,5" })).await;
    post_via_bot(&mut conn, &a, &fake, chat, "draft_product",
        serde_json::json!({ "operation": "цена закупки", "product": "Фильтр W712", "value": "300" })).await;
    let (sale, last_qty, last_cost): (i64, i64, i64) = sqlx::query_as(
        "select sale_price_tyiyn, last_cost_qty, last_cost_tyiyn from branch_products where product_id = $1",
    )
    .bind(w.filter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((sale, last_qty, last_cost), (55_000, 1, 30_000));
    let min: i64 =
        sqlx::query_scalar("select min_stock from branch_products where product_id = $1")
            .bind(w.oil)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(min, 12_500);

    // Каждое действие бота в журнале — с источником «бот» и чатом.
    let journal: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "select action, data from audit_log where device_id = $1 order by created_at",
    )
    .bind(avtodom_server::auth::TELEGRAM_DEVICE)
    .fetch_all(&pool)
    .await
    .unwrap();
    let actions: std::collections::BTreeSet<&str> =
        journal.iter().map(|(a, _)| a.as_str()).collect();
    for want in [
        "cash.transfer",
        "debt.adjust",
        "debt.repayment",
        "payroll.accrual",
        "payout.post",
        "payroll.salary",
        "product.prices",
        "stock.purchase_price",
    ] {
        assert!(actions.contains(want), "нет {want}: {actions:?}");
    }
    assert!(
        journal
            .iter()
            .all(|(_, d)| d["source"] == "bot" && d["chat"] == chat)
    );
    assert_eq!(
        count(
            &pool,
            "select count(*) from bot_drafts where status = 'posted'"
        )
        .await,
        11
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_owner_reports_come_from_program_functions(pool: PgPool) {
    // Отчёты бота — из тех же расчётов, что экраны: склад, деньги, чеки, сводка (SPEC-24).
    use avtodom_server::api::catalog::{ProductFilter, query_products};
    use avtodom_server::domain::money::format_som;
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    receive(&pool, &w.admin, w.filter, 5, 150_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 2, 50_000)], cash(100_000)),
    )
    .await
    .unwrap();
    let chat = 970_501i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();
    let report = |tool: &'static str, input: serde_json::Value| {
        fake.push(vec![Step::Reply(200, tool_use(tool, input)), Step::Echo]);
    };

    report("stock_report", serde_json::json!({ "report": "stock" }));
    let text = ask_reply(&mut conn, &a, chat, "сколько товара на складе?")
        .await
        .text;
    let products = query_products(
        &mut conn,
        &w.owner.user,
        ProductFilter {
            limit: 500,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let value: i64 = products.iter().map(|p| p.stock_value_tyiyn.unwrap()).sum();
    assert_ne!(value, 0);
    assert!(
        text.contains(&format!("стоимость остатков {}", format_som(value))),
        "{text}"
    );

    report(
        "money_report",
        serde_json::json!({ "report": "movements", "account": "касса" }),
    );
    let text = ask_reply(&mut conn, &a, chat, "движения по кассе")
        .await
        .text;
    assert!(
        text.contains(&format!(
            "Касса: сейчас {}",
            format_som(till_balance(&pool, till).await)
        )),
        "{text}"
    );
    assert!(text.contains("продажа наличными: +1 000 с"), "{text}");

    report("sales_report", serde_json::json!({ "report": "checks" }));
    let text = ask_reply(&mut conn, &a, chat, "чеки за сегодня").await.text;
    assert!(
        text.contains("продаж 1, итог с возвратами 1 000 с (наличными 1 000 с"),
        "{text}"
    );

    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();
    let totals =
        avtodom_server::api::reports::totals_for(&mut conn, w.owner.user.branch_id, today, today)
            .await
            .unwrap();
    report("sales_report", serde_json::json!({ "report": "dashboard" }));
    let text = ask_reply(&mut conn, &a, chat, "сводка").await.text;
    assert!(
        text.contains(&format!("чистая прибыль {}", format_som(totals.net_tyiyn))),
        "{text}"
    );
    report(
        "sales_report",
        serde_json::json!({ "report": "categories" }),
    );
    let text = ask_reply(&mut conn, &a, chat, "по категориям").await.text;
    assert!(
        text.contains(&format!(
            "— Фильтры: выручка 1 000 с, закупка 600 с, валовая {}",
            format_som(totals.gross_tyiyn)
        )),
        "{text}"
    );
    report("stock_report", serde_json::json!({ "report": "receipts" }));
    let text = ask_reply(&mut conn, &a, chat, "приходы").await.text;
    assert!(text.contains("1 накладная на 1 500 с"), "{text}");

    // Карточка клиента и акт сверки — те же функции, что экраны.
    let client = add_party(&pool, &w, "Азамат Осмонов", "customer").await;
    let mut tx = pool.begin().await.unwrap();
    avtodom_server::api::parties::post_adjust_tx(
        &mut tx,
        &w.owner,
        avtodom_server::api::parties::AdjustReq {
            op_id: Uuid::now_v7(),
            party_id: client,
            amount_tyiyn: 500_000,
            comment: "тетрадь".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    report(
        "party_report",
        serde_json::json!({ "report": "reconciliation", "query": "Азамат" }),
    );
    let text = ask_reply(&mut conn, &a, chat, "акт сверки Азамат")
        .await
        .text;
    assert!(
        text.contains("Сальдо на конец: 5 000 с (должны точке)"),
        "{text}"
    );
    assert!(
        text.contains("Корректировка долга: в пользу точки 5 000 с"),
        "{text}"
    );
    report(
        "party_report",
        serde_json::json!({ "report": "card", "query": "азамат" }),
    );
    let text = ask_reply(&mut conn, &a, chat, "карточка Азамата")
        .await
        .text;
    assert!(text.contains("Баланс: 5 000 с (должны точке)"), "{text}");
    report(
        "party_report",
        serde_json::json!({ "report": "oil_book", "query": "Азамат" }),
    );
    let text = ask_reply(&mut conn, &a, chat, "книжка Азамата").await.text;
    assert!(text.contains("машин в масляной книжке нет"), "{text}");
    // Отчёты бота журналируются как вопросы и ничего не проводят.
    assert_eq!(count(&pool, "select count(*) from bot_drafts").await, 0);
}

// ---------- Брак и возврат поставщику (SPEC-25) ----------

use avtodom_server::api::defects::{
    CommentReq, MarkReq, ResolveReq, mark_tx, resolve_tx, reverse_tx, unmark_tx,
};

/// Поставщик: запись справочника и контрагент с тем же id (SPEC-10).
async fn supplier(pool: &PgPool, w: &World, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("insert into suppliers (id, name) values ($1, $2)")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("insert into parties (id, branch_id, role, kind, name) values ($1, $2, 'supplier', 'company', $3)")
        .bind(id)
        .bind(w.owner.user.branch_id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Приход от поставщика: в долг или оплаченный из «Сейфа владельца».
async fn receive_from(
    pool: &PgPool,
    w: &World,
    sid: Uuid,
    product: Uuid,
    qty: i64,
    cost: i64,
    debt: bool,
) {
    let safe = owner_safe(pool, w.owner.user.branch_id).await;
    let mut tx = pool.begin().await.unwrap();
    post_receipt_tx(
        &mut tx,
        &w.owner,
        ReceiptReq {
            op_id: Uuid::now_v7(),
            payment: Some(if debt { "debt" } else { "paid" }.into()),
            paid_from: (!debt).then(|| safe.to_string()),
            paid_now_tyiyn: None,
            supplier_id: Some(sid),
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
}

async fn mark_defect(
    pool: &PgPool,
    ctx: &Ctx,
    product: Uuid,
    qty: i64,
) -> Result<avtodom_server::api::defects::DefectOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = mark_tx(
        &mut tx,
        ctx,
        MarkReq {
            op_id: Uuid::now_v7(),
            product_id: product,
            qty,
            reason: "течёт канистра".into(),
            supplier_id: None,
            no_supplier: false,
        },
    )
    .await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

fn resolution(kind: &str) -> ResolveReq {
    ResolveReq {
        op_id: Uuid::now_v7(),
        kind: kind.into(),
        replace_qty: None,
        refund_tyiyn: None,
        account_id: None,
        supplier_id: None,
        comment: String::new(),
    }
}

async fn decide(
    pool: &PgPool,
    ctx: &Ctx,
    id: Uuid,
    req: ResolveReq,
) -> Result<avtodom_server::api::defects::DefectOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = resolve_tx(&mut tx, ctx, id, req).await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

async fn undo(pool: &PgPool, ctx: &Ctx, id: Uuid) -> avtodom_server::api::defects::DefectOut {
    let mut tx = pool.begin().await.unwrap();
    let r = reverse_tx(
        &mut tx,
        ctx,
        id,
        CommentReq {
            op_id: Uuid::now_v7(),
            comment: "ошиблись".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    r
}

async fn today_totals(pool: &PgPool, w: &World) -> avtodom_server::api::reports::Totals {
    let mut conn = pool.acquire().await.unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    avtodom_server::api::reports::totals_for(&mut conn, w.owner.user.branch_id, today, today)
        .await
        .unwrap()
}

async fn party_balance(pool: &PgPool, id: Uuid) -> i64 {
    sqlx::query_scalar("select balance_tyiyn from parties where id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_mark_unmark_and_write_off(pool: PgPool) {
    let w = seed(&pool).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    // 10 шт по 1 000 и 10 шт по 2 000 — средняя 1 500.
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    receive(&pool, &w.owner, w.filter, 10, 2_000_000).await;

    // Больше остатка — отказ.
    let err = mark_defect(&pool, &w.admin, w.filter, 21)
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Conflict(_)), "{err:?}");

    // Администратор отмечает: товар уходит из остатка по средней, поставщик — из последнего
    // прихода с поставщиком; стоимость администратору не отдаётся.
    let d = mark_defect(&pool, &w.admin, w.filter, 2).await.unwrap();
    assert_eq!(d.status, "pending");
    assert_eq!(d.supplier_id, Some(sid));
    assert_eq!(d.cost_tyiyn, None);
    assert_eq!(pool_of(&pool, w.filter).await, (18, 2_700_000, false));
    let cost: i64 = sqlx::query_scalar("select cost_tyiyn from defects where id = $1")
        .bind(d.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cost, 300_000);
    assert_stock_consistent(&pool, &w).await;

    // Владелец узнаёт об отметке.
    let note: serde_json::Value =
        sqlx::query_scalar("select data from audit_log where action = 'defect.mark'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(note["cost"], 300_000);
    assert_eq!(note["reason"], "течёт канистра");

    // Решает только владелец.
    let err = decide(&pool, &w.admin, d.id, resolution("write_off"))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Forbidden), "{err:?}");

    // Снять отметку до решения: товар возвращается по той же стоимости.
    let mut tx = pool.begin().await.unwrap();
    let back = unmark_tx(
        &mut tx,
        &w.admin,
        d.id,
        CommentReq {
            op_id: Uuid::now_v7(),
            comment: "проверили — целый".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(back.status, "cancelled");
    assert_eq!(pool_of(&pool, w.filter).await, (20, 3_000_000, false));
    assert_stock_consistent(&pool, &w).await;
    // Снятую отметку не решают.
    let err = decide(&pool, &w.owner, d.id, resolution("write_off"))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Conflict(_)), "{err:?}");

    // Списание: убыток ровно на стоимость брака.
    let before = today_totals(&pool, &w).await;
    let d = mark_defect(&pool, &w.owner, w.filter, 2).await.unwrap();
    assert_eq!(d.cost_tyiyn, Some(300_000));
    let out = decide(&pool, &w.owner, d.id, resolution("write_off"))
        .await
        .unwrap();
    assert_eq!(out.status, "written_off");
    let after = today_totals(&pool, &w).await;
    assert_eq!(after.defect_writeoff_tyiyn, 300_000);
    assert_eq!(after.net_tyiyn, before.net_tyiyn - 300_000);
    assert_eq!(after.cost_tyiyn, before.cost_tyiyn);
    assert_stock_consistent(&pool, &w).await;

    // Сторно списания: запись снова ждёт решения, убыток уходит.
    let back = undo(&pool, &w.owner, d.id).await;
    assert_eq!(back.status, "pending");
    assert!(back.resolution.is_none());
    let after = today_totals(&pool, &w).await;
    assert_eq!(after.defect_writeoff_tyiyn, 0);
    assert_eq!(after.net_tyiyn, before.net_tyiyn);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_returned_for_new_goods(pool: PgPool) {
    let w = seed(&pool).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    let before = today_totals(&pool, &w).await;
    let d = mark_defect(&pool, &w.admin, w.filter, 4).await.unwrap();
    assert_eq!(pool_of(&pool, w.filter).await.0, 6);

    // Взамен столько же: остаток вернулся, прибыль не изменилась.
    let out = decide(&pool, &w.owner, d.id, resolution("replace"))
        .await
        .unwrap();
    assert_eq!(out.status, "returned");
    assert_eq!(pool_of(&pool, w.filter).await, (10, 1_000_000, false));
    let t = today_totals(&pool, &w).await;
    assert_eq!((t.defect_writeoff_tyiyn, t.defect_diff_tyiyn), (0, 0));
    assert_eq!(t.net_tyiyn, before.net_tyiyn);
    assert_stock_consistent(&pool, &w).await;

    // Сторно возвращает товар взамен обратно.
    undo(&pool, &w.owner, d.id).await;
    assert_eq!(pool_of(&pool, w.filter).await, (6, 600_000, false));
    assert_stock_consistent(&pool, &w).await;

    // Взамен меньше: недостающее списывается той же операцией.
    let mut req = resolution("replace");
    req.replace_qty = Some(3);
    decide(&pool, &w.owner, d.id, req).await.unwrap();
    assert_eq!(pool_of(&pool, w.filter).await, (9, 900_000, false));
    let t = today_totals(&pool, &w).await;
    assert_eq!(t.defect_writeoff_tyiyn, 100_000);
    assert_eq!(t.net_tyiyn, before.net_tyiyn - 100_000);
    // Больше брака взамен не бывает.
    assert_stock_consistent(&pool, &w).await;
    let d2 = mark_defect(&pool, &w.admin, w.filter, 1).await.unwrap();
    let mut req = resolution("replace");
    req.replace_qty = Some(2);
    let err = decide(&pool, &w.owner, d2.id, req).await.unwrap_err();
    assert!(matches!(err, AppError::Validation(_)), "{err:?}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_credit_nets_with_next_debt_receipt(pool: PgPool) {
    let w = seed(&pool).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    let before = today_totals(&pool, &w).await;
    let d = mark_defect(&pool, &w.admin, w.filter, 2).await.unwrap();
    // Поставщик дал меньше стоимости: 150 с вместо 200 с.
    let mut req = resolution("credit");
    req.refund_tyiyn = Some(150_000);
    let out = decide(&pool, &w.owner, d.id, req).await.unwrap();
    assert_eq!(out.resolution.as_ref().unwrap().diff_tyiyn, Some(-50_000));
    // Поставщик должен нам.
    assert_eq!(party_balance(&pool, sid).await, 150_000);
    let t = today_totals(&pool, &w).await;
    assert_eq!(t.defect_diff_tyiyn, -50_000);
    assert_eq!(t.net_tyiyn, before.net_tyiyn - 50_000);
    // Следующая накладная в долг на 1 000 с: наш долг — 850 с.
    receive_from(&pool, &w, sid, w.filter, 5, 100_000, true).await;
    assert_eq!(party_balance(&pool, sid).await, -100_000 + 150_000);
    receive_from(&pool, &w, sid, w.filter, 5, 1_000_000, true).await;
    assert_eq!(party_balance(&pool, sid).await, 150_000 - 1_100_000);
    assert_stock_consistent(&pool, &w).await;

    // Сторно решения откатывает долг и разницу.
    undo(&pool, &w.owner, d.id).await;
    assert_eq!(party_balance(&pool, sid).await, -1_100_000);
    let t = today_totals(&pool, &w).await;
    assert_eq!(t.defect_diff_tyiyn, 0);
    let entries: i64 = sqlx::query_scalar(
        "select count(*) from party_ledger where party_id = $1 and doc_type in ('defect', 'defect_reversal')",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(entries, 2);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_cash_refund_goes_to_till(pool: PgPool) {
    let w = seed(&pool).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let till = avtodom_server::api::cash::default_account(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    drop(conn);
    let before = today_totals(&pool, &w).await;
    let d = mark_defect(&pool, &w.admin, w.filter, 1).await.unwrap();
    let mut req = resolution("cash");
    req.account_id = Some(till);
    req.refund_tyiyn = Some(120_000);
    // В кассу смены — только в открытой смене.
    let err = decide(&pool, &w.owner, d.id, req.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Conflict(_)), "{err:?}");
    open_till(&pool, &w).await;
    // Администратору деньгами нельзя.
    let err = decide(&pool, &w.admin, d.id, req.clone())
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Forbidden), "{err:?}");
    req.op_id = Uuid::now_v7();
    decide(&pool, &w.owner, d.id, req).await.unwrap();
    let balance = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("select balance_tyiyn from cash_accounts where id = $1")
            .bind(till)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(balance(pool.clone()).await, 120_000);
    // Поставщик дал больше стоимости (100 с) — доход 20 с.
    let t = today_totals(&pool, &w).await;
    assert_eq!(t.defect_diff_tyiyn, 20_000);
    assert_eq!(t.net_tyiyn, before.net_tyiyn + 20_000);
    // Сторно забирает деньги из кассы.
    undo(&pool, &w.owner, d.id).await;
    assert_eq!(balance(pool.clone()).await, 0);
    assert_eq!(today_totals(&pool, &w).await.net_tyiyn, before.net_tyiyn);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_reports_show_defects_and_deliveries(pool: PgPool) {
    // Отчёты бота «брак» и «поставки» — из тех же функций, что экраны (SPEC-24, -25, -26).
    let w = seed(&pool).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    mark_defect(&pool, &w.admin, w.filter, 2).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    let text = avtodom_server::api::bot_reports::stock(
        &mut conn,
        &w.owner.user,
        "defects",
        None,
        (today, today),
    )
    .await
    .unwrap();
    assert!(text.contains("ждут решения 1 на 2 000 с"), "{text}");
    assert!(text.contains("Фильтр W712 × 2 шт"), "{text}");
    let text = avtodom_server::api::bot_reports::party(
        &mut conn,
        &w.owner.user,
        "deliveries",
        "Автоснаб",
        None,
    )
    .await
    .unwrap();
    assert!(
        text.contains("Поставки Автоснаб за всё время: на 10 000 с"),
        "{text}"
    );
    assert!(text.contains("привёз 10 шт"), "{text}");
    assert!(
        text.contains("на складе 8 шт, из них от него (оценка) 8 шт"),
        "{text}"
    );
}

// ---------- Исправления по ревью (ADR-070) ----------

async fn audit_count(pool: &PgPool, action: &str) -> i64 {
    sqlx::query_scalar("select count(*) from audit_log where action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn storno(pool: &PgPool, ctx: &Ctx, id: Uuid) -> Result<(), AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = reverse_receipt_tx(
        &mut tx,
        ctx,
        id,
        ReverseReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
            in_correction: false,
        },
    )
    .await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r.map(|_| ())
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_blocks_receipt_storno_and_archived_target(pool: PgPool) {
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    let sid = supplier(&pool, &w, "Автоснаб").await;
    receive_from(&pool, &w, sid, w.filter, 10, 1_000_000, false).await;
    let receipt: Uuid = sqlx::query_scalar(
        "select id from receipts where supplier_id = $1 and reversal_of is null",
    )
    .bind(sid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let d = mark_defect(&pool, &w.admin, w.filter, 2).await.unwrap();
    assert_eq!(d.supplier_id, Some(sid));
    // Брак этого поставщика ждёт решения — приход не сторнируется, номер брака назван.
    let err = storno(&pool, &w.owner, receipt).await.unwrap_err();
    assert!(
        matches!(&err, AppError::Conflict(m) if m.contains(&format!("№ {}", d.number))),
        "{err:?}"
    );
    // Взамен привезли новый товар — решение действует, сторно всё так же нельзя.
    decide(&pool, &w.owner, d.id, resolution("replace"))
        .await
        .unwrap();
    assert!(matches!(
        storno(&pool, &w.owner, receipt).await,
        Err(AppError::Conflict(_))
    ));
    // Весь товар продан, потом решение отменили: остаток ушёл в минус — владельцу уведомление.
    sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("piece", w.filter, 10, 150_000)],
            cash(1_500_000),
        ),
    )
    .await
    .unwrap();
    let back = undo(&pool, &w.owner, d.id).await;
    assert_eq!(back.status, "pending");
    assert_eq!(audit_count(&pool, "defect.reverse_negative").await, 1);
    let data: serde_json::Value =
        sqlx::query_scalar("select data from audit_log where action = 'defect.reverse_negative'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(data["lines"][0]["after"], -2);
    // Карточку убрали в архив в обход проверок: снять отметку на неё нельзя, понятный отказ.
    sqlx::query("update products set archived = true where id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    let err = unmark_tx(
        &mut tx,
        &w.admin,
        d.id,
        CommentReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
        },
    )
    .await
    .unwrap_err();
    drop(tx);
    assert!(
        matches!(&err, AppError::Conflict(m) if m.contains("в архиве")),
        "{err:?}"
    );
    let err = decide(&pool, &w.owner, d.id, resolution("replace"))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppError::Conflict(m) if m.contains("в архиве")),
        "{err:?}"
    );
    sqlx::query("update products set archived = false where id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    // Отметку сняли — приход сторнируется (остаток уходит в минус — это решает владелец).
    let mut tx = pool.begin().await.unwrap();
    unmark_tx(
        &mut tx,
        &w.admin,
        d.id,
        CommentReq {
            op_id: Uuid::now_v7(),
            comment: String::new(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    storno(&pool, &w.owner, receipt).await.unwrap();
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn defect_of_stock_without_value_uses_last_purchase_price(pool: PgPool) {
    let w = seed(&pool).await;
    // 5 шт по 1 000 с, потом стоимость остатка ушла в ноль: брак — по последней закупке.
    receive(&pool, &w.owner, w.filter, 5, 500_000).await;
    sqlx::query("update branch_products set stock_value_tyiyn = 0 where product_id = $1")
        .bind(w.filter)
        .execute(&pool)
        .await
        .unwrap();
    let d = mark_defect(&pool, &w.owner, w.filter, 2).await.unwrap();
    assert_eq!(d.cost_tyiyn, Some(200_000));
    // Остаток переоценён по последней закупке и стоит на проверке у владельца.
    assert_eq!(pool_of(&pool, w.filter).await, (3, 300_000, true));
    // Ни стоимости, ни одной закупки — цену брака не узнать.
    sqlx::query(
        "update branch_products set stock_qty = 3, stock_value_tyiyn = 0, last_cost_qty = 0, last_cost_tyiyn = 0 where product_id = $1",
    )
    .bind(w.oil)
    .execute(&pool)
    .await
    .unwrap();
    let err = mark_defect(&pool, &w.owner, w.oil, 1).await.unwrap_err();
    assert!(
        matches!(&err, AppError::Validation(m) if m.contains("укажите цену закупки")),
        "{err:?}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn returns_refund_exactly_the_original_bank_fee(pool: PgPool) {
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    let bank = {
        let mut conn = pool.acquire().await.unwrap();
        avtodom_server::api::cash::bank_account(&mut conn, w.owner.user.branch_id, "card")
            .await
            .unwrap()
            .unwrap()
    };
    // На счёте есть из чего вернуть деньги на карту.
    fund_till(&pool, &w, bank, 1_000_000).await;
    receive(&pool, &w.admin, w.filter, 10, 100_000).await;
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
    // Чек 3 × 333,33 с картой: комиссия 0,5 % — 5,00 с.
    let sale = sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("piece", w.filter, 3, 33_333)],
            pay(&[("card", 99_999)]),
        ),
    )
    .await
    .unwrap();
    let orig = fee(sale.id).await;
    assert_eq!(orig, 500);
    // Ставку потом подняли — возврат всё равно берёт комиссию исходного чека.
    sqlx::query("update payment_method_fees set rate_bp = 300 where method = 'card'")
        .execute(&pool)
        .await
        .unwrap();
    // Два частичных и последний возврат вместе возвращают ровно исходную комиссию.
    let mut back = 0;
    for _ in 0..3 {
        let r = return_one(&pool, &w.owner, sale.id, pay(&[("card", 33_333)]))
            .await
            .unwrap();
        back += fee(r.id).await;
    }
    assert_eq!(back, -orig);
    // Целиком одним возвратом — тоже ровно исходная.
    let sale = sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("piece", w.filter, 1, 99_999)],
            pay(&[("card", 99_999)]),
        ),
    )
    .await
    .unwrap();
    let orig = fee(sale.id).await;
    let r = return_one(&pool, &w.owner, sale.id, pay(&[("card", 99_999)]))
        .await
        .unwrap();
    assert_eq!(fee(r.id).await, -orig);
}

/// Обновление Телеграма: сообщение из чата вида `kind` от `from`.
fn tg_message(chat: i64, kind: &str, from: i64, text: &str) -> serde_json::Value {
    serde_json::json!({
        "update_id": 1,
        "message": { "chat": { "id": chat, "type": kind }, "from": { "id": from }, "text": text }
    })
}

/// Обновление Телеграма: нажатие кнопки в чате вида `kind` пользователем `from`.
fn tg_press(chat: i64, kind: &str, from: i64, data: &str) -> serde_json::Value {
    serde_json::json!({
        "update_id": 2,
        "callback_query": {
            "id": "cb-1",
            "from": { "id": from },
            "message": { "chat": { "id": chat, "type": kind } },
            "data": data
        }
    })
}

async fn draft_status(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("select status from bot_drafts order by created_at")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_answers_only_private_chats_and_the_linking_owner(pool: PgPool) {
    // Группа не привязывается и ничего не получает; кнопки черновика принимаются только от
    // того, кто привязал чат (ADR-070).
    use avtodom_server::api::telegram::{handle_callback_from, handle_update};
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 100_000).await;
    sqlx::query("insert into telegram_codes (code, user_id, expires_at) values ('482913', $1, now() + interval '10 minutes')")
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let group = -100_200_300i64;
    assert!(
        handle_update(&mut conn, None, tg_message(group, "group", 7001, "482913"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        handle_update(
            &mut conn,
            None,
            tg_message(group, "supergroup", 7001, "/start 482913")
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(count(&pool, "select count(*) from telegram_links").await, 0);
    assert_eq!(
        count(&pool, "select count(*) from telegram_code_failures").await,
        0
    );
    // Личный чат — привязка, номер владельца в Телеграме запомнен.
    let chat = 7001i64;
    let r = handle_update(&mut conn, None, tg_message(chat, "private", chat, "482913"))
        .await
        .unwrap()
        .unwrap();
    assert!(r.text.starts_with("Готово"), "{}", r.text);
    let tg: Option<i64> =
        sqlx::query_scalar("select tg_user_id from telegram_links where chat_id = $1")
            .bind(chat)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(tg, Some(chat));
    // Команды владельца из группы — молчание.
    assert!(
        handle_update(
            &mut conn,
            None,
            tg_message(group, "group", chat, "📊 Сегодня")
        )
        .await
        .unwrap()
        .is_none()
    );

    let (url, fake) = fake_service().await;
    let a = fake_assistant(url);
    start_asking(&mut conn, &a, chat).await;
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_money",
        serde_json::json!({ "operation": "внесение", "amount": "500", "comment": "размен" }),
    )
    .await;
    let (ok, _) = draft_buttons(&r);
    // Кнопка из группы — молчание; другой пользователь в том же чате — отказ.
    assert!(
        handle_update(&mut conn, Some(&a), tg_press(chat, "group", chat, &ok))
            .await
            .unwrap()
            .is_none()
    );
    let r = handle_callback_from(&mut conn, Some(&a), chat, 7002, &ok)
        .await
        .unwrap();
    assert!(r.text.contains("только владелец"), "{}", r.text);
    assert_eq!(draft_status(&pool).await, ["open"]);
    let r = handle_update(&mut conn, Some(&a), tg_press(chat, "private", chat, &ok))
        .await
        .unwrap()
        .unwrap();
    assert!(r.text.starts_with("Проведено: "), "{}", r.text);
    assert_eq!(draft_status(&pool).await, ["posted"]);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_ask_mode_codes_restart_stop_and_claims(pool: PgPool) {
    use avtodom_server::api::telegram::handle_callback_with;
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 100_000).await;
    let chat = 970_801i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;
    let mut conn = pool.acquire().await.unwrap();
    // Шесть цифр в режиме вопросов — вопрос, а не код привязки.
    fake.push(vec![Step::Reply(
        200,
        final_text("Не понял, уточните вопрос."),
    )]);
    assert_eq!(
        ask(&mut conn, &a, chat, "123456").await,
        "Не понял, уточните вопрос."
    );
    assert_eq!(
        count(&pool, "select count(*) from telegram_code_failures").await,
        0
    );
    assert_eq!(count(&pool, "select count(*) from bot_questions").await, 1);
    // Повторное «💬 Спросить» в открытом режиме не пишет второе включение.
    start_asking(&mut conn, &a, chat).await;
    assert_eq!(audit_data(&pool, "telegram.ask_on").await.len(), 1);
    // Кнопка под сообщением в режиме продлевает его.
    sqlx::query("update bot_question_mode set last_at = now() - interval '9 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    handle_callback_with(&mut conn, Some(&a), chat, "x")
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "select count(*) from bot_question_mode where last_at > now() - interval '1 minute'"
        )
        .await,
        1
    );
    // Режим истёк — «💬 Спросить» закрывает прежний как положено и открывает новый.
    sqlx::query("update bot_question_mode set last_at = now() - interval '11 minutes'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(count(&pool, "select count(*) from bot_dialog").await > 0);
    start_asking(&mut conn, &a, chat).await;
    let off = audit_data(&pool, "telegram.ask_off").await;
    assert_eq!(off.len(), 1);
    assert_eq!(off[0]["reason"], "время");
    assert_eq!(audit_data(&pool, "telegram.ask_on").await.len(), 2);
    assert_eq!(count(&pool, "select count(*) from bot_dialog").await, 0);

    // «Записал» после прочитанного отчёта — всё равно «ничего не проведено».
    fake.push(vec![
        Step::Reply(200, tool_use("shift_today", serde_json::json!({}))),
        Step::Reply(200, final_text("Записал расход 500 с.")),
    ]);
    assert!(
        ask(&mut conn, &a, chat, "запиши расход 500")
            .await
            .starts_with("Ничего не проведено")
    );
    // С черновиком — «черновик готов», черновик показан.
    fake.push(vec![
        Step::Reply(
            200,
            tool_use(
                "draft_money",
                serde_json::json!({ "operation": "внесение", "amount": "500", "comment": "размен" }),
            ),
        ),
        Step::Reply(200, final_text("Провёл внесение 500 с.")),
    ]);
    let r = ask_reply(&mut conn, &a, chat, "внеси 500").await;
    assert_eq!(r.text, "Черновик готов — проверьте и нажмите «✅ Провести»");
    assert!(r.follow.is_some());
    // Сумма с ошибочным отчётом — не отчёт: «нет данных».
    fake.push(vec![
        Step::Reply(200, tool_use("client_debt", serde_json::json!({}))),
        Step::Reply(200, final_text("Долг 1 500 с.")),
    ]);
    let text = ask(&mut conn, &a, chat, "сколько должен Азамат").await;
    assert!(!text.contains("1 500"), "{text}");

    // «✖ Закончить» снимает открытый черновик.
    assert_eq!(draft_status(&pool).await, ["open"]);
    ask(&mut conn, &a, chat, "✖ Закончить").await;
    assert_eq!(draft_status(&pool).await, ["cancelled"]);
    // Ответ, пришедший после «✖ Закончить», память разговора не пишет.
    fake.push(vec![Step::Reply(200, final_text("Поздний ответ."))]);
    avtodom_server::api::assistant::answer(
        &mut conn,
        &a,
        chat,
        w.owner.user.id,
        w.owner.user.branch_id,
        "поздний вопрос",
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, "select count(*) from bot_dialog").await, 0);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_drafts_are_one_line_exact_and_rechecked(pool: PgPool) {
    let w = seed(&pool).await;
    let till = open_till(&pool, &w).await;
    fund_till(&pool, &w, till, 100_000).await;
    let mut conn = pool.acquire().await.unwrap();
    avtodom_server::api::cash::ensure_accounts(&mut conn, w.owner.user.branch_id)
        .await
        .unwrap();
    add_party(&pool, &w, "Азамат", "customer").await;
    let chat = 970_901i64;
    let (a, fake) = owner_asking(&pool, &w, chat).await;

    // Комментарий не дописывает в черновик чужие строки, свободный текст — в кавычках.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_money",
        serde_json::json!({ "operation": "внесение", "amount": "500", "comment": "—\n\nИтого: 100,00 с" }),
    )
    .await;
    let text = r.follow.unwrap().text;
    assert_eq!(text.lines().count(), 4, "{text}");
    assert!(
        text.ends_with("Комментарий: «—  Итого: 100,00 с»"),
        "{text}"
    );

    // Погашение картой: касса — та, что возьмёт проведение, и комиссия банка.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_debt",
        serde_json::json!({ "operation": "погашение долга клиентом", "party": "Азамат", "amount": "1 000", "method": "карта" }),
    )
    .await;
    let text = r.follow.unwrap().text;
    assert!(text.contains("Куда: Счёт"), "{text}");
    assert!(
        text.contains("Комиссия банка: 5 с (на счёт придёт 995 с)"),
        "{text}"
    );
    // Перевод — на счёт по умолчанию для перевода, без комиссии его банка (ADR-077).
    {
        use avtodom_server::api::cash::{
            AccountCreateReq, AccountPatch, create_account_tx, update_account_tx,
        };
        let mut tx = pool.begin().await.unwrap();
        let card = create_account_tx(
            &mut tx,
            &w.owner,
            AccountCreateReq {
                op_id: Uuid::now_v7(),
                name: "Карта владельца".into(),
                kind: "bank".into(),
                allow_negative: true,
                owner_only: false,
                fee_rate_bp: None,
                default_card: None,
                default_transfer: None,
                default_invoice: None,
            },
        )
        .await
        .unwrap();
        update_account_tx(
            &mut tx,
            &w.owner,
            card.id,
            AccountPatch {
                default_transfer: Some(true),
                ..AccountPatch::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_debt",
        serde_json::json!({ "operation": "погашение долга клиентом", "party": "Азамат", "amount": "1 000", "method": "перевод" }),
    )
    .await;
    let text = r.follow.unwrap().text;
    assert!(text.contains("Куда: Карта владельца"), "{text}");
    assert!(!text.contains("Комиссия банка"), "{text}");

    // Оклад без суммы: показанная по ставке сумма записана в черновик и проводится она.
    sqlx::query("insert into employee_pay_rules (id, branch_id, employee_id, kind, amount_tyiyn, user_id) values ($1, $2, $3, 'monthly_salary', 2500000, $4)")
        .bind(Uuid::now_v7())
        .bind(w.owner.user.branch_id)
        .bind(w.cashier)
        .bind(w.owner.user.id)
        .execute(&pool)
        .await
        .unwrap();
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_staff",
        serde_json::json!({ "operation": "оклад", "employee": "Кассир" }),
    )
    .await;
    assert!(
        r.follow
            .as_ref()
            .unwrap()
            .text
            .contains("25 000 с (по ставке)")
    );
    let payload: serde_json::Value =
        sqlx::query_scalar("select payload from bot_drafts where status = 'open'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(payload["amount_tyiyn"], 2_500_000);
    sqlx::query("update employee_pay_rules set amount_tyiyn = 9900000 where employee_id = $1")
        .bind(w.cashier)
        .execute(&pool)
        .await
        .unwrap();
    let (ok, _) = draft_buttons(&r);
    assert!(
        press(&mut conn, chat, &ok)
            .await
            .text
            .starts_with("Проведено")
    );
    let salary: i64 = sqlx::query_scalar(
        "select amount_tyiyn from payroll_accruals where kind = 'monthly_salary'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(salary, 2_500_000);
    // Сводка расшифровывает оклад внутри оплаты труда, чистую прибыль это не меняет.
    let day: chrono::NaiveDate = sqlx::query_scalar(
        "select business_date from payroll_accruals where kind = 'monthly_salary'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let t = avtodom_server::api::reports::totals_for(&mut conn, w.owner.user.branch_id, day, day)
        .await
        .unwrap();
    assert_eq!(t.salary_tyiyn, 2_500_000);
    assert!(t.payroll_tyiyn >= t.salary_tyiyn);

    // «10к» — не количество: просим уточнить, черновика нет.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({ "paid_from": "Сейф владельца", "lines": [{ "product": "Фильтр W712", "quantity": "10к", "price": "100" }] }),
    )
    .await;
    assert!(r.follow.is_none());
    assert!(
        last_tool_result(&fake).contains("«10к» не понять"),
        "{}",
        last_tool_result(&fake)
    );
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({ "paid_from": "из своих", "lines": [{ "product": "Фильтр W712", "quantity": "10 шт", "price": "100" }] }),
    )
    .await;
    // «Из своих, мимо касс» больше нет (ADR-079): черновика нет, объяснение в ответе.
    assert!(r.follow.is_none());
    assert!(
        last_tool_result(&fake).contains("«из своих, мимо касс» больше нет"),
        "{}",
        last_tool_result(&fake)
    );
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({ "paid_from": "Сейф владельца", "lines": [{ "product": "Фильтр W712", "quantity": "10 шт", "price": "100" }] }),
    )
    .await;
    assert!(r.follow.is_some(), "{}", last_tool_result(&fake));
    // Касса не найдена — подсказка из настоящих касс филиала (ADR-092).
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({ "paid_from": "Конверт", "lines": [{ "product": "Фильтр W712", "quantity": "10 шт", "price": "100" }] }),
    )
    .await;
    assert!(r.follow.is_none());
    let said = last_tool_result(&fake);
    assert!(
        said.contains("Касса «Конверт» не найдена. Кассы и счета: ")
            && said.contains("«Сейф владельца»"),
        "{said}"
    );
    // Счёт со словом «свои» в названии — обычный счёт, черновик по нему готовится.
    sqlx::query(
        "insert into cash_accounts (id, branch_id, name, kind, owner_only, is_default) values ($1, $2, 'Свои деньги', 'other', true, false)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .execute(&mut *conn)
    .await
    .unwrap();
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_receipt",
        serde_json::json!({ "paid_from": "Свои деньги", "lines": [{ "product": "Фильтр W712", "quantity": "10 шт", "price": "100" }] }),
    )
    .await;
    assert!(r.follow.is_some(), "{}", last_tool_result(&fake));
    assert!(
        last_tool_result(&fake).contains("«Свои деньги"),
        "{}",
        last_tool_result(&fake)
    );

    // Сотрудника отключили, пока черновик ждал кнопки, — не проводится.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_staff",
        serde_json::json!({ "operation": "премия", "employee": "Мастер", "amount": "100", "comment": "за чистоту" }),
    )
    .await;
    assert!(r.follow.is_some(), "{}", last_tool_result(&fake));
    let (ok, _) = draft_buttons(&r);
    sqlx::query("update employees set active = false where id = $1")
        .bind(w.master)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        press(&mut conn, chat, &ok).await.text,
        "Не проведено: сотрудник «Мастер» отключён. Черновик закрыт — проверьте данные и опишите операцию заново."
    );
    assert_eq!(draft_status(&pool).await.last().unwrap(), "rejected");

    // Просроченный черновик закрывается фоновым проходом.
    let r = draft_turn(
        &mut conn,
        &a,
        &fake,
        chat,
        "draft_money",
        serde_json::json!({ "operation": "внесение", "amount": "500", "comment": "размен" }),
    )
    .await;
    assert!(r.follow.is_some());
    sqlx::query("alter table bot_drafts disable trigger bot_drafts_immutable")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "update bot_drafts set expires_at = now() - interval '1 second' where status = 'open'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("alter table bot_drafts enable trigger bot_drafts_immutable")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        avtodom_server::api::bot_drafts::expire_drafts(&mut conn)
            .await
            .unwrap(),
        1
    );
    assert_eq!(draft_status(&pool).await.last().unwrap(), "expired");
}

// ---------- SPEC-27 §2: аккумуляторы ----------

/// Цена приёма аккумуляторов за кг (настройка SPEC-03, ADR-048).
async fn battery_price(pool: &PgPool, w: &World, per_kg: i64) {
    sqlx::query(
        "insert into settings (branch_id, key, value) values ($1, 'batteries', jsonb_build_object('intake_per_kg_tyiyn', $2::bigint))
         on conflict (branch_id, key) do update set value = excluded.value",
    )
    .bind(w.owner.user.branch_id)
    .bind(per_kg)
    .execute(pool)
    .await
    .unwrap();
}

fn battery(grams: i64, amount: i64) -> PaymentReq {
    PaymentReq {
        account_id: None,
        price_per_kg_tyiyn: None,
        method: "battery".into(),
        amount_tyiyn: amount,
        grams: Some(grams),
    }
}

async fn scrap_id(pool: &PgPool) -> Uuid {
    sqlx::query_scalar(
        "select id from products where unit = 'g' and not archived order by created_at limit 1",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn percent_rule(pool: &PgPool, w: &World) {
    sqlx::query(
        "insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base, rate_bp, user_id)
         values ($1, $2, $3, 'revenue_percent', 'cashier', 'gross', 200, $4)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(w.cashier)
    .bind(w.owner.user.id)
    .execute(pool)
    .await
    .unwrap();
}

/// Чек 10 000 с: два фильтра по 5 000 с, закупка по 2 000 с.
fn ten_thousand(w: &World, payments: Vec<PaymentReq>) -> SaleReq {
    takeaway(w, vec![line("piece", w.filter, 2, 500_000)], payments)
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_payment_takes_scrap_by_weight(pool: PgPool) {
    // Пример заказчика (SPEC-27): чек 10 000 с, аккумулятор 12 кг × 50 с = 600 с — к оплате
    // 9 400 с, на складе +12 кг лома стоимостью 600 с, выручка 10 000 с; 2 % кассира — со всей
    // валовой прибыли (аккумулятор — настоящая оплата), комиссии банка нет.
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 2_000_000).await;
    battery_price(&pool, &w, 5_000).await;
    percent_rule(&pool, &w).await;
    // Сумма, которую видела касса, не совпала с ценой приёма — чек не проводится.
    let stale = sell(
        &pool,
        &w.admin,
        ten_thousand(&w, vec![battery(12_000, 50_000), cash(950_000).remove(0)]),
    )
    .await;
    assert!(
        matches!(&stale, Err(AppError::Validation(m)) if m.contains("цена приёма изменилась")),
        "{:?}",
        stale.err().map(|e| e.to_string())
    );
    // Без веса — тоже нет.
    let mut no_weight = battery(12_000, 60_000);
    no_weight.grams = None;
    assert!(matches!(
        sell(
            &pool,
            &w.admin,
            ten_thousand(&w, vec![no_weight, cash(940_000).remove(0)])
        )
        .await,
        Err(AppError::Validation(_))
    ));
    let sale = sell(
        &pool,
        &w.admin,
        ten_thousand(&w, vec![battery(12_000, 60_000), cash(940_000).remove(0)]),
    )
    .await
    .unwrap();
    assert_eq!(sale.total_tyiyn, 1_000_000);
    let bat = sale
        .payments
        .iter()
        .find(|p| p.method == "battery")
        .unwrap();
    assert_eq!((bat.amount_tyiyn, bat.grams), (60_000, Some(12_000)));
    // Лом на складе: 12 кг стоимостью 600 с, движение «аккумулятор в оплату».
    let scrap = scrap_id(&pool).await;
    assert_eq!(pool_of(&pool, scrap).await, (12_000, 60_000, false));
    let kinds: Vec<String> = sqlx::query_scalar(
        "select doc_type from stock_movements where product_id = $1 and doc_id = $2",
    )
    .bind(scrap)
    .bind(sale.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(kinds, vec!["battery_tradein".to_string()]);
    // В кассу — только наличные 9 400 с; комиссии нет.
    let cash_in: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from cash_movements where doc_id = $1",
    )
    .bind(sale.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(cash_in, 940_000);
    let fee: i64 = sqlx::query_scalar(
        "select coalesce(sum(fee_tyiyn), 0)::bigint from sale_payments where sale_id = $1",
    )
    .bind(sale.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fee, 0);
    // Выручка — 10 000 с: строки чека не уменьшаются.
    let revenue: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from sale_lines where sale_id = $1",
    )
    .bind(sale.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(revenue, 1_000_000);
    // 2 % с валовой 6 000 с = 120 с: часть аккумулятором входит в оплаченное.
    let (base, amount): (i64, i64) = sqlx::query_as(
        "select base_tyiyn, amount_tyiyn from payroll_accruals where doc_id = $1 and kind = 'revenue_percent'",
    )
    .bind(sale.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((base, amount), (600_000, 12_000));
    // Лента владельцу: «аккумулятор 12 кг — 600 с».
    let mut conn = pool.acquire().await.unwrap();
    let (_, text) = avtodom_server::api::telegram::sale_text(&mut conn, sale.id)
        .await
        .unwrap();
    assert!(text.contains("аккумулятор 12 кг — 600 с"), "{text}");
    drop(conn);
    // Последняя цена приёма — запасная себестоимость лома.
    let (last_qty, last_cost): (i64, i64) = sqlx::query_as(
        "select last_cost_qty, last_cost_tyiyn from branch_products where product_id = $1",
    )
    .bind(scrap)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((last_qty, last_cost), (12_000, 60_000));
    assert_stock_consistent(&pool, &w).await;
}

async fn give_back(
    pool: &PgPool,
    ctx: &Ctx,
    sale: Uuid,
    qty: i64,
    payments: Vec<PaymentReq>,
    battery_cash: bool,
) -> Result<avtodom_server::api::sales::SaleOut, AppError> {
    let mut tx = pool.begin().await.unwrap();
    let r = return_sale_tx(
        &mut tx,
        ctx,
        sale,
        ReturnReq {
            op_id: Uuid::now_v7(),
            battery_cash,
            comment: "вернули".into(),
            lines: vec![ReturnLineReq { line_no: 1, qty }],
            payments,
            business_date: None,
        },
    )
    .await;
    if r.is_ok() {
        tx.commit().await.unwrap();
    }
    r
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_payment_returns_by_weight_not_cash(pool: PgPool) {
    // Возврат чека с аккумулятором в оплату: вес уходит со склада, денег за него не выдают;
    // лом уже продан — эту часть деньгами возвращает только владелец явной отметкой (SPEC-27).
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    receive(&pool, &w.admin, w.filter, 10, 2_000_000).await;
    battery_price(&pool, &w, 5_000).await;
    percent_rule(&pool, &w).await;
    let pay = || vec![battery(12_000, 60_000), cash(940_000).remove(0)];
    let sale = sell(&pool, &w.admin, ten_thousand(&w, pay()))
        .await
        .unwrap();
    let scrap = scrap_id(&pool).await;
    // Весь чек наличными — нельзя: 600 с были аккумулятором.
    let all_cash = give_back(&pool, &w.admin, sale.id, 2, cash(1_000_000), false).await;
    assert!(
        matches!(&all_cash, Err(AppError::Validation(m)) if m.contains("верните аккумулятором")),
        "{:?}",
        all_cash.err().map(|e| e.to_string())
    );
    // Половина — наличными: денег по чеку платили 9 400 с, хватает.
    give_back(&pool, &w.admin, sale.id, 1, cash(500_000), false)
        .await
        .unwrap();
    // Вторая половина: 4 400 с деньгами и 600 с аккумулятором — 12 кг уходят со склада.
    let mut back = cash(440_000);
    back.push(PaymentReq {
        account_id: None,
        price_per_kg_tyiyn: None,
        method: "battery".into(),
        amount_tyiyn: 60_000,
        grams: None,
    });
    let r = give_back(&pool, &w.admin, sale.id, 1, back, false)
        .await
        .unwrap();
    let bat = r.payments.iter().find(|p| p.method == "battery").unwrap();
    assert_eq!((bat.amount_tyiyn, bat.grams), (-60_000, Some(-12_000)));
    assert_eq!(pool_of(&pool, scrap).await, (0, 0, false));
    // Процент кассира снят целиком: чек вернули полностью.
    let percent: i64 = sqlx::query_scalar(
        "select coalesce(sum(amount_tyiyn), 0)::bigint from payroll_accruals where kind = 'revenue_percent'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(percent, 0);

    // Второй чек: лом из него продали на вес — вернуть вес нельзя.
    let sale = sell(&pool, &w.admin, ten_thousand(&w, pay()))
        .await
        .unwrap();
    sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("weight", scrap, 12_000, 6_000)], cash(72_000)),
    )
    .await
    .unwrap();
    let mut back = cash(940_000);
    back.push(PaymentReq {
        account_id: None,
        price_per_kg_tyiyn: None,
        method: "battery".into(),
        amount_tyiyn: 60_000,
        grams: None,
    });
    let sold = give_back(&pool, &w.admin, sale.id, 2, back, false).await;
    assert!(
        matches!(&sold, Err(AppError::Conflict(m)) if m.contains("только владелец")),
        "{:?}",
        sold.err().map(|e| e.to_string())
    );
    // Деньгами за аккумулятор — администратору нельзя, владельцу с отметкой можно.
    assert!(matches!(
        give_back(&pool, &w.admin, sale.id, 2, cash(1_000_000), true).await,
        Err(AppError::Forbidden)
    ));
    give_back(&pool, &w.owner, sale.id, 2, cash(1_000_000), true)
        .await
        .unwrap();
    assert_eq!(audit_data(&pool, "sale.return_battery_cash").await.len(), 1);
    assert_eq!(pool_of(&pool, scrap).await.0, 0);
    assert_stock_consistent(&pool, &w).await;
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_polarity_in_line_name_and_filter(pool: PgPool) {
    // Полярность — характеристика аккумулятора: «(Л)» прямая, «(П)» обратная — в названии
    // строки чека; товары фильтруются по ней (SPEC-27).
    let w = seed(&pool).await;
    let cat = Uuid::now_v7();
    sqlx::query("insert into categories (id, name, kind, attributes) values ($1, 'АКБ', 'battery', '[{\"key\": \"polarity\", \"label\": \"Полярность\", \"type\": \"select\", \"options\": [\"прямая\", \"обратная\"], \"filterable\": true}]')")
        .bind(cat)
        .execute(&pool)
        .await
        .unwrap();
    let (left, right) = (Uuid::now_v7(), Uuid::now_v7());
    for (id, name, pol) in [
        (left, "Varta 60", "прямая"),
        (right, "Varta 60R", "обратная"),
    ] {
        sqlx::query("insert into products (id, category_id, name, unit, attrs) values ($1, $2, $3, 'piece', jsonb_build_object('polarity', $4::text))")
            .bind(id)
            .bind(cat)
            .bind(name)
            .bind(pol)
            .execute(&pool)
            .await
            .unwrap();
    }
    receive(&pool, &w.admin, right, 2, 1_000_000).await;
    let sale = sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("piece", right, 1, 700_000)], cash(700_000)),
    )
    .await
    .unwrap();
    assert_eq!(sale.lines[0].name, "Varta 60R (П)");
    let mut conn = pool.acquire().await.unwrap();
    let found = avtodom_server::api::catalog::query_products(
        &mut conn,
        &w.admin.user,
        avtodom_server::api::catalog::ProductFilter {
            category_id: Some(cat),
            attrs: Some(serde_json::json!({ "polarity": "прямая" })),
            limit: 50,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(found.iter().map(|p| p.id).collect::<Vec<_>>(), vec![left]);
}

// ---------- SPEC-27 §3: книжка по каждому фильтру ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn oil_book_lines_by_filter_with_own_intervals(pool: PgPool) {
    // Чек с маслом, масляным и воздушным фильтром — три строки книжки, у каждой свой
    // интервал (категории или по умолчанию); интервал правится у машины; напоминания — по
    // строкам; бот показывает строки (SPEC-27).
    use avtodom_server::api::telegram::{due_reminders, mark_reminded};
    let w = seed(&pool).await;
    let branch = w.owner.user.branch_id;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    // Воздушные фильтры — своя категория со строкой книжки и интервалом 20 000 км / 60 дней.
    let (air_cat, air) = (Uuid::now_v7(), Uuid::now_v7());
    sqlx::query("insert into categories (id, name, kind, book_slot, book_interval_km, book_interval_days) values ($1, 'Воздушные', 'filter', 'air_filter', 20000, 60)")
        .bind(air_cat)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit) values ($1, $2, 'Фильтр воздушный A1', 'piece')")
        .bind(air)
        .bind(air_cat)
        .execute(&pool)
        .await
        .unwrap();
    // Антифриз — «прочие работы» без интервала; категория без строки в книжку не идёт.
    let (other_cat, antifreeze, none_cat, wiper) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::query("insert into categories (id, name, kind, book_slot) values ($1, 'Жидкости', 'other', 'other'), ($2, 'Щётки', 'other', null)")
        .bind(other_cat)
        .bind(none_cat)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("insert into products (id, category_id, name, unit) values ($1, $2, 'Антифриз G12', 'piece'), ($3, $4, 'Щётка 60 см', 'piece')")
        .bind(antifreeze)
        .bind(other_cat)
        .bind(wiper)
        .bind(none_cat)
        .execute(&pool)
        .await
        .unwrap();
    for p in [air, antifreeze, wiper] {
        receive(&pool, &w.admin, p, 2, 20_000).await;
    }
    let chat = 990_001i64;
    let me = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    ready_for_service(&pool, me).await;
    let car = staff_car(&pool, &w, me, "01KG555AAA", true).await;
    let mut req = takeaway(
        &w,
        vec![
            line("container", w.oil, 1, 200_000),
            line("piece", w.filter, 1, 50_000),
            line("piece", air, 1, 30_000),
            line("piece", antifreeze, 1, 30_000),
            line("piece", wiper, 1, 30_000),
            kind_line(w.service, 1, 0),
        ],
        cash(340_000),
    );
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.party_id = Some(me);
    req.vehicle_id = Some(car);
    req.mileage_km = Some(85_000);
    sell(&pool, &w.owner, req).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let book = avtodom_server::api::oil_book::books(&mut conn, branch, None, Some(car))
        .await
        .unwrap()
        .pop()
        .unwrap();
    let r = &book.records[0];
    let slots: Vec<&str> = r.lines.iter().map(|l| l.slot.as_str()).collect();
    assert_eq!(slots, vec!["oil", "oil_filter", "air_filter", "other"]);
    assert!(r.lines[2].text.contains("Фильтр воздушный A1 · 1 шт"));
    let next = |slot: &str| {
        book.slots.iter().find(|n| n.slot == slot).map(|n| {
            (
                n.next_km,
                n.next_date.map(|d| (d - r.change_date).num_days()),
            )
        })
    };
    assert_eq!(next("oil"), Some((Some(93_000), Some(31))));
    assert_eq!(next("oil_filter"), Some((Some(93_000), Some(31))));
    assert_eq!(next("air_filter"), Some((Some(105_000), Some(60))));
    // У прочих работ интервала нет — строка есть, срока нет.
    assert_eq!(next("other"), Some((None, None)));
    assert!(next("cabin_filter").is_none());
    // Прежние поля записи заполнены, как раньше.
    assert!(r.oil_text.contains("Масло 5W-30 4л") && r.filter_text.contains("Фильтр W712"));

    // Интервал воздушного фильтра у этой машины — 3 дня: напоминание только по нему.
    sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) values ($1, $2, 'air_filter', 10000, 3)")
        .bind(car)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    let book = avtodom_server::api::oil_book::books(&mut conn, branch, None, Some(car))
        .await
        .unwrap()
        .pop()
        .unwrap();
    let air_next = book.slots.iter().find(|n| n.slot == "air_filter").unwrap();
    assert!(air_next.own_interval);
    assert_eq!(air_next.next_km, Some(95_000));
    assert_eq!(book.next_km, Some(93_000));
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].slots, vec!["air_filter".to_string()]);
    assert!(
        due[0].text.contains("воздушного фильтра"),
        "{}",
        due[0].text
    );
    mark_reminded(&mut conn, &due[0]).await.unwrap();
    assert!(due_reminders(&mut conn).await.unwrap().is_empty());
    // Масло и масляный фильтр подошли в один день — одно сообщение на обе строки.
    sqlx::query("insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days) values ($1, $2, 'oil', 8000, 5), ($1, $2, 'oil_filter', 8000, 5)")
        .bind(car)
        .bind(branch)
        .execute(&pool)
        .await
        .unwrap();
    let due = due_reminders(&mut conn).await.unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(
        due[0].slots,
        vec!["oil".to_string(), "oil_filter".to_string()]
    );
    assert!(
        due[0].text.contains("замена масла, масляного фильтра"),
        "{}",
        due[0].text
    );

    // Бот: карточка машины — строки книжки и следующая замена по каждой.
    let card = press(&mut conn, chat, &format!("car:{car}")).await.text;
    assert!(
        card.contains("Фильтр воздушный: на 95 000 км или до")
            && card.contains("Фильтр воздушный: Фильтр воздушный A1")
            && card.contains("Прочие работы: Антифриз G12")
            && !card.contains("Щётка"),
        "{card}"
    );
}

#[sqlx::test(migrations = false)]
async fn oil_book_migration_keeps_old_records(pool: PgPool) {
    // Миграция книжки: прежние записи становятся строками «масло» и «масляный фильтр», тексты
    // в записи остаются; прежний интервал машины — интервал масла и фильтра этой машины.
    let mut applied = 0;
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version < 39) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
        applied += 1;
    }
    assert!(applied >= 38);
    let (branch, user, party, car, rec, empty) = (
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
        Uuid::now_v7(),
    );
    sqlx::raw_sql(&format!(
        "insert into branches (id, name) values ('{branch}', 'Тест');
         insert into users (id, branch_id, login, password_hash, role, full_name) values ('{user}', '{branch}', 'o', 'x', 'owner', 'o');
         insert into parties (id, branch_id, role, kind, name) values ('{party}', '{branch}', 'customer', 'person', 'Эрлан');
         insert into party_vehicles (id, branch_id, party_id, plate, plate_norm, interval_km, interval_days)
           values ('{car}', '{branch}', '{party}', '01KG1', '01KG1', 10000, 45);
         insert into oil_changes (id, branch_id, vehicle_id, party_id, change_date, mileage_km, oil_text, filter_text, user_id)
           values ('{rec}', '{branch}', '{car}', '{party}', '2026-01-10', 70000, 'Тетрадное масло', 'Фильтр W712', '{user}'),
                  ('{empty}', '{branch}', '{car}', '{party}', '2025-12-01', null, 'Только масло', '', '{user}');"
    ))
    .execute(&pool)
    .await
    .unwrap();
    for m in avtodom_server::MIGRATOR.iter().filter(|m| m.version >= 39) {
        sqlx::raw_sql(m.sql.as_ref()).execute(&pool).await.unwrap();
    }
    let items: Vec<(Uuid, String, String)> = sqlx::query_as(
        "select oil_change_id, slot, text from oil_change_items order by oil_change_id, position",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![
        (rec, "oil".to_string(), "Тетрадное масло".to_string()),
        (rec, "oil_filter".to_string(), "Фильтр W712".to_string()),
        (empty, "oil".to_string(), "Только масло".to_string()),
    ];
    expected.sort_by_key(|x| x.0);
    assert_eq!(items, expected);
    let (oil_text, filter_text): (String, String) =
        sqlx::query_as("select oil_text, filter_text from oil_changes where id = $1")
            .bind(rec)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (oil_text.as_str(), filter_text.as_str()),
        ("Тетрадное масло", "Фильтр W712")
    );
    let mut conn = pool.acquire().await.unwrap();
    let book = avtodom_server::api::oil_book::books(&mut conn, branch, None, Some(car))
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!((book.interval_km, book.interval_days), (10_000, 45));
    assert_eq!(book.next_km, Some(80_000));
    let filter = book.slots.iter().find(|n| n.slot == "oil_filter").unwrap();
    assert_eq!(
        (filter.interval_km, filter.own_interval),
        (Some(10_000), true)
    );
}

// ---------- SPEC-27 §4–5: список товаров, минус, литры, продаваемые, накладные по частям ----------

mod point_feedback_stock {
    use super::*;
    use avtodom_server::api::catalog::{ProductFilter, product_list};
    use avtodom_server::api::parties::{
        AdjustReq, LedgerReverseReq, RepaymentReq, post_adjust_tx, post_repayment_tx,
        reverse_ledger_tx,
    };
    use avtodom_server::api::receipts::last_receipt_of;
    use avtodom_server::api::reports::{TopQuery, top_products_data};
    use avtodom_server::api::supplier_debts::{receipt_debt, supplier_invoices};
    use avtodom_server::domain::qty::stock_text;

    async fn sale_cost(pool: &PgPool, sale: Uuid) -> i64 {
        sqlx::query_scalar("select sum(cost_tyiyn)::bigint from sale_lines where sale_id = $1")
            .bind(sale)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn product_list_totals_equal_rows_and_admin_sees_no_purchase(pool: PgPool) {
        let w = seed(&pool).await;
        receive(&pool, &w.owner, w.filter, 10, 100_000).await;
        receive(&pool, &w.owner, w.oil, 12_000, 300_000).await;
        // Розлив 1,5 л: остаток 10,5 л = 2 кан. по 4 л + 2,5 л.
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("pour", w.oil, 1500, 33_333)], cash(50_000)),
        )
        .await
        .unwrap();
        // Фильтр уходит в минус: 12 из 10.
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 12, 50_000)], cash(600_000)),
        )
        .await
        .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let f = || ProductFilter {
            limit: 500,
            ..Default::default()
        };
        let list = product_list(&mut conn, &w.owner.user, f(), "stock_sale", true, 0)
            .await
            .unwrap();
        assert_eq!(list.totals.count, 2);
        let purchase: i64 = list
            .items
            .iter()
            .map(|r| r.stock_purchase_tyiyn.unwrap())
            .sum();
        let sale: i64 = list.items.iter().map(|r| r.stock_sale_tyiyn).sum();
        assert_eq!(list.totals.purchase_tyiyn, Some(purchase));
        assert_eq!(list.totals.sale_tyiyn, sale);
        // По убыванию суммы по продаже: масло (10,5 л × 2000 с / 4 л = 5250 с), потом фильтр в минусе.
        assert_eq!(list.items[0].product.id, w.oil);
        let oil = &list.items[0];
        assert_eq!(oil.stock_sale_tyiyn, 525_000);
        assert_eq!(oil.purchase_price_tyiyn, Some(100_000));
        assert_eq!(oil.stock_purchase_tyiyn, Some(262_500));
        assert_eq!(
            stock_text(oil.product.stock_qty, "ml", Some(4000)),
            "10,5 л = 2 кан. по 4 л + 2,5 л"
        );
        // Минус 2 фильтра стоит по последней закупочной: −2 × 100 с.
        let filter = &list.items[1];
        assert_eq!(filter.product.stock_qty, -2);
        assert_eq!(filter.stock_purchase_tyiyn, Some(-20_000));
        assert_eq!(filter.purchase_price_tyiyn, Some(10_000));
        assert_eq!(filter.stock_sale_tyiyn, -100_000);

        // Администратор: по продаже — те же суммы, закупки и стоимости остатка нет.
        let admin = product_list(&mut conn, &w.admin.user, f(), "purchase", false, 0)
            .await
            .unwrap();
        assert_eq!(admin.totals.purchase_tyiyn, None);
        assert_eq!(admin.totals.sale_tyiyn, sale);
        for r in &admin.items {
            assert_eq!(r.purchase_price_tyiyn, None);
            assert_eq!(r.stock_purchase_tyiyn, None);
            assert_eq!(r.product.stock_value_tyiyn, None);
            assert_eq!(r.product.avg_cost_tyiyn, None);
            assert_eq!(r.product.last_purchase_price_tyiyn, None);
        }
        // Сортировка «по закупке» администратору не открыта — порядок по названию.
        let json = serde_json::to_value(&admin).unwrap();
        let first = &json["items"][0];
        assert!(first.get("purchase_price_tyiyn").is_none());
        assert!(first.get("stock_purchase_tyiyn").is_none());
        assert!(json["totals"].get("purchase_tyiyn").is_none());
        assert_eq!(admin.items[0].product.name, "Масло 5W-30 4л");

        // Отбор по категории: итоги только по нему.
        let cat: Uuid = sqlx::query_scalar("select category_id from products where id = $1")
            .bind(w.filter)
            .fetch_one(&pool)
            .await
            .unwrap();
        let only = product_list(
            &mut conn,
            &w.owner.user,
            ProductFilter {
                category_id: Some(cat),
                limit: 500,
                ..Default::default()
            },
            "name",
            false,
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            (
                only.totals.count,
                only.totals.purchase_tyiyn,
                only.totals.sale_tyiyn
            ),
            (1, Some(-20_000), -100_000)
        );
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn negative_stock_sale_costs_last_purchase_price(pool: PgPool) {
        let w = seed(&pool).await;
        receive(&pool, &w.owner, w.filter, 1, 15_000).await;
        // 1 есть, 2 — в минус: все три по 150 с.
        let s = sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 3, 50_000)], cash(150_000)),
        )
        .await
        .unwrap();
        assert_eq!(sale_cost(&pool, s.id).await, 45_000);
        // Остаток уже в минусе — снова по последней закупочной, не ноль.
        let s = sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
        )
        .await
        .unwrap();
        assert_eq!(sale_cost(&pool, s.id).await, 15_000);
        assert_eq!(pool_of(&pool, w.filter).await.0, -3);
        // Масло розливом из пустой бочки: по цене последней канистры за мл.
        receive(&pool, &w.owner, w.oil, 4000, 160_000).await;
        sell(
            &pool,
            &w.owner,
            takeaway(
                &w,
                vec![line("container", w.oil, 1, 200_000)],
                cash(200_000),
            ),
        )
        .await
        .unwrap();
        let s = sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("pour", w.oil, 500, 100_000)], cash(50_000)),
        )
        .await
        .unwrap();
        assert_eq!(sale_cost(&pool, s.id).await, 20_000);
        // Приход закрывает минус: оставшееся стоит по цене этого прихода (ADR-044).
        receive(&pool, &w.owner, w.filter, 5, 100_000).await;
        assert_eq!(pool_of(&pool, w.filter).await.0, 2);
        assert_eq!(pool_of(&pool, w.filter).await.1, 40_000);
        assert_stock_consistent(&pool, &w).await;
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn canisters_and_pour_take_exact_milliliters(pool: PgPool) {
        let w = seed(&pool).await;
        receive(&pool, &w.owner, w.oil, 12_000, 300_000).await;
        // Канистра — ровно 4000 мл, розлив 1,5 л и 0,001 л — ровно столько мл.
        sell(
            &pool,
            &w.owner,
            takeaway(
                &w,
                vec![line("container", w.oil, 1, 200_000)],
                cash(200_000),
            ),
        )
        .await
        .unwrap();
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("pour", w.oil, 1500, 33_333)], cash(50_000)),
        )
        .await
        .unwrap();
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("pour", w.oil, 1, 100_000)], cash(100)),
        )
        .await
        .unwrap();
        let (qty, _, _) = pool_of(&pool, w.oil).await;
        assert_eq!(qty, 12_000 - 4000 - 1500 - 1);
        assert_eq!(
            stock_text(qty, "ml", Some(4000)),
            "6,499 л = 1 кан. по 4 л + 2,499 л"
        );
        let moved: i64 = sqlx::query_scalar(
            "select sum(qty_delta)::bigint from stock_movements where product_id = $1",
        )
        .bind(w.oil)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(moved, qty);
        assert_stock_consistent(&pool, &w).await;
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn top_products_by_qty_and_amount_net_of_returns(pool: PgPool) {
        let w = seed(&pool).await;
        let till = open_till(&pool, &w).await;
        fund_till(&pool, &w, till, 1_000_000).await;
        receive(&pool, &w.owner, w.filter, 20, 200_000).await;
        receive(&pool, &w.owner, w.oil, 8000, 300_000).await;
        let client = add_party(&pool, &w, "Бакыт", "customer").await;
        let mut req = takeaway(&w, vec![line("piece", w.filter, 3, 50_000)], cash(150_000));
        req.party_id = Some(client);
        let sold = sell(&pool, &w.owner, req).await.unwrap();
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
        )
        .await
        .unwrap();
        sell(
            &pool,
            &w.owner,
            takeaway(
                &w,
                vec![line("container", w.oil, 1, 200_000)],
                cash(200_000),
            ),
        )
        .await
        .unwrap();
        // Возврат одного фильтра клиента: 3 штуки за 150 000 становятся 2 за 100 000.
        let mut tx = pool.begin().await.unwrap();
        return_sale_tx(
            &mut tx,
            &w.owner,
            sold.id,
            ReturnReq {
                op_id: Uuid::now_v7(),
                battery_cash: false,
                comment: String::new(),
                lines: vec![ReturnLineReq { line_no: 1, qty: 1 }],
                payments: cash(50_000),
                business_date: None,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let by_amount = top_products_data(&mut conn, &w.admin.user, TopQuery::default())
            .await
            .unwrap();
        let rows: Vec<_> = by_amount
            .items
            .iter()
            .map(|r| (r.product_id, r.qty, r.amount_tyiyn, r.checks))
            .collect();
        assert_eq!(
            rows,
            vec![(w.oil, 4000, 200_000, 1), (w.filter, 3, 150_000, 2)]
        );
        let by_qty = top_products_data(
            &mut conn,
            &w.owner.user,
            TopQuery {
                sort: Some("qty".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(by_qty.items[0].product_id, w.oil);
        // Бот владельца: «больше всех приносят» — по валовой прибыли (SPEC-28 §4).
        let text = avtodom_server::api::bot_reports::sales(
            &mut conn,
            &w.owner.user,
            "top_profit",
            (by_qty.from, by_qty.to),
        )
        .await
        .unwrap();
        assert!(text.starts_with("Больше всех приносят"), "{text}");
        assert!(text.contains("прибыль"), "{text}");
        assert!(by_qty.items.iter().all(|r| r.profit_tyiyn.is_some()));
        // Администратору прибыль не отдаётся, «по прибыли» — отказ.
        assert!(by_amount.items.iter().all(|r| r.profit_tyiyn.is_none()));
        // Клиент: что покупает — только его чеки, за всё время.
        let mine = top_products_data(
            &mut conn,
            &w.owner.user,
            TopQuery {
                party_id: Some(client),
                sort: Some("checks".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let mine: Vec<_> = mine.items.iter().map(|r| (r.product_id, r.qty)).collect();
        assert_eq!(mine, vec![(w.filter, 2)]);
        // Фильтр по категории.
        let cat: Uuid = sqlx::query_scalar("select category_id from products where id = $1")
            .bind(w.oil)
            .fetch_one(&pool)
            .await
            .unwrap();
        let oils = top_products_data(
            &mut conn,
            &w.owner.user,
            TopQuery {
                category_id: Some(cat),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(oils.items.len(), 1);
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn receipt_shows_last_purchase_price_date_and_supplier(pool: PgPool) {
        let w = seed(&pool).await;
        let sid = supplier(&pool, &w, "Автоснаб").await;
        let mut conn = pool.acquire().await.unwrap();
        assert!(
            last_receipt_of(&mut conn, w.owner.user.branch_id, w.oil)
                .await
                .unwrap()
                .is_none()
        );
        // 3 канистры по 4 л за 1050 с: в прошлый раз — 350 с за канистру.
        receive_from(&pool, &w, sid, w.oil, 12_000, 105_000, true).await;
        let last = last_receipt_of(&mut conn, w.owner.user.branch_id, w.oil)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(last.price_tyiyn, 35_000);
        assert_eq!(last.supplier_name.as_deref(), Some("Автоснаб"));
        // Сторнированный приход прошлой ценой не считается.
        let later = receive(&pool, &w.owner, w.oil, 4000, 40_000).await;
        let r = last_receipt_of(&mut conn, w.owner.user.branch_id, w.oil)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((r.price_tyiyn, r.supplier_name.clone()), (40_000, None));
        let mut tx = pool.begin().await.unwrap();
        reverse_receipt_tx(
            &mut tx,
            &w.owner,
            later.id,
            ReverseReq {
                op_id: Uuid::now_v7(),
                comment: "ошибка".into(),
                in_correction: false,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let r = last_receipt_of(&mut conn, w.owner.user.branch_id, w.oil)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.price_tyiyn, 35_000);
    }

    async fn pay(
        pool: &PgPool,
        ctx: &Ctx,
        sid: Uuid,
        amount: i64,
        method: &str,
        receipt_id: Option<Uuid>,
    ) -> Result<i64, AppError> {
        let mut tx = pool.begin().await.unwrap();
        let r = post_repayment_tx(
            &mut tx,
            ctx,
            RepaymentReq {
                op_id: Uuid::now_v7(),
                party_id: sid,
                amount_tyiyn: amount,
                method: Some(method.into()),
                comment: String::new(),
                receipt_id,
                account_id: None,
            },
        )
        .await;
        if r.is_ok() {
            tx.commit().await.unwrap();
        }
        r.map(|r| r.balance_tyiyn)
    }

    async fn debt_receipts(pool: &PgPool, sid: Uuid) -> Vec<Uuid> {
        sqlx::query_scalar(
            "select id from receipts where supplier_id = $1 and reversal_of is null order by number",
        )
        .bind(sid)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Распределения сходятся: на долг легло не больше его суммы, с оплаты — не больше её,
    /// а открытые долги минус аванс — ровно долг поставщику.
    async fn assert_allocations_consistent(pool: &PgPool, w: &World, sid: Uuid) {
        let over: i64 = sqlx::query_scalar(
            r#"select count(*) from party_ledger l
               where l.party_id = $1 and (
                 (l.amount_tyiyn < 0 and -l.amount_tyiyn < coalesce((select sum(s.amount_tyiyn) from supplier_allocations s where s.debit_id = l.id), 0))
                 or (l.amount_tyiyn > 0 and l.amount_tyiyn < coalesce((select sum(s.amount_tyiyn) from supplier_allocations s where s.credit_id = l.id), 0))
                 or coalesce((select sum(s.amount_tyiyn) from supplier_allocations s where s.debit_id = l.id or s.credit_id = l.id), 0) < 0
                 or (not supplier_ledger_live(l)
                     and coalesce((select sum(s.amount_tyiyn) from supplier_allocations s where s.debit_id = l.id or s.credit_id = l.id), 0) <> 0))"#,
        )
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(over, 0, "распределения вышли за суммы записей");
        let mut conn = pool.acquire().await.unwrap();
        let inv = supplier_invoices(&mut conn, w.owner.user.branch_id, sid)
            .await
            .unwrap();
        let balance: i64 = sqlx::query_scalar("select balance_tyiyn from parties where id = $1")
            .bind(sid)
            .fetch_one(pool)
            .await
            .unwrap();
        assert_eq!(inv.advance_tyiyn - inv.debt_tyiyn, balance);
        for i in &inv.invoices {
            assert_eq!(i.total_tyiyn - i.paid_tyiyn, i.left_tyiyn);
        }
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn partial_invoice_payments_fifo_and_storno(pool: PgPool) {
        let w = seed(&pool).await;
        let sid = supplier(&pool, &w, "Автоснаб").await;
        receive_from(&pool, &w, sid, w.filter, 10, 100_000, true).await;
        receive_from(&pool, &w, sid, w.filter, 5, 50_000, true).await;
        let ids = debt_receipts(&pool, sid).await;
        let (a, b) = (ids[0], ids[1]);
        let mut conn = pool.acquire().await.unwrap();
        let status = |d: Option<avtodom_server::api::supplier_debts::ReceiptDebt>| {
            let d = d.unwrap();
            (d.paid_tyiyn, d.left_tyiyn, d.status)
        };
        let branch = w.owner.user.branch_id;

        // Оплата второй накладной частью: 300 с из 500 с.
        pay(&pool, &w.owner, sid, 30_000, "transfer", Some(b))
            .await
            .unwrap();
        assert_eq!(
            status(receipt_debt(&mut conn, branch, b).await.unwrap()),
            (30_000, 20_000, "partial".to_string())
        );
        assert_eq!(
            status(receipt_debt(&mut conn, branch, a).await.unwrap()),
            (0, 100_000, "unpaid".to_string())
        );
        // Больше остатка по накладной — нельзя.
        let err = pay(&pool, &w.owner, sid, 20_001, "transfer", Some(b))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)), "{err:?}");
        // Без накладной — на самые старые: 1000 с первой, 100 с второй.
        pay(&pool, &w.owner, sid, 110_000, "transfer", None)
            .await
            .unwrap();
        assert_eq!(
            status(receipt_debt(&mut conn, branch, a).await.unwrap()),
            (100_000, 0, "paid".to_string())
        );
        assert_eq!(
            status(receipt_debt(&mut conn, branch, b).await.unwrap()),
            (40_000, 10_000, "partial".to_string())
        );
        assert_allocations_consistent(&pool, &w, sid).await;
        // Оплаченную накладную ещё раз не оплатить.
        assert!(
            pay(&pool, &w.owner, sid, 1, "transfer", Some(a))
                .await
                .is_err()
        );

        // История: кто, когда, сколько, откуда — у накладной и у поставщика.
        let hist = receipt_debt(&mut conn, branch, b)
            .await
            .unwrap()
            .unwrap()
            .payments;
        assert_eq!(hist.len(), 2);
        assert!(
            hist.iter()
                .all(|p| p.user_name == "owner" && p.source == "Счёт")
        );
        let amounts: Vec<_> = hist
            .iter()
            .map(|p| (p.amount_tyiyn, p.payment_tyiyn))
            .collect();
        assert!(amounts.contains(&(30_000, 30_000)) && amounts.contains(&(10_000, 110_000)));

        // Сторно оплаты без накладной: её распределения отменяются, долги возвращаются.
        let fifo: Uuid = sqlx::query_scalar(
            "select id from party_ledger where party_id = $1 and amount_tyiyn = 110000",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let mut tx = pool.begin().await.unwrap();
        reverse_ledger_tx(
            &mut tx,
            &w.owner,
            fifo,
            LedgerReverseReq {
                op_id: Uuid::now_v7(),
                comment: "ошиблись суммой".into(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            status(receipt_debt(&mut conn, branch, a).await.unwrap()),
            (0, 100_000, "unpaid".to_string())
        );
        assert_eq!(
            status(receipt_debt(&mut conn, branch, b).await.unwrap()),
            (30_000, 20_000, "partial".to_string())
        );
        let hist = receipt_debt(&mut conn, branch, b)
            .await
            .unwrap()
            .unwrap()
            .payments;
        assert!(hist.iter().any(|p| p.reversed && p.amount_tyiyn == 10_000));
        assert_allocations_consistent(&pool, &w, sid).await;

        // Правка в нашу пользу и зачёт брака закрывают старые накладные так же.
        let mut tx = pool.begin().await.unwrap();
        post_adjust_tx(
            &mut tx,
            &w.owner,
            AdjustReq {
                op_id: Uuid::now_v7(),
                party_id: sid,
                amount_tyiyn: 5_000,
                comment: "скидка за опт".into(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            status(receipt_debt(&mut conn, branch, a).await.unwrap()),
            (5_000, 95_000, "partial".to_string())
        );
        // Сторно первой накладной: её распределения снимаются, правка ложится на следующую.
        let mut tx = pool.begin().await.unwrap();
        reverse_receipt_tx(
            &mut tx,
            &w.owner,
            a,
            ReverseReq {
                op_id: Uuid::now_v7(),
                comment: "не та накладная".into(),
                in_correction: false,
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(receipt_debt(&mut conn, branch, a).await.unwrap().is_none());
        assert_eq!(
            status(receipt_debt(&mut conn, branch, b).await.unwrap()),
            (35_000, 15_000, "partial".to_string())
        );
        assert_allocations_consistent(&pool, &w, sid).await;

        // Переплата — аванс: ложится на следующую накладную сама.
        pay(&pool, &w.owner, sid, 20_000, "transfer", None)
            .await
            .unwrap();
        let inv = supplier_invoices(&mut conn, branch, sid).await.unwrap();
        assert_eq!((inv.debt_tyiyn, inv.advance_tyiyn), (0, 5_000));
        receive_from(&pool, &w, sid, w.filter, 1, 8_000, true).await;
        let c = *debt_receipts(&pool, sid).await.last().unwrap();
        assert_eq!(
            status(receipt_debt(&mut conn, branch, c).await.unwrap()),
            (5_000, 3_000, "partial".to_string())
        );
        assert_allocations_consistent(&pool, &w, sid).await;
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn admin_pays_invoice_only_from_the_till_without_going_negative(pool: PgPool) {
        let w = seed(&pool).await;
        let sid = supplier(&pool, &w, "Автоснаб").await;
        receive_from(&pool, &w, sid, w.filter, 10, 100_000, true).await;
        let a = debt_receipts(&pool, sid).await[0];
        // «Из своих» больше нет ни у кого (ADR-079); со счёта — только владелец.
        assert!(matches!(
            pay(&pool, &w.admin, sid, 1_000, "outside", Some(a)).await,
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            pay(&pool, &w.owner, sid, 1_000, "outside", Some(a)).await,
            Err(AppError::Validation(_))
        ));
        assert!(matches!(
            pay(&pool, &w.admin, sid, 1_000, "transfer", Some(a)).await,
            Err(AppError::Forbidden)
        ));
        let till = open_till(&pool, &w).await;
        fund_till(&pool, &w, till, 40_000).await;
        // Владелец запретил кассе минус (по умолчанию он разрешён, ADR-077): в кассе 400 с —
        // заплатить 500 с нельзя.
        sqlx::query("update cash_accounts set allow_negative = false where id = $1")
            .bind(till)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            pay(&pool, &w.admin, sid, 50_000, "cash", Some(a))
                .await
                .is_err()
        );
        pay(&pool, &w.admin, sid, 40_000, "cash", Some(a))
            .await
            .unwrap();
        assert_eq!(till_balance(&pool, till).await, 0);
        let mut conn = pool.acquire().await.unwrap();
        let d = receipt_debt(&mut conn, w.owner.user.branch_id, a)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((d.paid_tyiyn, d.left_tyiyn), (40_000, 60_000));
        let p = &d.payments[0];
        assert_eq!(
            (p.user_name.as_str(), p.source.as_str()),
            ("admin", "Касса")
        );
    }

    #[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
    async fn migration_allocates_old_payments_fifo(pool: PgPool) {
        // Записи долга, как они лежали до миграции 0040: распределений нет. Функция миграции
        // раскладывает старые оплаты на самые старые накладные.
        let w = seed(&pool).await;
        let sid = supplier(&pool, &w, "Старый поставщик").await;
        let branch = w.owner.user.branch_id;
        let entry = |amount: i64, doc_type: &'static str, day: &'static str| {
            let pool = pool.clone();
            let user = w.owner.user.id;
            async move {
                let id = Uuid::now_v7();
                sqlx::query(
                    "insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date, doc_type, doc_id, user_id, device_id)
                     values ($1, $2, $3, case when $5 = 'repayment' then 'repayment' when $5 = 'adjust' then 'adjust' else 'debt' end,
                             $4, $6::date, $5, $1, $7, $1)",
                )
                .bind(id)
                .bind(branch)
                .bind(sid)
                .bind(amount)
                .bind(doc_type)
                .bind(day)
                .bind(user)
                .execute(&pool)
                .await
                .unwrap();
                sqlx::query("update parties set balance_tyiyn = balance_tyiyn + $2 where id = $1")
                    .bind(sid)
                    .bind(amount)
                    .execute(&pool)
                    .await
                    .unwrap();
                id
            }
        };
        let old_debt = entry(-20_000, "adjust", "2026-09-01").await;
        let inv1 = entry(-100_000, "receipt", "2026-09-05").await;
        let inv2 = entry(-50_000, "receipt", "2026-09-10").await;
        entry(70_000, "repayment", "2026-09-12").await;
        entry(60_000, "repayment", "2026-09-20").await;
        let n: i32 = sqlx::query_scalar("select supplier_allocate($1)")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(n > 0);
        // Повторный запуск ничего не меняет.
        let again: i32 = sqlx::query_scalar("select supplier_allocate($1)")
            .bind(sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(again, 0);
        let paid = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, i64>(
                    "select coalesce(sum(amount_tyiyn), 0)::bigint from supplier_allocations where debit_id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        // 1300 с: 200 с старого долга, 1000 с первой накладной, 100 с второй.
        assert_eq!(paid(old_debt).await, 20_000);
        assert_eq!(paid(inv1).await, 100_000);
        assert_eq!(paid(inv2).await, 10_000);
        let total: i64 = sqlx::query_scalar(
            "select sum(amount_tyiyn)::bigint from supplier_allocations where party_id = $1",
        )
        .bind(sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(total, 130_000);
        assert_allocations_consistent(&pool, &w, sid).await;
    }
}

// ---------- Проданное по накладным и напоминания о долге (SPEC-27 §6–7) ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn receipts_sold_fifo_bounds(pool: PgPool) {
    // Остаток раскладывается по накладным от новых к старым: продано + осталось = привезли,
    // осталось не больше привезённого и не меньше нуля; заработок — только владельцу.
    use avtodom_server::api::income::receipts_sold;
    let w = seed(&pool).await;
    let first = receive(&pool, &w.admin, w.filter, 10, 1_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 4, 1_000)], cash(4_000)),
    )
    .await
    .unwrap();
    let second = receive(&pool, &w.admin, w.filter, 5, 1_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 8, 1_000)], cash(8_000)),
    )
    .await
    .unwrap();
    // Остаток 3: все из второй накладной, первая продана целиком.
    let mut conn = pool.acquire().await.unwrap();
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();
    let list = receipts_sold(&mut conn, &w.owner.user, today, today)
        .await
        .unwrap();
    assert_eq!(list.len(), 2);
    for r in &list {
        for i in &r.items {
            assert!(i.left_qty >= 0 && i.left_qty <= i.qty, "{i:?}");
            assert_eq!(i.sold_qty + i.left_qty, i.qty);
            assert!(i.left_cost_tyiyn >= 0 && i.left_cost_tyiyn <= i.cost_tyiyn);
        }
        assert_eq!(r.sold_cost_tyiyn + r.left_cost_tyiyn, r.cost_tyiyn);
    }
    let by = |id: Uuid| list.iter().find(|r| r.receipt_id == id).unwrap();
    let (a, b) = (by(first.id), by(second.id));
    assert_eq!((a.items[0].sold_qty, a.items[0].left_qty), (10, 0));
    assert_eq!((b.items[0].sold_qty, b.items[0].left_qty), (2, 3));
    // Первая: 10 шт по 10 с продано по 10 с = 100 с выручки при закупке 10 с — заработано 90 с.
    assert_eq!(a.revenue_tyiyn, 10_000);
    assert_eq!(a.earned_tyiyn, 9_000);
    assert_eq!(b.left_cost_tyiyn, 600);
    // Ушли в минус — у накладной ничего не осталось, но и не меньше нуля.
    sell(
        &pool,
        &w.owner,
        takeaway(&w, vec![line("piece", w.filter, 5, 1_000)], cash(5_000)),
    )
    .await
    .unwrap();
    let list = receipts_sold(&mut conn, &w.owner.user, today, today)
        .await
        .unwrap();
    assert!(list.iter().all(|r| r.items.iter().all(|i| i.left_qty == 0)));
    assert!(matches!(
        receipts_sold(&mut conn, &w.admin.user, today, today).await,
        Err(AppError::Forbidden)
    ));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn debt_reminders_three_days_before_and_on_due_date_once(pool: PgPool) {
    // Напоминание о долге: за 3 дня и в день срока, каждое один раз и только в окно рассылок
    // (SPEC-27 §7, ADR-063). Просроченный долг каждый день не напоминается.
    use avtodom_server::api::telegram::notify_pass;
    let w = seed(&pool).await;
    let chat = 600_100_200i64;
    let party = bot_customer(&pool, &w, "Эрлан", "person", chat).await;
    let quiet = bot_customer(&pool, &w, "Без долга", "person", chat + 1).await;
    // Долг взят сегодня, платить 4 дня: последний день — через 3 дня.
    sqlx::query(
        "insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn, business_date, user_id, device_id)
         values ($1, $2, $3, 'debt', 150000, (now() at time zone 'Asia/Bishkek')::date, $4, $1)",
    )
    .bind(Uuid::now_v7())
    .bind(w.owner.user.branch_id)
    .bind(party)
    .bind(w.owner.user.id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("update parties set balance_tyiyn = 150000, due_days = 4 where id = $1")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update parties set due_days = 4 where id = $1")
        .bind(quiet)
        .execute(&pool)
        .await
        .unwrap();
    let (api, fake) = fake_telegram().await;
    let mut conn = pool.acquire().await.unwrap();
    // Вне окна — ничего.
    notify_pass(&mut conn, &api, 8, true).await.unwrap();
    assert!(fake.texts(chat).is_empty());
    notify_pass(&mut conn, &api, 10, true).await.unwrap();
    let got = fake.texts(chat);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].contains("1 500 с") && got[0].contains("через 3 дня"),
        "{got:?}"
    );
    notify_pass(&mut conn, &api, 11, true).await.unwrap();
    assert_eq!(fake.texts(chat).len(), 1);
    // Срок сегодня: второе напоминание, тоже один раз.
    sqlx::query("update parties set due_days = 1 where id = $1")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    notify_pass(&mut conn, &api, 19, true).await.unwrap();
    notify_pass(&mut conn, &api, 19, true).await.unwrap();
    let got = fake.texts(chat);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[1].contains("сегодня"), "{got:?}");
    // Просрочен — больше не напоминаем; без долга — ничего.
    sqlx::query("update parties set due_days = 0 where id = $1")
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    notify_pass(&mut conn, &api, 12, true).await.unwrap();
    assert_eq!(fake.texts(chat).len(), 2);
    assert!(fake.texts(chat + 1).is_empty());
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn bot_lists_all_accounts_with_balances(pool: PgPool) {
    // Бот владельца: «Обзор счетов» — все счета с типом и остатком, итог наличными и безналом,
    // архивные — отдельно; «Счёт» — по названию безналичного счёта (SPEC-28 §5, ADR-076).
    use avtodom_server::api::cash::{
        AccountCreateReq, AccountPatch, create_account_tx, update_account_tx,
    };
    let w = seed(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let optima = create_account_tx(
        &mut tx,
        &w.owner,
        AccountCreateReq {
            op_id: Uuid::now_v7(),
            name: "ОПТИМА БИЗНЕС".into(),
            kind: "bank".into(),
            allow_negative: true,
            owner_only: false,
            fee_rate_bp: None,
            default_card: None,
            default_transfer: None,
            default_invoice: None,
        },
    )
    .await
    .unwrap();
    let lead = create_account_tx(
        &mut tx,
        &w.owner,
        AccountCreateReq {
            op_id: Uuid::now_v7(),
            name: "СВИНЕЦ".into(),
            kind: "other".into(),
            allow_negative: false,
            owner_only: false,
            fee_rate_bp: None,
            default_card: None,
            default_transfer: None,
            default_invoice: None,
        },
    )
    .await
    .unwrap();
    update_account_tx(
        &mut tx,
        &w.owner,
        lead.id,
        AccountPatch {
            active: Some(false),
            ..AccountPatch::default()
        },
    )
    .await
    .unwrap();
    // Администратор счета не правит.
    assert!(matches!(
        update_account_tx(&mut tx, &w.admin, optima.id, AccountPatch::default()).await,
        Err(AppError::Forbidden)
    ));
    tx.commit().await.unwrap();
    let today = chrono::Utc::now().date_naive();
    let mut conn = pool.acquire().await.unwrap();
    let text = avtodom_server::api::bot_reports::money(
        &mut conn,
        &w.owner.user,
        "accounts",
        None,
        None,
        (today, today),
    )
    .await
    .unwrap();
    assert!(
        text.contains("— ОПТИМА БИЗНЕС: 0 с (безналичный)"),
        "{text}"
    );
    assert!(
        text.contains("— Сейф владельца: 0 с (наличный, только владелец)"),
        "{text}"
    );
    assert!(
        text.contains(
            "— Счёт: 0 с (безналичный, по умолчанию для карты, по умолчанию для перевода, комиссия 0,50 %)"
        ),
        "{text}"
    );
    assert!(
        text.contains("Итого: наличные 0 с · безнал 0 с · всего 0 с"),
        "{text}"
    );
    assert!(text.contains("В архиве: СВИНЕЦ"), "{text}");
    let text = avtodom_server::api::bot_reports::money(
        &mut conn,
        &w.owner.user,
        "bank",
        Some("оптима бизнес"),
        None,
        (today, today),
    )
    .await
    .unwrap();
    assert!(text.starts_with("ОПТИМА БИЗНЕС"), "{text}");
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn battery_payment_price_set_in_the_sale(pool: PgPool) {
    // Кассир меняет цену за кг прямо в чеке (ADR-078): 12 кг × 70 с = 840 с при цене в
    // настройках 50 с; лом приходует этой стоимостью. Цена 0 — отказ.
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 10, 2_000_000).await;
    battery_price(&pool, &w, 5_000).await;
    let mut zero = battery(12_000, 0);
    zero.price_per_kg_tyiyn = Some(0);
    assert!(matches!(
        sell(&pool, &w.admin, ten_thousand(&w, vec![zero, cash(1_000_000).remove(0)])).await,
        Err(AppError::Validation(m)) if m.contains("больше нуля")
    ));
    let mut own = battery(12_000, 84_000);
    own.price_per_kg_tyiyn = Some(7_000);
    let sale = sell(
        &pool,
        &w.admin,
        ten_thousand(&w, vec![own, cash(916_000).remove(0)]),
    )
    .await
    .unwrap();
    let bat = sale
        .payments
        .iter()
        .find(|p| p.method == "battery")
        .unwrap();
    assert_eq!((bat.amount_tyiyn, bat.grams), (84_000, Some(12_000)));
    let scrap = scrap_id(&pool).await;
    assert_eq!(pool_of(&pool, scrap).await, (12_000, 84_000, false));
}

// ---------- Проверка 20.10.2026: «Чаще берут», загрузка услуг ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn often_bought_counts_checks_not_mixed_units(pool: PgPool) {
    // «Чаще берут» — по числу чеков: фильтр в трёх чеках выше масла, проданного 8 л одним
    // чеком; в штуках и литрах масло было бы сверху всегда.
    use avtodom_server::api::reports::{TopQuery, top_products_data};
    let w = seed(&pool).await;
    receive(&pool, &w.owner, w.filter, 20, 200_000).await;
    receive(&pool, &w.owner, w.oil, 8000, 300_000).await;
    sell(
        &pool,
        &w.owner,
        takeaway(
            &w,
            vec![line("container", w.oil, 2, 200_000)],
            cash(400_000),
        ),
    )
    .await
    .unwrap();
    for _ in 0..3 {
        sell(
            &pool,
            &w.owner,
            takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000)),
        )
        .await
        .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    let often = top_products_data(
        &mut conn,
        &w.admin.user,
        TopQuery {
            sort: Some("checks".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let rows: Vec<_> = often
        .items
        .iter()
        .map(|r| (r.product_id, r.unit.as_str(), r.qty, r.checks))
        .collect();
    assert_eq!(
        rows,
        vec![(w.filter, "piece", 3, 3), (w.oil, "ml", 8000, 1)]
    );
    // По выручке масло (4000 с) выше фильтра (1500 с).
    let by_amount = top_products_data(&mut conn, &w.admin.user, TopQuery::default())
        .await
        .unwrap();
    assert_eq!(by_amount.items[0].product_id, w.oil);
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn services_import_skips_existing_names(pool: PgPool) {
    use avtodom_server::api::refs::{
        ServiceImportItem, ServicesImportOut, ServicesImportReq, import_services_tx,
    };
    let w = seed(&pool).await;
    let item = |name: &str, price: i64| ServiceImportItem {
        name: name.into(),
        price_tyiyn: price,
    };
    let op_id = Uuid::now_v7();
    let req = || ServicesImportReq {
        op_id,
        items: vec![
            // «Замена масла» уже есть в справочнике — в другом регистре тоже пропуск.
            item("  замена МАСЛА ", 30_000),
            item("Мойка двигателя", 50_000),
            item("Замена антифриза", 40_000),
            item("мойка двигателя", 55_000),
        ],
    };
    // Администратор — отказ: справочник услуг из файла заводит владелец.
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(
        import_services_tx(&mut tx, &w.admin, req()).await,
        Err(AppError::Forbidden)
    ));
    drop(tx);
    let mut tx = pool.begin().await.unwrap();
    let out = import_services_tx(&mut tx, &w.owner, req()).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        out,
        ServicesImportOut {
            created: 2,
            skipped: vec!["замена МАСЛА".into(), "мойка двигателя".into()],
        }
    );
    // Повтор с тем же op_id — тот же ответ, без вторых записей.
    let mut tx = pool.begin().await.unwrap();
    let again = import_services_tx(&mut tx, &w.owner, req()).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(again, out);
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "select name, price_tyiyn, master_fee_tyiyn from services where branch_id = $1 order by name",
    )
    .bind(w.owner.user.branch_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("Замена антифриза".into(), 40_000, 0),
            ("Замена масла".into(), 20_000, 3000),
            ("Мойка двигателя".into(), 50_000, 0),
        ]
    );
    // Пустой список и отрицательная цена — отказ.
    let mut tx = pool.begin().await.unwrap();
    assert!(matches!(
        import_services_tx(
            &mut tx,
            &w.owner,
            ServicesImportReq {
                op_id: Uuid::now_v7(),
                items: vec![],
            },
        )
        .await,
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        import_services_tx(
            &mut tx,
            &w.owner,
            ServicesImportReq {
                op_id: Uuid::now_v7(),
                items: vec![item("Шиномонтаж", -1)],
            },
        )
        .await,
        Err(AppError::Validation(_))
    ));
}

// ---------- SPEC-30 §2–4: вид замены, данные клиента, скидка ----------

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn service_kinds_price_by_our_oil_and_report(pool: PgPool) {
    // С нашим маслом вид замены «бесплатно с нашим маслом» стоит 0, мастеру — ставка услуги;
    // две услуги — две ставки; без масла — цена клиенту (ADR-096). Отчёт «Замены по видам».
    let w = seed(&pool).await;
    open_till(&pool, &w).await;
    receive(&pool, &w.admin, w.oil, 12_000, 360_000).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    let gearbox = Uuid::now_v7();
    sqlx::query("insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, free_with_oil) values ($1, $2, 'Замена масла в коробке', 15000, 4000, true)")
        .bind(gearbox)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let client = service_client(&pool, &w).await;
    // Наше масло (канистра и литр розливом) и два вида замены — оба бесплатно.
    let with_oil = sell(
        &pool,
        &w.admin,
        in_service(
            &w,
            client,
            takeaway(
                &w,
                vec![
                    line("container", w.oil, 1, 200_000),
                    line("pour", w.oil, 1000, 50_000),
                    kind_line(w.service, 1, 0),
                    kind_line(gearbox, 1, 0),
                ],
                cash(250_000),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(with_oil.lines[2].list_price_tyiyn, 0);
    assert_eq!(with_oil.lines[3].list_price_tyiyn, 0);
    assert_eq!(with_oil.master_fee_tyiyn, 3_000 + 4_000);
    // Цена выше каталожной — не скидка: администратор без права ставит 100 с за «бесплатную».
    let above = sell(
        &pool,
        &w.admin,
        in_service(
            &w,
            client,
            takeaway(
                &w,
                vec![
                    line("container", w.oil, 1, 200_000),
                    kind_line(w.service, 1, 10_000),
                ],
                cash(210_000),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(above.lines[1].list_price_tyiyn, 0);
    // В чеке только фильтр (не масло) — цена клиенту 200 с.
    let no_oil = sell(
        &pool,
        &w.admin,
        in_service(
            &w,
            client,
            takeaway(
                &w,
                vec![
                    line("piece", w.filter, 1, 50_000),
                    kind_line(w.service, 1, 20_000),
                ],
                cash(70_000),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(no_oil.lines[1].list_price_tyiyn, 20_000);
    // Ниже этой цены — скидка: администратор без права не проведёт.
    let cheap = in_service(
        &w,
        client,
        takeaway(
            &w,
            vec![
                line("piece", w.filter, 1, 50_000),
                kind_line(w.service, 1, 0),
            ],
            cash(50_000),
        ),
    );
    match sell(&pool, &w.admin, cheap).await {
        Err(AppError::ForbiddenWith(m)) => assert_eq!(m, "Скидку ставит владелец"),
        other => panic!("ждали 403, получили {:?}", other.map(|s| s.number)),
    }

    // Отчёт: по видам — сколько раз, сколько заплатили клиенты и мастерам, масла в этих чеках.
    let mut conn = pool.acquire().await.unwrap();
    let r = avtodom_server::api::reports::service_kinds_data(&mut conn, &w.admin.user, None, None)
        .await
        .unwrap();
    let kind = |name: &str| r.kinds.iter().find(|k| k.name == name).unwrap().clone();
    let motor = kind("Замена масла");
    assert_eq!(
        (motor.count, motor.clients_tyiyn, motor.masters_tyiyn),
        (3, 30_000, 9_000)
    );
    let oil = motor.oils.iter().find(|o| o.product_id == w.oil).unwrap();
    assert_eq!((oil.canisters, oil.pour_ml, oil.total_ml), (2, 1000, 9000));
    let gear = kind("Замена масла в коробке");
    assert_eq!(
        (gear.count, gear.clients_tyiyn, gear.masters_tyiyn),
        (1, 0, 4_000)
    );
    assert_eq!(gear.oils[0].total_ml, 5000);
    drop(conn);
    // Бот: отчёт владельцу и лента — услуги и сколько мастеру.
    let today: chrono::NaiveDate =
        sqlx::query_scalar("select (now() at time zone 'Asia/Bishkek')::date")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let text =
        avtodom_server::api::bot_reports::services_text(&mut conn, &w.owner.user, today, today)
            .await
            .unwrap();
    assert!(
        text.contains("— Замена масла: 3 раз, клиенты заплатили 300 с, мастерам 90 с"),
        "{text}"
    );
    assert!(
        text.contains("Масло 5W-30 4л: 2 кан. по 4 л + 1 л розливом = 9 л"),
        "{text}"
    );
    let (_, details) = avtodom_server::api::telegram::sale_text(&mut conn, with_oil.id)
        .await
        .unwrap();
    assert!(
        details.contains("Замена масла, Замена масла в коробке, мастеру 70 с"),
        "{details}"
    );
    drop(conn);
    // Возврат вида замены уменьшает «сколько раз» и ставку мастера.
    let mut tx = pool.begin().await.unwrap();
    let back = return_sale_tx(
        &mut tx,
        &w.owner,
        with_oil.id,
        ReturnReq {
            op_id: Uuid::now_v7(),
            battery_cash: false,
            comment: String::new(),
            lines: vec![ReturnLineReq { line_no: 4, qty: 1 }],
            payments: Vec::new(),
            business_date: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(back.master_fee_tyiyn, -4_000);
    let mut conn = pool.acquire().await.unwrap();
    let r = avtodom_server::api::reports::service_kinds_data(&mut conn, &w.owner.user, None, None)
        .await
        .unwrap();
    let gear = r
        .kinds
        .iter()
        .find(|k| k.name == "Замена масла в коробке")
        .unwrap();
    assert_eq!((gear.count, gear.masters_tyiyn), (0, 0));
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn service_needs_client_data_offline_reports_reasons(pool: PgPool) {
    // «В сервис» без телефона, машины и пробега — 422 со списком (ADR-100); чек без сети со старого
    // устройства без вида замены — по прежней ставке за чек и с причинами владельцу (ADR-096).
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 5, 5_000).await;
    sqlx::query("insert into settings (branch_id, key, value) values ($1, 'sales', '{\"oil_change_master_fee_tyiyn\": 5000}')")
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, first_name, last_name) values ($1, $2, 'customer', 'person', 'Асанов Бакыт', 'Бакыт', 'Асанов')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    let with_kind = || {
        let mut req = takeaway(
            &w,
            vec![
                line("piece", w.filter, 1, 50_000),
                kind_line(w.service, 1, 20_000),
            ],
            cash(70_000),
        );
        req.sale_type = "service".into();
        req.master_id = Some(w.master);
        req
    };
    let mut req = with_kind();
    req.party_id = Some(party);
    match sell(&pool, &w.admin, req).await {
        Err(AppError::Validation(m)) => assert_eq!(
            m,
            "Чек «в сервис» — нужны данные клиента: телефон клиента, машина с госномером, пробег"
        ),
        other => panic!("ждали 422, получили {:?}", other.map(|s| s.number)),
    }
    // Без клиента вовсе — и клиент в списке.
    match sell(&pool, &w.admin, with_kind()).await {
        Err(AppError::Validation(m)) => {
            assert!(m.contains("клиент (имя и фамилия или фирма)"), "{m}");
        }
        other => panic!("ждали 422, получили {:?}", other.map(|s| s.number)),
    }
    // Без сети: старое устройство, «в сервис» без вида замены и без клиента — проводится.
    let mut req = takeaway(&w, vec![line("piece", w.filter, 1, 50_000)], cash(50_000));
    req.sale_type = "service".into();
    req.master_id = Some(w.master);
    req.offline = true;
    req.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(3));
    let sale = sell(&pool, &w.admin, req).await.unwrap();
    assert_eq!(sale.master_fee_tyiyn, 5_000);
    let text = serde_json::to_string(&audit_data(&pool, "sale.offline_bypass").await).unwrap();
    assert!(text.contains("чек «в сервис» без вида замены"), "{text}");
    assert!(
        text.contains("в сервис без данных клиента: клиент (имя и фамилия или фирма), телефон клиента, машина с госномером, пробег"),
        "{text}"
    );
}

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn discount_needs_right(pool: PgPool) {
    // Цена ниже каталожной — только владелец или право «Скидка в чеке» (ADR-097): без права онлайн
    // — 403, без сети — проведено с причиной; подарок, баллы и цена выше — не скидка.
    let w = seed(&pool).await;
    receive(&pool, &w.admin, w.filter, 20, 20_000).await;
    receive(&pool, &w.admin, w.oil, 8000, 240_000).await;
    let cheap = || takeaway(&w, vec![line("piece", w.filter, 1, 45_000)], cash(45_000));
    match sell(&pool, &w.admin, cheap()).await {
        Err(AppError::ForbiddenWith(m)) => assert_eq!(m, "Скидку ставит владелец"),
        other => panic!("ждали 403, получили {:?}", other.map(|s| s.number)),
    }
    sell(&pool, &w.owner, cheap()).await.unwrap();
    // Без сети без права — проводится, владельцу причина с товаром и ценами.
    let mut offline = cheap();
    offline.offline = true;
    offline.client_time = Some(chrono::Utc::now() - chrono::Duration::minutes(2));
    sell(&pool, &w.admin, offline).await.unwrap();
    let reasons = serde_json::to_string(&audit_data(&pool, "sale.offline_bypass").await).unwrap();
    assert!(
        reasons.contains("скидка без права: Фильтр W712, 450 с, цена по каталогу 500 с"),
        "{reasons}"
    );
    // Цена выше каталожной — не скидка.
    sell(
        &pool,
        &w.admin,
        takeaway(&w, vec![line("piece", w.filter, 1, 55_000)], cash(55_000)),
    )
    .await
    .unwrap();
    // Подарок по правилу — не скидка.
    let rule = Uuid::now_v7();
    sqlx::query("insert into gift_rules (id, branch_id, trigger_product_id, user_id, min_units) values ($1, $2, $3, $4, 3000)")
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
    let mut gift = line("piece", w.filter, 1, 0);
    gift.gift = true;
    sell(
        &pool,
        &w.admin,
        takeaway(
            &w,
            vec![line("container", w.oil, 1, 200_000), gift],
            cash(200_000),
        ),
    )
    .await
    .unwrap();
    // Баллы — оплата, а не цена строки: не скидка.
    let party = Uuid::now_v7();
    sqlx::query("insert into parties (id, branch_id, role, kind, name, phone) values ($1, $2, 'customer', 'person', 'Баллов Азат', '0555123456')")
        .bind(party)
        .bind(w.owner.user.branch_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into telegram_customers (chat_id, party_id, phone) values (78, $1, '0555123456')",
    )
    .bind(party)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, doc_type, doc_id) values ($1, $2, $3, 'accrual', 10000, 'adjust', $1)")
        .bind(Uuid::now_v7())
        .bind(w.owner.user.branch_id)
        .bind(party)
        .execute(&pool)
        .await
        .unwrap();
    let pay = |method: &str, amount: i64| PaymentReq {
        account_id: None,
        price_per_kg_tyiyn: None,
        method: method.into(),
        amount_tyiyn: amount,
        grams: None,
    };
    let mut req = takeaway(
        &w,
        vec![line("piece", w.filter, 1, 50_000)],
        vec![pay("bonus", 10_000), pay("cash", 40_000)],
    );
    req.party_id = Some(party);
    sell(&pool, &w.admin, req).await.unwrap();
    // С правом администратор ставит скидку сам.
    grant(&pool, &w, "discount").await;
    sell(&pool, &w.admin, cheap()).await.unwrap();
}
