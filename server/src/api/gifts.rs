//! Подарки: к товару привязан список того, что можно подарить (SPEC-11).

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/gift-rules", routing::get(list).post(create))
        .route("/gift-rules/{id}", routing::patch(update))
}

#[derive(Serialize)]
struct GiftItem {
    gift_product_id: Uuid,
    name: String,
    gift_qty: i64,
}

#[derive(Serialize)]
struct GiftRuleOut {
    id: Uuid,
    trigger_product_id: Uuid,
    trigger_name: String,
    active: bool,
    items: Vec<GiftItem>,
}

#[derive(Deserialize)]
struct ListQuery {
    /// Товары чека: касса спрашивает правила только по ним.
    product_id: Option<Uuid>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<GiftRuleOut>>> {
    let rules = sqlx::query!(
        r#"select r.id, r.trigger_product_id, p.name as "trigger_name!", r.active
           from gift_rules r join products p on p.id = r.trigger_product_id
           where r.branch_id = $1 and ($2::uuid is null or r.trigger_product_id = $2)
           order by p.name"#,
        user.branch_id,
        q.product_id
    )
    .fetch_all(&state.pool)
    .await?;
    let ids: Vec<Uuid> = rules.iter().map(|r| r.id).collect();
    let items = sqlx::query!(
        r#"select i.rule_id, i.gift_product_id, i.gift_qty, p.name as "name!"
           from gift_rule_items i join products p on p.id = i.gift_product_id
           where i.rule_id = any($1) order by p.name"#,
        &ids
    )
    .fetch_all(&state.pool)
    .await?;
    let out = rules
        .into_iter()
        .map(|r| GiftRuleOut {
            id: r.id,
            trigger_product_id: r.trigger_product_id,
            trigger_name: r.trigger_name,
            active: r.active,
            items: items
                .iter()
                .filter(|i| i.rule_id == r.id)
                .map(|i| GiftItem {
                    gift_product_id: i.gift_product_id,
                    name: i.name.clone(),
                    gift_qty: i.gift_qty,
                })
                .collect(),
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ItemReq {
    gift_product_id: Uuid,
    gift_qty: Option<i64>,
}

#[derive(Deserialize)]
struct RuleReq {
    trigger_product_id: Uuid,
    items: Vec<ItemReq>,
}

async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RuleReq>,
) -> AppResult<Json<GiftRuleOut>> {
    // Что дарим — решает владелец (SPEC-11).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.items.is_empty() {
        return Err(invalid("добавьте хотя бы один подарок"));
    }
    let mut tx = state.pool.begin().await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into gift_rules (id, branch_id, trigger_product_id, user_id)
           values ($1, $2, $3, $4)
           on conflict (branch_id, trigger_product_id)
           do update set active = true returning id"#,
        id,
        ctx.user.branch_id,
        req.trigger_product_id,
        ctx.user.id
    )
    .fetch_one(&mut *tx)
    .await?;
    let rule_id = sqlx::query_scalar!(
        "select id from gift_rules where branch_id = $1 and trigger_product_id = $2",
        ctx.user.branch_id,
        req.trigger_product_id
    )
    .fetch_one(&mut *tx)
    .await?;
    for it in &req.items {
        let qty = it.gift_qty.unwrap_or(1);
        if qty <= 0 {
            return Err(invalid("количество подарка больше нуля"));
        }
        sqlx::query!(
            r#"insert into gift_rule_items (rule_id, gift_product_id, gift_qty) values ($1, $2, $3)
               on conflict (rule_id, gift_product_id) do update set gift_qty = excluded.gift_qty"#,
            rule_id,
            it.gift_product_id,
            qty
        )
        .execute(&mut *tx)
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "gift.rule",
        "product",
        Some(req.trigger_product_id),
        json!({ "items": req.items.len() }),
    )
    .await?;
    tx.commit().await?;
    let mut all = list(
        State(state),
        ctx.user.clone(),
        Query(ListQuery {
            product_id: Some(req.trigger_product_id),
        }),
    )
    .await?;
    all.0.pop().map(Json).ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct PatchReq {
    active: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PatchReq>,
) -> AppResult<Json<serde_json::Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let found = sqlx::query!(
        "update gift_rules set active = coalesce($3, active) where id = $1 and branch_id = $2 returning id",
        id,
        ctx.user.branch_id,
        req.active
    )
    .fetch_optional(&state.pool)
    .await?;
    if found.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}
