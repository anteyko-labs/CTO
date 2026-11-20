//! Временный разбор остатков из Умага (задача 042, дополняет SPEC-20 и SPEC-15): строка выгрузки
//! с количеством за 1–2 нажатия становится товаром учёта («Новый товар») или привязывается к
//! уже заведённому («Уже есть в Автодом»), а её количество записывается в черновик ревизии
//! «весь склад». Остатки встают, когда владелец проводит ревизию.

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{CreateProductReq, ProductOut, SimilarItem, escape_like, product_by_id};
use crate::api::old_catalog::{
    Hit, RIGHT, link_row, load_hits, lock_branch, product_unit, remember_reject, require_enabled,
    row_ref, sees_umag_cost,
};
use crate::api::paging::{PageQuery, Paged, paged};
use crate::api::revisions;
use crate::auth::{Ctx, CurrentUser, require_permission};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// Журнал и операции разбора.
const KIND: &str = "stock.umag_intake";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/old-catalog/intake", routing::get(list))
        .route("/old-catalog/{id}/intake/new", routing::post(intake_new))
        .route("/old-catalog/{id}/intake/link", routing::post(intake_link))
        .route("/old-catalog/{id}/intake/skip", routing::post(intake_skip))
        .route("/old-catalog/{id}/intake/undo", routing::post(intake_undo))
        .route("/old-catalog/{id}/similar", routing::get(similar))
}

/// Количество строки Умага в единицах товара: литры строки — миллилитры, штуки масла — канистры
/// (объём тары), кг и м — граммы и миллиметры; остальное — штуки. Округляется до целого.
pub fn base_qty(qty: f64, umag_unit: &str, unit: &str, container_ml: Option<i64>) -> i64 {
    let umag = product_unit(umag_unit);
    let k = match unit {
        "ml" if umag == "ml" => 1000.0,
        "ml" => container_ml.unwrap_or(1000) as f64,
        "g" | "mm" if umag == unit => 1000.0,
        _ => 1.0,
    };
    (qty * k).round().clamp(0.0, 1e9) as i64
}

#[derive(Deserialize)]
struct ListQuery {
    /// `todo` (по умолчанию), `new`, `linked`, `skipped`, `all`.
    #[serde(default)]
    status: String,
    #[serde(default)]
    q: String,
    // Не `flatten`: числа строки запроса во вложенной структуре не читаются.
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize)]
struct IntakeRow {
    #[serde(flatten)]
    hit: Hit,
    qty: Option<f64>,
    intake_status: String,
    intake_product_id: Option<Uuid>,
    intake_product_name: Option<String>,
    /// Записано в черновик ревизии, в единицах товара; `None` — не записано (нет черновика).
    intake_counted: Option<i64>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Paged<IntakeRow>> {
    let mut conn = state.pool.acquire().await?;
    require_permission(&mut conn, &user, RIGHT).await?;
    let status = match q.status.as_str() {
        "" => "todo",
        s @ ("todo" | "new" | "linked" | "skipped" | "all") => s,
        _ => return Err(invalid("неизвестный отбор")),
    };
    let page = PageQuery {
        limit: q.limit,
        offset: q.offset,
    };
    let like = format!("%{}%", escape_like(q.q.trim()));
    let rows = sqlx::query!(
        r#"select o.id, o.qty::float8 as qty, o.intake_status, o.intake_product_id,
                  p.name as "intake_product_name?", o.intake_counted,
                  count(*) over () as "total!"
           from old_catalog o left join products p on p.id = o.intake_product_id
           where o.branch_id = $1 and ($2 = 'all' or o.intake_status = $2)
             and ($3 = '%%' or o.name ilike $3 or o.barcode ilike $3 or o.article ilike $3)
           order by o.name, o.id
           limit $4 offset $5"#,
        user.branch_id,
        status,
        like,
        page.size(100),
        page.skip()
    )
    .fetch_all(&mut *conn)
    .await?;
    let total = rows.first().map_or(0, |r| r.total);
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let cost = sees_umag_cost(&mut conn, &user).await?;
    let hits = load_hits(&mut conn, user.branch_id, &ids, &Default::default(), cost).await?;
    let out = hits
        .into_iter()
        .zip(rows)
        .map(|(hit, r)| IntakeRow {
            hit,
            qty: r.qty,
            intake_status: r.intake_status,
            intake_product_id: r.intake_product_id,
            intake_product_name: r.intake_product_name,
            intake_counted: r.intake_counted,
        })
        .collect();
    Ok(paged(out, total))
}

/// Строка для разбора: ещё не разобрана, под замком.
struct Row {
    name: String,
    qty: Option<f64>,
    unit: String,
    status: String,
    product_id: Option<Uuid>,
    revision_id: Option<Uuid>,
    counted: Option<i64>,
}

async fn lock_row(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<Row> {
    let r = sqlx::query!(
        r#"select name, qty::float8 as qty, unit, intake_status, intake_product_id,
                  intake_revision_id, intake_counted
           from old_catalog where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Row {
        name: r.name,
        qty: r.qty,
        unit: r.unit,
        status: r.intake_status,
        product_id: r.intake_product_id,
        revision_id: r.intake_revision_id,
        counted: r.intake_counted,
    })
}

fn require_todo(row: &Row) -> AppResult<()> {
    if row.status == "todo" {
        Ok(())
    } else {
        Err(AppError::Conflict(
            "строка уже разобрана — сначала «Отменить»".into(),
        ))
    }
}

#[derive(Serialize, Deserialize)]
struct IntakeOut {
    intake_status: String,
    product: Option<ProductOut>,
    /// Черновик ревизии, куда записано количество; `None` — количество ждёт.
    revision_id: Option<Uuid>,
    intake_counted: Option<i64>,
}

/// Количество строки — в черновик «весь склад». Возвращает черновик и записанное.
async fn count_into_draft(
    conn: &mut PgConnection,
    ctx: &Ctx,
    row: &Row,
    product_id: Uuid,
) -> AppResult<(Option<Uuid>, Option<i64>)> {
    let Some(qty) = row.qty else {
        return Ok((None, None));
    };
    let p = sqlx::query!(
        "select unit, container_ml from products where id = $1",
        product_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let counted = base_qty(qty, &row.unit, &p.unit, p.container_ml);
    let Some(rev) = revisions::whole_draft_for_intake(&mut *conn, ctx).await? else {
        return Ok((None, None));
    };
    revisions::add_intake_count(&mut *conn, ctx, rev, product_id, counted).await?;
    Ok((Some(rev), Some(counted)))
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    row: &Row,
    status: &str,
    product_id: Option<Uuid>,
    revision_id: Option<Uuid>,
    counted: Option<i64>,
) -> AppResult<()> {
    sqlx::query!(
        r#"update old_catalog set intake_status = $2, intake_product_id = $3,
                                  intake_revision_id = $4, intake_counted = $5
           where id = $1"#,
        id,
        status,
        product_id,
        revision_id,
        counted
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "product",
        product_id.or(row.product_id),
        json!({
            "umag_id": id,
            "umag_name": row.name,
            "status": status,
            "from": row.status,
            "qty": row.qty,
            "revision_id": revision_id.or(row.revision_id),
            "counted": counted.or(row.counted),
        }),
    )
    .await
}

/// Начало операции разбора: право, Умаг включён, повтор `op_id` — прежний ответ, замки.
async fn begin(
    tx: &mut PgConnection,
    ctx: &Ctx,
    op_id: Uuid,
    kind: &str,
) -> AppResult<Option<IntakeOut>> {
    require_permission(&mut *tx, &ctx.user, RIGHT).await?;
    require_enabled(&mut *tx, ctx.user.branch_id).await?;
    if let Some(done) = ops::begin_op(&mut *tx, ctx, op_id, kind).await? {
        return Ok(Some(done));
    }
    // Порядок замков — как у «Завести» в ревизии: черновики филиала, затем список Умага.
    revisions::lock_branch_drafts(&mut *tx, ctx.user.branch_id).await?;
    Ok(None)
}

/// «Новый товар»: карточка из строки (тело — как `POST /products`), коды строки — коды товара.
async fn intake_new(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(mut req): Json<CreateProductReq>,
) -> AppResult<Json<IntakeOut>> {
    const OP: &str = "stock.umag_intake.new";
    let op_id = req.op_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = begin(&mut tx, &ctx, op_id, OP).await? {
        return Ok(Json(done));
    }
    let row = lock_row(&mut tx, ctx.user.branch_id, id).await?;
    require_todo(&row)?;
    if req.sale_price_tyiyn <= 0 {
        return Err(invalid("укажите цену продажи"));
    }
    req.op_id = new_id();
    req.old_catalog_id = Some(id);
    let p = crate::api::catalog::create_product_tx(&mut tx, &ctx, req).await?;
    let (rev, counted) = count_into_draft(&mut tx, &ctx, &row, p.id).await?;
    finish(&mut tx, &ctx, id, &row, "new", Some(p.id), rev, counted).await?;
    let out = IntakeOut {
        intake_status: "new".into(),
        product: Some(product_by_id(&mut tx, &ctx.user, p.id).await?),
        revision_id: rev,
        intake_counted: counted,
    };
    ops::finish_op(&mut tx, &ctx, op_id, OP, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct LinkReq {
    op_id: Uuid,
    product_id: Uuid,
}

/// «Это он»: коды строки — товару, количество — в черновик (прибавкой, ADR-059).
async fn intake_link(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LinkReq>,
) -> AppResult<Json<IntakeOut>> {
    const OP: &str = "stock.umag_intake.link";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = begin(&mut tx, &ctx, req.op_id, OP).await? {
        return Ok(Json(done));
    }
    lock_branch(&mut tx, ctx.user.branch_id).await?;
    let row = lock_row(&mut tx, ctx.user.branch_id, id).await?;
    require_todo(&row)?;
    link_row(&mut tx, &ctx, id, req.product_id, "manual").await?;
    let product_id = ops::live_product(&mut tx, req.product_id).await?;
    let (rev, counted) = count_into_draft(&mut tx, &ctx, &row, product_id).await?;
    finish(
        &mut tx,
        &ctx,
        id,
        &row,
        "linked",
        Some(product_id),
        rev,
        counted,
    )
    .await?;
    let out = IntakeOut {
        intake_status: "linked".into(),
        product: Some(product_by_id(&mut tx, &ctx.user, product_id).await?),
        revision_id: rev,
        intake_counted: counted,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, OP, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct OpReq {
    op_id: Uuid,
}

/// «Пропустить»: строка не нужна (услуга, давно нет в продаже).
async fn intake_skip(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<OpReq>,
) -> AppResult<Json<IntakeOut>> {
    const OP: &str = "stock.umag_intake.skip";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = begin(&mut tx, &ctx, req.op_id, OP).await? {
        return Ok(Json(done));
    }
    let row = lock_row(&mut tx, ctx.user.branch_id, id).await?;
    require_todo(&row)?;
    finish(&mut tx, &ctx, id, &row, "skipped", None, None, None).await?;
    let out = IntakeOut {
        intake_status: "skipped".into(),
        product: None,
        revision_id: None,
        intake_counted: None,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, OP, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// «Отменить» (задача 042 §4): строка снова «Не разобрано», связь с товаром снимается (коды
/// остаются у товара, как у «Отвязать»), записанное количество убирается из черновика;
/// заведённый товар остаётся. Ревизию уже провели — 409.
async fn intake_undo(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<OpReq>,
) -> AppResult<Json<IntakeOut>> {
    const OP: &str = "stock.umag_intake.undo";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = begin(&mut tx, &ctx, req.op_id, OP).await? {
        return Ok(Json(done));
    }
    lock_branch(&mut tx, ctx.user.branch_id).await?;
    let row = lock_row(&mut tx, ctx.user.branch_id, id).await?;
    if row.status == "todo" {
        return Err(AppError::Conflict("строка ещё не разобрана".into()));
    }
    if let (Some(rev), Some(counted), Some(product)) =
        (row.revision_id, row.counted, row.product_id)
    {
        revisions::remove_intake_count(&mut tx, &ctx, rev, product, counted).await?;
    }
    let link = row_ref(&mut tx, ctx.user.branch_id, id).await?;
    if let Some(product_id) = link.product_id {
        sqlx::query!(
            "update old_catalog set product_id = null, code_conflicts = '[]' where id = $1",
            id
        )
        .execute(&mut *tx)
        .await?;
        remember_reject(&mut tx, ctx.user.branch_id, &link.key, product_id).await?;
    }
    finish(&mut tx, &ctx, id, &row, "todo", None, None, None).await?;
    let out = IntakeOut {
        intake_status: "todo".into(),
        product: None,
        revision_id: None,
        intake_counted: None,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, OP, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Похожие товары общей базы для строки Умага: тот же подбор, что «Похожие в базе» (задача 041):
/// коды строки, артикул, слова названия; совпавшие по коду — сверху.
async fn similar(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<SimilarItem>>> {
    let mut conn = state.pool.acquire().await?;
    require_permission(&mut conn, &user, RIGHT).await?;
    let r = sqlx::query!(
        "select name, article, keys from old_catalog where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(
        crate::api::catalog::similar_to(&mut conn, Uuid::nil(), &r.name, &r.article, &r.keys)
            .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::base_qty;

    #[test]
    fn umag_qty_in_product_units() {
        assert_eq!(base_qty(3.0, "шт", "piece", None), 3);
        assert_eq!(base_qty(2.5, "л", "ml", Some(4000)), 2500);
        assert_eq!(base_qty(2.0, "шт", "ml", Some(4000)), 8000);
        assert_eq!(base_qty(1.25, "кг", "g", None), 1250);
        assert_eq!(base_qty(-1.0, "шт", "piece", None), 0);
    }
}
