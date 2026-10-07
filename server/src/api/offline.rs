//! Снимок каталога для работы кассы без сети (SPEC-09).

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::AppResult;
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/offline/snapshot", routing::get(snapshot))
        .route("/sales/offline-rejected", routing::post(rejected))
}

#[derive(Serialize)]
struct SnapProduct {
    id: Uuid,
    name: String,
    brand: String,
    article: String,
    category_id: Uuid,
    unit: String,
    container_ml: Option<i64>,
    barcodes: Vec<String>,
    sale_price_tyiyn: i64,
    pour_price_per_l_tyiyn: Option<i64>,
    stock_qty: i64,
}

#[derive(Serialize)]
struct SnapEmployee {
    id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
}

#[derive(Serialize)]
struct Snapshot {
    version: String,
    server_time: DateTime<Utc>,
    products: Vec<SnapProduct>,
    employees: Vec<SnapEmployee>,
    oil_change_master_fee_tyiyn: i64,
}

/// Снимок без себестоимости и стоимости остатка: касса их не использует (SPEC-09).
async fn snapshot(
    State(state): State<AppState>,
    user: CurrentUser,
    headers: HeaderMap,
) -> AppResult<Response> {
    let mut conn = state.pool.acquire().await?;
    let products = sqlx::query!(
        r#"select p.id, p.name, p.brand, p.article, p.category_id, p.unit,
                  p.container_ml as "container_ml?",
                  coalesce(bp.sale_price_tyiyn, 0) as "sale_price_tyiyn!",
                  bp.pour_price_per_l_tyiyn as "pour_price_per_l_tyiyn?",
                  coalesce(bp.stock_qty, 0) as "stock_qty!",
                  coalesce((select array_agg(b.code order by b.created_at)
                            from product_barcodes b where b.product_id = p.id), '{}') as "barcodes!: Vec<String>"
           from products p
           left join branch_products bp on bp.product_id = p.id and bp.branch_id = $1
           where not p.archived
           order by p.name"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| SnapProduct {
        id: r.id,
        name: r.name,
        brand: r.brand,
        article: r.article,
        category_id: r.category_id,
        unit: r.unit,
        container_ml: r.container_ml,
        barcodes: r.barcodes,
        sale_price_tyiyn: r.sale_price_tyiyn,
        pour_price_per_l_tyiyn: r.pour_price_per_l_tyiyn,
        stock_qty: r.stock_qty,
    })
    .collect::<Vec<_>>();
    let employees = sqlx::query_as!(
        SnapEmployee,
        r#"select id, full_name, is_cashier, is_master from employees
           where branch_id = $1 and active order by full_name"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let fee = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'sales'",
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .and_then(|v| {
        v.get("oil_change_master_fee_tyiyn")
            .and_then(serde_json::Value::as_i64)
    })
    .unwrap_or(crate::api::sales::DEFAULT_OIL_CHANGE_FEE);

    // Версия снимка — отпечаток содержимого: не изменилось, отвечаем 304.
    let body = Snapshot {
        version: String::new(),
        server_time: Utc::now(),
        products,
        employees,
        oil_change_master_fee_tyiyn: fee,
    };
    let payload = serde_json::to_string(&json!({
        "products": &body.products,
        "employees": &body.employees,
        "fee": fee,
    }))
    .unwrap_or_default();
    let version = format!("{:x}", seahash(&payload));
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_matches('"') == version)
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let out = Snapshot {
        version: version.clone(),
        ..body
    };
    Ok(([(header::ETAG, format!("\"{version}\""))], Json(out)).into_response())
}

/// Простой отпечаток строки: нужен только чтобы заметить изменение снимка.
fn seahash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[derive(Deserialize)]
struct RejectedReq {
    op_id: Uuid,
    error: String,
    body: serde_json::Value,
}

/// Чек, который сервер не принял, не исчезает молча: он попадает в журнал
/// и в уведомления владельцу (SPEC-09).
async fn rejected(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RejectedReq>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await?;
    ops::audit(
        &mut tx,
        &ctx,
        "sale.offline_rejected",
        "sale",
        None,
        json!({ "op_id": req.op_id, "error": req.error, "body": req.body }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
