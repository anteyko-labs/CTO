//! Брак: отметка, решение владельца и его сторно (SPEC-25, ADR-069).
//!
//! Отметка снимает товар из продаваемого остатка движением `defect` по средней себестоимости
//! (инвариант 3), стоимость фиксируется в записи брака. Решение владельца — списать или вернуть
//! поставщику (новый товар, скидка на следующий приход, деньги) — одна операция; отмена решения
//! и снятие отметки — сторно со ссылкой. Стоимость брака и возмещения видит только владелец.

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/defects", routing::get(list).post(mark))
        .route("/defects/supplier-hint", routing::get(supplier_hint))
        .route("/defects/{id}/unmark", routing::post(unmark))
        .route("/defects/{id}/resolve", routing::post(resolve))
        .route("/defects/{id}/resolution/reverse", routing::post(reverse))
}

/// Решение владельца по записи брака: действует, пока его не сторнировали.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ResolutionOut {
    pub id: Uuid,
    /// `write_off`, `replace`, `credit`, `cash`.
    pub kind: String,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    pub replace_qty: Option<i64>,
    /// Возмещение: скидка на следующий приход или деньги. Только владельцу.
    pub refund_tyiyn: Option<i64>,
    pub account_name: Option<String>,
    /// Убыток «Списание брака». Только владельцу.
    pub writeoff_tyiyn: Option<i64>,
    /// «Брак: разница возмещения»: возмещение минус стоимость. Только владельцу.
    pub diff_tyiyn: Option<i64>,
    pub comment: String,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DefectOut {
    pub id: Uuid,
    pub number: i64,
    pub product_id: Uuid,
    pub product_name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    /// Стоимость по средней на момент отметки — себестоимость, только владельцу (инвариант 13).
    pub cost_tyiyn: Option<i64>,
    pub reason: String,
    pub supplier_id: Option<Uuid>,
    pub supplier_name: Option<String>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    /// `pending`, `written_off`, `returned`, `cancelled` (отметку сняли).
    pub status: String,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolution: Option<ResolutionOut>,
}

/// Записи брака филиала: одна по `id` или все с отбором `pending` / `resolved`.
pub(crate) async fn load_defects(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Option<Uuid>,
    status: Option<&str>,
) -> AppResult<Vec<DefectOut>> {
    let owner = user.is_owner();
    let rows = sqlx::query!(
        r#"select d.id, d.number, d.product_id, p.name as product_name, p.unit, p.container_ml,
                  d.qty, d.cost_tyiyn, d.reason, d.supplier_id, s.name as "supplier_name?",
                  u.full_name as user_name, d.created_at, d.status, d.resolved_at,
                  r.id as "res_id?", r.kind as "res_kind?", r.supplier_id as "res_supplier_id?",
                  rs.name as "res_supplier_name?", r.replace_qty as "res_replace_qty?",
                  r.refund_tyiyn as "res_refund?", a.name as "res_account?",
                  r.writeoff_tyiyn as "res_writeoff?", r.diff_tyiyn as "res_diff?",
                  r.comment as "res_comment?", ru.full_name as "res_user?", r.created_at as "res_at?"
           from defects d
           join products p on p.id = d.product_id
           join users u on u.id = d.user_id
           left join suppliers s on s.id = d.supplier_id
           left join lateral (
             select x.* from defect_resolutions x
             where x.defect_id = d.id and x.reversal_of is null
               and not exists (select 1 from defect_resolutions y where y.reversal_of = x.id)
             order by x.created_at desc limit 1
           ) r on true
           left join suppliers rs on rs.id = r.supplier_id
           left join cash_accounts a on a.id = r.account_id
           left join users ru on ru.id = r.user_id
           where d.branch_id = $1 and ($2::uuid is null or d.id = $2)
             and ($3::text is null
                  or ($3 = 'pending' and d.status = 'pending')
                  or ($3 = 'resolved' and d.status <> 'pending'))
           order by d.created_at desc
           limit 500"#,
        user.branch_id,
        id,
        status
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let resolution = match (r.res_id, r.res_kind, r.res_at) {
                (Some(rid), Some(kind), Some(at)) => Some(ResolutionOut {
                    id: rid,
                    kind,
                    supplier_id: r.res_supplier_id,
                    supplier_name: r.res_supplier_name,
                    replace_qty: r.res_replace_qty,
                    refund_tyiyn: r.res_refund.filter(|_| owner),
                    account_name: r.res_account,
                    writeoff_tyiyn: r.res_writeoff.filter(|_| owner),
                    diff_tyiyn: r.res_diff.filter(|_| owner),
                    comment: r.res_comment.unwrap_or_default(),
                    user_name: r.res_user.unwrap_or_default(),
                    created_at: at,
                }),
                _ => None,
            };
            DefectOut {
                id: r.id,
                number: r.number,
                product_id: r.product_id,
                product_name: r.product_name,
                unit: r.unit,
                container_ml: r.container_ml,
                qty: r.qty,
                cost_tyiyn: owner.then_some(r.cost_tyiyn),
                reason: r.reason,
                supplier_id: r.supplier_id,
                supplier_name: r.supplier_name,
                user_name: r.user_name,
                created_at: r.created_at,
                status: r.status,
                resolved_at: r.resolved_at,
                resolution,
            }
        })
        .collect())
}

async fn load_one(conn: &mut PgConnection, user: &CurrentUser, id: Uuid) -> AppResult<DefectOut> {
    load_defects(conn, user, Some(id), None)
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<DefectOut>>> {
    let status = match q.status.as_deref() {
        None | Some("") | Some("all") => None,
        Some(s @ ("pending" | "resolved")) => Some(s),
        Some(_) => return Err(invalid("status: pending, resolved или all")),
    };
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_defects(&mut conn, &user, None, status).await?))
}

/// Поставщик последнего непогашенного прихода товара — его предлагаем по умолчанию.
pub(crate) async fn last_supplier(
    conn: &mut PgConnection,
    branch_id: Uuid,
    product_id: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"select r.supplier_id as "supplier_id!" from receipt_lines l join receipts r on r.id = l.receipt_id
           where r.branch_id = $1 and l.product_id = $2 and l.qty > 0 and r.supplier_id is not null
             and r.reversal_of is null
             and not exists (select 1 from receipts x where x.reversal_of = r.id)
           order by r.created_at desc, l.line_no desc limit 1"#,
        branch_id,
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

#[derive(Deserialize)]
struct HintQuery {
    product_id: Uuid,
}

#[derive(Serialize)]
struct SupplierHint {
    supplier_id: Option<Uuid>,
    supplier_name: Option<String>,
}

async fn supplier_hint(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<HintQuery>,
) -> AppResult<Json<SupplierHint>> {
    let mut conn = state.pool.acquire().await?;
    let id = last_supplier(&mut conn, user.branch_id, q.product_id).await?;
    let name = match id {
        Some(id) => {
            sqlx::query_scalar!("select name from suppliers where id = $1", id)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    Ok(Json(SupplierHint {
        supplier_id: id,
        supplier_name: name,
    }))
}

// ---------- Отметка ----------

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct MarkReq {
    pub op_id: Uuid,
    pub product_id: Uuid,
    /// В единицах товара: штуки, миллилитры или граммы.
    pub qty: i64,
    pub reason: String,
    /// Поставщик; без поля — из последнего прихода товара.
    #[serde(default)]
    pub supplier_id: Option<Uuid>,
    /// Поставщика нет или неизвестен: не подставлять из прихода.
    #[serde(default)]
    pub no_supplier: bool,
}

async fn mark(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<MarkReq>,
) -> AppResult<Json<DefectOut>> {
    let mut tx = state.pool.begin().await?;
    let out = mark_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Количество словами: «2 шт», «4 л», «1,5 кг».
fn qty_words(qty: i64, unit: &str) -> String {
    crate::api::bot_drafts::qty_text(qty, unit)
}

/// Отметить брак (администратор или владелец): товар уходит из остатка по средней себестоимости.
pub async fn mark_tx(conn: &mut PgConnection, ctx: &Ctx, req: MarkReq) -> AppResult<DefectOut> {
    const KIND: &str = "defect.mark";
    if req.qty <= 0 || req.qty > 1_000_000_000 {
        return Err(invalid("количество больше нуля"));
    }
    let reason = req.reason.trim();
    if reason.is_empty() {
        return Err(invalid("укажите причину брака"));
    }
    if reason.chars().count() > 500 {
        return Err(invalid("причина не длиннее 500 знаков"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let p = sqlx::query!(
        "select name, unit, archived from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.archived {
        return Err(invalid("товар в архиве: отметьте брак у основной карточки"));
    }
    let supplier_id = if req.no_supplier {
        None
    } else if let Some(sid) = req.supplier_id {
        sqlx::query_scalar!("select id from suppliers where id = $1", sid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("поставщик не найден"))?;
        Some(sid)
    } else {
        last_supplier(conn, branch_id, req.product_id).await?
    };
    let pool = ops::lock_pool(conn, branch_id, req.product_id).await?;
    ops::ensure_not_merged(conn, &[req.product_id]).await?;
    // Брак отмечается только из того, что есть на складе.
    if pool.qty < req.qty {
        return Err(AppError::Conflict(format!(
            "на складе только {} — брака больше остатка не бывает",
            qty_words(pool.qty.max(0), &p.unit)
        )));
    }
    // Остаток есть, а стоимости у него нет (ушла в ноль или минус): брак — по последней цене
    // закупки, как себестоимость продажи в таком случае; товар остаётся на проверке. Закупок
    // не было вовсе — цену брака не узнать (ADR-070).
    let cost = if pool.value <= 0 {
        if pool.last_qty <= 0 {
            return Err(invalid(format!(
                "у «{}» нет стоимости остатка и ни одной закупки — укажите цену закупки, потом отметьте брак",
                p.name
            )));
        }
        div_round(
            i128::from(req.qty) * i128::from(pool.last_cost),
            i128::from(pool.last_qty),
        )
        .ok_or_else(overflow)?
    } else {
        cost_of(req.qty, &pool).ok_or_else(overflow)?
    };
    let number = ops::next_counter(conn, branch_id, "defect").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into defects (id, branch_id, number, product_id, qty, cost_tyiyn, reason, supplier_id,
                                user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        req.product_id,
        req.qty,
        cost,
        reason,
        supplier_id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: req.product_id,
            qty_delta: -req.qty,
            value_delta: -cost,
            doc_type: "defect",
            doc_id: id,
        },
    )
    .await?;
    let supplier_name = match supplier_id {
        Some(sid) => {
            sqlx::query_scalar!("select name from suppliers where id = $1", sid)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    ops::audit(
        conn,
        ctx,
        KIND,
        "defect",
        Some(id),
        json!({
            "number": number,
            "name": p.name,
            "qty": req.qty,
            "qty_text": qty_words(req.qty, &p.unit),
            "cost": cost,
            "reason": reason,
            "supplier": supplier_name,
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Карточка, на которую вернётся товар, не в архиве: иначе остаток лёг бы на невидимую карточку.
async fn ensure_live_not_archived(conn: &mut PgConnection, product_id: Uuid) -> AppResult<()> {
    let p = sqlx::query!(
        "select name, archived from products where id = $1",
        product_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if p.archived {
        return Err(AppError::Conflict(format!(
            "товар «{}» в архиве — верните карточку из архива, потом повторите",
            p.name
        )));
    }
    Ok(())
}

// ---------- Снять отметку ----------

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CommentReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

struct Locked {
    number: i64,
    product_id: Uuid,
    name: String,
    unit: String,
    qty: i64,
    cost: i64,
    supplier_id: Option<Uuid>,
    status: String,
}

/// Блокирует запись брака до конца транзакции: решение и снятие отметки не идут параллельно.
async fn lock_defect(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<Locked> {
    let d = sqlx::query!(
        r#"select d.number, d.product_id, p.name, p.unit, d.qty, d.cost_tyiyn, d.supplier_id, d.status
           from defects d join products p on p.id = d.product_id
           where d.id = $1 and d.branch_id = $2
           for update of d"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Locked {
        number: d.number,
        product_id: d.product_id,
        name: d.name,
        unit: d.unit,
        qty: d.qty,
        cost: d.cost_tyiyn,
        supplier_id: d.supplier_id,
        status: d.status,
    })
}

async fn unmark(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CommentReq>,
) -> AppResult<Json<DefectOut>> {
    let mut tx = state.pool.begin().await?;
    let out = unmark_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Снять отметку до решения владельца: сторно отметки, товар возвращается в остаток
/// по той же стоимости движением `defect_back`.
pub async fn unmark_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: CommentReq,
) -> AppResult<DefectOut> {
    const KIND: &str = "defect.unmark";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let d = lock_defect(conn, branch_id, id).await?;
    if d.status != "pending" {
        return Err(AppError::Conflict(
            "по браку уже есть решение: сначала отмените его".into(),
        ));
    }
    // Карточку могли влить в другую: товар возвращается на основную.
    let target = ops::live_product(conn, d.product_id).await?;
    ensure_live_not_archived(conn, target).await?;
    ops::lock_pool(conn, branch_id, target).await?;
    ops::ensure_not_merged(conn, &[target]).await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: target,
            qty_delta: d.qty,
            value_delta: d.cost,
            doc_type: "defect_back",
            doc_id: id,
        },
    )
    .await?;
    sqlx::query!(
        "update defects set status = 'cancelled', resolved_at = now() where id = $1",
        id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "defect",
        Some(id),
        json!({
            "number": d.number,
            "name": d.name,
            "qty_text": qty_words(d.qty, &d.unit),
            "cost": d.cost,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

// ---------- Решение владельца ----------

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ResolveReq {
    pub op_id: Uuid,
    /// `write_off`, `replace`, `credit`, `cash`.
    pub kind: String,
    /// Новым товаром: сколько привезли взамен (не больше брака; без поля — столько же).
    #[serde(default)]
    pub replace_qty: Option<i64>,
    /// Скидкой или деньгами: сколько возместил поставщик (без поля — стоимость брака).
    #[serde(default)]
    pub refund_tyiyn: Option<i64>,
    /// Деньгами: в какую кассу.
    #[serde(default)]
    pub account_id: Option<Uuid>,
    /// Кому вернули; без поля — поставщик из отметки.
    #[serde(default)]
    pub supplier_id: Option<Uuid>,
    #[serde(default)]
    pub comment: String,
}

async fn resolve(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ResolveReq>,
) -> AppResult<Json<DefectOut>> {
    let mut tx = state.pool.begin().await?;
    let out = resolve_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Решение по браку — только владелец; одна операция, одна транзакция.
pub async fn resolve_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ResolveReq,
) -> AppResult<DefectOut> {
    const KIND: &str = "defect.resolve";
    ctx.user.require_owner()?;
    if !matches!(
        req.kind.as_str(),
        "write_off" | "replace" | "credit" | "cash"
    ) {
        return Err(invalid("решение: write_off, replace, credit или cash"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let d = lock_defect(conn, branch_id, id).await?;
    if d.status != "pending" {
        return Err(AppError::Conflict(
            "по этому браку решение уже принято или отметку сняли".into(),
        ));
    }
    let supplier_id = if req.kind == "write_off" {
        None
    } else {
        let sid = req
            .supplier_id
            .or(d.supplier_id)
            .ok_or_else(|| invalid("укажите поставщика, которому возвращаем брак"))?;
        sqlx::query_scalar!("select id from suppliers where id = $1", sid)
            .fetch_optional(&mut *conn)
            .await?
            .ok_or_else(|| invalid("поставщик не найден"))?;
        Some(sid)
    };
    let rid = new_id();
    let comment = req.comment.trim();
    // Убыток списания и разница возмещения — строки прибыли (SPEC-25).
    let (mut replace_qty, mut replace_value, mut refund, mut account) = (None, 0i64, None, None);
    let (writeoff, diff) = match req.kind.as_str() {
        "write_off" => (d.cost, 0),
        "replace" => {
            let rq = req.replace_qty.unwrap_or(d.qty);
            if rq <= 0 || rq > d.qty {
                return Err(invalid(format!(
                    "взамен можно получить от 1 до {}",
                    qty_words(d.qty, &d.unit)
                )));
            }
            // Новый товар приходит по стоимости брака; недостающее списывается той же операцией.
            let value = div_round(i128::from(d.cost) * i128::from(rq), i128::from(d.qty))
                .ok_or_else(overflow)?;
            replace_qty = Some(rq);
            replace_value = value;
            (d.cost.checked_sub(value).ok_or_else(overflow)?, 0)
        }
        _ => {
            let r = req.refund_tyiyn.unwrap_or(d.cost);
            if r <= 0 {
                return Err(invalid("сумма возмещения больше нуля"));
            }
            ops::check_amount(r)?;
            refund = Some(r);
            if req.kind == "cash" {
                let acc_id = req
                    .account_id
                    .ok_or_else(|| invalid("выберите кассу, куда пришли деньги"))?;
                let acc = sqlx::query!(
                    "select active from cash_accounts where id = $1 and branch_id = $2",
                    acc_id,
                    branch_id
                )
                .fetch_optional(&mut *conn)
                .await?
                .ok_or_else(|| invalid("касса не найдена"))?;
                if !acc.active {
                    return Err(invalid("касса отключена"));
                }
                account = Some(acc_id);
            }
            (0, r.checked_sub(d.cost).ok_or_else(overflow)?)
        }
    };
    sqlx::query!(
        r#"insert into defect_resolutions (id, branch_id, defect_id, kind, supplier_id, replace_qty,
                                           replace_tyiyn, refund_tyiyn, account_id, writeoff_tyiyn,
                                           diff_tyiyn, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
        rid,
        branch_id,
        id,
        req.kind,
        supplier_id,
        replace_qty,
        replace_value,
        refund,
        account,
        writeoff,
        diff,
        comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    let note = format!("брак № {}: {}", d.number, d.name);
    if let Some(rq) = replace_qty {
        let target = ops::live_product(conn, d.product_id).await?;
        ensure_live_not_archived(conn, target).await?;
        ops::lock_pool(conn, branch_id, target).await?;
        ops::ensure_not_merged(conn, &[target]).await?;
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: target,
                qty_delta: rq,
                value_delta: replace_value,
                doc_type: "defect_replace",
                doc_id: rid,
            },
        )
        .await?;
    }
    if let (Some(r), Some(sid), "credit") = (refund, supplier_id, req.kind.as_str()) {
        // Поставщик должен нам: его баланс растёт, следующая накладная в долг зачтётся сама.
        crate::api::parties::add_ledger(
            conn,
            ctx,
            crate::api::parties::LedgerEntry {
                party_id: sid,
                kind: "debt",
                amount: r,
                doc_type: "defect",
                doc_id: Some(rid),
                comment: &note,
            },
        )
        .await
        .map_err(|e| match e {
            AppError::NotFound => invalid("поставщик не найден в этом филиале"),
            other => other,
        })?;
    }
    if let (Some(r), Some(acc)) = (refund, account) {
        crate::api::cash::require_open_shift(conn, branch_id, acc).await?;
        crate::api::cash::add_movement(
            conn,
            ctx,
            crate::api::cash::CashEntry {
                account_id: acc,
                kind: "supplier_refund",
                amount: r,
                doc_type: "defect",
                doc_id: Some(rid),
                comment: &note,
            },
        )
        .await?;
    }
    let status = if req.kind == "write_off" {
        "written_off"
    } else {
        "returned"
    };
    sqlx::query!(
        "update defects set status = $2, resolved_at = now() where id = $1",
        id,
        status
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "defect",
        Some(id),
        json!({
            "number": d.number,
            "name": d.name,
            "kind": req.kind,
            "qty_text": qty_words(d.qty, &d.unit),
            "cost": d.cost,
            "replace_qty_text": replace_qty.map(|q| qty_words(q, &d.unit)),
            "refund": refund,
            "writeoff": writeoff,
            "diff": diff,
            "resolution": rid,
            "comment": comment,
        }),
    )
    .await?;
    let out = load_one(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

// ---------- Сторно решения ----------

async fn reverse(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CommentReq>,
) -> AppResult<Json<DefectOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно решения (владелец): склад, деньги и долг откатываются, запись брака снова ждёт решения.
pub async fn reverse_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: CommentReq,
) -> AppResult<DefectOut> {
    const KIND: &str = "defect.resolve_reverse";
    ctx.user.require_owner()?;
    let comment = req.comment.trim();
    if comment.is_empty() {
        return Err(invalid("укажите причину отмены решения"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let d = lock_defect(conn, branch_id, id).await?;
    if !matches!(d.status.as_str(), "written_off" | "returned") {
        return Err(AppError::Conflict("по браку нет решения".into()));
    }
    let r = sqlx::query!(
        r#"select x.id, x.kind, x.supplier_id, x.replace_qty, x.replace_tyiyn, x.refund_tyiyn,
                  x.account_id, x.writeoff_tyiyn, x.diff_tyiyn
           from defect_resolutions x
           where x.defect_id = $1 and x.reversal_of is null
             and not exists (select 1 from defect_resolutions y where y.reversal_of = x.id)
           order by x.created_at desc limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| AppError::Internal("решение по браку не найдено".into()))?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into defect_resolutions (id, branch_id, defect_id, kind, supplier_id, replace_qty,
                                           replace_tyiyn, refund_tyiyn, account_id, writeoff_tyiyn,
                                           diff_tyiyn, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
        rid,
        branch_id,
        id,
        r.kind,
        r.supplier_id,
        r.replace_qty.map(|q| -q),
        -r.replace_tyiyn,
        r.refund_tyiyn.map(|v| -v),
        r.account_id,
        -r.writeoff_tyiyn,
        -r.diff_tyiyn,
        comment,
        r.id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("решение уже отменено".into())
        } else {
            AppError::from(e)
        }
    })?;
    let note = format!("отмена решения по браку № {}", d.number);
    let mut negative = None;
    if let Some(rq) = r.replace_qty {
        // Полученный взамен товар уходит со склада по той же стоимости.
        let target = ops::live_product(conn, d.product_id).await?;
        let pool = ops::lock_pool(conn, branch_id, target).await?;
        ops::ensure_not_merged(conn, &[target]).await?;
        // Товар взамен уже продан: остаток уйдёт в минус — проводим, владельцу уведомление,
        // как при сторно прихода (ADR-065, ADR-070).
        if rq > 0 && pool.qty < rq {
            let name = sqlx::query_scalar!("select name from products where id = $1", target)
                .fetch_one(&mut *conn)
                .await?;
            negative = Some(json!([{
                "name": name,
                "stock": pool.qty,
                "qty": rq,
                "after": pool.qty - rq,
            }]));
        }
        ops::apply_movement(
            conn,
            Movement {
                branch_id,
                product_id: target,
                qty_delta: -rq,
                value_delta: -r.replace_tyiyn,
                doc_type: "defect_replace",
                doc_id: rid,
            },
        )
        .await?;
    }
    if let (Some(v), Some(sid), "credit") = (r.refund_tyiyn, r.supplier_id, r.kind.as_str()) {
        crate::api::parties::add_ledger(
            conn,
            ctx,
            crate::api::parties::LedgerEntry {
                party_id: sid,
                kind: "debt",
                amount: -v,
                doc_type: "defect_reversal",
                doc_id: Some(rid),
                comment: &note,
            },
        )
        .await?;
    }
    if let (Some(v), Some(acc), "cash") = (r.refund_tyiyn, r.account_id, r.kind.as_str()) {
        crate::api::cash::require_open_shift(conn, branch_id, acc).await?;
        crate::api::cash::add_movement(
            conn,
            ctx,
            crate::api::cash::CashEntry {
                account_id: acc,
                kind: "reversal",
                amount: -v,
                doc_type: "defect_reversal",
                doc_id: Some(rid),
                comment: &note,
            },
        )
        .await?;
    }
    sqlx::query!(
        "update defects set status = 'pending', resolved_at = null where id = $1",
        id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "defect",
        Some(id),
        json!({
            "number": d.number,
            "name": d.name,
            "kind": r.kind,
            "resolution": r.id,
            "reversal": rid,
            "comment": comment,
        }),
    )
    .await?;
    if let Some(lines) = negative {
        ops::audit(
            conn,
            ctx,
            "defect.reverse_negative",
            "defect",
            Some(id),
            json!({ "number": d.number, "reversal": rid, "lines": lines }),
        )
        .await?;
    }
    let out = load_one(conn, &ctx.user, id).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Решение словами — для уведомлений, бота и экрана.
pub(crate) fn kind_words(kind: &str) -> &'static str {
    match kind {
        "write_off" => "списан в убыток",
        "replace" => "возвращён поставщику: новый товар",
        "credit" => "возвращён поставщику: скидка на следующий приход",
        "cash" => "возвращён поставщику: деньгами",
        _ => "решение",
    }
}
