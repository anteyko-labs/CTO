//! Касса: продажа и возврат (SPEC-04).

use std::collections::{BTreeSet, HashSet};

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::{self, CashEntry};
use crate::api::parties;
use crate::api::payroll;
use crate::api::receipts::lock_products;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::{div_round, mul};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sales", routing::get(list_sales).post(post_sale))
        .route("/sales/{id}", routing::get(get_sale))
        .route("/sales/{id}/return", routing::post(return_sale))
}

/// Ставка мастера за замену по умолчанию — 30 сом (ADR-027).
pub const DEFAULT_OIL_CHANGE_FEE: i64 = 3000;

/// Ставка замены из настроек филиала.
async fn oil_change_fee(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i64> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'sales'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| {
        v.get("oil_change_master_fee_tyiyn")
            .and_then(serde_json::Value::as_i64)
    })
    .filter(|v| *v >= 0)
    .unwrap_or(DEFAULT_OIL_CHANGE_FEE))
}

#[derive(Deserialize, Clone)]
pub struct SaleLineReq {
    pub kind: String,
    /// Подарок: цена ноль, товар из списка правила (SPEC-11).
    #[serde(default)]
    pub gift: bool,
    pub product_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
    pub qty: i64,
    pub unit_price_tyiyn: i64,
}

#[derive(Deserialize, Clone)]
pub struct PaymentReq {
    pub method: String,
    pub amount_tyiyn: i64,
}

#[derive(Deserialize)]
pub struct SaleReq {
    pub op_id: Uuid,
    pub client_time: Option<DateTime<Utc>>,
    pub sale_type: String,
    pub cashier_id: Uuid,
    pub master_id: Option<Uuid>,
    /// Покупатель и, для фирмы, кто приехал и на какой машине (SPEC-10).
    pub party_id: Option<Uuid>,
    pub contact_id: Option<Uuid>,
    pub vehicle_id: Option<Uuid>,
    #[serde(default)]
    pub comment: String,
    pub lines: Vec<SaleLineReq>,
    pub payments: Vec<PaymentReq>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SaleLineOut {
    pub line_no: i32,
    pub kind: String,
    pub gift: bool,
    pub product_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
    pub name: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    pub units: i64,
    pub unit_price_tyiyn: i64,
    pub list_price_tyiyn: i64,
    pub amount_tyiyn: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_tyiyn: Option<i64>,
    pub master_fee_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PaymentOut {
    pub method: String,
    pub amount_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SaleRef {
    pub id: Uuid,
    pub number: i64,
    pub total_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SaleOut {
    pub id: Uuid,
    pub number: i64,
    pub kind: String,
    pub sale_type: String,
    pub cashier_id: Uuid,
    pub cashier_name: String,
    pub master_id: Option<Uuid>,
    pub master_name: Option<String>,
    pub party_id: Option<Uuid>,
    pub party_name: Option<String>,
    pub contact_name: Option<String>,
    pub vehicle_plate: Option<String>,
    /// Баланс клиента после чека: сколько он теперь должен.
    pub party_balance_tyiyn: Option<i64>,
    pub total_tyiyn: i64,
    /// Начисление мастеру за замену: ставка на чек, а не строка услуги (ADR-027).
    pub master_fee_tyiyn: i64,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<SaleLineOut>,
    pub payments: Vec<PaymentOut>,
    pub returns: Vec<SaleRef>,
}

pub async fn load_sale(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
) -> AppResult<SaleOut> {
    let h = sqlx::query!(
        r#"select s.id, s.number, s.kind, s.sale_type, s.cashier_id, c.full_name as cashier_name,
                  s.master_id, m.full_name as "master_name?", s.total_tyiyn, s.master_fee_tyiyn,
                  s.comment, s.reversal_of,
                  s.party_id as "party_id?", pt.name as "party_name?", pt.balance_tyiyn as "party_balance_tyiyn?",
                  pc.full_name as "contact_name?", pv.plate as "vehicle_plate?",
                  u.full_name as user_name, s.created_at
           from sales s
           join employees c on c.id = s.cashier_id
           left join employees m on m.id = s.master_id
           left join parties pt on pt.id = s.party_id
           left join party_contacts pc on pc.id = s.contact_id
           left join party_vehicles pv on pv.id = s.vehicle_id
           join users u on u.id = s.user_id
           where s.id = $1 and s.branch_id = $2"#,
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let owner = user.is_owner();
    let lines = sqlx::query!(
        r#"select l.line_no, l.kind, l.product_id, l.service_id,
                  coalesce(p.name, sv.name) as "name!", p.container_ml as "container_ml?",
                  l.qty, l.units, l.unit_price_tyiyn, l.list_price_tyiyn, l.amount_tyiyn, l.cost_tyiyn,
                  l.master_fee_tyiyn, l.gift
           from sale_lines l
           left join products p on p.id = l.product_id
           left join services sv on sv.id = l.service_id
           where l.sale_id = $1 order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| SaleLineOut {
        line_no: r.line_no,
        kind: r.kind,
        gift: r.gift,
        product_id: r.product_id,
        service_id: r.service_id,
        name: r.name,
        container_ml: r.container_ml,
        qty: r.qty,
        units: r.units,
        unit_price_tyiyn: r.unit_price_tyiyn,
        list_price_tyiyn: r.list_price_tyiyn,
        amount_tyiyn: r.amount_tyiyn,
        cost_tyiyn: owner.then_some(r.cost_tyiyn),
        master_fee_tyiyn: r.master_fee_tyiyn,
    })
    .collect();
    let payments = sqlx::query_as!(
        PaymentOut,
        "select method, amount_tyiyn from sale_payments where sale_id = $1 order by method",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let returns = sqlx::query_as!(
        SaleRef,
        "select id, number, total_tyiyn from sales where reversal_of = $1 order by created_at",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(SaleOut {
        id: h.id,
        number: h.number,
        kind: h.kind,
        sale_type: h.sale_type,
        cashier_id: h.cashier_id,
        cashier_name: h.cashier_name,
        master_id: h.master_id,
        master_name: h.master_name,
        party_id: h.party_id,
        party_name: h.party_name,
        contact_name: h.contact_name,
        vehicle_plate: h.vehicle_plate,
        party_balance_tyiyn: h.party_balance_tyiyn,
        total_tyiyn: h.total_tyiyn,
        master_fee_tyiyn: h.master_fee_tyiyn,
        comment: h.comment,
        reversal_of: h.reversal_of,
        user_name: h.user_name,
        created_at: h.created_at,
        lines,
        payments,
        returns,
    })
}

/// Строка, подготовленная к проведению.
struct Prepared {
    gift: bool,
    kind: String,
    product_id: Option<Uuid>,
    service_id: Option<Uuid>,
    qty: i64,
    units: i64,
    unit_price: i64,
    list_price: i64,
    amount: i64,
    master_fee: i64,
}

async fn prepare_line(
    conn: &mut PgConnection,
    branch_id: Uuid,
    l: &SaleLineReq,
) -> AppResult<Prepared> {
    if l.qty <= 0 || l.unit_price_tyiyn < 0 {
        return Err(invalid("количество больше нуля, цена не отрицательна"));
    }
    if l.kind == "service" {
        let sid = l.service_id.ok_or_else(|| invalid("не указана услуга"))?;
        let s = sqlx::query!(
            "select price_tyiyn from services where id = $1 and branch_id = $2 and active",
            sid,
            branch_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("услуга не найдена"))?;
        return Ok(Prepared {
            gift: false,
            kind: l.kind.clone(),
            product_id: None,
            service_id: Some(sid),
            qty: l.qty,
            units: 0,
            unit_price: l.unit_price_tyiyn,
            list_price: s.price_tyiyn,
            amount: mul(l.qty, l.unit_price_tyiyn).ok_or_else(overflow)?,
            // Мастеру начисляет отметка замены в чеке, а не строка работы (ADR-027):
            // иначе за один и тот же чек начислилось бы дважды.
            master_fee: 0,
        });
    }
    let pid = l.product_id.ok_or_else(|| invalid("не указан товар"))?;
    let p = sqlx::query!(
        r#"select p.unit, p.container_ml, coalesce(bp.sale_price_tyiyn, 0) as "sale_price!",
                  bp.pour_price_per_l_tyiyn as "pour_price?"
           from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
           where p.id = $1"#,
        pid,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    let (units, amount, list_price) = match (l.kind.as_str(), p.unit.as_str(), p.container_ml) {
        ("piece", "piece", _) => (l.qty, mul(l.qty, l.unit_price_tyiyn), p.sale_price),
        ("container", "ml", Some(c)) => (
            mul(l.qty, c).ok_or_else(overflow)?,
            mul(l.qty, l.unit_price_tyiyn),
            p.sale_price,
        ),
        ("pour", "ml", Some(_)) => (
            l.qty,
            div_round(i128::from(l.qty) * i128::from(l.unit_price_tyiyn), 1000),
            p.pour_price.unwrap_or(0),
        ),
        ("piece" | "container" | "pour", _, _) => {
            return Err(invalid("вид строки не подходит товару"));
        }
        _ => return Err(invalid("неизвестный вид строки")),
    };
    Ok(Prepared {
        gift: l.gift,
        kind: l.kind.clone(),
        product_id: Some(pid),
        service_id: None,
        qty: l.qty,
        units,
        unit_price: l.unit_price_tyiyn,
        list_price,
        amount: amount.ok_or_else(overflow)?,
        master_fee: 0,
    })
}

fn check_payments(payments: &[PaymentReq], total: i64) -> AppResult<()> {
    let mut sum: i64 = 0;
    for p in payments {
        if !matches!(p.method.as_str(), "cash" | "card" | "transfer" | "debt") {
            return Err(invalid("способ оплаты: cash, card, transfer или debt"));
        }
        if p.amount_tyiyn <= 0 {
            return Err(invalid("сумма платежа больше нуля"));
        }
        sum = sum.checked_add(p.amount_tyiyn).ok_or_else(overflow)?;
    }
    if sum != total {
        return Err(invalid(format!("сумма оплат {sum} не равна итогу {total}")));
    }
    Ok(())
}

async fn check_employee(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    master: bool,
) -> AppResult<()> {
    let r = sqlx::query!(
        "select is_cashier, is_master from employees where id = $1 and branch_id = $2 and active",
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match r {
        Some(r) if master && r.is_master => Ok(()),
        Some(r) if !master && r.is_cashier => Ok(()),
        _ if master => Err(invalid("мастер не найден")),
        _ => Err(invalid("кассир не найден")),
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_line(
    conn: &mut PgConnection,
    sale_id: Uuid,
    line_no: i32,
    p: &Prepared,
    amount: i64,
    cost: i64,
    fee: i64,
    units: i64,
    qty: i64,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into sale_lines (id, sale_id, line_no, kind, product_id, service_id, qty, units,
                                   unit_price_tyiyn, list_price_tyiyn, amount_tyiyn, cost_tyiyn, master_fee_tyiyn,
                                   gift)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
        new_id(),
        sale_id,
        line_no,
        p.kind,
        p.product_id,
        p.service_id,
        qty,
        units,
        p.unit_price,
        p.list_price,
        amount,
        cost,
        fee,
        p.gift
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Платежи чека: деньги попадают в кассу и на счёт, комиссия банка фиксируется в платеже
/// и уходит со счёта (ADR-022, ADR-024). Долг кассу не трогает.
async fn insert_payments(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    payments: &[PaymentReq],
    sign: i64,
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let register = cash::default_account(conn, branch_id).await?;
    let bank = cash::bank_account(conn, branch_id).await?;
    let movement_kind = if sign > 0 { "sale" } else { "sale_return" };
    for p in payments {
        let amount = sign * p.amount_tyiyn;
        let fee = match p.method.as_str() {
            "card" | "transfer" => {
                let rate = sqlx::query_scalar!(
                    "select rate_bp from payment_method_fees where branch_id = $1 and method = $2",
                    branch_id,
                    p.method
                )
                .fetch_optional(&mut *conn)
                .await?
                .unwrap_or(0);
                div_round(i128::from(amount) * i128::from(rate), 10_000).unwrap_or(0)
            }
            _ => 0,
        };
        match p.method.as_str() {
            "cash" => {
                cash::add_movement(
                    conn,
                    ctx,
                    CashEntry {
                        account_id: register,
                        kind: movement_kind,
                        amount,
                        doc_type: "sale",
                        doc_id: Some(sale_id),
                        comment: "",
                    },
                )
                .await?;
            }
            "card" | "transfer" => {
                if let Some(bank_id) = bank {
                    cash::add_movement(
                        conn,
                        ctx,
                        CashEntry {
                            account_id: bank_id,
                            kind: movement_kind,
                            amount,
                            doc_type: "sale",
                            doc_id: Some(sale_id),
                            comment: "",
                        },
                    )
                    .await?;
                    if fee != 0 {
                        cash::add_movement(
                            conn,
                            ctx,
                            CashEntry {
                                account_id: bank_id,
                                kind: "bank_fee",
                                amount: -fee,
                                doc_type: "sale",
                                doc_id: Some(sale_id),
                                comment: "",
                            },
                        )
                        .await?;
                    }
                }
            }
            _ => {}
        }
        sqlx::query!(
            "insert into sale_payments (id, sale_id, method, amount_tyiyn, fee_tyiyn) values ($1, $2, $3, $4, $5)",
            new_id(),
            sale_id,
            p.method,
            amount,
            fee
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn post_sale_tx(conn: &mut PgConnection, ctx: &Ctx, req: SaleReq) -> AppResult<SaleOut> {
    const KIND: &str = "sale.post";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    if req.lines.is_empty() {
        return Err(invalid("чек пуст"));
    }
    let has_service = req.lines.iter().any(|l| l.kind == "service");
    match req.sale_type.as_str() {
        "takeaway" if has_service => return Err(invalid("услуги только в продаже «в сервис»")),
        "takeaway" if req.master_id.is_some() => {
            return Err(invalid("мастер указывается только в сервисе"));
        }
        "takeaway" => {}
        "service" => {
            let master = req
                .master_id
                .ok_or_else(|| invalid("в сервисе нужен мастер"))?;
            check_employee(conn, branch_id, master, true).await?;
        }
        _ => return Err(invalid("тип продажи: takeaway или service")),
    }
    check_employee(conn, branch_id, req.cashier_id, false).await?;

    let debt_total: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "debt")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    match req.party_id {
        Some(pid) => {
            parties::check_sale_party(conn, branch_id, pid, req.contact_id, req.vehicle_id).await?
        }
        None if debt_total > 0 => return Err(invalid("для продажи в долг укажите клиента")),
        None if req.contact_id.is_some() || req.vehicle_id.is_some() => {
            return Err(invalid("работник и машина указываются вместе с клиентом"));
        }
        None => {}
    }

    let mut prepared = Vec::with_capacity(req.lines.len());
    let mut total: i64 = 0;
    for l in &req.lines {
        let p = prepare_line(conn, branch_id, l).await?;
        total = total.checked_add(p.amount).ok_or_else(overflow)?;
        prepared.push(p);
    }
    let gifts: Vec<Uuid> = prepared
        .iter()
        .filter(|p| p.gift)
        .filter_map(|p| p.product_id)
        .collect();
    if !gifts.is_empty() {
        if prepared.iter().any(|p| p.gift && p.amount != 0) {
            return Err(invalid("подарок идёт с нулевой ценой"));
        }
        let in_cart: Vec<Uuid> = prepared
            .iter()
            .filter(|p| !p.gift)
            .filter_map(|p| p.product_id)
            .collect();
        let allowed = sqlx::query_scalar!(
            r#"select count(distinct i.gift_product_id) as "n!"
               from gift_rules r join gift_rule_items i on i.rule_id = r.id
               where r.branch_id = $1 and r.active
                 and r.trigger_product_id = any($2) and i.gift_product_id = any($3)"#,
            branch_id,
            &in_cart,
            &gifts
        )
        .fetch_one(&mut *conn)
        .await?;
        let distinct: BTreeSet<Uuid> = gifts.iter().copied().collect();
        if usize::try_from(allowed).ok() != Some(distinct.len()) {
            return Err(invalid("этот товар нельзя подарить к покупке"));
        }
    }
    check_payments(&req.payments, total)?;
    // Замена строкой в чеке не печатается: мастеру идёт одна ставка за чек (ADR-027).
    let master_fee = if req.sale_type == "service" {
        oil_change_fee(conn, branch_id).await?
    } else {
        0
    };

    lock_products(
        conn,
        branch_id,
        prepared.iter().filter_map(|p| p.product_id),
    )
    .await?;
    let number = ops::next_counter(conn, branch_id, "sale").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into sales (id, branch_id, number, kind, sale_type, cashier_id, master_id, total_tyiyn,
                              master_fee_tyiyn, comment, party_id, contact_id, vehicle_id,
                              user_id, device_id, client_time, business_date)
           values ($1, $2, $3, 'sale', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   (now() at time zone 'Asia/Bishkek')::date)"#,
        id,
        branch_id,
        number,
        req.sale_type,
        req.cashier_id,
        req.master_id,
        total,
        master_fee,
        req.comment.trim(),
        req.party_id,
        req.contact_id,
        req.vehicle_id,
        ctx.user.id,
        ctx.device_id,
        req.client_time
    )
    .execute(&mut *conn)
    .await?;

    let mut overrides = Vec::new();
    // Валовая прибыль чека: с неё считается процент кассира (ADR-036).
    let mut gross: i64 = 0;
    for (i, p) in prepared.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        let mut cost = 0;
        if let Some(pid) = p.product_id {
            let pool = ops::lock_pool(conn, branch_id, pid).await?;
            cost = cost_of(p.units, &pool).ok_or_else(overflow)?;
            ops::apply_movement(
                conn,
                Movement {
                    branch_id,
                    product_id: pid,
                    qty_delta: -p.units,
                    value_delta: -cost,
                    doc_type: "sale",
                    doc_id: id,
                },
            )
            .await?;
        }
        insert_line(
            conn,
            id,
            line_no,
            p,
            p.amount,
            cost,
            p.master_fee,
            p.units,
            p.qty,
        )
        .await?;
        gross = gross.checked_add(p.amount - cost).ok_or_else(overflow)?;
        if p.unit_price != p.list_price {
            overrides
                .push(json!({ "line_no": line_no, "list": p.list_price, "price": p.unit_price }));
        }
    }
    insert_payments(conn, ctx, id, &req.payments, 1).await?;
    if debt_total > 0 {
        let party_id = req
            .party_id
            .ok_or_else(|| invalid("для долга нужен клиент"))?;
        let left = parties::credit_limit_left(conn, party_id).await?;
        parties::add_ledger(
            conn,
            ctx,
            parties::LedgerEntry {
                party_id,
                kind: "debt",
                amount: debt_total,
                doc_type: "sale",
                doc_id: Some(id),
                comment: "",
            },
        )
        .await?;
        if left.is_some_and(|l| debt_total > l) {
            // Лимит не запрещает продажу, но владелец должен это увидеть (SPEC-10).
            ops::audit(
                conn,
                ctx,
                "sale.credit_limit_exceeded",
                "sale",
                Some(id),
                json!({ "party_id": party_id, "debt": debt_total, "left": left }),
            )
            .await?;
        }
    }
    if !overrides.is_empty() {
        ops::audit(
            conn,
            ctx,
            "sale.price_override",
            "sale",
            Some(id),
            json!({ "lines": overrides }),
        )
        .await?;
    }
    // Начисления сотрудникам в той же транзакции, что и чек (инвариант 7).
    let business_date = sqlx::query_scalar!(
        r#"select business_date as "d!" from sales where id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if let Some(master_id) = req.master_id
        && master_fee != 0
    {
        payroll::add_accrual(
            conn,
            ctx,
            Some(business_date),
            payroll::Accrual {
                employee_id: master_id,
                kind: "service_fee",
                amount: master_fee,
                base: None,
                rule_id: None,
                doc_type: "sale",
                doc_id: Some(id),
                comment: "",
            },
        )
        .await?;
    }
    payroll::accrue_for_sale(
        conn,
        ctx,
        payroll::SaleAccrual {
            sale_id: id,
            business_date,
            cashier_id: req.cashier_id,
            party_id: req.party_id,
            gross,
            total,
            debt: debt_total,
        },
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "sale",
        Some(id),
        json!({ "number": number, "total": total }),
    )
    .await?;
    let out = load_sale(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_sale(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SaleReq>,
) -> AppResult<Json<SaleOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_sale_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReturnLineReq {
    pub line_no: i32,
    pub qty: i64,
}

#[derive(Deserialize)]
pub struct ReturnReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    pub lines: Vec<ReturnLineReq>,
    pub payments: Vec<PaymentReq>,
}

/// Часть величины `part` для `qty` из `orig_qty`; последняя часть забирает точный остаток.
fn share(orig: i64, already: i64, qty: i64, orig_qty: i64, returned_qty: i64) -> AppResult<i64> {
    if returned_qty + qty == orig_qty {
        return Ok(orig - already);
    }
    div_round(i128::from(orig) * i128::from(qty), i128::from(orig_qty)).ok_or_else(overflow)
}

pub async fn return_sale_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReturnReq,
) -> AppResult<SaleOut> {
    const KIND: &str = "sale.return";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    if req.lines.is_empty() {
        return Err(invalid("не выбраны строки возврата"));
    }
    let mut seen = HashSet::new();
    if !req.lines.iter().all(|l| seen.insert(l.line_no)) {
        return Err(invalid("строки возврата повторяются"));
    }
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 2))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let orig = sqlx::query!(
        r#"select kind, sale_type, cashier_id, master_id, master_fee_tyiyn,
                  party_id as "party_id?", contact_id as "contact_id?", vehicle_id as "vehicle_id?"
           from sales where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if orig.kind != "sale" {
        return Err(AppError::Conflict("возврат по возврату невозможен".into()));
    }

    struct Planned {
        p: Prepared,
        line_no: i32,
        amount: i64,
        cost: i64,
        fee: i64,
    }
    let mut planned = Vec::new();
    let mut total: i64 = 0;
    for rl in &req.lines {
        let o = sqlx::query!(
            r#"select kind, product_id, service_id, qty, units, unit_price_tyiyn, list_price_tyiyn,
                      amount_tyiyn, cost_tyiyn, master_fee_tyiyn, gift
               from sale_lines where sale_id = $1 and line_no = $2"#,
            id,
            rl.line_no
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid(format!("строки {} нет в чеке", rl.line_no)))?;
        let done = sqlx::query!(
            r#"select coalesce(sum(l.qty), 0)::bigint as "qty!", coalesce(sum(l.units), 0)::bigint as "units!",
                      coalesce(sum(-l.amount_tyiyn), 0)::bigint as "amount!", coalesce(sum(-l.cost_tyiyn), 0)::bigint as "cost!",
                      coalesce(sum(-l.master_fee_tyiyn), 0)::bigint as "fee!"
               from sale_lines l join sales s on s.id = l.sale_id
               where s.reversal_of = $1 and l.line_no = $2"#,
            id,
            rl.line_no
        )
        .fetch_one(&mut *conn)
        .await?;
        if rl.qty <= 0 || rl.qty > o.qty - done.qty {
            return Err(invalid(format!(
                "по строке {} можно вернуть не больше {}",
                rl.line_no,
                o.qty - done.qty
            )));
        }
        let units = share(o.units, done.units, rl.qty, o.qty, done.qty)?;
        let amount = share(o.amount_tyiyn, done.amount, rl.qty, o.qty, done.qty)?;
        let cost = share(o.cost_tyiyn, done.cost, rl.qty, o.qty, done.qty)?;
        let fee = share(o.master_fee_tyiyn, done.fee, rl.qty, o.qty, done.qty)?;
        total = total.checked_add(amount).ok_or_else(overflow)?;
        planned.push(Planned {
            p: Prepared {
                gift: o.gift,
                kind: o.kind,
                product_id: o.product_id,
                service_id: o.service_id,
                qty: rl.qty,
                units,
                unit_price: o.unit_price_tyiyn,
                list_price: o.list_price_tyiyn,
                amount,
                master_fee: fee,
            },
            line_no: rl.line_no,
            amount,
            cost,
            fee,
        });
    }
    check_payments(&req.payments, total)?;
    // Работу мастер уже сделал: ставка снимается только если вернули чек целиком (ADR-027).
    let sold_qty = sqlx::query_scalar!(
        r#"select coalesce(sum(qty), 0)::bigint as "v!" from sale_lines where sale_id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let returned_qty = sqlx::query_scalar!(
        r#"select coalesce(sum(l.qty), 0)::bigint as "v!"
           from sale_lines l join sales s on s.id = l.sale_id where s.reversal_of = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let now_qty: i64 = req.lines.iter().map(|l| l.qty).sum();
    let full_return = returned_qty + now_qty >= sold_qty;
    let fee_back = if full_return {
        -orig.master_fee_tyiyn
    } else {
        0
    };

    let products: BTreeSet<Uuid> = planned.iter().filter_map(|x| x.p.product_id).collect();
    lock_products(conn, branch_id, products).await?;
    let number = ops::next_counter(conn, branch_id, "sale").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into sales (id, branch_id, number, kind, sale_type, cashier_id, master_id, total_tyiyn,
                              master_fee_tyiyn, comment, party_id, contact_id, vehicle_id,
                              reversal_of, user_id, device_id, business_date)
           values ($1, $2, $3, 'return', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   (now() at time zone 'Asia/Bishkek')::date)"#,
        rid,
        branch_id,
        number,
        orig.sale_type,
        orig.cashier_id,
        orig.master_id,
        -total,
        fee_back,
        req.comment.trim(),
        orig.party_id,
        orig.contact_id,
        orig.vehicle_id,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for x in &planned {
        if let Some(pid) = x.p.product_id {
            ops::lock_pool(conn, branch_id, pid).await?;
            ops::apply_movement(
                conn,
                Movement {
                    branch_id,
                    product_id: pid,
                    qty_delta: x.p.units,
                    value_delta: x.cost,
                    doc_type: "sale_return",
                    doc_id: rid,
                },
            )
            .await?;
        }
        insert_line(
            conn, rid, x.line_no, &x.p, -x.amount, -x.cost, -x.fee, x.p.units, x.p.qty,
        )
        .await?;
    }
    insert_payments(conn, ctx, rid, &req.payments, -1).await?;
    let debt_back: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "debt")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    if debt_back > 0 {
        let party_id = orig
            .party_id
            .ok_or_else(|| invalid("в исходном чеке не было клиента"))?;
        parties::add_ledger(
            conn,
            ctx,
            parties::LedgerEntry {
                party_id,
                kind: "debt",
                amount: -debt_back,
                doc_type: "sale_return",
                doc_id: Some(rid),
                comment: "",
            },
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "sale",
        Some(rid),
        json!({ "original": id, "number": number, "total": -total }),
    )
    .await?;
    let out = load_sale(conn, &ctx.user, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn return_sale(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReturnReq>,
) -> AppResult<Json<SaleOut>> {
    let mut tx = state.pool.begin().await?;
    let out = return_sale_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn get_sale(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<SaleOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_sale(&mut conn, &user, id).await?))
}

#[derive(Deserialize)]
struct DayQuery {
    date: Option<NaiveDate>,
}

#[derive(Serialize)]
struct SaleListItem {
    id: Uuid,
    number: i64,
    kind: String,
    sale_type: String,
    cashier_name: String,
    master_name: Option<String>,
    party_name: Option<String>,
    /// Сколько из этого чека ушло в долг.
    debt_tyiyn: i64,
    total_tyiyn: i64,
    reversal_of: Option<Uuid>,
    created_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
struct DayTotals {
    count: i64,
    total_tyiyn: i64,
    cash_tyiyn: i64,
    card_tyiyn: i64,
    transfer_tyiyn: i64,
    debt_tyiyn: i64,
}

#[derive(Serialize)]
struct DayOut {
    date: NaiveDate,
    sales: Vec<SaleListItem>,
    totals: DayTotals,
}

async fn list_sales(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<DayQuery>,
) -> AppResult<Json<DayOut>> {
    let date = match q.date {
        Some(d) => d,
        None => {
            sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                .fetch_one(&state.pool)
                .await?
        }
    };
    let sales = sqlx::query_as!(
        SaleListItem,
        r#"select s.id, s.number, s.kind, s.sale_type, c.full_name as cashier_name, m.full_name as "master_name?",
                  pt.name as "party_name?",
                  coalesce((select sum(p.amount_tyiyn) from sale_payments p
                            where p.sale_id = s.id and p.method = 'debt'), 0)::bigint as "debt_tyiyn!",
                  s.total_tyiyn, s.reversal_of, s.created_at
           from sales s
           join employees c on c.id = s.cashier_id
           left join employees m on m.id = s.master_id
           left join parties pt on pt.id = s.party_id
           where s.branch_id = $1
             and s.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek'
             and s.created_at < ($2::date + 1)::timestamp at time zone 'Asia/Bishkek'
           order by s.created_at desc"#,
        user.branch_id,
        date
    )
    .fetch_all(&state.pool)
    .await?;
    let t = sqlx::query!(
        r#"select
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'cash'), 0)::bigint as "cash!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as "card!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as "transfer!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!"
           from sale_payments p join sales s on s.id = p.sale_id
           where s.branch_id = $1
             and s.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek'
             and s.created_at < ($2::date + 1)::timestamp at time zone 'Asia/Bishkek'"#,
        user.branch_id,
        date
    )
    .fetch_one(&state.pool)
    .await?;
    let totals = DayTotals {
        count: i64::try_from(sales.iter().filter(|s| s.kind == "sale").count()).unwrap_or(i64::MAX),
        total_tyiyn: sales.iter().map(|s| s.total_tyiyn).sum(),
        cash_tyiyn: t.cash,
        card_tyiyn: t.card,
        transfer_tyiyn: t.transfer,
        debt_tyiyn: t.debt,
    };
    Ok(Json(DayOut {
        date,
        sales,
        totals,
    }))
}
