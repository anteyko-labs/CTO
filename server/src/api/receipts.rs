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
use crate::auth::{Ctx, CurrentUser, Owner};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/receipts", routing::get(list_receipts).post(post_receipt))
        .route("/receipts/{id}", routing::get(get_receipt))
        .route("/receipts/{id}/reverse", routing::post(reverse_receipt))
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
}

#[derive(Deserialize)]
pub struct ReceiptLineReq {
    pub product_id: Uuid,
    pub qty: i64,
    pub cost_tyiyn: i64,
}

#[derive(Deserialize)]
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
}

async fn load_receipt(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<ReceiptOut> {
    let h = sqlx::query!(
        r#"select r.id, r.number, r.supplier_id, s.name as "supplier_name?", r.supplier_doc, r.comment,
                  r.total_tyiyn, r.reversal_of, u.full_name as user_name, r.created_at,
                  (select x.id from receipts x where x.reversal_of = r.id) as reversed_by
           from receipts r
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
        r#"select l.line_no, l.product_id, p.name as product_name, p.unit, p.container_ml, l.qty, l.cost_tyiyn
           from receipt_lines l join products p on p.id = l.product_id
           where l.receipt_id = $1 order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
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

pub async fn post_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ReceiptReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.post";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if req.lines.is_empty() {
        return Err(invalid("в приходе нет строк"));
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
        total = total.checked_add(l.cost_tyiyn).ok_or_else(overflow)?;
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
    sqlx::query!(
        r#"insert into receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        branch_id,
        number,
        req.supplier_id,
        req.supplier_doc.trim(),
        req.comment.trim(),
        total,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (i, l) in req.lines.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            "insert into receipt_lines (id, receipt_id, line_no, product_id, qty, cost_tyiyn) values ($1, $2, $3, $4, $5, $6)",
            new_id(),
            id,
            line_no,
            l.product_id,
            l.qty,
            l.cost_tyiyn
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
            r#"select p.name, p.unit, p.container_ml, coalesce(bp.sale_price_tyiyn, 0) as "sale!",
                      bp.pour_price_per_l_tyiyn as "pour?"
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
        let per_unit = match (p.unit.as_str(), p.container_ml) {
            ("ml", Some(c)) => i128::from(p.sale) * qty < cost * i128::from(c),
            _ => i128::from(p.sale) * qty < cost,
        };
        let per_liter = p
            .pour
            .is_some_and(|pour| i128::from(pour) * qty < cost * 1000);
        if (per_unit || per_liter) && !dearer.contains(&p.name) {
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
    let out = load_receipt(conn, branch_id, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_receipt(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ReceiptReq>,
) -> AppResult<Json<ReceiptOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_receipt_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

pub async fn reverse_receipt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<ReceiptOut> {
    const KIND: &str = "receipt.reverse";
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
        // Ошибочная цена не должна остаться «последней закупочной»: берём её из последнего
        // непогашенного прихода, иначе обнуляем (SPEC-03). До движения: если остаток уйдёт
        // в минус по стоимости, переоценка возьмёт уже восстановленную цену (ADR-044).
        // Ищем по основной карточке и всем, что в неё влиты: цена дубля не должна затереть основную.
        let target = live.get(&l.product_id).copied().unwrap_or(l.product_id);
        let last = sqlx::query!(
            r#"with recursive fam as (
                 select $2::uuid as id
                 union select p.id from products p join fam on p.merged_into = fam.id
               )
               select l.qty, l.cost_tyiyn from receipt_lines l join receipts r on r.id = l.receipt_id
               where r.branch_id = $1 and l.product_id in (select id from fam) and l.qty > 0
                 and r.reversal_of is null and r.id <> $3
                 and not exists (select 1 from receipts x where x.reversal_of = r.id)
               order by r.created_at desc, l.line_no desc limit 1"#,
            branch_id,
            target,
            id
        )
        .fetch_optional(&mut *conn)
        .await?;
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            live.get(&l.product_id).copied().unwrap_or(l.product_id),
            last.as_ref().map_or(0, |r| r.qty),
            last.as_ref().map_or(0, |r| r.cost_tyiyn)
        )
        .execute(&mut *conn)
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
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(rid),
        json!({ "original": id, "number": number }),
    )
    .await?;
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

#[derive(Deserialize)]
pub struct PeriodQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// Накладные одного поставщика — для его карточки (SPEC-10).
    pub supplier_id: Option<Uuid>,
}

#[derive(Serialize)]
struct ReceiptListItem {
    id: Uuid,
    number: i64,
    supplier_name: Option<String>,
    supplier_doc: String,
    total_tyiyn: i64,
    reversal_of: Option<Uuid>,
    reversed: bool,
    user_name: String,
    created_at: DateTime<Utc>,
}

async fn list_receipts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ReceiptListItem>>> {
    let rows = sqlx::query_as!(
        ReceiptListItem,
        r#"select r.id, r.number, s.name as "supplier_name?", r.supplier_doc, r.total_tyiyn, r.reversal_of,
                  exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!",
                  u.full_name as user_name, r.created_at
           from receipts r
           join users u on u.id = r.user_id
           left join suppliers s on s.id = r.supplier_id
           where r.branch_id = $1
             and ($2::date is null or r.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek')
             and ($3::date is null or r.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek')
             and ($4::uuid is null or r.supplier_id = $4)
           order by r.created_at desc
           limit 500"#,
        user.branch_id,
        q.from,
        q.to,
        q.supplier_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
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
