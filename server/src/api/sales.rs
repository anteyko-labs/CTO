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
    /// Прайсовая цена, которую видела касса (из снимка без сети, SPEC-09): по ней видно,
    /// менял ли кассир цену, а не по прайсу на момент отправки.
    #[serde(default)]
    pub seen_list_price_tyiyn: Option<i64>,
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
    /// Чек пришёл из очереди устройства, проведённый без сети (SPEC-09): продажа уже
    /// состоялась, поэтому нарушение цены не отклоняет его, а уходит владельцу.
    #[serde(default)]
    pub offline: bool,
    /// Пробег машины при замене — в масляную книжку (SPEC-16).
    #[serde(default)]
    pub mileage_km: Option<i32>,
    /// Адрес доставки; пусто — забрали сами.
    #[serde(default)]
    pub delivery_address: String,
    /// Отказанные работы для масляной книжки: «воздушный фильтр» (ADR-055).
    #[serde(default)]
    pub declined: String,
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
    /// Куда везти; доставка бесплатная, поэтому только адрес (BLUEPRINT §6, вопросы 16 и 21).
    #[serde(default)]
    pub delivery_address: String,
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
                  s.comment, s.delivery_address, s.reversal_of,
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
        delivery_address: h.delivery_address,
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
    seen_list: Option<i64>,
}

async fn prepare_line(
    conn: &mut PgConnection,
    branch_id: Uuid,
    l: &SaleLineReq,
    offline: bool,
    bypass: &mut Vec<String>,
) -> AppResult<Prepared> {
    if l.qty <= 0 || l.unit_price_tyiyn < 0 {
        return Err(invalid("количество больше нуля, цена не отрицательна"));
    }
    if l.qty > 1_000_000_000 {
        return Err(invalid("слишком большое количество — проверьте ввод"));
    }
    ops::check_amount(l.unit_price_tyiyn)?;
    if l.kind == "service" {
        let sid = l.service_id.ok_or_else(|| invalid("не указана услуга"))?;
        // Чек без сети принимаем и с услугой, которую успели отключить: работа уже сделана.
        let s = sqlx::query!(
            "select name, active, price_tyiyn, master_fee_tyiyn from services where id = $1 and branch_id = $2 and (active or $3)",
            sid,
            branch_id,
            offline
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("услуга не найдена"))?;
        if !s.active {
            bypass.push(format!("услуга «{}» отключена", s.name));
        }
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
            // Ставка мастера за работу фиксируется в строке, как себестоимость (ADR-043).
            master_fee: mul(l.qty, s.master_fee_tyiyn).ok_or_else(overflow)?,
            seen_list: seen_list(l, offline),
        });
    }
    // Карточку могли влить в другую, пока чек лежал на устройстве: продаём основную.
    let pid = ops::live_product(
        conn,
        l.product_id.ok_or_else(|| invalid("не указан товар"))?,
    )
    .await?;
    let p = sqlx::query!(
        r#"select p.name, p.unit, p.container_ml, p.archived, coalesce(bp.sale_price_tyiyn, 0) as "sale_price!",
                  bp.pour_price_per_l_tyiyn as "pour_price?"
           from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
           where p.id = $1"#,
        pid,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    // Карточка в архиве (дубль после объединения) не продаётся; чек без сети уже состоялся.
    if p.archived {
        if !offline {
            return Err(invalid("товар в архиве: найдите основную карточку"));
        }
        bypass.push(format!("товар «{}» в архиве", p.name));
    }
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
        // На вес: граммы × цена за кг (ADR-048).
        ("weight", "g", _) => (
            l.qty,
            div_round(i128::from(l.qty) * i128::from(l.unit_price_tyiyn), 1000),
            p.sale_price,
        ),
        ("piece" | "container" | "pour" | "weight", _, _) => {
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
        seen_list: seen_list(l, offline),
    })
}

/// Прайс, который видела касса, учитывается только у чека без сети: прайс мог смениться, пока
/// чек лежал на устройстве. Онлайн-чек сверяется с прайсом сервера — иначе касса сама решала бы,
/// была ли правка цены (ADR-062).
fn seen_list(l: &SaleLineReq, offline: bool) -> Option<i64> {
    if offline {
        l.seen_list_price_tyiyn
    } else {
        None
    }
}

fn check_payments(payments: &[PaymentReq], total: i64) -> AppResult<()> {
    let mut sum: i64 = 0;
    for p in payments {
        if !matches!(
            p.method.as_str(),
            "cash" | "card" | "transfer" | "debt" | "bonus"
        ) {
            return Err(invalid(
                "способ оплаты: cash, card, transfer, debt или bonus",
            ));
        }
        if p.amount_tyiyn <= 0 || p.amount_tyiyn > ops::MAX_AMOUNT_TYIYN {
            return Err(invalid("сумма платежа больше нуля"));
        }
        sum = sum.checked_add(p.amount_tyiyn).ok_or_else(overflow)?;
    }
    if sum != total {
        return Err(invalid(format!("сумма оплат {sum} не равна итогу {total}")));
    }
    Ok(())
}

/// Сотрудник чека. Чек без сети принимаем и с сотрудником, которого отключили, пока чек лежал
/// на устройстве, — причина уходит владельцу (ADR-062).
async fn check_employee(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    master: bool,
    offline: bool,
    bypass: &mut Vec<String>,
) -> AppResult<()> {
    let r = sqlx::query!(
        "select full_name, active, is_cashier, is_master from employees where id = $1 and branch_id = $2 and (active or $3)",
        id,
        branch_id,
        offline
    )
    .fetch_optional(&mut *conn)
    .await?;
    let r = match r {
        Some(r) if (master && r.is_master) || (!master && r.is_cashier) => r,
        _ if master => return Err(invalid("мастер не найден")),
        _ => return Err(invalid("кассир не найден")),
    };
    if !r.active {
        bypass.push(format!(
            "{} «{}» отключён",
            if master {
                "мастер"
            } else {
                "кассир"
            },
            r.full_name
        ));
    }
    Ok(())
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
    cost_unknown: bool,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into sale_lines (id, sale_id, line_no, kind, product_id, service_id, qty, units,
                                   unit_price_tyiyn, list_price_tyiyn, amount_tyiyn, cost_tyiyn, master_fee_tyiyn,
                                   gift, cost_unknown)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
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
        p.gift,
        cost_unknown
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Комиссия банка исходного чека, которую возвращают возвраты (ADR-070): пропорционально
/// возвращённой на карту или переводом сумме от записанной в чеке комиссии, а не по ставке.
/// Считается нарастающим итогом, поэтому несколько частичных возвратов всего чека вернут ровно
/// исходную комиссию; больше неё — никогда (чек наличными, возвращённый на карту, — ноль).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeBack {
    /// Комиссия исходного чека по карте и переводу.
    pub fee: i64,
    /// Сколько исходный чек получил картой и переводом.
    pub base: i64,
    /// Сколько уже вернули на карту и переводом прежними возвратами.
    pub refunded: i64,
    /// Сколько комиссии уже вернули прежними возвратами.
    pub fee_back: i64,
}

impl FeeBack {
    /// Комиссия, которую возвращает возврат `amount` на карту или переводом.
    pub fn take(&mut self, amount: i64) -> i64 {
        if self.base <= 0 || self.fee <= 0 || amount <= 0 {
            self.refunded = self.refunded.saturating_add(amount.max(0));
            return 0;
        }
        let done = self.refunded.saturating_add(amount).min(self.base);
        let due = div_round(
            i128::from(self.fee) * i128::from(done),
            i128::from(self.base),
        )
        .unwrap_or(0)
        .min(self.fee);
        let now = due.saturating_sub(self.fee_back).max(0);
        self.refunded = self.refunded.saturating_add(amount);
        self.fee_back = self.fee_back.saturating_add(now);
        now
    }
}

/// Платежи чека: деньги попадают в кассу и на счёт, комиссия банка фиксируется в платеже
/// и уходит со счёта (ADR-022, ADR-024). Долг кассу не трогает.
async fn insert_payments(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    payments: &[PaymentReq],
    sign: i64,
    // Возврат: комиссия исходного чека по карте и переводу (`None` — продажа).
    mut back: Option<FeeBack>,
    // Учётный день чека: ставка банка берётся действовавшая в него.
    on: NaiveDate,
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let register = cash::default_account(conn, branch_id).await?;
    let bank = cash::bank_account(conn, branch_id).await?;
    let movement_kind = if sign > 0 { "sale" } else { "sale_return" };
    for p in payments {
        let amount = sign * p.amount_tyiyn;
        let fee = match (p.method.as_str(), back.as_mut()) {
            ("card" | "transfer", Some(b)) => -b.take(p.amount_tyiyn),
            ("card" | "transfer", None) => {
                let rate = crate::api::fees::rate_on(conn, branch_id, &p.method, on).await?;
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
    if req.declined.chars().count() > 300 {
        return Err(invalid("отказанные работы — не длиннее 300 знаков"));
    }
    if req.delivery_address.chars().count() > 300 {
        return Err(invalid("адрес доставки не длиннее 300 знаков"));
    }
    // Чек без сети признаём, только если он и правда пролежал на устройстве: признак ставит
    // клиент, а время чека — раньше отправки хотя бы на полминуты (ADR-042).
    let offline = req.offline
        && req
            .client_time
            .is_some_and(|t| t < Utc::now() - chrono::Duration::seconds(30));
    // Что чек без сети обошёл из проверок онлайн-продажи — владельцу одним уведомлением (ADR-062).
    let mut bypass: Vec<String> = Vec::new();
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
            check_employee(conn, branch_id, master, true, offline, &mut bypass).await?;
        }
        _ => return Err(invalid("тип продажи: takeaway или service")),
    }
    // Кассир чека — кассир открытой смены (ADR-071): касса при смене кассира не спрашивает,
    // а присланный другой кассир не учитывается. Чек без сети — по смене, открытой в момент
    // чека на устройстве; если смены тогда не было, остаётся кассир, выбранный на устройстве.
    let register = cash::default_account(conn, branch_id).await?;
    let at_shift = cash::shift_open_at(
        conn,
        branch_id,
        register,
        if offline { req.client_time } else { None },
    )
    .await?;
    let cashier_id = at_shift
        .as_ref()
        .map_or(req.cashier_id, |s| s.cashier_employee_id);
    // Кассира смены могли отключить посреди смены — чек смены всё равно его.
    check_employee(
        conn,
        branch_id,
        cashier_id,
        false,
        offline || at_shift.is_some(),
        &mut bypass,
    )
    .await?;

    let debt_total: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "debt")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    match req.party_id {
        Some(pid) => {
            let reasons = parties::check_sale_party(
                conn,
                ctx,
                pid,
                req.contact_id,
                req.vehicle_id,
                offline,
                debt_total > 0,
            )
            .await?;
            bypass.extend(reasons);
        }
        None if debt_total > 0 => return Err(invalid("для продажи в долг укажите клиента")),
        None if req.contact_id.is_some() || req.vehicle_id.is_some() => {
            return Err(invalid("работник и машина указываются вместе с клиентом"));
        }
        None => {}
    }
    // Долг оформляется документом с ПИН или ИНН покупателя (SPEC-10): без него расписка
    // недействительна. Чек без сети уже состоялся — его принимаем, ПИН допишут в карточке.
    let mut debt_without_inn = false;
    if debt_total > 0
        && let Some(pid) = req.party_id
    {
        let inn = sqlx::query_scalar!("select inn from parties where id = $1", pid)
            .fetch_one(&mut *conn)
            .await?;
        if inn.trim().is_empty() {
            if !offline {
                return Err(invalid(
                    "для продажи в долг укажите ПИН (физлицо) или ИНН (фирма) клиента",
                ));
            }
            // Поблажка офлайн-чека — владельцу на проверку (ADR-058).
            debt_without_inn = true;
        }
    }

    let mut prepared = Vec::with_capacity(req.lines.len());
    let mut total: i64 = 0;
    for l in &req.lines {
        let p = prepare_line(conn, branch_id, l, offline, &mut bypass).await?;
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
        // Сколько каждого товара-условия в чеке, в его единицах учёта (мл или штуки).
        let mut bought: std::collections::HashMap<Uuid, i64> = std::collections::HashMap::new();
        for p in prepared.iter().filter(|p| !p.gift) {
            if let Some(pid) = p.product_id {
                let e = bought.entry(pid).or_insert(0);
                *e = e.checked_add(p.units).ok_or_else(overflow)?;
            }
        }
        let in_cart: Vec<Uuid> = bought.keys().copied().collect();
        let rules = sqlx::query!(
            r#"select r.trigger_product_id, r.min_units, i.gift_product_id, i.gift_qty, r.active
               from gift_rules r join gift_rule_items i on i.rule_id = r.id
               where r.branch_id = $1 and (r.active or $4)
                 and r.trigger_product_id = any($2) and i.gift_product_id = any($3)"#,
            branch_id,
            &in_cart,
            &gifts,
            offline
        )
        .fetch_all(&mut *conn)
        .await?;
        // Подарок положен, если хоть одно правило с ним выполнено по порогу (SPEC-11);
        // дарим не больше, чем правило разрешает.
        let distinct: BTreeSet<Uuid> = gifts.iter().copied().collect();
        for gift in distinct {
            let allowed = rules
                .iter()
                .filter(|r| r.gift_product_id == gift)
                .filter(|r| bought.get(&r.trigger_product_id).copied().unwrap_or(0) >= r.min_units)
                .map(|r| r.gift_qty)
                .max();
            let Some(allowed) = allowed else {
                return Err(invalid("этот товар нельзя подарить к такой покупке"));
            };
            let given = prepared
                .iter()
                .filter(|p| p.gift && p.product_id == Some(gift))
                .fold(0i64, |acc, p| acc.saturating_add(p.qty));
            if given > allowed {
                return Err(invalid(format!("подарка можно дать не больше {allowed}")));
            }
            // Чек без сети принят по правилу, которое уже отключили: владелец должен знать.
            let by_active = rules
                .iter()
                .filter(|r| r.gift_product_id == gift && r.active)
                .filter(|r| bought.get(&r.trigger_product_id).copied().unwrap_or(0) >= r.min_units)
                .map(|r| r.gift_qty)
                .max()
                .unwrap_or(0);
            if given > by_active {
                bypass.push("подарок по отключённому правилу".into());
            }
        }
    }
    ops::check_amount(total)?;
    check_payments(&req.payments, total)?;
    // Мастеру: ставка замены за чек «в сервис» с товаром (ADR-027) плюс ставки услуг из строк.
    // Чек из одних услуг — клиент приехал со своим маслом — ставку замены не даёт (ADR-043).
    let has_goods = prepared.iter().any(|p| p.product_id.is_some() && !p.gift);
    let check_fee = if req.sale_type == "service" && has_goods {
        oil_change_fee(conn, branch_id).await?
    } else {
        0
    };
    let master_fee = prepared
        .iter()
        .try_fold(check_fee, |acc, p| acc.checked_add(p.master_fee))
        .ok_or_else(overflow)?;

    lock_products(
        conn,
        branch_id,
        prepared.iter().filter_map(|p| p.product_id),
    )
    .await?;
    // Пока ждали блокировку, карточку могли объединить: остаток не должен застрять в архиве.
    // Просто архивная карточка (остаток 0) чек без сети не отклоняет — товар уже выдан.
    let ids: Vec<Uuid> = prepared.iter().filter_map(|p| p.product_id).collect();
    let archived = sqlx::query_scalar!(
        r#"select count(*) as "n!" from products where id = any($1) and archived and merged_into is not null"#,
        &ids
    )
    .fetch_one(&mut *conn)
    .await?;
    if archived > 0 {
        return Err(AppError::Conflict(
            "товар только что объединили с другой карточкой — проведите чек ещё раз".into(),
        ));
    }
    let number = ops::next_counter(conn, branch_id, "sale").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into sales (id, branch_id, number, kind, sale_type, cashier_id, master_id, total_tyiyn,
                              master_fee_tyiyn, comment, party_id, contact_id, vehicle_id,
                              user_id, device_id, client_time, delivery_address, business_date)
           values ($1, $2, $3, 'sale', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                   case when $15::timestamptz is not null
                         and $15::timestamptz > now() - interval '2 days'
                         and $15::timestamptz < now() + interval '5 minutes'
                        then ($15::timestamptz at time zone 'Asia/Bishkek')::date
                        else (now() at time zone 'Asia/Bishkek')::date end)"#,
        id,
        branch_id,
        number,
        req.sale_type,
        cashier_id,
        req.master_id,
        total,
        master_fee,
        req.comment.trim(),
        req.party_id,
        req.contact_id,
        req.vehicle_id,
        ctx.user.id,
        ctx.device_id,
        // Время устройства задаёт учётный день только чеку без сети: онлайн-чек задним
        // числом в закрытый день не проводится (SPEC-08).
        if offline { req.client_time } else { None },
        req.delivery_address.trim()
    )
    .execute(&mut *conn)
    .await?;

    let mut overrides = Vec::new();
    let mut below_cost = Vec::new();
    let mut stale = Vec::new();
    // Валовая прибыль чека: с неё считается процент кассира (ADR-036). Строки без цены закупки
    // в неё не входят: их процент ждёт, пока владелец не укажет цену (ADR-062).
    let mut gross: i64 = 0;
    let mut unknown: Vec<payroll::UnknownCost> = Vec::new();
    // Сумма строк без цены закупки: на неё приходится часть скидки баллами (ADR-065).
    let mut unknown_amount: i64 = 0;
    for (i, p) in prepared.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        let mut cost = 0;
        let mut unknown_units = 0;
        if let Some(pid) = p.product_id {
            let pool = ops::lock_pool(conn, branch_id, pid).await?;
            cost = cost_of(p.units, &pool).ok_or_else(overflow)?;
            unknown_units = crate::domain::costing::unknown_units(p.units, &pool);
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
        // Дешевле закупки не продаём, кроме подарка (ADR-042). Сравнение со своей
        // себестоимостью строки: у розлива она за миллилитры, у канистры — за штуки.
        if p.product_id.is_some() && !p.gift && p.amount < cost {
            if !offline {
                return Err(invalid(if ctx.user.is_owner() {
                    format!(
                        "строка {line_no}: цена ниже закупочной ({}), дешевле продать нельзя",
                        crate::domain::money::format_som(cost)
                    )
                } else {
                    format!("строка {line_no}: цена ниже закупочной, дешевле продать нельзя")
                }));
            }
            below_cost.push(json!({ "line_no": line_no, "amount": p.amount, "cost": cost }));
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
            unknown_units > 0,
        )
        .await?;
        match p.product_id {
            Some(product_id) if unknown_units > 0 => {
                unknown_amount = unknown_amount.checked_add(p.amount).ok_or_else(overflow)?;
                unknown.push(payroll::UnknownCost {
                    line_no,
                    product_id,
                    units: unknown_units,
                });
            }
            _ => gross = gross.checked_add(p.amount - cost).ok_or_else(overflow)?,
        }
        // Цену меняли на кассе — если она отличается от прайса, который видела касса; прайс
        // мог смениться, пока чек лежал без сети: это не правка кассира, а старая цена (SPEC-09).
        let seen = p.seen_list.unwrap_or(p.list_price);
        if p.unit_price != seen && !p.gift {
            overrides.push(json!({ "line_no": line_no, "list": seen, "price": p.unit_price }));
        }
        if offline && seen != p.list_price && !p.gift {
            stale.push(json!({ "line_no": line_no, "seen": seen, "list": p.list_price }));
        }
    }
    // Учётный день чека: по нему ставка банка (SPEC-08) и начисления сотрудникам.
    let business_date = sqlx::query_scalar!(
        r#"select business_date as "d!" from sales where id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    insert_payments(conn, ctx, id, &req.payments, 1, None, business_date).await?;
    // Баллы: списание оплатой «bonus» и начисление с денег (SPEC-19).
    crate::api::loyalty::on_sale(conn, ctx, id, req.party_id, &req.payments).await?;
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
    if debt_without_inn {
        ops::audit(
            conn,
            ctx,
            "sale.debt_without_inn",
            "sale",
            Some(id),
            json!({ "number": number, "debt": debt_total }),
        )
        .await?;
    }
    if !stale.is_empty() {
        ops::audit(
            conn,
            ctx,
            "sale.stale_price",
            "sale",
            Some(id),
            json!({ "number": number, "lines": stale }),
        )
        .await?;
    }
    if !below_cost.is_empty() {
        ops::audit(
            conn,
            ctx,
            "sale.below_cost",
            "sale",
            Some(id),
            json!({ "number": number, "lines": below_cost }),
        )
        .await?;
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
    if offline {
        offline_after_close(conn, ctx, id, number, req.client_time, &req.payments).await?;
        // Задним числом: учётный день чека раньше сегодняшнего дня сервера (SPEC-08).
        let today =
            sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                .fetch_one(&mut *conn)
                .await?;
        if business_date < today {
            bypass.push(format!(
                "проведён задним числом: {}",
                business_date.format("%d.%m.%Y")
            ));
        }
        if !bypass.is_empty() {
            ops::audit(
                conn,
                ctx,
                "sale.offline_bypass",
                "sale",
                Some(id),
                json!({ "number": number, "reasons": bypass }),
            )
            .await?;
        }
    }
    if !unknown.is_empty() {
        let ids: Vec<Uuid> = unknown.iter().map(|u| u.product_id).collect();
        let names = sqlx::query_scalar!(
            "select name from products where id = any($1) order by name",
            &ids
        )
        .fetch_all(&mut *conn)
        .await?;
        ops::audit(
            conn,
            ctx,
            "sale.cost_unknown",
            "sale",
            Some(id),
            json!({ "number": number, "names": names }),
        )
        .await?;
    }
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
    // Замена в сервисе на машине клиента — запись в масляную книжку (SPEC-16).
    if req.sale_type == "service"
        && let (Some(party_id), Some(vehicle_id)) = (req.party_id, req.vehicle_id)
    {
        if req
            .mileage_km
            .is_some_and(|m| !(0..=5_000_000).contains(&m))
        {
            return Err(invalid("пробег от 0 до 5 000 000 км"));
        }
        crate::api::oil_book::record_from_sale(
            conn,
            ctx,
            id,
            party_id,
            vehicle_id,
            business_date,
            req.mileage_km,
            &req.declined,
        )
        .await?;
    }
    payroll::accrue_for_sale(
        conn,
        ctx,
        payroll::SaleAccrual {
            sale_id: id,
            business_date,
            cashier_id,
            party_id: req.party_id,
            gross,
            total,
            debt: debt_total,
            bonus: req
                .payments
                .iter()
                .filter(|p| p.method == "bonus")
                .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
                .ok_or_else(overflow)?,
            unknown_amount,
            unknown,
        },
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "sale",
        Some(id),
        json!({ "number": number, "total": total, "offline": offline, "client_time": req.client_time }),
    )
    .await?;
    let out = load_sale(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Наличные чека без сети легли в кассу уже после пересчёта: чек пробит в смену, которую с тех пор
/// закрыли, или до открытия более поздней смены (её пересчёт при открытии этих денег ещё не знал).
/// Смен в день несколько (ADR-071), поэтому смена ищется по времени чека, а не по учётному дню.
/// Расчёт недостачи не меняем — владелец получает уведомление с суммой и понимает, откуда
/// разница (ADR-059).
async fn offline_after_close(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    number: i64,
    client_time: Option<DateTime<Utc>>,
    payments: &[PaymentReq],
) -> AppResult<()> {
    let cash: i64 = payments
        .iter()
        .filter(|p| p.method == "cash")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    let Some(at) = client_time else {
        return Ok(());
    };
    if cash == 0 {
        return Ok(());
    }
    let register = cash::default_account(conn, ctx.user.branch_id).await?;
    let during = cash::shift_open_at(conn, ctx.user.branch_id, register, Some(at)).await?;
    let now = cash::open_shift_id(conn, ctx.user.branch_id, register).await?;
    // Смена момента чека всё ещё открыта — деньги войдут в её пересчёт, всё сходится.
    let shift_number = match during {
        Some(s) if now == Some(s.id) => None,
        Some(s) => Some(s.number),
        // Смены в момент чека не было: важна первая смена, открытая после него. Деньги не
        // попали в её пересчёт, если при открытии кассу пересчитали или смену уже закрыли.
        None => sqlx::query!(
            r#"select id, number, opening_counted_tyiyn from shifts
               where branch_id = $1 and account_id = $2 and opened_at > $3
               order by opened_at limit 1"#,
            ctx.user.branch_id,
            register,
            at
        )
        .fetch_optional(&mut *conn)
        .await?
        .filter(|s| s.opening_counted_tyiyn.is_some() || now != Some(s.id))
        .map(|s| s.number),
    };
    if let Some(shift_number) = shift_number {
        ops::audit(
            conn,
            ctx,
            "sale.offline_after_close",
            "sale",
            Some(sale_id),
            json!({ "number": number, "cash": cash, "shift_number": shift_number }),
        )
        .await?;
    }
    Ok(())
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
        r#"select kind, sale_type, cashier_id, master_id, master_fee_tyiyn, business_date,
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
        /// Себестоимость строки чека была неизвестна (ADR-062) и сколько в строке было всего.
        cost_unknown: bool,
        orig_qty: i64,
    }
    let mut planned = Vec::new();
    let mut total: i64 = 0;
    for rl in &req.lines {
        let o = sqlx::query!(
            r#"select kind, product_id, service_id, qty, units, unit_price_tyiyn, list_price_tyiyn,
                      amount_tyiyn, cost_tyiyn, master_fee_tyiyn, gift, cost_unknown
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
                // Возврат по влитой карточке ложится на основную (ADR-044).
                product_id: match o.product_id {
                    Some(p) => Some(ops::live_product(conn, p).await?),
                    None => None,
                },
                service_id: o.service_id,
                qty: rl.qty,
                units,
                unit_price: o.unit_price_tyiyn,
                list_price: o.list_price_tyiyn,
                amount,
                master_fee: fee,
                seen_list: None,
            },
            line_no: rl.line_no,
            amount,
            cost,
            fee,
            cost_unknown: o.cost_unknown,
            orig_qty: o.qty,
        });
    }
    check_payments(&req.payments, total)?;
    // Сколько по чеку ещё в долгу и сколько заплачено деньгами, за вычетом прежних возвратов.
    let left = sqlx::query!(
        r#"select coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
                  coalesce(sum(p.amount_tyiyn) filter (where p.method not in ('debt', 'bonus')), 0)::bigint as "money!"
           from sale_payments p join sales s on s.id = p.sale_id
           where s.id = $1 or s.reversal_of = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let (debt_req, money_req) = req
        .payments
        .iter()
        .try_fold((0i64, 0i64), |(d, m), p| {
            if p.method == "debt" {
                d.checked_add(p.amount_tyiyn).map(|d| (d, m))
            } else if p.method == "bonus" {
                Some((d, m))
            } else {
                m.checked_add(p.amount_tyiyn).map(|m| (d, m))
            }
        })
        .ok_or_else(overflow)?;
    // Списать с долга больше, чем этот чек в долг дал, — значит уменьшить чужой долг.
    if debt_req > left.debt.max(0) {
        return Err(invalid(format!(
            "с долга по этому чеку можно списать не больше {}",
            crate::domain::money::format_som(left.debt.max(0))
        )));
    }
    // Долг уже погасили — списывать с долга нечего: иначе клиент окажется с авансом,
    // а процент кассира за возвращённый товар останется (возврат — деньгами).
    if debt_req > 0 {
        let owes = match orig.party_id {
            Some(pid) => {
                sqlx::query_scalar!("select balance_tyiyn from parties where id = $1", pid)
                    .fetch_one(&mut *conn)
                    .await?
                    .max(0)
            }
            None => 0,
        };
        if debt_req > owes {
            return Err(invalid(format!(
                "клиент сейчас должен {}: с долга можно снять не больше, остальное верните деньгами",
                crate::domain::money::format_som(owes)
            )));
        }
    }
    // Чек оплачен баллами — деньгами больше, чем платили деньгами, не вернуть: баллы не обналичиваются.
    let bonus_paid = sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from sale_payments where sale_id = $1 and method = 'bonus'"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if bonus_paid > 0 && money_req > left.money.max(0) {
        return Err(invalid(format!(
            "деньгами по этому чеку можно вернуть не больше {}, остальное — баллами",
            crate::domain::money::format_som(left.money.max(0))
        )));
    }
    // Наличные из «Кассы» выдаются только в открытую смену (ADR-065): иначе деньги уйдут мимо
    // пересчёта. Возврат на карту и списанием долга смены не требует, продажа без смены — тоже.
    if req
        .payments
        .iter()
        .any(|p| p.method == "cash" && p.amount_tyiyn > 0)
    {
        let till = cash::default_account(conn, branch_id).await?;
        if cash::open_shift_id(conn, branch_id, till).await?.is_none() {
            return Err(AppError::Conflict(
                "возврат наличными — только при открытой смене: откройте смену на экране «Смена»"
                    .into(),
            ));
        }
    }
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
    let now_qty = req
        .lines
        .iter()
        .fold(0i64, |acc, l| acc.saturating_add(l.qty));
    let full_return = returned_qty + now_qty >= sold_qty;
    let fee_back = if full_return {
        -orig.master_fee_tyiyn
    } else {
        0
    };

    let products: BTreeSet<Uuid> = planned.iter().filter_map(|x| x.p.product_id).collect();
    let ids: Vec<Uuid> = products.iter().copied().collect();
    lock_products(conn, branch_id, products).await?;
    ops::ensure_not_merged(conn, &ids).await?;
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
            conn,
            rid,
            x.line_no,
            &x.p,
            -x.amount,
            -x.cost,
            -x.fee,
            x.p.units,
            x.p.qty,
            x.cost_unknown,
        )
        .await?;
    }
    // Комиссия банка возвращается из записанной в исходном чеке, а не по ставке (ADR-070).
    let fb = sqlx::query!(
        r#"select coalesce(sum(p.fee_tyiyn) filter (where s.id = $1), 0)::bigint as "fee!",
                  coalesce(sum(p.amount_tyiyn) filter (where s.id = $1), 0)::bigint as "base!",
                  coalesce(sum(-p.amount_tyiyn) filter (where s.id <> $1), 0)::bigint as "refunded!",
                  coalesce(sum(-p.fee_tyiyn) filter (where s.id <> $1), 0)::bigint as "fee_back!"
           from sale_payments p join sales s on s.id = p.sale_id
           where (s.id = $1 or s.reversal_of = $1) and p.method in ('card', 'transfer')"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    insert_payments(
        conn,
        ctx,
        rid,
        &req.payments,
        -1,
        Some(FeeBack {
            fee: fb.fee,
            base: fb.base,
            refunded: fb.refunded,
            fee_back: fb.fee_back,
        }),
        orig.business_date,
    )
    .await?;
    crate::api::loyalty::on_return(conn, ctx, id, rid, orig.party_id, &req.payments).await?;
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
    // Строка без цены закупки в процент кассира не входила, пока владелец не дал цену; после —
    // входит по его цене (ADR-062). Так возврат снимает ровно то, что начислили.
    let mut gross_back: i64 = 0;
    let mut counted_back: i64 = 0;
    for x in &planned {
        let g = if x.cost_unknown {
            payroll::unknown_line_gross_back(conn, id, x.line_no, x.amount, x.p.qty, x.orig_qty)
                .await?
        } else {
            Some(x.amount.checked_sub(x.cost).ok_or_else(overflow)?)
        };
        if let Some(g) = g {
            gross_back = gross_back.checked_add(g).ok_or_else(overflow)?;
            counted_back = counted_back.checked_add(x.amount).ok_or_else(overflow)?;
        }
    }
    let return_date = sqlx::query_scalar!(
        r#"select business_date as "d!" from sales where id = $1"#,
        rid
    )
    .fetch_one(&mut *conn)
    .await?;
    payroll::reverse_for_return(
        conn,
        ctx,
        payroll::ReturnAccrual {
            orig_sale_id: id,
            return_id: rid,
            business_date: return_date,
            gross_back,
            total_back: total,
            debt_back,
            counted_back,
            bonus_back: req
                .payments
                .iter()
                .filter(|p| p.method == "bonus")
                .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
                .ok_or_else(overflow)?,
            full_return,
        },
    )
    .await?;
    // Деньгами вернули больше, чем по чеку платили деньгами: законно, если долг уже погашен,
    // но владелец должен это видеть.
    if money_req > left.money.max(0) {
        ops::audit(
            conn,
            ctx,
            "sale.return_over_paid",
            "sale",
            Some(rid),
            json!({ "number": number, "paid": left.money.max(0), "refund": money_req }),
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
pub(crate) struct SaleListItem {
    pub(crate) id: Uuid,
    pub(crate) number: i64,
    pub(crate) kind: String,
    pub(crate) sale_type: String,
    pub(crate) cashier_name: String,
    pub(crate) master_name: Option<String>,
    pub(crate) party_name: Option<String>,
    pub(crate) party_phone: Option<String>,
    /// Сколько из этого чека ушло в долг.
    pub(crate) debt_tyiyn: i64,
    pub(crate) total_tyiyn: i64,
    pub(crate) reversal_of: Option<Uuid>,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
pub(crate) struct DayTotals {
    pub(crate) count: i64,
    pub(crate) total_tyiyn: i64,
    pub(crate) cash_tyiyn: i64,
    pub(crate) card_tyiyn: i64,
    pub(crate) transfer_tyiyn: i64,
    pub(crate) debt_tyiyn: i64,
    pub(crate) bonus_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct DayOut {
    pub(crate) date: NaiveDate,
    pub(crate) sales: Vec<SaleListItem>,
    pub(crate) totals: DayTotals,
}

async fn list_sales(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<DayQuery>,
) -> AppResult<Json<DayOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(sales_day(&mut conn, &user, q.date).await?))
}

/// Чеки учётного дня с итогами по способам оплаты — экран и бот владельца (SPEC-24).
pub(crate) async fn sales_day(
    conn: &mut PgConnection,
    user: &CurrentUser,
    date: Option<NaiveDate>,
) -> AppResult<DayOut> {
    // День — учётный день чека, как в прибыли: офлайн-чек вчерашнего дня остаётся во вчерашнем (SPEC-08).
    let date = match date {
        Some(d) => d,
        None => {
            sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                .fetch_one(&mut *conn)
                .await?
        }
    };
    let sales = sqlx::query_as!(
        SaleListItem,
        r#"select s.id, s.number, s.kind, s.sale_type, c.full_name as cashier_name, m.full_name as "master_name?",
                  pt.name as "party_name?", nullif(pt.phone, '') as "party_phone?",
                  coalesce((select sum(p.amount_tyiyn) from sale_payments p
                            where p.sale_id = s.id and p.method = 'debt'), 0)::bigint as "debt_tyiyn!",
                  s.total_tyiyn, s.reversal_of, s.created_at
           from sales s
           join employees c on c.id = s.cashier_id
           left join employees m on m.id = s.master_id
           left join parties pt on pt.id = s.party_id
           where s.branch_id = $1
             and s.business_date = $2
           order by s.created_at desc"#,
        user.branch_id,
        date
    )
    .fetch_all(&mut *conn)
    .await?;
    let t = sqlx::query!(
        r#"select
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'cash'), 0)::bigint as "cash!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as "card!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as "transfer!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'bonus'), 0)::bigint as "bonus!"
           from sale_payments p join sales s on s.id = p.sale_id
           where s.branch_id = $1
             and s.business_date = $2"#,
        user.branch_id,
        date
    )
    .fetch_one(&mut *conn)
    .await?;
    let totals = DayTotals {
        count: i64::try_from(sales.iter().filter(|s| s.kind == "sale").count()).unwrap_or(i64::MAX),
        total_tyiyn: sales
            .iter()
            .fold(0i64, |acc, s| acc.saturating_add(s.total_tyiyn)),
        cash_tyiyn: t.cash,
        card_tyiyn: t.card,
        transfer_tyiyn: t.transfer,
        debt_tyiyn: t.debt,
        bonus_tyiyn: t.bonus,
    };
    Ok(DayOut {
        date,
        sales,
        totals,
    })
}
