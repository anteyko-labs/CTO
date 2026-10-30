//! Запомненный выбор товара по накладной поставщика (задача 002, П3; SPEC-30 §9).
//!
//! В «Новом приходе» по фото человек выбирает товар для строки накладной («похож», несколько
//! точных совпадений или не найден) — выбор запоминается: «название строки у поставщика → товар».
//! В следующий раз такая строка этого поставщика совпадает точно. Запоминает филиал (инвариант
//! 16), список — в карточке поставщика, удаляют там же. Всё — в журнале действий.

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::receipt_drafts::line_key;
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/suppliers/{id}/line-map", routing::get(list))
        .route("/supplier-line-map", routing::post(remember))
        .route("/supplier-line-map/{id}/remove", routing::post(remove))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MapOut {
    pub id: Uuid,
    pub supplier_id: Uuid,
    pub name: String,
    pub product_id: Uuid,
    pub product_name: String,
    pub product_article: String,
    pub user_name: String,
    pub updated_at: DateTime<Utc>,
}

async fn load(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<MapOut> {
    sqlx::query_as!(
        MapOut,
        r#"select m.id, m.supplier_id, m.name, m.product_id, p.name as product_name,
                  p.article as product_article, u.full_name as user_name, m.updated_at
           from supplier_line_map m
           join products p on p.id = m.product_id
           join users u on u.id = m.user_id
           where m.id = $1 and m.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

/// Запомненное у поставщика в филиале — для его карточки.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(supplier_id): Path<Uuid>,
) -> AppResult<Json<Vec<MapOut>>> {
    let rows = sqlx::query_as!(
        MapOut,
        r#"select m.id, m.supplier_id, m.name, m.product_id, p.name as product_name,
                  p.article as product_article, u.full_name as user_name, m.updated_at
           from supplier_line_map m
           join products p on p.id = m.product_id
           join users u on u.id = m.user_id
           where m.branch_id = $1 and m.supplier_id = $2
           order by m.name_key"#,
        user.branch_id,
        supplier_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct RememberReq {
    op_id: Uuid,
    supplier_id: Uuid,
    /// Название строки, как в накладной.
    name: String,
    product_id: Uuid,
}

/// Запомнить выбор из «Нового прихода»: право «Приход товара». Такое же название у этого
/// поставщика — выбор заменяется. Повтор с тем же `op_id` — тот же ответ.
async fn remember(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RememberReq>,
) -> AppResult<Json<MapOut>> {
    const KIND: &str = "supplier_line_map.set";
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "receipts").await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let name: String = req
        .name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(200)
        .collect();
    let key = line_key(&name);
    if key.is_empty() {
        return Err(invalid("название строки накладной пустое"));
    }
    // Поставщики и каталог общие для филиалов (SPEC-33).
    let supplier = sqlx::query_scalar!("select name from suppliers where id = $1", req.supplier_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("поставщик не найден"))?;
    let product = sqlx::query!(
        "select name, archived from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    if product.archived {
        return Err(invalid("товар в архиве — выберите действующий"));
    }
    let branch_id = ctx.user.branch_id;
    let was = sqlx::query!(
        r#"select m.id, m.product_id, p.name as product_name from supplier_line_map m
           join products p on p.id = m.product_id
           where m.branch_id = $1 and m.supplier_id = $2 and m.name_key = $3 for update of m"#,
        branch_id,
        req.supplier_id,
        key
    )
    .fetch_optional(&mut *tx)
    .await?;
    let id = match &was {
        Some(w) => {
            sqlx::query!(
                r#"update supplier_line_map
                   set name = $2, product_id = $3, op_id = $4, user_id = $5, updated_at = now()
                   where id = $1"#,
                w.id,
                name,
                req.product_id,
                req.op_id,
                ctx.user.id
            )
            .execute(&mut *tx)
            .await?;
            w.id
        }
        None => {
            let id = new_id();
            sqlx::query!(
                r#"insert into supplier_line_map (id, branch_id, supplier_id, name, name_key,
                                                  product_id, op_id, user_id)
                   values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
                id,
                branch_id,
                req.supplier_id,
                name,
                key,
                req.product_id,
                req.op_id,
                ctx.user.id
            )
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "supplier_line_map",
        Some(id),
        json!({
            "supplier": req.supplier_id, "supplier_name": supplier, "name": name,
            "product": req.product_id, "product_name": product.name,
            "was": was.as_ref().map(|w| json!({ "product": w.product_id, "product_name": w.product_name })),
        }),
    )
    .await?;
    let out = load(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct RemoveReq {
    op_id: Uuid,
}

#[derive(Serialize, Deserialize)]
struct Removed {
    removed: bool,
}

/// Удалить запомненное из карточки поставщика: право «Товары» (как правки каталога) или
/// владелец. Справочник, а не документ — запись удаляется, след остаётся в журнале.
async fn remove(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<RemoveReq>,
) -> AppResult<Json<Removed>> {
    const KIND: &str = "supplier_line_map.remove";
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "products").await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let gone = sqlx::query!(
        r#"delete from supplier_line_map m using products p
           where m.id = $1 and m.branch_id = $2 and p.id = m.product_id
           returning m.supplier_id, m.name, m.product_id, p.name as product_name"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "supplier_line_map",
        Some(id),
        json!({ "supplier": gone.supplier_id, "name": gone.name, "product": gone.product_id,
                "product_name": gone.product_name }),
    )
    .await?;
    let out = Removed { removed: true };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
