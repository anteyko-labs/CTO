//! Старые аккумуляторы на вес: приём за наличные и продажа по средней цене кг (ADR-048).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::{self, CashEntry};
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/batteries", routing::get(info))
        .route("/batteries/intake", routing::post(intake))
        .route("/batteries/intakes", routing::get(list_intakes))
        .route(
            "/batteries/intakes/{id}/reverse",
            routing::post(reverse_intake),
        )
        .route("/settings/batteries", routing::put(put_settings))
}

/// Цена приёма по умолчанию — 100 сом за кг (BLUEPRINT §6, вопрос 24), правится владельцем.
const DEFAULT_INTAKE_PER_KG: i64 = 10_000;
const SCRAP_NAME: &str = "Аккумуляторы б/у (на вес)";

/// Карточка «аккумуляторы на вес»: заводится сама при первом обращении, с внутренним штрихкодом.
pub async fn scrap_product(conn: &mut PgConnection) -> AppResult<Uuid> {
    if let Some(id) = sqlx::query_scalar!(
        "select id from products where unit = 'g' and not archived order by created_at limit 1"
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(id);
    }
    // Один заводит — остальные ждут: две карточки «на вес» не появятся.
    sqlx::query!("select pg_advisory_xact_lock(hashtextextended('scrap_product', 0))")
        .execute(&mut *conn)
        .await?;
    if let Some(id) = sqlx::query_scalar!(
        "select id from products where unit = 'g' and not archived order by created_at limit 1"
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(id);
    }
    let category = match sqlx::query_scalar!(
        "select id from categories where kind = 'battery' order by name limit 1"
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        Some(c) => c,
        None => {
            let c = new_id();
            sqlx::query!(
                "insert into categories (id, name, kind) values ($1, 'Аккумуляторы', 'battery')",
                c
            )
            .execute(&mut *conn)
            .await?;
            c
        }
    };
    let id = new_id();
    sqlx::query!(
        "insert into products (id, category_id, name, unit) values ($1, $2, $3, 'g')",
        id,
        category,
        SCRAP_NAME
    )
    .execute(&mut *conn)
    .await?;
    let code = crate::api::catalog::next_internal_code(conn).await?;
    crate::api::catalog::insert_barcode(conn, id, &code, true).await?;
    Ok(id)
}

async fn intake_price(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i64> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'batteries'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| {
        v.get("intake_per_kg_tyiyn")
            .and_then(serde_json::Value::as_i64)
    })
    .unwrap_or(DEFAULT_INTAKE_PER_KG))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BatteryInfo {
    pub product_id: Uuid,
    pub name: String,
    pub stock_g: i64,
    /// Средняя закупка за кг. Показывается и администратору: цены приёма он вводит сам,
    /// как закупочные в приходе (ADR-008, ADR-048), и по ней видно, дешевле чего не продать.
    pub avg_per_kg_tyiyn: Option<i64>,
    pub intake_per_kg_tyiyn: i64,
    pub sale_per_kg_tyiyn: i64,
}

async fn load_info(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<BatteryInfo> {
    let id = scrap_product(conn).await?;
    let r = sqlx::query!(
        r#"select p.name, coalesce(bp.stock_qty, 0) as "qty!", coalesce(bp.stock_value_tyiyn, 0) as "value!",
                  coalesce(bp.sale_price_tyiyn, 0) as "sale!"
           from products p left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
           where p.id = $1"#,
        id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(BatteryInfo {
        product_id: id,
        name: r.name,
        stock_g: r.qty,
        avg_per_kg_tyiyn: (r.qty > 0)
            .then(|| div_round(i128::from(r.value) * 1000, i128::from(r.qty)))
            .flatten(),
        intake_per_kg_tyiyn: intake_price(conn, branch_id).await?,
        sale_per_kg_tyiyn: r.sale,
    })
}

async fn info(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<BatteryInfo>> {
    let mut tx = state.pool.begin().await?;
    let out = load_info(&mut tx, user.branch_id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct IntakeReq {
    pub op_id: Uuid,
    pub grams: i64,
    pub price_per_kg_tyiyn: i64,
    #[serde(default)]
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct IntakeOut {
    pub id: Uuid,
    pub number: i64,
    pub amount_tyiyn: i64,
    pub info: BatteryInfo,
}

/// Приём: вес × цена за кг выдаётся из кассы смены, на склад — граммы с этой стоимостью.
/// Средняя за кг складывается из всех приёмов (ADR-011), продать дешевле неё нельзя (ADR-042).
pub async fn intake_tx(conn: &mut PgConnection, ctx: &Ctx, req: IntakeReq) -> AppResult<IntakeOut> {
    const KIND: &str = "battery.intake";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if !(1..=10_000_000).contains(&req.grams) {
        return Err(invalid("вес от 1 г до 10 т"));
    }
    if req.price_per_kg_tyiyn < 0 {
        return Err(invalid("цена не может быть меньше нуля"));
    }
    ops::check_amount(req.price_per_kg_tyiyn)?;
    let amount = div_round(
        i128::from(req.grams) * i128::from(req.price_per_kg_tyiyn),
        1000,
    )
    .ok_or_else(crate::error::overflow)?;
    ops::check_amount(amount)?;
    let branch_id = ctx.user.branch_id;
    let product = scrap_product(conn).await?;
    let till = cash::default_account(conn, branch_id).await?;
    cash::require_open_shift(conn, branch_id, till).await?;
    ops::lock_pool(conn, branch_id, product).await?;
    let number = ops::next_counter(conn, branch_id, "battery_intake").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into battery_intakes (id, branch_id, number, product_id, grams, price_per_kg_tyiyn,
                                        amount_tyiyn, account_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        id,
        branch_id,
        number,
        product,
        req.grams,
        req.price_per_kg_tyiyn,
        amount,
        till,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    if amount > 0 {
        cash::add_movement(
            conn,
            ctx,
            CashEntry {
                account_id: till,
                kind: "battery_intake",
                amount: -amount,
                doc_type: "battery_intake",
                doc_id: Some(id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: product,
            qty_delta: req.grams,
            value_delta: amount,
            doc_type: "battery_intake",
            doc_id: id,
        },
    )
    .await?;
    // Последняя цена приёма — запасная себестоимость, если продадут больше, чем приняли.
    sqlx::query!(
        "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
        branch_id,
        product,
        req.grams,
        amount
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "product",
        Some(product),
        json!({ "number": number, "grams": req.grams, "price_per_kg": req.price_per_kg_tyiyn, "amount": amount }),
    )
    .await?;
    let out = IntakeOut {
        id,
        number,
        amount_tyiyn: amount,
        info: load_info(conn, branch_id).await?,
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn intake(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<IntakeReq>,
) -> AppResult<Json<IntakeOut>> {
    let mut tx = state.pool.begin().await?;
    let out = intake_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct SettingsReq {
    intake_per_kg_tyiyn: i64,
}

/// Цену приёма по умолчанию правит владелец; цена продажи за кг — обычной правкой цены товара.
async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsReq>,
) -> AppResult<Json<BatteryInfo>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.intake_per_kg_tyiyn < 0 {
        return Err(invalid("цена не может быть меньше нуля"));
    }
    ops::check_amount(req.intake_per_kg_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    let value = json!({ "intake_per_kg_tyiyn": req.intake_per_kg_tyiyn });
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'batteries', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(&mut tx, &ctx, "settings.batteries", "settings", None, value).await?;
    let out = load_info(&mut tx, ctx.user.branch_id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct IntakeRow {
    id: Uuid,
    number: i64,
    grams: i64,
    price_per_kg_tyiyn: i64,
    amount_tyiyn: i64,
    comment: String,
    user_name: String,
    created_at: DateTime<Utc>,
    reversed: bool,
}

/// Открытая смена кассы и время её открытия: приёмы после него — приёмы этой смены.
async fn open_shift_start(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
) -> AppResult<Option<DateTime<Utc>>> {
    let Some(shift) = cash::open_shift_id(conn, branch_id, account_id).await? else {
        return Ok(None);
    };
    Ok(Some(
        sqlx::query_scalar!("select opened_at from shifts where id = $1", shift)
            .fetch_one(&mut *conn)
            .await?,
    ))
}

/// Приёмы: владельцу — последние 50, администратору — приёмы открытой смены (их он может отменить).
async fn list_intakes(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<IntakeRow>>> {
    let mut tx = state.pool.begin().await?;
    let till = cash::default_account(&mut tx, user.branch_id).await?;
    let since = if user.is_owner() {
        None
    } else {
        match open_shift_start(&mut tx, user.branch_id, till).await? {
            Some(t) => Some(t),
            None => return Ok(Json(Vec::new())),
        }
    };
    let rows = sqlx::query_as!(
        IntakeRow,
        r#"select i.id, i.number, i.grams, i.price_per_kg_tyiyn, i.amount_tyiyn, i.comment,
                  u.full_name as "user_name!", i.created_at,
                  exists (select 1 from battery_intake_reversals r where r.intake_id = i.id) as "reversed!"
           from battery_intakes i join users u on u.id = i.user_id
           where i.branch_id = $1 and ($2::timestamptz is null or i.created_at >= $2)
           order by i.created_at desc limit 50"#,
        user.branch_id,
        since
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct IntakeReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Отмена приёма (ADR-065): вес уходит со склада той же стоимостью, деньги возвращаются в ту
/// же кассу. Документ приёма не меняется, отмена — отдельной записью, второй раз — 409.
/// Администратор отменяет только приём своей открытой смены, владелец — любой.
pub async fn reverse_intake_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: IntakeReverseReq,
) -> AppResult<IntakeOut> {
    const KIND: &str = "battery.intake_reverse";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let it = sqlx::query!(
        r#"select number, product_id, grams, amount_tyiyn, account_id, created_at
           from battery_intakes where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let reversed = sqlx::query_scalar!(
        r#"select exists (select 1 from battery_intake_reversals where intake_id = $1) as "v!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if reversed {
        return Err(AppError::Conflict("приём уже отменён".into()));
    }
    if !ctx.user.is_owner() {
        let since = open_shift_start(conn, branch_id, it.account_id).await?;
        if since.is_none_or(|t| it.created_at < t) {
            return Err(AppError::Forbidden);
        }
    }
    // Деньги возвращаются в кассу смены — только в открытую смену, как сторно выплаты.
    cash::require_open_shift(conn, branch_id, it.account_id).await?;
    let product = ops::live_product(conn, it.product_id).await?;
    let pool = ops::lock_pool(conn, branch_id, product).await?;
    // Принятое уже продали — отменять нечего: остаток ушёл бы в минус.
    if pool.qty < it.grams {
        return Err(AppError::Conflict(format!(
            "на складе {} г, а приняли {} г: часть уже продана, приём не отменить",
            pool.qty, it.grams
        )));
    }
    let rid = new_id();
    sqlx::query!(
        r#"insert into battery_intake_reversals (id, branch_id, intake_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        rid,
        branch_id,
        id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("приём уже отменён".into())
        } else {
            AppError::from(e)
        }
    })?;
    // Ошибочная цена не должна остаться «последней закупочной»: берём её из последнего
    // неотменённого приёма, иначе обнуляем — как при сторно прихода (SPEC-03).
    let last = sqlx::query!(
        r#"select grams, amount_tyiyn from battery_intakes i
           where i.branch_id = $1 and i.id <> $2
             and not exists (select 1 from battery_intake_reversals r where r.intake_id = i.id)
           order by i.created_at desc limit 1"#,
        branch_id,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    sqlx::query!(
        "update branch_products set last_cost_qty = $3, last_cost_tyiyn = $4 where branch_id = $1 and product_id = $2",
        branch_id,
        product,
        last.as_ref().map_or(0, |r| r.grams),
        last.as_ref().map_or(0, |r| r.amount_tyiyn)
    )
    .execute(&mut *conn)
    .await?;
    ops::apply_movement(
        conn,
        Movement {
            branch_id,
            product_id: product,
            qty_delta: -it.grams,
            value_delta: -it.amount_tyiyn,
            doc_type: "battery_intake_reversal",
            doc_id: rid,
        },
    )
    .await?;
    if it.amount_tyiyn > 0 {
        cash::add_movement(
            conn,
            ctx,
            CashEntry {
                account_id: it.account_id,
                kind: "reversal",
                amount: it.amount_tyiyn,
                doc_type: "battery_intake_reversal",
                doc_id: Some(rid),
                comment: &format!("отмена приёма аккумуляторов № {}", it.number),
            },
        )
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "battery_intake",
        Some(id),
        json!({
            "number": it.number,
            "grams": it.grams,
            "amount": it.amount_tyiyn,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    let out = IntakeOut {
        id: rid,
        number: it.number,
        amount_tyiyn: it.amount_tyiyn,
        info: load_info(conn, branch_id).await?,
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn reverse_intake(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<IntakeReverseReq>,
) -> AppResult<Json<IntakeOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_intake_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}
