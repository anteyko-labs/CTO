//! Касса: продажа и возврат (SPEC-04).

use std::collections::{BTreeSet, HashMap, HashSet};

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
use crate::domain::oil_service::{OilLine, canister_ml, enough_oil, withhold};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sales", routing::get(list_sales).post(post_sale))
        .route("/sales/{id}", routing::get(get_sale))
        .route("/sales/{id}/return", routing::post(return_sale))
        .route(
            "/sales/return-without-receipt",
            routing::post(no_receipt_return),
        )
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
    /// Аккумулятор в оплату (`battery`, SPEC-27): вес в граммах. Сумму считает сервер по цене
    /// приёма за кг; присланная сумма — то, что видела касса, и должна с ней совпасть. В
    /// возврате вес не присылается: сервер берёт долю веса исходного чека.
    #[serde(default)]
    pub grams: Option<i64>,
    /// Аккумулятор в оплату: цена приёма за кг, которую кассир поставил в этом чеке (ADR-078);
    /// без неё — цена из настроек.
    #[serde(default)]
    pub price_per_kg_tyiyn: Option<i64>,
    /// Карта, перевод и «По счёту»: безналичный счёт, куда пришли деньги (ADR-076); без него —
    /// счёт по умолчанию для этого способа (ADR-077, ADR-095).
    /// У возврата не учитывается: деньги уходят со счёта исходной оплаты.
    #[serde(default)]
    pub account_id: Option<Uuid>,
    /// «Карта / QR банка» оплачена по QR на сумму (SPEC-31): сервер проверяет, что QR оплачен,
    /// сумма та же и он не привязан к другому чеку. У возврата не учитывается.
    #[serde(default)]
    pub bank_qr_id: Option<Uuid>,
}

/// Интервал строки книжки из чека: км (мили экран переводит сам, ADR-084) и дни.
#[derive(Deserialize, Clone)]
pub struct BookIntervalReq {
    pub slot: String,
    pub interval_km: i32,
    pub interval_days: i32,
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
    /// Чей это чек на устройстве, если его отправляет владелец от себя (SPEC-09, ADR-093).
    #[serde(default)]
    pub claimed_user_id: Option<Uuid>,
    /// Пробег машины при замене — в масляную книжку (SPEC-16).
    #[serde(default, deserialize_with = "crate::api::oil_book::lenient_mileage")]
    pub mileage_km: Option<i64>,
    /// Пробег, как его вписали, и единица (ADR-084); без единицы — единица машины. Любое число:
    /// слишком большое не роняет чек ошибкой формата (ADR-093).
    #[serde(default, deserialize_with = "crate::api::oil_book::lenient_mileage")]
    pub mileage: Option<i64>,
    #[serde(default)]
    pub mileage_unit: Option<String>,
    /// «Через сколько менять»: интервал строк книжки этой машины, поставленный в чеке
    /// (SPEC-29 §23). Сохраняется интервалом машины в транзакции чека.
    #[serde(default)]
    pub book_intervals: Vec<BookIntervalReq>,
    /// Адрес доставки; пусто — забрали сами.
    #[serde(default)]
    pub delivery_address: String,
    /// Отказанные работы для масляной книжки: «воздушный фильтр» (ADR-055).
    #[serde(default)]
    pub declined: String,
    /// Строки, удалённые из чека до проведения (SPEC-30 §8, ADR-099): как прислала касса;
    /// неверная строка пропускается, а не отклоняет чек.
    #[serde(default)]
    pub removed_lines: serde_json::Value,
    /// Чек проводит «Исправить» владельца (SPEC-29 §24): кассир — из исправления, а не смены,
    /// учётный день — заданный. С экрана не приходит.
    #[serde(skip)]
    pub correction: Option<SaleCorrection>,
}

/// Как проводится исправленный чек (SPEC-29 §24, ADR-085).
#[derive(Clone, Copy, Debug)]
pub struct SaleCorrection {
    pub business_date: NaiveDate,
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
    /// Строка возврата — удержание за замену: номер строки услуги исходного чека (ADR-104).
    #[serde(default)]
    pub withheld_for: Option<i32>,
    /// Категория масла: у масла — его категория, у услуги «бесплатно с нашим маслом» — категория
    /// её масла. По ней окно возврата считает удержание так же, как сервер.
    #[serde(default)]
    pub oil_category_id: Option<Uuid>,
    /// Услуга чека была бесплатной по маслу и ещё не удержана: цена, которую удержит возврат
    /// масла ниже канистры (ADR-104).
    #[serde(default)]
    pub withhold_price_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PaymentOut {
    pub method: String,
    pub amount_tyiyn: i64,
    /// Вес аккумулятора, принятого в оплату (у возврата — с минусом).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grams: Option<i64>,
    /// Карта и перевод: на какой безналичный счёт (ADR-076).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
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
    /// Начисление мастеру: сумма ставок услуг чека (ADR-096); у чеков до SPEC-30 и чеков без
    /// сети без вида замены — ставка за чек (ADR-027).
    pub master_fee_tyiyn: i64,
    pub comment: String,
    /// Куда везти; доставка бесплатная, поэтому только адрес (BLUEPRINT §6, вопросы 16 и 21).
    #[serde(default)]
    pub delivery_address: String,
    pub reversal_of: Option<Uuid>,
    /// Возврат без чека (ADR-105).
    #[serde(default)]
    pub no_receipt: bool,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    /// Учётный день чека: его печатают чек и документ о долге, от него считается срок оплаты
    /// (чек без сети после полуночи, исправленный чек — день продажи, а не записи).
    #[serde(default)]
    pub business_date: Option<NaiveDate>,
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
                  s.comment, s.delivery_address, s.reversal_of, s.no_receipt,
                  s.party_id as "party_id?", pt.name as "party_name?", pt.balance_tyiyn as "party_balance_tyiyn?",
                  pc.full_name as "contact_name?", pv.plate as "vehicle_plate?",
                  u.full_name as user_name, s.created_at, s.business_date
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
                  -- Полярность аккумулятора — в названии строки: «(Л)» прямая, «(П)» обратная (SPEC-27).
                  coalesce(l.name,
                           p.name || case p.attrs ->> 'polarity' when 'прямая' then ' (Л)'
                                                                when 'обратная' then ' (П)' else '' end,
                           sv.name) as "name!",
                  p.container_ml as "container_ml?",
                  l.qty, l.units, l.unit_price_tyiyn, l.list_price_tyiyn, l.amount_tyiyn, l.cost_tyiyn,
                  l.master_fee_tyiyn, l.gift, l.withheld_for,
                  case when l.kind = 'service' then (case when sv.free_with_oil then sv.oil_category_id end)
                       when pc.kind = 'oil' then p.category_id end as "oil_category_id?",
                  case when l.kind = 'service' and l.withheld_for is null and l.amount_tyiyn = 0
                            and l.list_price_tyiyn = 0 and sv.free_with_oil
                            and sv.oil_category_id is not null and sv.price_tyiyn > 0
                            and not exists (select 1 from sale_lines w join sales r on r.id = w.sale_id
                                            where r.reversal_of = l.sale_id and w.withheld_for = l.line_no)
                       then sv.price_tyiyn end as "withhold_price_tyiyn?"
           from sale_lines l
           left join products p on p.id = l.product_id
           left join categories pc on pc.id = p.category_id
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
        withheld_for: r.withheld_for,
        oil_category_id: r.oil_category_id,
        withhold_price_tyiyn: r.withhold_price_tyiyn,
    })
    .collect();
    let payments = sqlx::query_as!(
        PaymentOut,
        r#"select p.method, p.amount_tyiyn, p.grams, p.account_id, a.name as "account_name?"
           from sale_payments p left join cash_accounts a on a.id = p.account_id
           where p.sale_id = $1 order by p.method, a.name"#,
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
        no_receipt: h.no_receipt,
        user_name: h.user_name,
        created_at: h.created_at,
        business_date: Some(h.business_date),
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
    /// Услуга «бесплатно с нашим маслом» (ADR-096): с нашим маслом её категории (ADR-104) не
    /// меньше канистры цена по каталогу — 0.
    free_with_oil: bool,
    oil_category: Option<Uuid>,
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
            "select name, active, price_tyiyn, master_fee_tyiyn, free_with_oil, oil_category_id from services where id = $1 and branch_id = $2 and (active or $3)",
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
            free_with_oil: s.free_with_oil,
            oil_category: s.oil_category_id,
        });
    }
    // Карточку могли влить в другую, пока чек лежал на устройстве: продаём основную.
    let pid = ops::live_product(
        conn,
        l.product_id.ok_or_else(|| invalid("не указан товар"))?,
    )
    .await?;
    let p = sqlx::query!(
        r#"select p.name, p.unit, p.container_ml, p.archived, coalesce(bp.sale_price_tyiyn, 0) as "sale_price!"
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
        // Прайс розлива — от цены канистры (ADR-074): 4 л розливом стоят как канистра 4 л.
        ("pour", "ml", Some(c)) => (
            l.qty,
            div_round(i128::from(l.qty) * i128::from(l.unit_price_tyiyn), 1000),
            crate::domain::money::canister_pour_price(p.sale_price, c).unwrap_or(0),
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
        free_with_oil: false,
        oil_category: None,
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

/// Масла среди товаров: товар → (категория вида «масло», объём канистры).
async fn oil_products(
    conn: &mut PgConnection,
    ids: &[Uuid],
) -> AppResult<HashMap<Uuid, (Uuid, Option<i64>)>> {
    let rows = sqlx::query!(
        r#"select p.id, p.category_id, p.container_ml from products p
           join categories c on c.id = p.category_id
           where p.id = any($1) and c.kind = 'oil'"#,
        ids
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.id, (r.category_id, r.container_ml)))
        .collect())
}

/// Строки масла чека по категориям: товар категории вида «масло», не подарок (ADR-104).
/// `lines` — (товар, мл, подарок).
async fn oil_by_category(
    conn: &mut PgConnection,
    lines: &[(Uuid, i64, bool)],
) -> AppResult<HashMap<Uuid, Vec<OilLine>>> {
    let ids: Vec<Uuid> = lines.iter().filter(|l| !l.2).map(|l| l.0).collect();
    let info = oil_products(conn, &ids).await?;
    let mut out: HashMap<Uuid, Vec<OilLine>> = HashMap::new();
    for (pid, units, gift) in lines {
        if *gift {
            continue;
        }
        if let Some((cat, container_ml)) = info.get(pid) {
            out.entry(*cat).or_default().push(OilLine {
                units_ml: *units,
                container_ml: *container_ml,
            });
        }
    }
    Ok(out)
}

/// Цена по каталогу услуги «бесплатно с нашим маслом» — 0, если в чеке нашего масла её
/// категории (не подарок) не меньше канистры (ADR-096, ADR-104). Иначе — цена клиенту из
/// справочника. Без категории масла флажок не действует.
async fn with_our_oil(conn: &mut PgConnection, prepared: &mut [Prepared]) -> AppResult<()> {
    if !prepared
        .iter()
        .any(|p| p.free_with_oil && p.oil_category.is_some())
    {
        return Ok(());
    }
    let goods: Vec<(Uuid, i64, bool)> = prepared
        .iter()
        .filter_map(|p| p.product_id.map(|id| (id, p.units, p.gift)))
        .collect();
    let oil = oil_by_category(conn, &goods).await?;
    for p in prepared.iter_mut().filter(|p| p.free_with_oil) {
        if p.oil_category
            .and_then(|c| oil.get(&c))
            .is_some_and(|lines| enough_oil(lines))
        {
            p.list_price = 0;
        }
    }
    Ok(())
}

/// Клиент назван (ADR-100): фирма — названием, человек — именем и фамилией. У прежних карточек
/// имя и фамилия отдельно не записаны (ADR-083) — тогда хватает полного имени из двух слов.
fn full_named(kind: &str, name: &str, first: &str, last: &str) -> bool {
    if kind == "company" {
        return !name.trim().is_empty();
    }
    let (first, last) = (first.trim(), last.trim());
    if !first.is_empty() || !last.is_empty() {
        return !first.is_empty() && !last.is_empty();
    }
    name.split_whitespace().count() >= 2
}

/// Чего не хватает чеку «в сервис» из данных клиента (ADR-100): клиент (человек с именем и
/// фамилией или фирма), телефон, машина с госномером, пробег. Пусто — всё есть.
async fn service_client_missing(
    conn: &mut PgConnection,
    req: &SaleReq,
) -> AppResult<Vec<&'static str>> {
    let mut missing = Vec::new();
    let party = match req.party_id {
        Some(id) => {
            sqlx::query!(
                "select kind, name, first_name, last_name, phone from parties where id = $1",
                id
            )
            .fetch_optional(&mut *conn)
            .await?
        }
        None => None,
    };
    if !party
        .as_ref()
        .is_some_and(|p| full_named(&p.kind, &p.name, &p.first_name, &p.last_name))
    {
        missing.push("клиент (имя и фамилия или фирма)");
    }
    // Телефон — клиента или того, кто приехал от фирмы.
    let contact_phone = match (req.party_id, req.contact_id) {
        (Some(pid), Some(cid)) => {
            sqlx::query_scalar!(
                "select phone from party_contacts where id = $1 and party_id = $2",
                cid,
                pid
            )
            .fetch_optional(&mut *conn)
            .await?
        }
        _ => None,
    };
    if !party.as_ref().is_some_and(|p| !p.phone.trim().is_empty())
        && !contact_phone.is_some_and(|p| !p.trim().is_empty())
    {
        missing.push("телефон клиента");
    }
    let plate = match (req.party_id, req.vehicle_id) {
        (Some(pid), Some(vid)) => {
            sqlx::query_scalar!(
                "select plate from party_vehicles where id = $1 and party_id = $2",
                vid,
                pid
            )
            .fetch_optional(&mut *conn)
            .await?
        }
        _ => None,
    };
    if !plate.is_some_and(|p| !p.trim().is_empty()) {
        missing.push("машина с госномером");
    }
    if !req.mileage.or(req.mileage_km).is_some_and(|m| m > 0) {
        missing.push("пробег");
    }
    Ok(missing)
}

/// Скидка — цена строки ниже цены по каталогу (ADR-097): у товара, розлива, веса и услуги.
/// Ставит владелец или администратор с правом «Скидка в чеке». Подарок, баллы (это оплата, а не
/// цена строки) и цена выше каталожной — не скидка. Чек без сети сверяется с прайсом, который
/// видела касса: прайс мог смениться, пока чек лежал на устройстве. Без права онлайн — 403,
/// без сети — чек проводится, а владелец получает причину.
async fn check_discounts(
    conn: &mut PgConnection,
    ctx: &Ctx,
    prepared: &[Prepared],
    claimed: Option<Uuid>,
    offline: bool,
    bypass: &mut Vec<String>,
) -> AppResult<()> {
    let lines: Vec<(&Prepared, i64)> = prepared
        .iter()
        .filter(|p| !p.gift)
        .map(|p| (p, p.seen_list.unwrap_or(p.list_price)))
        .filter(|(p, list)| p.unit_price < *list)
        .collect();
    if lines.is_empty() {
        return Ok(());
    }
    // Чек другого пользователя кассы отправил владелец (SPEC-09): право — того, чей это чек.
    let user = match claimed {
        Some(id) => {
            let u = sqlx::query!(
                "select login, full_name, role from users where id = $1 and branch_id = $2",
                id,
                ctx.user.branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("пользователь чека не найден"))?;
            CurrentUser {
                id,
                branch_id: ctx.user.branch_id,
                login: u.login,
                full_name: u.full_name,
                role: crate::auth::Role::parse(&u.role)?,
            }
        }
        None => ctx.user.clone(),
    };
    if crate::auth::has_permission(conn, &user, "discount").await? {
        return Ok(());
    }
    if !offline {
        return Err(AppError::ForbiddenWith("Скидку ставит владелец".into()));
    }
    for (p, list) in lines {
        let name = sqlx::query_scalar!(
            r#"select coalesce((select name from products where id = $1),
                               (select name from services where id = $2), '') as "n!""#,
            p.product_id,
            p.service_id
        )
        .fetch_one(&mut *conn)
        .await?;
        let per = match p.kind.as_str() {
            "pour" => " за л",
            "weight" => " за кг",
            _ => "",
        };
        bypass.push(format!(
            "скидка без права: {name}, {}{per}, цена по каталогу {}{per}",
            crate::domain::money::format_som(p.unit_price),
            crate::domain::money::format_som(list)
        ));
    }
    Ok(())
}

fn check_payments(payments: &[PaymentReq], total: i64) -> AppResult<()> {
    let mut sum: i64 = 0;
    if payments.iter().filter(|p| p.method == "battery").count() > 1 {
        return Err(invalid("аккумулятор в оплату — одной строкой"));
    }
    for p in payments {
        if !matches!(
            p.method.as_str(),
            "cash" | "card" | "transfer" | "invoice" | "debt" | "bonus" | "battery"
        ) {
            return Err(invalid(
                "способ оплаты: cash, card, transfer, invoice, debt, bonus или battery",
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

/// Аккумулятор в оплату чека (SPEC-27): сумма = вес × цена приёма за кг — из чека, если кассир
/// её поменял (ADR-078), иначе из настроек; считает сервер. Касса показывает ту же сумму заранее; если цену за это время поменяли, чек не
/// проводится — касса покажет новую сумму. У остальных способов вес не учитывается.
async fn price_battery(
    conn: &mut PgConnection,
    ctx: &Ctx,
    payments: &[PaymentReq],
) -> AppResult<Vec<PaymentReq>> {
    let branch_id = ctx.user.branch_id;
    let mut out = Vec::with_capacity(payments.len());
    for p in payments {
        let mut p = p.clone();
        if p.method == "battery" {
            let grams = p.grams.ok_or_else(|| invalid("укажите вес аккумулятора"))?;
            crate::api::batteries::check_grams(grams)?;
            let settings = crate::api::batteries::intake_price(conn, branch_id).await?;
            let price = match p.price_per_kg_tyiyn {
                Some(v) if v <= 0 => {
                    return Err(invalid("цена приёма за кг должна быть больше нуля"));
                }
                Some(v) => {
                    crate::ops::check_amount(v)?;
                    v
                }
                None => settings,
            };
            // Дороже трёх цен приёма из настроек ставит только владелец: иначе 1 г лома
            // «оплатил» бы любой товар (ADR-090).
            if !ctx.user.is_owner() && settings > 0 && price > settings.saturating_mul(3) {
                return Err(invalid(format!(
                    "цену выше {} за кг ставит владелец",
                    crate::domain::money::format_som(settings.saturating_mul(3))
                )));
            }
            let amount = crate::api::batteries::amount_for(grams, price)?;
            if price != settings && amount > 0 {
                crate::ops::audit(
                    conn,
                    ctx,
                    "sale.battery_price",
                    "product",
                    None,
                    json!({
                        "grams": grams,
                        "price_per_kg": price,
                        "settings_per_kg": settings,
                        "amount": amount,
                    }),
                )
                .await?;
            }
            if amount <= 0 {
                return Err(invalid("цена приёма аккумуляторов не задана"));
            }
            if p.amount_tyiyn != amount {
                return Err(invalid(format!(
                    "аккумулятор {} по {} за кг — {}: цена приёма изменилась, проверьте оплату",
                    crate::domain::money::format_kg(grams),
                    crate::domain::money::format_som(price),
                    crate::domain::money::format_som(amount)
                )));
            }
        } else {
            p.grams = None;
        }
        out.push(p);
    }
    Ok(out)
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
                                   gift, cost_unknown, name)
           select $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                  -- Название на момент продажи: печать позже покажет то, что продали.
                  coalesce(p.name || case p.attrs ->> 'polarity' when 'прямая' then ' (Л)'
                                                                when 'обратная' then ' (П)' else '' end,
                           sv.name)
           from (select 1) x
           left join products p on p.id = $5
           left join services sv on sv.id = $6"#,
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
    // Возврат: комиссия исходного чека по карте и переводу по каждому счёту (`None` — продажа).
    mut back: Option<HashMap<Uuid, FeeBack>>,
    // Учётный день чека: ставка банка берётся действовавшая в него.
    on: NaiveDate,
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let register = cash::default_account(conn, branch_id).await?;
    let movement_kind = if sign > 0 { "sale" } else { "sale_return" };
    for p in payments {
        let amount = sign * p.amount_tyiyn;
        // Счёт выбран и проверен до проведения (ADR-076); без него — счёт по умолчанию для
        // этого способа (ADR-077, ADR-095).
        let bank = match p.method.as_str() {
            "card" | "transfer" | "invoice" => match p.account_id {
                Some(id) => Some(id),
                None => cash::bank_account(conn, branch_id, &p.method).await?,
            },
            _ => None,
        };
        // Ставка — того счёта, куда легли деньги, в учётный день чека (ADR-077). «По счёту» —
        // перечисление, а не терминал: комиссии нет ни при продаже, ни при возврате (ADR-095).
        let fee = match (bank, back.as_mut()) {
            _ if p.method == "invoice" => 0,
            // Комиссия возвращается тому счёту, который её заплатил, и не больше его доли (ADR-077).
            (Some(bank_id), Some(b)) => -b.get_mut(&bank_id).map_or(0, |f| f.take(p.amount_tyiyn)),
            (Some(bank_id), None) => crate::api::fees::fee_on(conn, bank_id, on, amount).await?,
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
            "card" | "transfer" | "invoice" => {
                // Комиссия — на тот же счёт.
                if let Some(bank_id) = bank {
                    let money = CashEntry {
                        account_id: bank_id,
                        kind: movement_kind,
                        amount,
                        doc_type: "sale",
                        doc_id: Some(sale_id),
                        comment: "",
                    };
                    let fee_entry = CashEntry {
                        account_id: bank_id,
                        kind: "bank_fee",
                        amount: -fee,
                        doc_type: "sale",
                        doc_id: Some(sale_id),
                        comment: "",
                    };
                    // Возврат: сначала возвращается комиссия, потом уходят деньги — со счёта
                    // уходит ровно то, что он получил за эту часть чека.
                    if sign < 0 && fee != 0 {
                        cash::add_movement(conn, ctx, fee_entry).await?;
                        cash::add_movement(conn, ctx, money).await?;
                    } else {
                        cash::add_movement(conn, ctx, money).await?;
                        if fee != 0 {
                            cash::add_movement(conn, ctx, fee_entry).await?;
                        }
                    }
                }
            }
            // Аккумулятор в оплату: денег нет, на складе — лом по весу этой стоимостью (SPEC-27).
            "battery" => {
                let grams = p
                    .grams
                    .ok_or_else(|| invalid("укажите вес аккумулятора"))?
                    .checked_mul(sign)
                    .ok_or_else(overflow)?;
                crate::api::batteries::tradein_movement(conn, branch_id, sale_id, grams, amount)
                    .await?;
            }
            _ => {}
        }
        let grams = if p.method == "battery" {
            p.grams.map(|g| g.saturating_mul(sign))
        } else {
            None
        };
        let account_id = bank;
        // Ссылка на QR банка — только у продажи (SPEC-31).
        let bank_qr_id = if sign > 0 { p.bank_qr_id } else { None };
        sqlx::query!(
            "insert into sale_payments (id, sale_id, method, amount_tyiyn, fee_tyiyn, grams, account_id, bank_qr_id) values ($1, $2, $3, $4, $5, $6, $7, $8)",
            new_id(),
            sale_id,
            p.method,
            amount,
            fee,
            grams,
            account_id,
            bank_qr_id
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
    // Тысячи строк одним запросом надолго заняли бы базу и кассу (ADR-090).
    if req.lines.len() > 300 || req.payments.len() > 20 {
        return Err(invalid("в чеке — не больше 300 строк и 20 оплат"));
    }
    // Аккумулятор в оплату: сумму по весу считает сервер (SPEC-27).
    let mut req = req;
    req.payments = price_battery(conn, ctx, &req.payments).await?;
    let battery_scrap = if req.payments.iter().any(|p| p.method == "battery") {
        Some(crate::api::batteries::scrap_product(conn).await?)
    } else {
        None
    };
    if req.declined.chars().count() > 300 {
        return Err(invalid("отказанные работы — не длиннее 300 знаков"));
    }
    if req.delivery_address.chars().count() > 300 {
        return Err(invalid("адрес доставки не длиннее 300 знаков"));
    }
    // Чек без сети — признак `offline` ставит только отправка очереди устройства, вместе со
    // временем чека (ADR-093). Чек мог уйти из очереди и через секунды после продажи, поэтому
    // давность не проверяется. Признак только смягчает проверки, а не даёт прав: всё, что
    // онлайн-продажа отклонила бы, уходит владельцу (`sale.offline_bypass`), права на счета и
    // скрытые от администратора данные проверяются так же, как онлайн.
    let offline = req.offline && req.client_time.is_some();
    // Что чек без сети обошёл из проверок онлайн-продажи — владельцу одним уведомлением (ADR-062).
    let mut bypass: Vec<String> = Vec::new();
    if offline
        && let Some(t) = req.client_time
        && (t < Utc::now() - chrono::Duration::days(2)
            || t > Utc::now() + chrono::Duration::minutes(5))
    {
        // Часы устройства сбиты (или оно давно не включалось): день чека — день проведения (SPEC-08).
        // Бишкек — UTC+6 круглый год.
        let shown = chrono::FixedOffset::east_opt(6 * 3600).map_or_else(
            || t.format("%d.%m.%Y %H:%M UTC").to_string(),
            |tz| t.with_timezone(&tz).format("%d.%m.%Y %H:%M").to_string(),
        );
        bypass.push(format!(
            "время на устройстве {shown} расходится с сервером больше чем на 2 дня — проверьте часы кассы; чек записан днём проведения"
        ));
    }
    // Чек другого пользователя этой кассы владелец отправляет от себя (SPEC-09): сервер
    // проводит его от владельца, а в журнал пишет, чей он был.
    let claimed_user = match req.claimed_user_id {
        Some(u) if u != ctx.user.id => {
            if !ctx.user.is_owner() {
                return Err(AppError::Forbidden);
            }
            Some(
                sqlx::query_scalar!(
                    "select full_name from users where id = $1 and branch_id = $2",
                    u,
                    branch_id
                )
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| invalid("пользователь чека не найден"))?,
            )
        }
        _ => None,
    };
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
    let at_shift = if req.correction.is_some() {
        // Исправление: кассир — тот, кого указал владелец (обычно кассир исходного чека).
        None
    } else {
        cash::shift_open_at(
            conn,
            branch_id,
            register,
            if offline { req.client_time } else { None },
        )
        .await?
    };
    let cashier_id = at_shift
        .as_ref()
        .map_or(req.cashier_id, |s| s.cashier_employee_id);
    // Кассира смены могли отключить посреди смены — чек смены всё равно его. Исправление
    // сохраняет кассира исходного чека, даже если его уже отключили.
    check_employee(
        conn,
        branch_id,
        cashier_id,
        false,
        offline || at_shift.is_some() || req.correction.is_some(),
        &mut bypass,
    )
    .await?;

    let debt_total: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "debt")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    // Продажа в долг — владелец или администратор с правом (ADR-097). Чек без сети уже
    // состоялся: проводится, а владелец получает причину (инвариант 5).
    if debt_total > 0 && !crate::auth::has_permission(conn, &ctx.user, "sale_debt").await? {
        if !offline {
            return Err(crate::auth::no_permission("sale_debt"));
        }
        bypass.push("продажа в долг без права".into());
    }
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
        // «По счёту» платит юрлицо по выставленному счёту — без клиента счёт не выставить (ADR-095).
        None if req.payments.iter().any(|p| p.method == "invoice") => {
            return Err(invalid("По счёту платит клиент — выберите его"));
        }
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
    with_our_oil(conn, &mut prepared).await?;
    // Чек «в сервис» — с видом замены из справочника (ADR-096) и с данными клиента (ADR-100).
    // Чек без сети уже состоялся: проводится, а владелец получает причину.
    let service_kinds = prepared.iter().any(|p| p.service_id.is_some());
    if req.sale_type == "service" {
        if !service_kinds {
            if !offline {
                return Err(invalid("Выберите вид замены"));
            }
            bypass.push("чек «в сервис» без вида замены".into());
        }
        let missing = service_client_missing(conn, &req).await?;
        if !missing.is_empty() {
            if !offline {
                return Err(invalid(format!(
                    "Чек «в сервис» — нужны данные клиента: {}",
                    missing.join(", ")
                )));
            }
            bypass.push(format!(
                "в сервис без данных клиента: {}",
                missing.join(", ")
            ));
        }
    }
    check_discounts(
        conn,
        ctx,
        &prepared,
        claimed_user.as_ref().and(req.claimed_user_id),
        offline,
        &mut bypass,
    )
    .await?;
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
    // Карта, перевод и «По счёту» — на выбранный безналичный счёт или счёт по умолчанию для
    // способа (ADR-076, ADR-077, ADR-095). Чек без сети со счётом, который за это время убрали в
    // архив, ложится на счёт по умолчанию, владелец узнает.
    for p in req.payments.iter_mut() {
        p.account_id = if matches!(p.method.as_str(), "card" | "transfer" | "invoice") {
            Some(
                cash::sale_bank_account(
                    conn,
                    &ctx.user,
                    &p.method,
                    p.account_id,
                    offline,
                    &mut bypass,
                )
                .await?,
            )
        } else {
            None
        };
    }
    // Оплата по QR банка (SPEC-31): QR оплачен, сумма та же, не у другого чека.
    let bank_qrs =
        crate::api::bank_qr::check_for_sale(conn, ctx, &mut req.payments, offline, &mut bypass)
            .await?;
    // Мастеру — сумма ставок услуг чека, ставка фиксируется в строке (ADR-096). Ставка замены
    // за чек из настроек (ADR-027) новым чекам не начисляется: её получает только чек без сети
    // со старого устройства «в сервис» без вида замены, с товаром (одни услуги её не давали,
    // ADR-043).
    let has_goods = prepared.iter().any(|p| p.product_id.is_some() && !p.gift);
    let check_fee = if req.sale_type == "service" && !service_kinds && has_goods {
        oil_change_fee(conn, branch_id).await?
    } else {
        0
    };
    let master_fee = prepared
        .iter()
        .try_fold(check_fee, |acc, p| acc.checked_add(p.master_fee))
        .ok_or_else(overflow)?;

    // Лом аккумуляторов из оплаты блокируется вместе с товарами чека, в общем порядке.
    lock_products(
        conn,
        branch_id,
        prepared
            .iter()
            .filter_map(|p| p.product_id)
            .chain(battery_scrap),
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
                   coalesce($17::date,
                   case when $15::timestamptz is not null
                         and $15::timestamptz > now() - interval '2 days'
                         and $15::timestamptz < now() + interval '5 minutes'
                        then ($15::timestamptz at time zone 'Asia/Bishkek')::date
                        else (now() at time zone 'Asia/Bishkek')::date end))"#,
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
        req.delivery_address.trim(),
        req.correction.map(|c| c.business_date)
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
    crate::api::bank_qr::link_sale(conn, id, &bank_qrs).await?;
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
        let car_unit = crate::api::oil_book::vehicle_unit(conn, vehicle_id).await?;
        let mileage = match crate::api::oil_book::resolve_mileage(
            req.mileage,
            req.mileage_unit.as_deref(),
            req.mileage_km,
            &car_unit,
        ) {
            Ok(m) => m,
            // Чек без сети уже пробит: опечатка в пробеге не отменяет продажу (инвариант 10) —
            // замена записывается без пробега, владелец видит причину (ADR-090).
            Err(AppError::Validation(why)) if offline => {
                ops::audit(
                    conn,
                    ctx,
                    "sale.offline_bypass",
                    "sale",
                    Some(id),
                    json!({ "number": number, "reasons": [format!("пробег не записан в книжку: {why}")] }),
                )
                .await?;
                None
            }
            Err(e) => return Err(e),
        };
        crate::api::oil_book::record_from_sale(
            conn,
            ctx,
            id,
            party_id,
            vehicle_id,
            business_date,
            mileage,
            &req.declined,
        )
        .await?;
        // «Через сколько менять» из чека — интервал строки у этой машины (SPEC-29 §23).
        if req.book_intervals.len() > crate::api::oil_book::SLOTS.len() {
            return Err(invalid("интервалов больше, чем строк книжки"));
        }
        for b in &req.book_intervals {
            match crate::api::oil_book::set_interval_tx(
                conn,
                ctx,
                vehicle_id,
                &b.slot,
                Some((b.interval_km, b.interval_days)),
                "sale",
            )
            .await
            {
                Ok(_) => {}
                // Без сети — так же, как пробег: чек проводится, интервал остаётся прежним.
                Err(AppError::Validation(why)) if offline => {
                    ops::audit(
                        conn,
                        ctx,
                        "sale.offline_bypass",
                        "sale",
                        Some(id),
                        json!({ "number": number, "reasons": [format!("интервал замены не сохранён: {why}")] }),
                    )
                    .await?;
                }
                Err(e) => return Err(e),
            }
        }
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
    // Строки, удалённые из чека до проведения, — в «Отменённые товары» (SPEC-30 §8).
    crate::api::cancelled_lines::record(
        conn,
        ctx,
        crate::api::cancelled_lines::Target {
            op_id: req.op_id,
            sale_id: Some(id),
            shift: at_shift.as_ref().map(|s| (s.id, s.cashier_employee_id)),
        },
        crate::api::cancelled_lines::lines_of(&req.removed_lines),
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "sale",
        Some(id),
        json!({
            "number": number, "total": total, "offline": offline, "client_time": req.client_time,
            "claimed_user_id": claimed_user.as_ref().and(req.claimed_user_id),
            "claimed_user_name": claimed_user,
        }),
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
    /// Вернуть деньгами часть, оплаченную аккумулятором (лом уже продан): только владелец,
    /// явной отметкой (SPEC-27).
    #[serde(default)]
    pub battery_cash: bool,
    /// Учётный день возврата: «Исправить» сторнирует чек в его же день (SPEC-29 §24). С экрана
    /// не приходит — возврат ложится в сегодняшний день.
    #[serde(skip)]
    pub business_date: Option<NaiveDate>,
}

/// Счёт возврата на карту или переводом (ADR-076): деньги уходят с того безналичного счёта,
/// куда пришла исходная оплата этим способом (если счетов было несколько — с того, куда пришло
/// больше), иначе — с любого счёта исходного безнала. Чек был оплачен не безналом — выбранный
/// счёт или счёт по умолчанию для способа. Счёт исходной оплаты уже в архиве — по умолчанию.
async fn refund_account(
    conn: &mut PgConnection,
    user: &CurrentUser,
    sale_id: Uuid,
    p: &PaymentReq,
) -> AppResult<Uuid> {
    let orig = sqlx::query_scalar!(
        r#"select p.account_id as "account_id!" from sale_payments p
           join cash_accounts a on a.id = p.account_id
           where p.sale_id = $1 and p.method in ('card', 'transfer', 'invoice') and a.active
           group by p.account_id
           order by bool_or(p.method = $2) desc, sum(p.amount_tyiyn) desc, p.account_id
           limit 1"#,
        sale_id,
        p.method
    )
    .fetch_optional(&mut *conn)
    .await?;
    match orig {
        Some(id) => Ok(id),
        None => {
            let mut ignored = Vec::new();
            cash::sale_bank_account(conn, user, &p.method, p.account_id, false, &mut ignored).await
        }
    }
}

/// Куда вернуть безнал `p`: на счета исходной оплаты, каждому — не больше того, что он получил
/// по этому чеку за вычетом прежних возвратов. Сначала счёт, указанный кассой, если он был в
/// исходной оплате, затем счета того же способа, затем другого безналичного; что не легло —
/// по прежнему правилу [`refund_account`].
async fn refund_split(
    conn: &mut PgConnection,
    user: &CurrentUser,
    sale_id: Uuid,
    p: &PaymentReq,
) -> AppResult<Vec<(Uuid, i64)>> {
    let rows = sqlx::query!(
        r#"select p.account_id as "account_id!",
                  bool_or(p.method = $2 and s.id = $1) as "same!",
                  (coalesce(sum(p.amount_tyiyn) filter (where s.id = $1), 0)
                   + coalesce(sum(p.amount_tyiyn) filter (where s.id <> $1), 0))::bigint as "left!"
           from sale_payments p join sales s on s.id = p.sale_id
           join cash_accounts a on a.id = p.account_id
           where (s.id = $1 or s.reversal_of = $1) and p.method in ('card', 'transfer', 'invoice')
             and a.active
           group by p.account_id"#,
        sale_id,
        p.method
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut order: Vec<(bool, bool, i64, Uuid)> = rows
        .into_iter()
        .filter(|r| r.left > 0)
        .map(|r| {
            (
                Some(r.account_id) == p.account_id,
                r.same,
                r.left,
                r.account_id,
            )
        })
        .collect();
    order.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    let mut rest = p.amount_tyiyn;
    let mut out = Vec::new();
    for (_, _, left, account_id) in order {
        if rest <= 0 {
            break;
        }
        let take = rest.min(left);
        out.push((account_id, take));
        rest -= take;
    }
    if rest > 0 {
        let account_id = refund_account(conn, user, sale_id, p).await?;
        match out.iter_mut().find(|(a, _)| *a == account_id) {
            Some(x) => x.1 += rest,
            None => out.push((account_id, rest)),
        }
    }
    Ok(out)
}

/// Часть величины `part` для `qty` из `orig_qty`; последняя часть забирает точный остаток.
fn share(orig: i64, already: i64, qty: i64, orig_qty: i64, returned_qty: i64) -> AppResult<i64> {
    if returned_qty + qty == orig_qty {
        return Ok(orig - already);
    }
    div_round(i128::from(orig) * i128::from(qty), i128::from(orig_qty)).ok_or_else(overflow)
}

/// Номер строки удержания в возврате: строка услуги исходного чека плюс это число.
const WITHHELD_LINE_BASE: i32 = 1000;

/// Удержание за бесплатную замену в возврате (ADR-104): строка услуги исходного чека, сколько
/// услуг остаётся, цена услуги и сумма удержания.
struct Withhold {
    line_no: i32,
    service_id: Uuid,
    price: i64,
    amount: i64,
}

/// Что удержать из возврата `now` (строка исходного чека → количество): услуга «бесплатно с нашим
/// маслом» была бесплатной, этот возврат опускает масло её категории ниже канистры, а сама услуга
/// остаётся. Удерживается один раз на строку услуги — повторный возврат второй раз не удерживает.
async fn plan_withhold(
    conn: &mut PgConnection,
    sale_id: Uuid,
    now: &HashMap<i32, (i64, i64)>,
) -> AppResult<Vec<Withhold>> {
    let free = sqlx::query!(
        r#"select l.line_no, l.service_id as "service_id!", l.qty, sv.price_tyiyn,
                  sv.oil_category_id as "oil_category_id!"
           from sale_lines l join services sv on sv.id = l.service_id
           where l.sale_id = $1 and l.kind = 'service' and l.amount_tyiyn = 0
             and l.list_price_tyiyn = 0 and sv.free_with_oil and sv.oil_category_id is not null
             and sv.price_tyiyn > 0
             and not exists (select 1 from sale_lines w join sales r on r.id = w.sale_id
                             where r.reversal_of = $1 and w.withheld_for = l.line_no)"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if free.is_empty() {
        return Ok(Vec::new());
    }
    // Строки чека и сколько по ним уже вернули прежними возвратами.
    let lines = sqlx::query!(
        r#"select l.line_no, l.product_id, l.gift, l.qty, l.units,
                  coalesce((select sum(x.qty) from sale_lines x join sales r on r.id = x.sale_id
                            where r.reversal_of = $1 and x.line_no = l.line_no
                              and x.withheld_for is null), 0)::bigint as "qty_back!",
                  coalesce((select sum(x.units) from sale_lines x join sales r on r.id = x.sale_id
                            where r.reversal_of = $1 and x.line_no = l.line_no
                              and x.withheld_for is null), 0)::bigint as "units_back!"
           from sale_lines l where l.sale_id = $1"#,
        sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = lines.iter().filter_map(|l| l.product_id).collect();
    let info = oil_products(conn, &ids).await?;
    let mut out = Vec::new();
    for f in free {
        // Строки нашего масла категории услуги (не подарок).
        let cat_lines: Vec<_> = lines
            .iter()
            .filter(|l| !l.gift)
            .filter_map(|l| {
                let (cat, container) = info.get(&l.product_id?)?;
                (*cat == f.oil_category_id).then_some((l, *container))
            })
            .collect();
        let orig_oil: Vec<OilLine> = cat_lines
            .iter()
            .map(|(l, c)| OilLine {
                units_ml: l.units,
                container_ml: *c,
            })
            .collect();
        let Some(can) = canister_ml(&orig_oil) else {
            continue;
        };
        let before = cat_lines.iter().fold(0i64, |acc, (l, _)| {
            acc.saturating_add(l.units - l.units_back)
        });
        let back_now = cat_lines.iter().fold(0i64, |acc, (l, _)| {
            acc.saturating_add(now.get(&l.line_no).map_or(0, |x| x.1))
        });
        if back_now <= 0 {
            continue;
        }
        let svc = lines.iter().find(|l| l.line_no == f.line_no);
        let left = svc.map_or(0, |l| l.qty - l.qty_back) - now.get(&f.line_no).map_or(0, |x| x.0);
        let amount = withhold(before, before - back_now, can, left, f.price_tyiyn);
        if amount > 0 {
            out.push(Withhold {
                line_no: f.line_no,
                service_id: f.service_id,
                price: f.price_tyiyn,
                amount,
            });
        }
    }
    Ok(out)
}

pub async fn return_sale_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReturnReq,
) -> AppResult<SaleOut> {
    const KIND: &str = "sale.return";
    crate::auth::require_permission(conn, &ctx.user, "sale_return").await?;
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
        /// Сколько из возвращаемых единиц ушли без цены закупки (себестоимость 0).
        unknown_units: i64,
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
        let unknown_units = if o.cost_unknown {
            let all = sqlx::query_scalar!(
                "select unknown_units from payroll_cost_pending where sale_id = $1 and line_no = $2",
                id,
                rl.line_no
            )
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or(0);
            let already = div_round(i128::from(all) * i128::from(done.qty), i128::from(o.qty))
                .ok_or_else(overflow)?;
            share(all, already, rl.qty, o.qty, done.qty)?.clamp(0, units.max(0))
        } else {
            0
        };
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
                free_with_oil: false,
                oil_category: None,
            },
            line_no: rl.line_no,
            amount,
            cost,
            fee,
            cost_unknown: o.cost_unknown,
            orig_qty: o.qty,
            unknown_units,
        });
    }
    // Возврат масла сделал бесплатную замену платной — её цена удерживается из возврата
    // (ADR-104), но не больше суммы возврата.
    let now: HashMap<i32, (i64, i64)> = planned
        .iter()
        .map(|x| (x.line_no, (x.p.qty, x.p.units)))
        .collect();
    let mut withholds = plan_withhold(conn, id, &now).await?;
    let mut room = total;
    for w in withholds.iter_mut() {
        w.amount = w.amount.min(room);
        room -= w.amount;
    }
    withholds.retain(|w| w.amount > 0);
    let withheld = total - room;
    // Сколько отдаётся клиенту: сумма возвращённых строк за вычетом удержания.
    let refund = room;
    check_payments(&req.payments, refund)?;
    // Часть чека, оплаченная аккумулятором (SPEC-27), возвращается аккумулятором: вес уходит со
    // склада той же стоимостью, денег за него не выдают. Сколько веса — доля от веса чека по
    // сумме; последняя часть забирает точный остаток.
    let bat = sqlx::query!(
        r#"select coalesce(sum(p.amount_tyiyn) filter (where s.id = $1), 0)::bigint as "paid!",
                  coalesce(sum(p.grams) filter (where s.id = $1), 0)::bigint as "grams!",
                  coalesce(-sum(p.amount_tyiyn) filter (where s.id <> $1), 0)::bigint as "back!",
                  coalesce(-sum(p.grams) filter (where s.id <> $1), 0)::bigint as "grams_back!"
           from sale_payments p join sales s on s.id = p.sale_id
           where (s.id = $1 or s.reversal_of = $1) and p.method = 'battery'"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let bat_left = bat.paid.saturating_sub(bat.back).max(0);
    let battery_req: i64 = req
        .payments
        .iter()
        .filter(|p| p.method == "battery")
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)?;
    if battery_req > bat_left {
        return Err(invalid(format!(
            "аккумулятором по этому чеку можно вернуть не больше {}",
            crate::domain::money::format_som(bat_left)
        )));
    }
    let mut battery_cash = false;
    if bat_left > 0 {
        // Всё, что по чеку платили не аккумулятором, за вычетом прежних возвратов.
        let other_left = sqlx::query_scalar!(
            r#"select coalesce(sum(p.amount_tyiyn), 0)::bigint as "v!"
               from sale_payments p join sales s on s.id = p.sale_id
               where (s.id = $1 or s.reversal_of = $1) and p.method <> 'battery'"#,
            id
        )
        .fetch_one(&mut *conn)
        .await?;
        if refund.saturating_sub(battery_req) > other_left.max(0) {
            if !req.battery_cash {
                return Err(invalid(format!(
                    "часть чека оплачена аккумулятором: {} верните аккумулятором (вес уйдёт со склада). Деньгами за аккумулятор — только по решению владельца",
                    crate::domain::money::format_som(
                        refund.saturating_sub(other_left.max(0)).min(bat_left)
                    )
                )));
            }
            if !ctx.user.is_owner() {
                return Err(AppError::Forbidden);
            }
            battery_cash = true;
        }
    }
    let mut payments = Vec::with_capacity(req.payments.len());
    let mut battery_grams: i64 = 0;
    for p in &req.payments {
        let mut p = p.clone();
        p.grams = None;
        if matches!(p.method.as_str(), "card" | "transfer" | "invoice") {
            // Безнал возвращается на те счета, куда он пришёл, не больше их доли (ADR-076);
            // «По счёту» — с того же счёта (ADR-095).
            for (account_id, amount) in refund_split(conn, &ctx.user, id, &p).await? {
                let mut part = p.clone();
                part.account_id = Some(account_id);
                part.amount_tyiyn = amount;
                payments.push(part);
            }
            continue;
        }
        p.account_id = None;
        if p.method == "battery" {
            let g = if p.amount_tyiyn == bat_left {
                bat.grams.saturating_sub(bat.grams_back)
            } else {
                div_round(
                    i128::from(bat.grams) * i128::from(p.amount_tyiyn),
                    i128::from(bat.paid),
                )
                .ok_or_else(overflow)?
            };
            if g <= 0 {
                return Err(invalid("сумма аккумулятором слишком мала: нет даже грамма"));
            }
            battery_grams = g;
            p.grams = Some(g);
        }
        payments.push(p);
    }
    // Сколько по чеку ещё в долгу и сколько заплачено деньгами, за вычетом прежних возвратов.
    let left = sqlx::query!(
        r#"select coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
                  coalesce(sum(p.amount_tyiyn) filter (where p.method not in ('debt', 'bonus', 'battery')), 0)::bigint as "money!"
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
            } else if p.method == "bonus" || p.method == "battery" {
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
    // Ставка услуги (ADR-096) снимается вместе с возвращённой строкой услуги — её долей, как
    // сумма строки; возврат товара без услуги ставку не трогает: работу мастер сделал. Ставка за
    // чек у прежних чеков (ADR-027) — только при возврате чека целиком.
    let lines_fee_back = planned
        .iter()
        .try_fold(0i64, |acc, x| acc.checked_add(x.fee))
        .ok_or_else(overflow)?;
    let orig_lines_fee = sqlx::query_scalar!(
        r#"select coalesce(sum(master_fee_tyiyn), 0)::bigint as "v!" from sale_lines where sale_id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let check_fee_back = if full_return {
        orig.master_fee_tyiyn.saturating_sub(orig_lines_fee).max(0)
    } else {
        0
    };
    let fee_back = -lines_fee_back.saturating_add(check_fee_back);

    let scrap = if battery_grams > 0 {
        Some(crate::api::batteries::scrap_product(conn).await?)
    } else {
        None
    };
    let products: BTreeSet<Uuid> = planned.iter().filter_map(|x| x.p.product_id).collect();
    let ids: Vec<Uuid> = products.iter().copied().collect();
    lock_products(conn, branch_id, products.into_iter().chain(scrap)).await?;
    ops::ensure_not_merged(conn, &ids).await?;
    // Лом, принятый в оплату, уже продали — вес не вернуть: эту часть деньгами отдаёт владелец.
    if let Some(scrap) = scrap {
        let pool = ops::lock_pool(conn, branch_id, scrap).await?;
        // Возвращаемые строки чека могут сами быть ломом на вес: они вернутся на склад раньше.
        let coming: i64 = planned
            .iter()
            .filter(|x| x.p.product_id == Some(scrap))
            .fold(0i64, |acc, x| acc.saturating_add(x.p.units));
        if pool.qty.saturating_add(coming) < battery_grams {
            return Err(AppError::Conflict(format!(
                "лома аккумуляторов на складе {}, а вернуть нужно {}: его уже продали. Эту часть может вернуть деньгами только владелец",
                crate::domain::money::format_kg(pool.qty.max(0)),
                crate::domain::money::format_kg(battery_grams)
            )));
        }
    }
    let number = ops::next_counter(conn, branch_id, "sale").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into sales (id, branch_id, number, kind, sale_type, cashier_id, master_id, total_tyiyn,
                              master_fee_tyiyn, comment, party_id, contact_id, vehicle_id,
                              reversal_of, user_id, device_id, business_date)
           values ($1, $2, $3, 'return', $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
                   coalesce($16::date, (now() at time zone 'Asia/Bishkek')::date))"#,
        rid,
        branch_id,
        number,
        orig.sale_type,
        orig.cashier_id,
        orig.master_id,
        -refund,
        fee_back,
        req.comment.trim(),
        orig.party_id,
        orig.contact_id,
        orig.vehicle_id,
        id,
        ctx.user.id,
        ctx.device_id,
        req.business_date
    )
    .execute(&mut *conn)
    .await?;
    for x in &planned {
        if let Some(pid) = x.p.product_id {
            let pool = ops::lock_pool(conn, branch_id, pid).await?;
            // Единицы, проданные без цены закупки, вернулись, а цена уже есть (приход закрыл минус
            // с переоценкой или владелец её указал): они возвращаются по последней закупочной,
            // разница — обратной переоценкой. Иначе себестоимость этих единиц списана дважды:
            // переоценкой при приходе и снова при следующей продаже (ADR-092).
            if x.unknown_units > 0 && pool.last_qty > 0 && pool.qty.saturating_add(x.p.units) != 0 {
                let back = div_round(
                    i128::from(x.unknown_units) * i128::from(pool.last_cost),
                    i128::from(pool.last_qty),
                )
                .ok_or_else(overflow)?;
                if back > 0 {
                    ops::apply_movement(
                        conn,
                        Movement {
                            branch_id,
                            product_id: pid,
                            qty_delta: 0,
                            value_delta: back,
                            doc_type: "revaluation",
                            doc_id: rid,
                        },
                    )
                    .await?;
                }
            }
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
    // Удержание за замену — строкой услуги с плюсом: выручка услуги, как оплата замены (ADR-104).
    // Номер строки — за пределами строк чека (их не больше 300), количество 0: замену не вернули.
    for w in &withholds {
        sqlx::query!(
            r#"insert into sale_lines (id, sale_id, line_no, kind, service_id, qty, units,
                                       unit_price_tyiyn, list_price_tyiyn, amount_tyiyn, cost_tyiyn,
                                       master_fee_tyiyn, withheld_for, name)
               select $1, $2, $3, 'service', $4, 0, 0, $5, $5, $6, 0, 0, $7,
                      'Удержано за замену: ' || sv.name
               from services sv where sv.id = $4"#,
            new_id(),
            rid,
            WITHHELD_LINE_BASE + w.line_no,
            w.service_id,
            w.price,
            w.amount,
            w.line_no
        )
        .execute(&mut *conn)
        .await?;
    }
    // Комиссия банка возвращается из записанной в исходном чеке, а не по ставке (ADR-070), и по
    // каждому счёту отдельно: счёт без комиссии не получает чужую (ADR-077).
    let fb = sqlx::query!(
        r#"select p.account_id as "account_id!",
                  coalesce(sum(p.fee_tyiyn) filter (where s.id = $1), 0)::bigint as "fee!",
                  coalesce(sum(p.amount_tyiyn) filter (where s.id = $1), 0)::bigint as "base!",
                  coalesce(sum(-p.amount_tyiyn) filter (where s.id <> $1), 0)::bigint as "refunded!",
                  coalesce(sum(-p.fee_tyiyn) filter (where s.id <> $1), 0)::bigint as "fee_back!"
           from sale_payments p join sales s on s.id = p.sale_id
           where (s.id = $1 or s.reversal_of = $1) and p.method in ('card', 'transfer')
             and p.account_id is not null
           group by p.account_id"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let back: HashMap<Uuid, FeeBack> = fb
        .into_iter()
        .map(|r| {
            (
                r.account_id,
                FeeBack {
                    fee: r.fee,
                    base: r.base,
                    refunded: r.refunded,
                    fee_back: r.fee_back,
                },
            )
        })
        .collect();
    insert_payments(
        conn,
        ctx,
        rid,
        &payments,
        -1,
        Some(back),
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
            fee_back: -fee_back,
        },
    )
    .await?;
    if battery_cash {
        ops::audit(
            conn,
            ctx,
            "sale.return_battery_cash",
            "sale",
            Some(rid),
            json!({ "number": number, "original": id, "battery_left": bat_left, "refund": refund }),
        )
        .await?;
    }
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
        json!({ "original": id, "number": number, "total": -refund, "withheld": withheld }),
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

/// Строка возврата без чека (ADR-105): товар, количество в единицах строки (как в чеке:
/// штуки, канистры, мл розлива, граммы) и сумма строки, которую отдают клиенту.
#[derive(Deserialize)]
pub struct NoReceiptLineReq {
    pub kind: String,
    pub product_id: Uuid,
    pub qty: i64,
    pub amount_tyiyn: i64,
}

/// Возврат без чека (SPEC-30 §16): только владелец.
#[derive(Deserialize)]
pub struct NoReceiptReq {
    pub op_id: Uuid,
    pub lines: Vec<NoReceiptLineReq>,
    /// Как вернуть деньги: `cash` — из «Кассы» при открытой смене, `card`, `transfer`,
    /// `invoice` — со счёта.
    pub method: String,
    #[serde(default)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub party_id: Option<Uuid>,
    /// Кассир документа, если смена не открыта; при открытой — кассир смены.
    #[serde(default)]
    pub cashier_id: Option<Uuid>,
    /// Причина — обязательна.
    #[serde(default)]
    pub comment: String,
}

pub async fn no_receipt_return_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: NoReceiptReq,
) -> AppResult<SaleOut> {
    const KIND: &str = "sale.return_no_receipt";
    // Возврат мимо продаж — только владелец (ADR-105).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let reason = req.comment.trim();
    if reason.is_empty() {
        return Err(invalid("укажите причину возврата"));
    }
    if reason.chars().count() > 300 {
        return Err(invalid("причина — не длиннее 300 знаков"));
    }
    if req.lines.is_empty() {
        return Err(invalid("добавьте товар"));
    }
    if req.lines.len() > 100 {
        return Err(invalid("в возврате — не больше 100 строк"));
    }
    if !matches!(
        req.method.as_str(),
        "cash" | "card" | "transfer" | "invoice"
    ) {
        return Err(invalid(
            "как вернуть: наличными, на карту, по QR или со счёта",
        ));
    }
    if let Some(pid) = req.party_id {
        sqlx::query_scalar!(
            "select id from parties where id = $1 and branch_id = $2",
            pid,
            branch_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("клиент не найден"))?;
    }
    let register = cash::default_account(conn, branch_id).await?;
    let shift = cash::shift_open_at(conn, branch_id, register, None).await?;
    // Наличные из «Кассы» — только при открытой смене (ADR-065).
    if req.method == "cash" && shift.is_none() {
        return Err(AppError::Conflict(
            "возврат наличными — только при открытой смене: откройте смену или верните на карту"
                .into(),
        ));
    }
    let cashier_id = match (&shift, req.cashier_id) {
        (Some(s), _) => s.cashier_employee_id,
        (None, Some(c)) => {
            let mut ignored = Vec::new();
            check_employee(conn, branch_id, c, false, false, &mut ignored).await?;
            c
        }
        (None, None) => return Err(invalid("выберите кассира")),
    };
    let mut prepared = Vec::with_capacity(req.lines.len());
    let mut total: i64 = 0;
    for l in &req.lines {
        if l.qty <= 0 || l.qty > 1_000_000_000 {
            return Err(invalid("количество больше нуля"));
        }
        if l.amount_tyiyn < 0 {
            return Err(invalid("сумма строки не отрицательна"));
        }
        ops::check_amount(l.amount_tyiyn)?;
        let pid = ops::live_product(conn, l.product_id).await?;
        let p = sqlx::query!(
            r#"select p.unit, p.container_ml, coalesce(bp.sale_price_tyiyn, 0) as "sale_price!"
               from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               where p.id = $1"#,
            pid,
            branch_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("товар не найден"))?;
        let (units, list_price) = match (l.kind.as_str(), p.unit.as_str(), p.container_ml) {
            ("piece", "piece", _) => (l.qty, p.sale_price),
            ("container", "ml", Some(c)) => (mul(l.qty, c).ok_or_else(overflow)?, p.sale_price),
            ("pour", "ml", Some(c)) => (
                l.qty,
                crate::domain::money::canister_pour_price(p.sale_price, c).unwrap_or(0),
            ),
            ("weight", "g", _) => (l.qty, p.sale_price),
            _ => return Err(invalid("вид строки не подходит товару")),
        };
        // Цена за единицу строки — для печати: у розлива и веса — за литр и килограмм.
        let per = if matches!(l.kind.as_str(), "pour" | "weight") {
            1000
        } else {
            1
        };
        let unit_price = div_round(
            i128::from(l.amount_tyiyn) * i128::from(per),
            i128::from(l.qty),
        )
        .ok_or_else(overflow)?;
        total = total.checked_add(l.amount_tyiyn).ok_or_else(overflow)?;
        prepared.push(Prepared {
            gift: false,
            kind: l.kind.clone(),
            product_id: Some(pid),
            service_id: None,
            qty: l.qty,
            units,
            unit_price,
            list_price,
            amount: l.amount_tyiyn,
            master_fee: 0,
            seen_list: None,
            free_with_oil: false,
            oil_category: None,
        });
    }
    if total <= 0 {
        return Err(invalid("сумма возврата больше нуля"));
    }
    ops::check_amount(total)?;
    let account_id = if req.method == "cash" {
        None
    } else {
        let mut ignored = Vec::new();
        Some(
            cash::sale_bank_account(
                conn,
                &ctx.user,
                &req.method,
                req.account_id,
                false,
                &mut ignored,
            )
            .await?,
        )
    };
    let ids: Vec<Uuid> = prepared.iter().filter_map(|p| p.product_id).collect();
    lock_products(conn, branch_id, ids.iter().copied()).await?;
    ops::ensure_not_merged(conn, &ids).await?;
    let number = ops::next_counter(conn, branch_id, "sale").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into sales (id, branch_id, number, kind, sale_type, cashier_id, total_tyiyn,
                              comment, party_id, no_receipt, user_id, device_id, business_date)
           values ($1, $2, $3, 'return', 'takeaway', $4, $5, $6, $7, true, $8, $9,
                   (now() at time zone 'Asia/Bishkek')::date)"#,
        rid,
        branch_id,
        number,
        cashier_id,
        -total,
        reason,
        req.party_id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    let mut names = Vec::new();
    for (i, p) in prepared.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        let mut value = 0;
        if let Some(pid) = p.product_id {
            // Товар возвращается по текущей средней себестоимости (ADR-105).
            let pool = ops::lock_pool(conn, branch_id, pid).await?;
            value = crate::domain::costing::return_value(p.units, &pool).ok_or_else(overflow)?;
            ops::apply_movement(
                conn,
                Movement {
                    branch_id,
                    product_id: pid,
                    qty_delta: p.units,
                    value_delta: value,
                    doc_type: "sale_return",
                    doc_id: rid,
                },
            )
            .await?;
        }
        insert_line(
            conn, rid, line_no, p, -p.amount, -value, 0, p.units, p.qty, false,
        )
        .await?;
        names.push(
            sqlx::query_scalar!(
                r#"select name as "n!" from sale_lines where sale_id = $1 and line_no = $2"#,
                rid,
                line_no
            )
            .fetch_one(&mut *conn)
            .await?,
        );
    }
    let business_date = sqlx::query_scalar!(
        r#"select business_date as "d!" from sales where id = $1"#,
        rid
    )
    .fetch_one(&mut *conn)
    .await?;
    let payments = [PaymentReq {
        method: req.method.clone(),
        amount_tyiyn: total,
        grams: None,
        price_per_kg_tyiyn: None,
        account_id,
        bank_qr_id: None,
    }];
    // Исходного чека нет — комиссии банка возвращать нечего.
    insert_payments(
        conn,
        ctx,
        rid,
        &payments,
        -1,
        Some(HashMap::new()),
        business_date,
    )
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "sale",
        Some(rid),
        json!({ "number": number, "total": -total, "method": req.method, "reason": reason, "names": names }),
    )
    .await?;
    let out = load_sale(conn, &ctx.user, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn no_receipt_return(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<NoReceiptReq>,
) -> AppResult<Json<SaleOut>> {
    let mut tx = state.pool.begin().await?;
    let out = no_receipt_return_tx(&mut tx, &ctx, req).await?;
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
    /// Возврат без чека (ADR-105).
    pub(crate) no_receipt: bool,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
pub(crate) struct DayTotals {
    pub(crate) count: i64,
    pub(crate) total_tyiyn: i64,
    pub(crate) cash_tyiyn: i64,
    pub(crate) card_tyiyn: i64,
    pub(crate) transfer_tyiyn: i64,
    /// «По счёту» (ADR-095).
    pub(crate) invoice_tyiyn: i64,
    pub(crate) debt_tyiyn: i64,
    pub(crate) bonus_tyiyn: i64,
    /// Принято аккумуляторами в оплату (SPEC-27).
    pub(crate) battery_tyiyn: i64,
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
                  s.total_tyiyn, s.reversal_of, s.no_receipt, s.created_at
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
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'invoice'), 0)::bigint as "invoice!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'bonus'), 0)::bigint as "bonus!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'battery'), 0)::bigint as "battery!"
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
        invoice_tyiyn: t.invoice,
        debt_tyiyn: t.debt,
        bonus_tyiyn: t.bonus,
        battery_tyiyn: t.battery,
    };
    Ok(DayOut {
        date,
        sales,
        totals,
    })
}
