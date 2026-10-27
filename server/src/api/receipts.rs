//! Приход, сторно прихода и остатки (SPEC-03).

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductFilter, ProductOut, query_products};
use crate::api::paging::{MAX_ROWS, PageQuery, Paged, paged};
use crate::auth::{Ctx, CurrentUser, Owner};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/receipts", routing::get(list_receipts).post(post_receipt))
        .route("/receipts/{id}", routing::get(get_receipt))
        .route("/receipts/{id}/reverse", routing::post(reverse_receipt))
        .route("/products/{id}/last-receipt", routing::get(last_receipt))
        .route("/stock", routing::get(stock))
        .route("/stock/verify", routing::get(verify_stock))
        .route("/stock/stale", routing::get(stale_stock))
        .route(
            "/settings/stock",
            routing::get(get_stock_settings).put(put_stock_settings),
        )
        .route(
            "/stock/{product_id}/review-done",
            routing::post(review_done),
        )
        .route(
            "/stock/{product_id}/purchase-price",
            routing::post(purchase_price),
        )
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReceiptLineReq {
    pub product_id: Uuid,
    pub qty: i64,
    pub cost_tyiyn: i64,
    /// Новая цена продажи товара — за штуку, канистру (тару) или килограмм, как цена товара
    /// (SPEC-28 §1). Пусто — цена не меняется. Ставят владелец и администратор (ADR-075).
    #[serde(default)]
    pub sale_price_tyiyn: Option<i64>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReceiptReq {
    pub op_id: Uuid,
    pub supplier_id: Option<Uuid>,
    /// `paid` — рассчитались сразу, `debt` — остались должны поставщику (SPEC-10).
    pub payment: Option<String>,
    #[serde(default)]
    pub supplier_doc: String,
    #[serde(default)]
    pub comment: String,
    pub lines: Vec<ReceiptLineReq>,
    /// Откуда оплачена накладная `paid` или её часть (ADR-062): id кассы или счёта. «Из своих,
    /// мимо касс» (`outside`) больше не принимается — 422 (ADR-079). Без поля (старый клиент):
    /// администратор платит из «Кассы» — другого ему нельзя, владельцу нужно выбрать явно.
    #[serde(default)]
    pub paid_from: Option<String>,
    /// Накладная в долг, часть оплачена сразу (SPEC-29 §4): сколько заплатили сейчас из
    /// `paid_from`; остаток — долг поставщику. Больше нуля и меньше суммы накладной.
    #[serde(default)]
    pub paid_now_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct ReceiptLineOut {
    pub line_no: i32,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    pub cost_tyiyn: i64,
    /// Цена продажи товара после этого прихода; пусто — приход до SPEC-28.
    #[serde(default)]
    pub sale_price_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct ReceiptOut {
    pub id: Uuid,
    pub number: i64,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    pub supplier_doc: String,
    pub comment: String,
    pub total_tyiyn: i64,
    pub reversal_of: Option<Uuid>,
    pub reversed_by: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub lines: Vec<ReceiptLineOut>,
    /// Откуда оплачена: название кассы или `outside`; пусто — в долг или накладная до ADR-062.
    #[serde(default)]
    pub paid_from: Option<String>,
    /// Накладная в долг: оплачено, осталось, статус и платежи (SPEC-27, ADR-073).
    #[serde(default)]
    pub debt: Option<crate::api::supplier_debts::ReceiptDebt>,
}

pub(crate) async fn load_receipt(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<ReceiptOut> {
    let h = sqlx::query!(
        r#"select r.id, r.number, r.supplier_id, s.name as "supplier_name?", r.supplier_doc, r.comment,
                  r.total_tyiyn, r.reversal_of, u.full_name as user_name, r.created_at,
                  (select x.id from receipts x where x.reversal_of = r.id) as reversed_by,
                  case when r.paid_from = 'outside' then 'outside' else a.name end as "paid_from?"
           from receipts r
           left join cash_accounts a on a.id = r.paid_account_id
           join users u on u.id = r.user_id
           left join suppliers s on s.id = r.supplier_id
           where r.id = $1 and r.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let lines = sqlx::query_as!(
        ReceiptLineOut,
        r#"select l.line_no, l.product_id, p.name as product_name, p.unit, p.container_ml, l.qty, l.cost_tyiyn,
                  l.sale_price_tyiyn
           from receipt_lines l join products p on p.id = l.product_id
           where l.receipt_id = $1 order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let debt = crate::api::supplier_debts::receipt_debt(&mut *conn, branch_id, id).await?;
    Ok(ReceiptOut {
        id: h.id,
        number: h.number,
        supplier_id: h.supplier_id,
        supplier_name: h.supplier_name,
        supplier_doc: h.supplier_doc,
        comment: h.comment,
        total_tyiyn: h.total_tyiyn,
        reversal_of: h.reversal_of,
        reversed_by: h.reversed_by,
        user_name: h.user_name,
        created_at: h.created_at,
        lines,
        paid_from: h.paid_from,
        debt,
    })
}

/// Блокирует строки остатков в порядке идентификаторов, чтобы параллельные документы не взаимоблокировались.
/// Срок, после которого товар без продаж считается залежалым (ADR-039). По умолчанию 60 дней.
pub async fn stale_days(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i32> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'stock'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| v.get("stale_days").and_then(serde_json::Value::as_i64))
            .and_then(|d| i32::try_from(d).ok())
            .filter(|d| *d > 0)
            .unwrap_or(60),
    )
}

#[derive(Serialize)]
pub struct StaleRow {
    pub product_id: Uuid,
    pub name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub stock_qty: i64,
    /// Последняя продажа, а если продаж не было — первый приход.
    pub since: chrono::DateTime<chrono::Utc>,
    pub days: i32,
    pub sold_ever: bool,
}

/// Залежалые товары: остаток есть, а продаж нет дольше срока (ADR-039).
pub async fn stale_list(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<(i32, Vec<StaleRow>)> {
    let days = stale_days(conn, branch_id).await?;
    let rows = sqlx::query!(
        r#"select p.id, p.name, p.unit, p.container_ml, bp.stock_qty,
                  x.last_sale, x.first_receipt
           from branch_products bp
           join products p on p.id = bp.product_id
           cross join lateral (
             select (select max(m.created_at) from stock_movements m
                      where m.branch_id = bp.branch_id and m.product_id = bp.product_id
                        and m.doc_type = 'sale') as last_sale,
                    (select min(m.created_at) from stock_movements m
                      where m.branch_id = bp.branch_id and m.product_id = bp.product_id
                        and m.doc_type = 'receipt') as first_receipt
           ) x
           where bp.branch_id = $1 and bp.stock_qty > 0 and not p.archived
             and coalesce(x.last_sale, x.first_receipt) < now() - make_interval(days => $2)
           order by coalesce(x.last_sale, x.first_receipt)"#,
        branch_id,
        days
    )
    .fetch_all(&mut *conn)
    .await?;
    let now = chrono::Utc::now();
    let out = rows
        .into_iter()
        .filter_map(|r| {
            let since = r.last_sale.or(r.first_receipt)?;
            Some(StaleRow {
                product_id: r.id,
                name: r.name,
                unit: r.unit,
                container_ml: r.container_ml,
                stock_qty: r.stock_qty,
                since,
                days: i32::try_from((now - since).num_days()).unwrap_or(i32::MAX),
                sold_ever: r.last_sale.is_some(),
            })
        })
        .collect();
    Ok((days, out))
}

#[derive(Serialize)]
struct StaleOut {
    stale_days: i32,
    items: Vec<StaleRow>,
}

async fn stale_stock(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<StaleOut>> {
    let mut conn = state.pool.acquire().await?;
    let (stale_days, items) = stale_list(&mut conn, user.branch_id).await?;
    Ok(Json(StaleOut { stale_days, items }))
}

#[derive(Serialize, Deserialize)]
struct StockSettings {
    stale_days: i32,
    oil_norm_bp: i32,
}

/// Что поменять: пустое поле остаётся как было.
#[derive(Deserialize)]
struct StockSettingsReq {
    stale_days: Option<i32>,
    oil_norm_bp: Option<i32>,
}

/// Норма расхождения по маслу на ревизии, в сотых долях процента (вопрос 32, ADR-052). По умолчанию 0,5 %.
pub async fn oil_norm_bp(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i32> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'stock'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| v.get("oil_norm_bp").and_then(serde_json::Value::as_i64))
            .and_then(|d| i32::try_from(d).ok())
            .filter(|d| (0..=10_000).contains(d))
            .unwrap_or(50),
    )
}

async fn get_stock_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<StockSettings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(StockSettings {
        stale_days: stale_days(&mut conn, user.branch_id).await?,
        oil_norm_bp: oil_norm_bp(&mut conn, user.branch_id).await?,
    }))
}

/// Срок залежалости и норму по маслу правит владелец (ADR-039, ADR-052).
async fn put_stock_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<StockSettingsReq>,
) -> AppResult<Json<StockSettings>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    let stale = match req.stale_days {
        Some(d) if !(1..=3650).contains(&d) => return Err(invalid("срок от 1 до 3650 дней")),
        Some(d) => d,
        None => stale_days(&mut tx, ctx.user.branch_id).await?,
    };
    let norm = match req.oil_norm_bp {
        Some(n) if !(0..=1000).contains(&n) => return Err(invalid("норма от 0 до 10 %")),
        Some(n) => n,
        None => oil_norm_bp(&mut tx, ctx.user.branch_id).await?,
    };
    let value = serde_json::json!({ "stale_days": stale, "oil_norm_bp": norm });
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'stock', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(&mut tx, &ctx, "settings.stock", "settings", None, value).await?;
    tx.commit().await?;
    Ok(Json(StockSettings {
        stale_days: stale,
        oil_norm_bp: norm,
    }))
}

pub async fn lock_products(
    conn: &mut PgConnection,
    branch_id: Uuid,
    ids: impl IntoIterator<Item = Uuid>,
) -> AppResult<()> {
    let sorted: BTreeSet<Uuid> = ids.into_iter().collect();
    for id in sorted {
        ops::lock_pool(conn, branch_id, id).await?;
    }
    Ok(())
}

/// Ответ на «из своих, мимо касс» в новых операциях (ADR-079): так больше не платят — деньги
/// сначала вносят на счёт, потом платят с него, и остаток каждого счёта сходится с деньгами.
pub const NO_OUTSIDE: &str = "«из своих, мимо касс» больше нет: выберите кассу или счёт — если платили свои деньги, сначала внесите их на счёт";

/// Счёт, из которого платят за накладную: касса или счёт филиала. Администратор платит
/// только из «Кассы» (как и выдаёт из неё деньги, SPEC-05); счёт и сейфы — владелец.
async fn resolve_paid_from(
    conn: &mut PgConnection,
    ctx: &Ctx,
    paid_from: Option<&str>,
) -> AppResult<Uuid> {
    let branch_id = ctx.user.branch_id;
    let owner = ctx.user.is_owner();
    let id = match paid_from.map(str::trim) {
        // Старый клиент поле не шлёт. Администратору выбирать не из чего — «Касса»; владелец
        // мог платить откуда угодно, и угадывать нельзя: остаток кассы разошёлся бы с деньгами.
        None | Some("") if owner => {
            return Err(invalid(
                "укажите, откуда оплачена накладная: касса, счёт или сейф",
            ));
        }
        None | Some("") => crate::api::cash::default_account(conn, branch_id).await?,
        Some("outside") => return Err(invalid(NO_OUTSIDE)),
        Some(s) => Uuid::parse_str(s).map_err(|_| invalid("неизвестная касса"))?,
    };
    // Администратор — только из кассы смены, архивный счёт не выбирается (ADR-076).
    crate::api::cash::check_pay_account(conn, &ctx.user, id).await?;
    Ok(id)
}

pub async fn post_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ReceiptReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.post";
    // Приход и оплата поставщику — право администратора `receipts` (ADR-097).
    crate::auth::require_permission(&mut *conn, &ctx.user, "receipts").await?;
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if req.lines.is_empty() {
        return Err(invalid("в приходе нет строк"));
    }
    if req.lines.len() > 500 {
        return Err(invalid(
            "в приходе — не больше 500 строк: разбейте накладную",
        ));
    }
    let mut total: i64 = 0;
    for l in &req.lines {
        if l.qty <= 0 || l.cost_tyiyn < 0 {
            return Err(invalid("количество больше нуля, сумма не отрицательна"));
        }
        if l.qty > 1_000_000_000 {
            return Err(invalid("слишком большое количество — проверьте ввод"));
        }
        ops::check_amount(l.cost_tyiyn)?;
        if let Some(p) = l.sale_price_tyiyn {
            if p < 0 {
                return Err(invalid("цена продажи не отрицательна"));
            }
            ops::check_amount(p)?;
        }
        total = total.checked_add(l.cost_tyiyn).ok_or_else(overflow)?;
    }
    // Одна цена продажи на товар: две строки одного товара с разными новыми ценами — ошибка ввода.
    let mut prices: std::collections::BTreeMap<Uuid, i64> = std::collections::BTreeMap::new();
    for l in &req.lines {
        if let Some(p) = l.sale_price_tyiyn
            && *prices.entry(l.product_id).or_insert(p) != p
        {
            return Err(invalid(
                "у одного товара в приходе две разные цены продажи — оставьте одну",
            ));
        }
    }
    let ids: Vec<Uuid> = req.lines.iter().map(|l| l.product_id).collect();
    let known = sqlx::query_scalar!(
        "select count(*) as \"n!\" from products where id = any($1) and not archived",
        &ids
    )
    .fetch_one(&mut *conn)
    .await?;
    let distinct: BTreeSet<Uuid> = ids.iter().copied().collect();
    if usize::try_from(known).ok() != Some(distinct.len()) {
        return Err(invalid(
            "товар не найден или в архиве: если карточку объединили, примите на основную",
        ));
    }
    if let Some(sid) = req.supplier_id {
        sqlx::query_scalar!("select id from suppliers where id = $1", sid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("поставщик не найден"))?;
    }
    let on_debt = match req.payment.as_deref() {
        None | Some("paid") => false,
        Some("debt") => true,
        Some(_) => return Err(invalid("расчёт: paid или debt")),
    };
    if on_debt && req.supplier_id.is_none() {
        return Err(invalid("для накладной в долг укажите поставщика"));
    }
    // Часть сразу, остаток в долг (SPEC-29 §4): только у накладной в долг и меньше её суммы.
    let paid_now = match req.paid_now_tyiyn {
        None | Some(0) => None,
        Some(_) if !on_debt => {
            return Err(invalid(
                "часть суммы платят только у накладной в долг: оплаченная сразу оплачивается целиком",
            ));
        }
        Some(p) if p < 0 => return Err(invalid("оплачено сейчас — не меньше нуля")),
        Some(p) if p >= total => {
            return Err(invalid(
                "оплачено сейчас не меньше суммы накладной — отметьте её оплаченной сразу",
            ));
        }
        Some(p) => Some(p),
    };
    let paid_from = if on_debt && paid_now.is_none() {
        if req.paid_from.is_some() {
            return Err(invalid("накладная в долг ничем не оплачивается"));
        }
        None
    } else {
        Some(resolve_paid_from(conn, ctx, req.paid_from.as_deref()).await?)
    };
    let branch_id = ctx.user.branch_id;
    lock_products(conn, branch_id, distinct).await?;
    // Пока ждали блокировку, карточку могли объединить с другой.
    let archived = sqlx::query_scalar!(
        r#"select count(*) as "n!" from products where id = any($1) and archived"#,
        &ids
    )
    .fetch_one(&mut *conn)
    .await?;
    if archived > 0 {
        return Err(AppError::Conflict(
            "товар только что объединили с другой карточкой — проведите приход ещё раз".into(),
        ));
    }
    let number = ops::next_counter(conn, branch_id, "receipt").await?;
    let id = new_id();
    // Частично оплаченная — накладная в долг: откуда платили часть, видно в её платежах.
    let (paid_kind, paid_account) = match paid_from {
        Some(a) if !on_debt => (Some("account"), Some(a)),
        _ => (None, None),
    };
    sqlx::query!(
        r#"insert into receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn, user_id, device_id,
                                 paid_from, paid_account_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        id,
        branch_id,
        number,
        req.supplier_id,
        req.supplier_doc.trim(),
        req.comment.trim(),
        total,
        ctx.user.id,
        ctx.device_id,
        paid_kind,
        paid_account
    )
    .execute(&mut *conn)
    .await?;
    // Новые цены продажи — до строк: строка запоминает цену, с которой товар вышел из прихода,
    // а проверка «закупка дороже цены» ниже видит уже новую цену (SPEC-28 §1, ADR-075).
    for (product_id, sale) in &prices {
        // Наценка — к цене по накладной первой строки товара, за единицу цены товара (канистру,
        // штуку, килограмм): только для журнала и ленты владельца.
        let unit = sqlx::query!(
            "select unit, container_ml from products where id = $1",
            product_id
        )
        .fetch_one(&mut *conn)
        .await?;
        let per = crate::api::catalog::price_unit(&unit.unit, unit.container_ml);
        let markup = req
            .lines
            .iter()
            .find(|l| l.product_id == *product_id)
            .and_then(|l| {
                crate::domain::money::div_round(
                    i128::from(l.cost_tyiyn) * i128::from(per),
                    i128::from(l.qty),
                )
            })
            .and_then(|cost| crate::domain::money::markup_permille(cost, *sale));
        crate::api::catalog::set_price_from_receipt(
            conn,
            ctx,
            *product_id,
            *sale,
            crate::api::catalog::PriceDoc::Receipt(id, number),
            markup,
        )
        .await?;
    }
    for (i, l) in req.lines.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            r#"insert into receipt_lines (id, receipt_id, line_no, product_id, qty, cost_tyiyn, sale_price_tyiyn)
               values ($1, $2, $3, $4, $5, $6,
                       (select sale_price_tyiyn from branch_products where branch_id = $7 and product_id = $4))"#,
            new_id(),
            id,
            line_no,
            l.product_id,
            l.qty,
            l.cost_tyiyn,
            branch_id
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: l.product_id,
                qty_delta: l.qty,
                value_delta: l.cost_tyiyn,
                doc_type: "receipt",
                doc_id: id,
            },
        )
        .await?;
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            l.product_id,
            l.qty,
            l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
    }
    // Дешевле закупки продавать нельзя (ADR-042): если новая закупка дороже цены продажи,
    // кассир этот товар не продаст, пока владелец не поднимет цену. Он узнаёт об этом сразу.
    let mut dearer = Vec::new();
    for l in &req.lines {
        let p = sqlx::query!(
            r#"select p.name, p.unit, p.container_ml, coalesce(bp.sale_price_tyiyn, 0) as "sale!"
               from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               where p.id = $1"#,
            l.product_id,
            branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        let qty = i128::from(l.qty);
        let cost = i128::from(l.cost_tyiyn);
        // Сравнение без деления: цена × количество против стоимости в тех же единицах.
        // Розлив за литр считается от цены канистры (ADR-074) — отдельной проверки ему не нужно.
        let dearer_price = match (p.unit.as_str(), p.container_ml) {
            ("ml", Some(c)) => i128::from(p.sale) * qty < cost * i128::from(c),
            // На вес: количество в граммах, цена продажи за кг (SPEC-36 §13).
            ("g", _) => i128::from(p.sale) * qty < cost * 1000,
            _ => i128::from(p.sale) * qty < cost,
        };
        if dearer_price && !dearer.contains(&p.name) {
            dearer.push(p.name);
        }
    }
    if !dearer.is_empty() {
        ops::audit(
            conn,
            ctx,
            "receipt.cost_above_price",
            "receipt",
            Some(id),
            json!({ "number": number, "names": dearer }),
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(id),
        json!({ "number": number, "total": total }),
    )
    .await?;
    if on_debt {
        let sid = req.supplier_id.ok_or_else(|| invalid("нужен поставщик"))?;
        // Мы должны поставщику: его баланс уходит в минус (SPEC-10).
        crate::api::parties::add_ledger(
            conn,
            ctx,
            crate::api::parties::LedgerEntry {
                party_id: sid,
                kind: "debt",
                amount: -total,
                doc_type: "receipt",
                doc_id: Some(id),
                comment: "",
            },
        )
        .await?;
    }
    // Оплачена сразу: деньги уходят из выбранной кассы в той же транзакции (SPEC-10). Это не
    // расход в прибыли — товар лёг в стоимость остатка и уйдёт в себестоимость при продаже.
    match (paid_from, paid_now) {
        (Some(account), Some(part)) => {
            // Часть сразу — оплата этой накладной той же функцией, что окно оплаты (ADR-073):
            // ложится на неё, деньги уходят с выбранного счёта, остаток — долг (SPEC-29 §4).
            let sid = req.supplier_id.ok_or_else(|| invalid("нужен поставщик"))?;
            let kind = crate::api::cash::check_pay_account(conn, &ctx.user, account).await?;
            crate::api::parties::post_repayment_tx(
                conn,
                ctx,
                crate::api::parties::RepaymentReq {
                    op_id: new_id(),
                    party_id: sid,
                    amount_tyiyn: part,
                    method: Some(if kind == "bank" { "transfer" } else { "cash" }.into()),
                    comment: format!("накладная № {number}, оплачено при приходе"),
                    receipt_id: Some(id),
                    account_id: Some(account),
                },
            )
            .await?;
            // Владелец узнаёт и об оплате части сразу — как о полной оплате (ADR-092).
            let name = sqlx::query_scalar!("select name from cash_accounts where id = $1", account)
                .fetch_one(&mut *conn)
                .await?;
            ops::audit(
                conn,
                ctx,
                "receipt.supplier_payment",
                "receipt",
                Some(id),
                json!({
                    "number": number,
                    "amount": part,
                    "account": name,
                    "partial": true,
                    "debt": total.saturating_sub(part),
                }),
            )
            .await?;
        }
        (Some(account), None) => pay_supplier(conn, ctx, id, number, total, account).await?,
        _ => {}
    }
    let out = load_receipt(conn, branch_id, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Оплата накладной: `supplier_payment` из кассы или со счёта (ADR-062).
async fn pay_supplier(
    conn: &mut PgConnection,
    ctx: &Ctx,
    receipt_id: Uuid,
    number: i64,
    total: i64,
    account_id: Uuid,
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    // Касса с наличными — только в открытой смене: иначе расход не с чем сверить (SPEC-10).
    crate::api::cash::require_open_shift(conn, branch_id, account_id).await?;
    let acc = sqlx::query!(
        "select name, balance_tyiyn, allow_negative from cash_accounts where id = $1 and branch_id = $2 for update",
        account_id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    // Минус по факту разрешён признаком счёта (ADR-077): тогда касса уходит ниже нуля.
    if acc.balance_tyiyn < total && !acc.allow_negative {
        return Err(AppError::Conflict(format!(
            "в «{}» не хватает денег на оплату накладной: оформите её в долг или выберите другую кассу",
            acc.name
        )));
    }
    if total > 0 {
        crate::api::cash::add_movement(
            conn,
            ctx,
            crate::api::cash::CashEntry {
                account_id,
                kind: "supplier_payment",
                amount: -total,
                doc_type: "receipt",
                doc_id: Some(receipt_id),
                comment: &format!("накладная № {number}"),
            },
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        "receipt.supplier_payment",
        "receipt",
        Some(receipt_id),
        json!({
            "number": number,
            "amount": total,
            "account": acc.name,
        }),
    )
    .await?;
    Ok(())
}

/// «Провести приход»: накладная и, если её заполнили по фото, черновик — его фото прикрепляется
/// к накладной в той же транзакции (SPEC-30 §9).
#[derive(Deserialize)]
struct PostReceiptBody {
    #[serde(flatten)]
    receipt: ReceiptReq,
    #[serde(default)]
    draft_id: Option<Uuid>,
}

async fn post_receipt(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(body): Json<PostReceiptBody>,
) -> AppResult<Json<ReceiptOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_receipt_tx(&mut tx, &ctx, body.receipt).await?;
    if let Some(draft) = body.draft_id {
        crate::api::receipt_drafts::attach(&mut tx, &ctx, draft, out.id).await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    /// Сторно внутри «Исправить»: верный приход проводится той же операцией, поэтому о минусе
    /// судит исправление после него (ADR-092), а не сторно.
    #[serde(skip)]
    pub in_correction: bool,
}

/// Остатки товаров накладной (по живым карточкам, ADR-044): id, название, единица, остаток.
/// «Исправить» сравнивает их до сторно и после верного прихода (ADR-092).
pub async fn receipt_stock(
    conn: &mut PgConnection,
    branch_id: Uuid,
    receipt_id: Uuid,
) -> AppResult<Vec<(Uuid, String, String, i64)>> {
    let ids = sqlx::query_scalar!(
        "select distinct product_id from receipt_lines where receipt_id = $1",
        receipt_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut live = std::collections::BTreeSet::new();
    for id in ids {
        live.insert(ops::live_product(conn, id).await?);
    }
    let mut out = Vec::with_capacity(live.len());
    for id in live {
        let r = sqlx::query!(
            r#"select p.name, p.unit, coalesce(bp.stock_qty, 0) as "stock!"
               from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               where p.id = $1"#,
            id,
            branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        out.push((id, r.name, r.unit, r.stock));
    }
    Ok(out)
}

pub async fn reverse_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.reverse";
    crate::auth::require_permission(&mut *conn, &ctx.user, "receipts").await?;
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    // Блокировка исходного документа от параллельного сторно.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let orig = load_receipt(conn, branch_id, id).await?;
    if orig.reversal_of.is_some() {
        return Err(AppError::Conflict("сторно не сторнируется".into()));
    }
    if orig.reversed_by.is_some() {
        return Err(AppError::Conflict("приход уже сторнирован".into()));
    }
    // Товар с влитой карточки уже лежит на основной: сторно снимает его оттуда (ADR-044).
    let mut live = std::collections::HashMap::new();
    for l in &orig.lines {
        let p = ops::live_product(conn, l.product_id).await?;
        live.insert(l.product_id, p);
    }
    lock_products(conn, branch_id, live.values().copied()).await?;
    ops::ensure_not_merged(conn, &live.values().copied().collect::<Vec<_>>()).await?;
    // Товар прихода уже продан и остаток уйдёт в минус — так исправляют только ошибку прихода,
    // и решает это владелец (ADR-065). Строки одного товара складываются.
    let mut take: std::collections::BTreeMap<Uuid, i64> = std::collections::BTreeMap::new();
    for l in &orig.lines {
        let target = live.get(&l.product_id).copied().unwrap_or(l.product_id);
        let e = take.entry(target).or_insert(0);
        *e = e.checked_add(l.qty).ok_or_else(crate::error::overflow)?;
    }
    let mut negative = Vec::new();
    for (product_id, qty) in &take {
        let r = sqlx::query!(
            r#"select p.name, p.unit, coalesce(bp.stock_qty, 0) as "stock!"
               from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               where p.id = $1"#,
            product_id,
            branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if *qty > 0 && r.stock < *qty {
            negative.push(json!({
                "name": r.name,
                "unit": r.unit,
                "stock": r.stock,
                "qty": qty,
                "after": r.stock - qty,
            }));
        }
    }
    if !negative.is_empty() && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // Брак этого поставщика по товарам прихода, отмеченный после него, опирается на приход:
    // пока решение по нему действует или он ждёт решения, приход не сторнируется (ADR-070).
    if let Some(supplier_id) = orig.supplier_id {
        let mut ids: Vec<Uuid> = live.values().copied().collect();
        ids.extend(orig.lines.iter().map(|l| l.product_id));
        let defects = sqlx::query_scalar!(
            r#"select d.number from defects d
               where d.branch_id = $1 and d.supplier_id = $2 and d.product_id = any($3)
                 and d.status <> 'cancelled'
                 and d.created_at > (select created_at from receipts where id = $4)
               order by d.number"#,
            branch_id,
            supplier_id,
            &ids,
            id
        )
        .fetch_all(&mut *conn)
        .await?;
        if !defects.is_empty() {
            return Err(AppError::Conflict(format!(
                "по товарам этого прихода есть брак от поставщика (№ {}): сначала отмените решение по нему или снимите отметку",
                defects
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    let number = ops::next_counter(conn, branch_id, "receipt").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        rid,
        branch_id,
        number,
        orig.supplier_id,
        orig.supplier_doc,
        req.comment.trim(),
        -orig.total_tyiyn,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for l in &orig.lines {
        sqlx::query!(
            "insert into receipt_lines (id, receipt_id, line_no, product_id, qty, cost_tyiyn) values ($1, $2, $3, $4, $5, $6)",
            new_id(),
            rid,
            l.line_no,
            l.product_id,
            -l.qty,
            -l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
        // Ошибочная цена не должна остаться «последней закупочной»: берём её из последней живой
        // закупки — прихода, оприходования, приёма аккумулятора (SPEC-03, ADR-086). До движения:
        // если остаток уйдёт в минус по стоимости, переоценка возьмёт уже восстановленную цену
        // (ADR-044). Цена ищется по основной карточке и всем, что в неё влиты.
        ops::restore_last_cost(
            conn,
            branch_id,
            live.get(&l.product_id).copied().unwrap_or(l.product_id),
        )
        .await?;
        let target = live.get(&l.product_id).copied().unwrap_or(l.product_id);
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: target,
                qty_delta: -l.qty,
                value_delta: -l.cost_tyiyn,
                doc_type: "receipt_reversal",
                doc_id: rid,
            },
        )
        .await?;
    }
    // Приход был в долг: долг поставщику уходит вместе с товаром (SPEC-10).
    let debt = sqlx::query!(
        r#"select party_id, coalesce(sum(amount_tyiyn), 0)::bigint as "sum!" from party_ledger
           where branch_id = $1 and doc_type = 'receipt' and doc_id = $2
           group by party_id"#,
        branch_id,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    for d in debt.into_iter().filter(|d| d.sum != 0) {
        crate::api::parties::add_ledger(
            conn,
            ctx,
            crate::api::parties::LedgerEntry {
                party_id: d.party_id,
                kind: "debt",
                amount: -d.sum,
                doc_type: "receipt_reversal",
                doc_id: Some(rid),
                comment: "сторно прихода",
            },
        )
        .await?;
    }
    // Накладную оплатили из кассы: деньги возвращаются в ту же кассу (ADR-062).
    let paid = sqlx::query!(
        r#"select m.account_id, a.kind, a.owner_only, coalesce(sum(m.amount_tyiyn), 0)::bigint as "sum!"
           from cash_movements m join cash_accounts a on a.id = m.account_id
           where m.branch_id = $1 and m.doc_type = 'receipt' and m.doc_id = $2
             and m.kind = 'supplier_payment'
           group by m.account_id, a.kind, a.owner_only
           order by m.account_id"#,
        branch_id,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    for p in paid.into_iter().filter(|p| p.sum != 0) {
        // Счёт и сейфы трогает владелец (SPEC-05): сторно такой накладной — тоже он.
        if !ctx.user.is_owner() && (p.kind != "register" || p.owner_only) {
            return Err(AppError::Forbidden);
        }
        crate::api::cash::add_movement(
            conn,
            ctx,
            crate::api::cash::CashEntry {
                account_id: p.account_id,
                kind: "reversal",
                amount: -p.sum,
                doc_type: "receipt_reversal",
                doc_id: Some(rid),
                comment: &format!("сторно накладной № {}", orig.number),
            },
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(rid),
        json!({ "original": id, "number": orig.number, "reversal_number": number }),
    )
    .await?;
    if !negative.is_empty() && !req.in_correction {
        ops::audit(
            conn,
            ctx,
            "receipt.reverse_negative",
            "receipt",
            Some(rid),
            json!({ "original": id, "number": orig.number, "lines": negative }),
        )
        .await?;
    }
    let out = load_receipt(conn, branch_id, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn reverse_receipt(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<ReceiptOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_receipt_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn get_receipt(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ReceiptOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_receipt(&mut conn, user.branch_id, id).await?))
}

/// Прошлая закупка товара: «в прошлый раз: 350 с, 12.10, Автоснаб» (SPEC-27 §5).
#[derive(Serialize, Deserialize, Debug)]
pub struct LastReceipt {
    pub receipt_id: Uuid,
    pub number: i64,
    /// Цена за штуку, канистру или килограмм — как вводят в приходе.
    pub price_tyiyn: i64,
    pub created_at: DateTime<Utc>,
    pub supplier_name: Option<String>,
}

async fn last_receipt(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Option<LastReceipt>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(last_receipt_of(&mut conn, user.branch_id, id).await?))
}

/// Последний непогашенный приход товара (и влитых в него карточек). Закупочные цены видит и
/// администратор: он сам вводит их в приходе (ADR-008, ADR-040).
pub async fn last_receipt_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    product_id: Uuid,
) -> AppResult<Option<LastReceipt>> {
    let r = sqlx::query!(
        r#"with recursive fam as (
             select $2::uuid as id
             union select p.id from products p join fam on p.merged_into = fam.id
           )
           select r.id, r.number, r.created_at, s.name as "supplier_name?", l.qty, l.cost_tyiyn,
                  p.unit, p.container_ml
           from receipt_lines l
           join receipts r on r.id = l.receipt_id
           join products p on p.id = $2
           left join suppliers s on s.id = r.supplier_id
           where r.branch_id = $1 and l.product_id in (select id from fam) and l.qty > 0
             and r.reversal_of is null
             and not exists (select 1 from receipts x where x.reversal_of = r.id)
           order by r.created_at desc, l.line_no desc
           limit 1"#,
        branch_id,
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(r) = r else {
        return Ok(None);
    };
    let per = match (r.unit.as_str(), r.container_ml) {
        ("ml", Some(c)) if c > 0 => c,
        ("g", _) => 1000,
        _ => 1,
    };
    let price = crate::domain::money::div_round(
        i128::from(r.cost_tyiyn) * i128::from(per),
        i128::from(r.qty),
    )
    .ok_or_else(overflow)?;
    Ok(Some(LastReceipt {
        receipt_id: r.id,
        number: r.number,
        price_tyiyn: price,
        created_at: r.created_at,
        supplier_name: r.supplier_name,
    }))
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Накладные одного поставщика — для его карточки (SPEC-10).
    pub supplier_id: Option<Uuid>,
}

#[derive(Serialize)]
pub(crate) struct ReceiptListItem {
    /// `receipt` — приход от поставщика, `stock_in` — оприходование без поставщика (ADR-086).
    pub(crate) kind: String,
    pub(crate) id: Uuid,
    pub(crate) number: i64,
    pub(crate) supplier_name: Option<String>,
    pub(crate) supplier_doc: String,
    pub(crate) total_tyiyn: i64,
    pub(crate) reversal_of: Option<Uuid>,
    pub(crate) reversed: bool,
    pub(crate) user_name: String,
    pub(crate) created_at: DateTime<Utc>,
    /// Взято в долг у поставщика по этой накладной (0 — оплачено сразу).
    pub(crate) on_debt_tyiyn: i64,
    /// Осталось заплатить по накладной в долг (ADR-073); 0 — оплачена или не в долг.
    pub(crate) debt_left_tyiyn: i64,
    pub(crate) comment: String,
    /// Товары оприходования словами («Хомут, Шайба»); у прихода — пусто.
    pub(crate) goods: String,
}

/// Что показывать на экране «Приход» (ADR-086): `all` — приходы и оприходования вместе,
/// `receipt` — только от поставщиков, `stock_in` — только оприходования. Без отбора — только
/// приходы, как раньше (карточка поставщика, бот).
#[derive(Deserialize, Default)]
pub struct KindQuery {
    pub kind: Option<String>,
}

async fn list_receipts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PeriodQuery>,
    Query(k): Query<KindQuery>,
    Query(page): Query<PageQuery>,
) -> AppResult<Paged<ReceiptListItem>> {
    let (receipts, stock_ins) = match k.kind.as_deref() {
        None | Some("receipt") => (true, false),
        Some("all") => (true, q.supplier_id.is_none()),
        Some("stock_in") => (false, q.supplier_id.is_none()),
        Some(_) => return Err(invalid("вид: all, receipt или stock_in")),
    };
    let mut conn = state.pool.acquire().await?;
    let mut rows = Vec::new();
    if stock_ins {
        let filter = crate::api::stock_in::Filter {
            product_id: None,
            from: q.from,
            to: q.to,
        };
        let docs = crate::api::stock_in::load(&mut conn, user.branch_id, None, filter).await?;
        rows.extend(docs.into_iter().map(|d| {
            let mut names: Vec<&str> = Vec::new();
            for l in &d.lines {
                if !names.contains(&l.product_name.as_str()) {
                    names.push(&l.product_name);
                }
            }
            ReceiptListItem {
                kind: "stock_in".into(),
                goods: names.join(", "),
                id: d.id,
                number: d.number,
                supplier_name: None,
                supplier_doc: String::new(),
                total_tyiyn: d.cost_tyiyn,
                reversal_of: d.reversal_of,
                reversed: d.reversed_by.is_some(),
                user_name: d.user_name,
                created_at: d.created_at,
                on_debt_tyiyn: 0,
                debt_left_tyiyn: 0,
                comment: d.comment,
            }
        }));
    }
    if receipts {
        rows.extend(receipts_data(&mut conn, &user, q).await?);
    }
    // Вместе — по дате, новые сверху; дальше отбора сервер не перебирает.
    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    rows.truncate(usize::try_from(MAX_ROWS).unwrap_or(usize::MAX));
    let (items, total) = page.slice(rows, 500);
    Ok(paged(items, total))
}

/// Приходы за период — экран «Приход» и бот владельца (SPEC-24).
pub(crate) async fn receipts_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: PeriodQuery,
) -> AppResult<Vec<ReceiptListItem>> {
    let rows = sqlx::query_as!(
        ReceiptListItem,
        r#"select 'receipt' as "kind!", r.id, r.number, s.name as "supplier_name?", r.supplier_doc, r.total_tyiyn, r.reversal_of,
                  exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!",
                  u.full_name as user_name, r.created_at,
                  abs(coalesce((select sum(l.amount_tyiyn) from party_ledger l
                                where l.doc_type = 'receipt' and l.doc_id = r.id), 0))::bigint as "on_debt_tyiyn!",
                  coalesce((select sum(-l.amount_tyiyn) - coalesce(sum((select sum(s.amount_tyiyn)
                                                                         from supplier_allocations s
                                                                         where s.debit_id = l.id)), 0)
                            from party_ledger l
                            where l.doc_type = 'receipt' and l.doc_id = r.id and l.amount_tyiyn < 0
                              and supplier_ledger_live(l)), 0)::bigint as "debt_left_tyiyn!",
                  r.comment, '' as "goods!"
           from receipts r
           join users u on u.id = r.user_id
           left join suppliers s on s.id = r.supplier_id
           where r.branch_id = $1
             and ($2::date is null or r.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek')
             and ($3::date is null or r.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek')
             and ($4::uuid is null or r.supplier_id = $4)
           order by r.created_at desc
           limit $5"#,
        user.branch_id,
        q.from,
        q.to,
        q.supplier_id,
        MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

#[derive(Deserialize)]
struct StockQuery {
    category_id: Option<Uuid>,
    #[serde(default)]
    low: bool,
    q: Option<String>,
}

async fn stock(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<StockQuery>,
) -> AppResult<Json<Vec<ProductOut>>> {
    let mut conn = state.pool.acquire().await?;
    let f = ProductFilter {
        q: q.q,
        category_id: q.category_id,
        low_only: q.low,
        limit: 500,
        ..Default::default()
    };
    Ok(Json(query_products(&mut conn, &user, f).await?))
}

#[derive(Serialize)]
pub struct StockMismatch {
    pub product_id: Uuid,
    pub cached_qty: i64,
    pub moved_qty: i64,
    pub cached_value_tyiyn: i64,
    pub moved_value_tyiyn: i64,
}

/// Расхождения кэша остатков с суммой движений (инвариант 6). Пустой список — норма.
pub async fn verify_stock_tx(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<Vec<StockMismatch>> {
    let rows = sqlx::query_as!(
        StockMismatch,
        r#"select bp.product_id, bp.stock_qty as cached_qty,
                  coalesce(m.qty, 0) as "moved_qty!",
                  bp.stock_value_tyiyn as cached_value_tyiyn,
                  coalesce(m.value, 0) as "moved_value_tyiyn!"
           from branch_products bp
           left join (select product_id, sum(qty_delta)::bigint as qty, sum(value_delta_tyiyn)::bigint as value
                      from stock_movements where branch_id = $1 group by product_id) m
             on m.product_id = bp.product_id
           where bp.branch_id = $1
             and (bp.stock_qty <> coalesce(m.qty, 0) or bp.stock_value_tyiyn <> coalesce(m.value, 0))"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

async fn verify_stock(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<Vec<StockMismatch>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(verify_stock_tx(&mut conn, user.branch_id).await?))
}

async fn review_done(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(product_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    ctx.user.require_owner()?;
    let mut tx = state.pool.begin().await?;
    // Продажи без цены закупки ждут её: отметку не снять, пока владелец цену не указал —
    // иначе процент кассира по ним не начислится никогда (ADR-062).
    let waiting = sqlx::query_scalar!(
        r#"select count(*) as "n!" from payroll_cost_pending
           where branch_id = $1 and product_id = $2 and resolved_at is null"#,
        ctx.user.branch_id,
        product_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if waiting > 0 {
        return Err(AppError::Conflict(
            "товар продан без цены закупки: сначала укажите цену закупки".into(),
        ));
    }
    let n = sqlx::query!(
        "update branch_products set needs_review = false where branch_id = $1 and product_id = $2",
        ctx.user.branch_id,
        product_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::NotFound);
    }
    ops::audit(
        &mut tx,
        &ctx,
        "stock.review_done",
        "product",
        Some(product_id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PurchasePriceReq {
    pub op_id: Uuid,
    /// Цена закупки за штуку, за канистру (масло) или за килограмм (на вес).
    pub price_tyiyn: i64,
}

/// Владелец указывает цену закупки товара, проданного без неё (ADR-062): строки чеков получают
/// себестоимость для процента кассира, процент начисляется сегодня. Проведённые строки чеков не
/// меняются (инвариант 3), прибыль прошлых дней — тоже; отчёт прибыли о них предупреждает.
async fn purchase_price(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(product_id): Path<Uuid>,
    Json(req): Json<PurchasePriceReq>,
) -> AppResult<Json<crate::api::payroll::CostResolved>> {
    let mut tx = state.pool.begin().await?;
    let out = purchase_price_tx(&mut tx, &ctx, product_id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Цена закупки товара без прихода (ADR-062).
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn purchase_price_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    product_id: Uuid,
    req: PurchasePriceReq,
) -> AppResult<crate::api::payroll::CostResolved> {
    const KIND: &str = "stock.purchase_price";
    ctx.user.require_owner()?;
    if req.price_tyiyn <= 0 {
        return Err(invalid("цена закупки больше нуля"));
    }
    ops::check_amount(req.price_tyiyn)?;
    let branch_id = ctx.user.branch_id;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let p = sqlx::query!(
        "select name, unit, container_ml, archived from products where id = $1",
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.archived {
        return Err(invalid("товар в архиве: укажите цену у основной карточки"));
    }
    // Те же единицы, что у цены последнего прихода в карточке: штука, канистра; на вес — килограмм.
    let per = match (p.unit.as_str(), p.container_ml) {
        ("g", _) => 1000,
        (_, Some(c)) => c,
        _ => 1,
    };
    ops::lock_pool(&mut *conn, branch_id, product_id).await?;
    // Закупок не было — цена владельца становится «последней закупочной»: следующие продажи
    // при нехватке остатка возьмут себестоимость по ней (SPEC-03). Цену прихода не перебиваем.
    // Цена владельца хранится и отдельно: сторно закупки без других закупок вернёт её (ADR-092).
    sqlx::query!(
        r#"update branch_products
           set last_cost_qty = case when last_cost_qty = 0 then $3 else last_cost_qty end,
               last_cost_tyiyn = case when last_cost_qty = 0 then $4 else last_cost_tyiyn end,
               owner_cost_qty = $3, owner_cost_tyiyn = $4
           where branch_id = $1 and product_id = $2"#,
        branch_id,
        product_id,
        per,
        req.price_tyiyn
    )
    .execute(&mut *conn)
    .await?;
    let out = crate::api::payroll::resolve_cost_pending(
        &mut *conn,
        ctx,
        product_id,
        req.price_tyiyn,
        per,
    )
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "product",
        Some(product_id),
        json!({
            "name": p.name,
            "price_tyiyn": req.price_tyiyn,
            "per": per,
            "lines": out.lines,
            "accrued_tyiyn": out.accrued_tyiyn,
        }),
    )
    .await?;
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
