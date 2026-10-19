//! Оприходование без поставщика (SPEC-29 §8, ADR-086): товар пришёл без накладной и без денег.
//!
//! Вариант прихода: на экране «Новый приход» — «Оприходование (без поставщика)», строки как у
//! прихода. Количество ложится на склад по цене закупки, которую вписали (за штуку, канистру масла
//! или килограмм), — как приход, но без поставщика и без движения денег. Комментарий обязателен.
//! Один документ — одна операция, одна транзакция, несколько строк. Проводят владелец и
//! администратор (цены закупки администратор видит и вводит, ADR-008); владелец получает
//! уведомление. Исправление — только сторно со ссылкой (инвариант 4) или «Исправить» (ADR-085).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{PriceDoc, price_unit};
use crate::api::paging::{MAX_ROWS, PageQuery, Paged, paged};
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::{div_round, markup_permille};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/stock-ins", routing::get(list).post(post))
        .route("/stock-ins/{id}", routing::get(get_one))
        .route("/stock-ins/{id}/reverse", routing::post(reverse))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StockInLineOut {
    pub line_no: i32,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// В единицах учёта: штуки, миллилитры, граммы; у сторно — с минусом.
    pub qty: i64,
    /// Цена закупки за штуку, канистру или килограмм.
    pub price_tyiyn: i64,
    /// Стоимость строки.
    pub cost_tyiyn: i64,
    /// Цена продажи товара после оприходования; пусто — документ до ADR-086 или сторно.
    #[serde(default)]
    pub sale_price_tyiyn: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StockInOut {
    pub id: Uuid,
    pub number: i64,
    /// Итог документа; у сторно — с минусом.
    pub cost_tyiyn: i64,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub reversed_by: Option<Uuid>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub lines: Vec<StockInLineOut>,
}

/// Отбор списка оприходований: товар и период по дню Бишкека (оба края включительно).
#[derive(Deserialize, Default, Clone, Copy)]
pub(crate) struct Filter {
    pub(crate) product_id: Option<Uuid>,
    pub(crate) from: Option<NaiveDate>,
    pub(crate) to: Option<NaiveDate>,
}

pub(crate) async fn load(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Option<Uuid>,
    f: Filter,
) -> AppResult<Vec<StockInOut>> {
    let heads = sqlx::query!(
        r#"select s.id, s.number, s.cost_tyiyn, s.comment, s.reversal_of,
                  (select x.id from stock_ins x where x.reversal_of = s.id) as reversed_by,
                  u.full_name as user_name, s.created_at
           from stock_ins s
           join users u on u.id = s.user_id
           where s.branch_id = $1 and ($2::uuid is null or s.id = $2)
             and ($3::uuid is null
                  or exists (select 1 from stock_in_lines l where l.stock_in_id = s.id and l.product_id = $3))
             and ($4::date is null or s.created_at >= ($4::date)::timestamp at time zone 'Asia/Bishkek')
             and ($5::date is null or s.created_at < ($5::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by s.created_at desc
           limit $6"#,
        branch_id,
        id,
        f.product_id,
        f.from,
        f.to,
        MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = heads.iter().map(|h| h.id).collect();
    let rows = sqlx::query!(
        r#"select l.stock_in_id, l.line_no, l.product_id, p.name as product_name, p.unit, p.container_ml,
                  l.qty, l.price_tyiyn, l.cost_tyiyn, l.sale_price_tyiyn
           from stock_in_lines l join products p on p.id = l.product_id
           where l.stock_in_id = any($1)
           order by l.stock_in_id, l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut lines: HashMap<Uuid, Vec<StockInLineOut>> = HashMap::new();
    for r in rows {
        lines
            .entry(r.stock_in_id)
            .or_default()
            .push(StockInLineOut {
                line_no: r.line_no,
                product_id: r.product_id,
                product_name: r.product_name,
                unit: r.unit,
                container_ml: r.container_ml,
                qty: r.qty,
                price_tyiyn: r.price_tyiyn,
                cost_tyiyn: r.cost_tyiyn,
                sale_price_tyiyn: r.sale_price_tyiyn,
            });
    }
    Ok(heads
        .into_iter()
        .map(|h| StockInOut {
            lines: lines.remove(&h.id).unwrap_or_default(),
            id: h.id,
            number: h.number,
            cost_tyiyn: h.cost_tyiyn,
            comment: h.comment,
            reversal_of: h.reversal_of,
            reversed_by: h.reversed_by,
            user_name: h.user_name,
            created_at: h.created_at,
        })
        .collect())
}

pub(crate) async fn load_one(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<StockInOut> {
    load(conn, branch_id, Some(id), Filter::default())
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

/// Список оприходований: период, товар и страницы. Экран «Приход» показывает их вместе с
/// приходами (`GET /receipts?kind=`); без `limit` — как раньше, до 500 строк.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(f): Query<Filter>,
    Query(page): Query<PageQuery>,
) -> AppResult<Paged<StockInOut>> {
    let mut conn = state.pool.acquire().await?;
    let (items, total) = page.slice(load(&mut conn, user.branch_id, None, f).await?, 500);
    Ok(paged(items, total))
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<StockInOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_one(&mut conn, user.branch_id, id).await?))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StockInLineReq {
    pub product_id: Uuid,
    /// В единицах учёта: штуки, миллилитры (масло), граммы (на вес).
    pub qty: i64,
    /// Цена закупки за штуку, канистру масла или килограмм.
    pub price_tyiyn: i64,
    /// Новая цена продажи товара — как у строки прихода (SPEC-28 §1); пусто — не меняется.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_price_tyiyn: Option<i64>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StockInReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub lines: Vec<StockInLineReq>,
    /// Прежний вид запроса — один товар без `lines` (до ADR-086): принимается как одна строка.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qty: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_tyiyn: Option<i64>,
}

impl StockInReq {
    /// Строки документа: `lines` или прежний один товар.
    fn into_lines(self) -> AppResult<Vec<StockInLineReq>> {
        match (self.product_id, self.lines.is_empty()) {
            (None, _) => Ok(self.lines),
            (Some(product_id), true) => Ok(vec![StockInLineReq {
                product_id,
                qty: self.qty.unwrap_or(0),
                price_tyiyn: self.price_tyiyn.unwrap_or(-1),
                sale_price_tyiyn: None,
            }]),
            (Some(_), false) => Err(invalid("товар — либо строками, либо одним товаром")),
        }
    }
}

async fn post(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<StockInReq>,
) -> AppResult<Json<StockInOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Оприходование — одна транзакция: документ и строки, движения склада, последняя закупочная
/// цена и новая цена продажи по строкам, журнал.
pub async fn post_tx(conn: &mut PgConnection, ctx: &Ctx, req: StockInReq) -> AppResult<StockInOut> {
    const KIND: &str = "stock_in.post";
    let op_id = req.op_id;
    let comment = req.comment.trim().to_string();
    if comment.is_empty() {
        return Err(invalid(
            "напишите, откуда товар: без накладной оприходование только с комментарием",
        ));
    }
    if comment.chars().count() > 500 {
        return Err(invalid("комментарий не длиннее 500 знаков"));
    }
    let lines = req.into_lines()?;
    if lines.is_empty() {
        return Err(invalid("в оприходовании нет строк"));
    }
    if lines.len() > 500 {
        return Err(invalid("слишком много строк"));
    }
    for l in &lines {
        if l.qty <= 0 || l.qty > 1_000_000_000 {
            return Err(invalid("количество больше нуля"));
        }
        if l.price_tyiyn < 0 {
            return Err(invalid("цена закупки не отрицательна"));
        }
        ops::check_amount(l.price_tyiyn)?;
        if let Some(p) = l.sale_price_tyiyn {
            if p < 0 {
                return Err(invalid("цена продажи не отрицательна"));
            }
            ops::check_amount(p)?;
        }
    }
    // Одна цена продажи на товар — как у прихода.
    let mut prices: BTreeMap<Uuid, (i64, i64)> = BTreeMap::new();
    for l in &lines {
        if let Some(p) = l.sale_price_tyiyn
            && prices.entry(l.product_id).or_insert((p, l.price_tyiyn)).0 != p
        {
            return Err(invalid(
                "у одного товара в оприходовании две разные цены продажи — оставьте одну",
            ));
        }
    }
    if let Some(done) = ops::begin_op(conn, ctx, op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let distinct: BTreeSet<Uuid> = lines.iter().map(|l| l.product_id).collect();
    let mut products = HashMap::new();
    for id in &distinct {
        let p = sqlx::query!(
            "select name, unit, container_ml, archived from products where id = $1",
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("товар не найден"))?;
        if p.archived {
            return Err(invalid(
                "товар в архиве: если карточку объединили, оприходуйте на основную",
            ));
        }
        products.insert(*id, p);
    }
    let mut costs = Vec::with_capacity(lines.len());
    let mut total: i64 = 0;
    for l in &lines {
        let p = products
            .get(&l.product_id)
            .ok_or_else(|| AppError::Internal("нет товара строки".into()))?;
        let per = price_unit(&p.unit, p.container_ml);
        let cost = div_round(
            i128::from(l.price_tyiyn) * i128::from(l.qty),
            i128::from(per),
        )
        .ok_or_else(overflow)?;
        ops::check_amount(cost)?;
        total = total.checked_add(cost).ok_or_else(overflow)?;
        costs.push(cost);
    }
    ops::check_amount(total)?;
    crate::api::receipts::lock_products(conn, branch_id, distinct.iter().copied()).await?;
    let ids: Vec<Uuid> = distinct.iter().copied().collect();
    ops::ensure_not_merged(conn, &ids).await?;
    let number = ops::next_counter(conn, branch_id, "stock_in").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into stock_ins (id, branch_id, number, cost_tyiyn, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        id,
        branch_id,
        number,
        total,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    // Новые цены продажи — до строк: строка запоминает цену, с которой товар вышел из документа.
    for (product_id, (sale, price)) in &prices {
        crate::api::catalog::set_price_from_receipt(
            conn,
            ctx,
            *product_id,
            *sale,
            PriceDoc::StockIn(id, number),
            markup_permille(*price, *sale),
        )
        .await?;
    }
    for (i, (l, cost)) in lines.iter().zip(&costs).enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            r#"insert into stock_in_lines (id, stock_in_id, line_no, product_id, qty, price_tyiyn, cost_tyiyn,
                                           sale_price_tyiyn)
               values ($1, $2, $3, $4, $5, $6, $7,
                       (select sale_price_tyiyn from branch_products where branch_id = $8 and product_id = $4))"#,
            new_id(),
            id,
            line_no,
            l.product_id,
            l.qty,
            l.price_tyiyn,
            cost,
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
                value_delta: *cost,
                doc_type: "stock_in",
                doc_id: id,
            },
        )
        .await?;
        // Цена закупки — последняя закупочная: по ней пойдёт продажа в минус и излишек ревизии.
        if l.price_tyiyn > 0 {
            sqlx::query!(
                "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
                branch_id,
                l.product_id,
                l.qty,
                cost
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    let audit_lines: Vec<_> = lines
        .iter()
        .zip(&costs)
        .filter_map(|(l, cost)| {
            products.get(&l.product_id).map(|p| {
                json!({
                    "product_id": l.product_id,
                    "name": p.name,
                    "qty_text": crate::api::bot_drafts::qty_text(l.qty, &p.unit),
                    "price": l.price_tyiyn,
                    "cost": cost,
                })
            })
        })
        .collect();
    ops::audit(
        conn,
        ctx,
        KIND,
        "stock_in",
        Some(id),
        json!({
            "number": number,
            "stock_in": id,
            "lines": audit_lines,
            "cost": total,
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, branch_id, id).await?;
    ops::finish_op(conn, ctx, op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

async fn reverse(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<StockInOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно оприходования целиком: товар всех строк уходит со склада по той же стоимости. Если его
/// уже продали и остаток уйдёт в минус — так исправляют только ошибку, и решает владелец (как у
/// прихода, ADR-065).
pub async fn reverse_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<StockInOut> {
    const KIND: &str = "stock_in.reverse";
    let comment = req.comment.trim();
    if comment.is_empty() {
        return Err(invalid("укажите причину сторно"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let orig = load_one(conn, branch_id, id).await?;
    if orig.reversal_of.is_some() {
        return Err(AppError::Conflict("сторно не сторнируется".into()));
    }
    if orig.reversed_by.is_some() {
        return Err(AppError::Conflict("оприходование уже сторнировано".into()));
    }
    // Карточку могли объединить с другой: товар уходит с той, в которую её влили.
    let mut targets = Vec::with_capacity(orig.lines.len());
    for l in &orig.lines {
        targets.push(ops::live_product(conn, l.product_id).await?);
    }
    let mut leaving: BTreeMap<Uuid, i64> = BTreeMap::new();
    for (l, t) in orig.lines.iter().zip(&targets) {
        let q = leaving.entry(*t).or_insert(0);
        *q = q.checked_add(l.qty).ok_or_else(overflow)?;
    }
    let mut pools = HashMap::new();
    for t in leaving.keys() {
        pools.insert(*t, ops::lock_pool(conn, branch_id, *t).await?);
    }
    let live: Vec<Uuid> = leaving.keys().copied().collect();
    ops::ensure_not_merged(conn, &live).await?;
    let mut negative = Vec::new();
    for (t, qty) in &leaving {
        let have = pools.get(t).map_or(0, |p| p.qty);
        if have < *qty {
            let unit = orig
                .lines
                .iter()
                .zip(&targets)
                .find(|(_, x)| *x == t)
                .map(|(l, _)| (l.product_name.clone(), l.unit.clone()))
                .unwrap_or_default();
            negative.push(format!(
                "{} {}",
                unit.0,
                crate::api::bot_drafts::qty_text(have - qty, &unit.1)
            ));
        }
    }
    if !negative.is_empty() && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let number = ops::next_counter(conn, branch_id, "stock_in").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into stock_ins (id, branch_id, number, cost_tyiyn, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        rid,
        branch_id,
        number,
        -orig.cost_tyiyn,
        comment,
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    for (l, target) in orig.lines.iter().zip(&targets) {
        sqlx::query!(
            r#"insert into stock_in_lines (id, stock_in_id, line_no, product_id, qty, price_tyiyn, cost_tyiyn)
               values ($1, $2, $3, $4, $5, $6, $7)"#,
            new_id(),
            rid,
            l.line_no,
            l.product_id,
            -l.qty,
            l.price_tyiyn,
            -l.cost_tyiyn
        )
        .execute(&mut *conn)
        .await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: *target,
                qty_delta: -l.qty,
                value_delta: -l.cost_tyiyn,
                doc_type: "stock_in_reversal",
                doc_id: rid,
            },
        )
        .await?;
    }
    // Ошибочная цена не остаётся «последней закупочной»: берём последнюю живую закупку —
    // приход или другое оприходование; не было — ноль (как при сторно прихода, SPEC-03).
    let priced: BTreeSet<Uuid> = orig
        .lines
        .iter()
        .zip(&targets)
        .filter(|(l, _)| l.price_tyiyn > 0)
        .map(|(_, t)| *t)
        .collect();
    for target in priced {
        let last = sqlx::query!(
            r#"with recursive fam as (
                 select $2::uuid as id
                 union select p.id from products p join fam on p.merged_into = fam.id
               )
               select qty as "qty!", cost as "cost!" from (
                 select l.qty, l.cost_tyiyn as cost, r.created_at as at, l.line_no
                 from receipt_lines l join receipts r on r.id = l.receipt_id
                 where r.branch_id = $1 and l.product_id in (select id from fam) and l.qty > 0
                   and r.reversal_of is null
                   and not exists (select 1 from receipts x where x.reversal_of = r.id)
                 union all
                 select l.qty, l.cost_tyiyn, s.created_at, l.line_no
                 from stock_in_lines l join stock_ins s on s.id = l.stock_in_id
                 where s.branch_id = $1 and l.product_id in (select id from fam) and l.qty > 0
                   and s.id <> $3 and l.price_tyiyn > 0 and s.reversal_of is null
                   and not exists (select 1 from stock_ins x where x.reversal_of = s.id)
               ) u
               order by at desc, line_no desc limit 1"#,
            branch_id,
            target,
            id
        )
        .fetch_optional(&mut *conn)
        .await?;
        sqlx::query!(
            "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
            branch_id,
            target,
            last.as_ref().map_or(0, |r| r.qty),
            last.as_ref().map_or(0, |r| r.cost)
        )
        .execute(&mut *conn)
        .await?;
    }
    let audit_lines: Vec<_> = orig
        .lines
        .iter()
        .map(|l| {
            json!({
                "product_id": l.product_id,
                "name": l.product_name,
                "qty_text": crate::api::bot_drafts::qty_text(l.qty, &l.unit),
                "cost": l.cost_tyiyn,
            })
        })
        .collect();
    ops::audit(
        conn,
        ctx,
        KIND,
        "stock_in",
        Some(rid),
        json!({
            "number": orig.number,
            "stock_in": id,
            "reversal": rid,
            "lines": audit_lines,
            "cost": orig.cost_tyiyn,
            "comment": comment,
            "negative": (!negative.is_empty()).then(|| negative.join(", ")),
        }),
    )
    .await?;
    let out = load_one(conn, branch_id, rid).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
