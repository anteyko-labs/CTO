//! Плитки «ходовое» на экране кассы: товары и услуги одним касанием (SPEC-04, ADR-055).

use axum::extract::State;
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, product_by_id};
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/settings/favorites",
        routing::get(get_favorites).put(put_favorites),
    )
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FavRef {
    /// `product` или `service`.
    pub kind: String,
    pub id: Uuid,
}

#[derive(Serialize)]
struct FavService {
    id: Uuid,
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: i64,
    active: bool,
}

#[derive(Serialize)]
struct FavItem {
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    product: Option<ProductOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    service: Option<FavService>,
}

const MAX_TILES: usize = 24;

/// Плитки в порядке, заданном владельцем; влитые карточки ведут на основную, архивные и
/// отключённые пропадают сами.
async fn get_favorites(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<FavItem>>> {
    let mut conn = state.pool.acquire().await?;
    let refs: Vec<FavRef> = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'favorites'",
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .and_then(|v| serde_json::from_value(v).ok())
    .unwrap_or_default();
    let mut out = Vec::new();
    for r in refs {
        match r.kind.as_str() {
            "product" => {
                let id = ops::live_product(&mut conn, r.id).await?;
                if let Ok(p) = product_by_id(&mut conn, &user, id).await
                    && !p.archived
                {
                    out.push(FavItem {
                        kind: r.kind,
                        product: Some(p),
                        service: None,
                    });
                }
            }
            "service" => {
                if let Some(s) = sqlx::query_as!(
                    FavService,
                    "select id, name, price_tyiyn, master_fee_tyiyn, active from services where id = $1 and branch_id = $2 and active",
                    r.id,
                    user.branch_id
                )
                .fetch_optional(&mut *conn)
                .await?
                {
                    out.push(FavItem {
                        kind: r.kind,
                        product: None,
                        service: Some(s),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(Json(out))
}

/// Состав плиток задаёт владелец.
async fn put_favorites(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<Vec<FavRef>>,
) -> AppResult<Json<serde_json::Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.len() > MAX_TILES {
        return Err(invalid(format!("плиток не больше {MAX_TILES}")));
    }
    if req
        .iter()
        .any(|r| !matches!(r.kind.as_str(), "product" | "service"))
    {
        return Err(invalid("плитка — товар или услуга"));
    }
    let mut seen = std::collections::HashSet::new();
    let list: Vec<FavRef> = req.into_iter().filter(|r| seen.insert(r.id)).collect();
    let value = json!(list);
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'favorites', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.favorites",
        "settings",
        None,
        json!({ "count": list.len() }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
