//! Список товаров из старой программы: загружается из Excel и подсказывает при заведении
//! товаров по штрихкоду или названию (SPEC-20, ADR-056).

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        // Выгрузка на тысячи строк больше обычного предела тела запроса.
        .route(
            "/old-catalog",
            routing::get(stats)
                .put(import)
                .layer(axum::extract::DefaultBodyLimit::max(20 * 1024 * 1024)),
        )
        .route("/old-catalog/search", routing::get(search))
}

#[derive(Deserialize)]
pub struct ImportRow {
    pub name: String,
    #[serde(default)]
    pub barcode: String,
    #[serde(default)]
    pub article: String,
    #[serde(default)]
    pub extra_code: String,
    #[serde(default)]
    pub cost_tyiyn: i64,
    #[serde(default)]
    pub price_tyiyn: i64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub supplier: String,
    #[serde(default)]
    pub category: String,
}

const MAX_ROWS: usize = 50_000;

/// Заменить список целиком: новая выгрузка из старой программы вытесняет прежнюю.
/// Строки-пустышки («Новый продукт», без названия) отбрасываются.
async fn import(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(rows): Json<Vec<ImportRow>>,
) -> AppResult<Json<serde_json::Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if rows.len() > MAX_ROWS {
        return Err(invalid(format!("строк не больше {MAX_ROWS}")));
    }
    let clean = |s: &str, max: usize| s.trim().chars().take(max).collect::<String>();
    let rows: Vec<ImportRow> = rows
        .into_iter()
        .filter(|r| {
            let n = r.name.trim().to_lowercase();
            !n.is_empty() && n != "новый продукт"
        })
        .collect();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "delete from old_catalog where branch_id = $1",
        ctx.user.branch_id
    )
    .execute(&mut *tx)
    .await?;
    for r in &rows {
        sqlx::query!(
            r#"insert into old_catalog (id, branch_id, name, barcode, article, extra_code, cost_tyiyn,
                                        price_tyiyn, unit, supplier, category)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
            new_id(),
            ctx.user.branch_id,
            clean(&r.name, 300),
            clean(&r.barcode, 64),
            clean(&r.article, 100),
            clean(&r.extra_code, 100),
            r.cost_tyiyn.clamp(0, ops::MAX_AMOUNT_TYIYN),
            r.price_tyiyn.clamp(0, ops::MAX_AMOUNT_TYIYN),
            clean(&r.unit, 20),
            clean(&r.supplier, 200),
            clean(&r.category, 200)
        )
        .execute(&mut *tx)
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "old_catalog.import",
        "settings",
        None,
        json!({ "rows": rows.len() }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "rows": rows.len() })))
}

#[derive(Serialize)]
struct Stats {
    rows: i64,
    imported_at: Option<DateTime<Utc>>,
}

async fn stats(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Stats>> {
    let r = sqlx::query!(
        r#"select count(*) as "rows!", max(imported_at) as imported_at from old_catalog where branch_id = $1"#,
        user.branch_id
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(Stats {
        rows: r.rows,
        imported_at: r.imported_at,
    }))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    barcode: String,
    #[serde(default)]
    q: String,
}

#[derive(Serialize)]
struct Hit {
    id: Uuid,
    name: String,
    barcode: String,
    article: String,
    cost_tyiyn: i64,
    price_tyiyn: i64,
    unit: String,
    supplier: String,
    category: String,
    /// Совпал штрихкод — это та самая позиция.
    exact: bool,
    /// Товар учёта с этим штрихкодом уже заведён.
    linked_product_id: Option<Uuid>,
}

/// Подсказки: сначала точное совпадение штрихкода (или артикула, доп. кода), потом похожие названия.
async fn search(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<Vec<Hit>>> {
    let code = q.barcode.trim();
    let text = q.q.trim();
    if code.is_empty() && text.chars().count() < 2 {
        return Ok(Json(vec![]));
    }
    // Набранный текст, похожий на код (цифры и латиница без пробелов, есть цифра), ищется и
    // среди кодов Умага: штрихкод, артикул, «доп. коды» — целиком и по началу (замечание
    // заказчика 26.10.2026: «в приходе по коду тоже чтобы искал»).
    let text_code = if text.len() >= 3
        && text.chars().any(|c| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        text
    } else {
        ""
    };
    let rows = sqlx::query!(
        r#"select o.id, o.name, o.barcode, o.article, o.cost_tyiyn, o.price_tyiyn, o.unit, o.supplier, o.category,
                  (m.by_code or m.by_text_code) as "exact!",
                  -- Уже заведена: любой её код есть у товара учёта — не открывать как новую.
                  (select b.product_id from product_barcodes b
                   where b.code = any(array[o.barcode] || x.extras)
                     and b.code <> '' limit 1) as linked
           from old_catalog o
           cross join lateral (
               select regexp_split_to_array(o.extra_code, '\s*[;,]\s*|\s+') as extras
           ) x
           cross join lateral (
               select
                 -- Код совпал: основной, артикул, любой из «доп. кодов» через «;» или «,», или
                 -- тот же числовой код без потерянных Excel нулей впереди (проверка 20.10.2026).
                 ($2 <> '' and (o.barcode = $2 or o.article = $2 or $2 = any(x.extras)
                                or (o.barcode ~ '^[0-9]+$' and $2 ~ '^[0-9]+$'
                                    and ltrim(o.barcode, '0') = ltrim($2, '0')))) as by_code,
                 ($4 <> '' and (o.barcode = $4 or lower(o.article) = lower($4) or $4 = any(x.extras)
                                or (o.barcode ~ '^[0-9]+$' and $4 ~ '^[0-9]+$'
                                    and ltrim(o.barcode, '0') = ltrim($4, '0')))) as by_text_code,
                 ($4 <> '' and length($4) >= 4
                  and (o.barcode like $4 || '%'
                       or exists (select 1 from unnest(x.extras) e where e <> '' and e like $4 || '%'))) as code_prefix
           ) m
           where o.branch_id = $1
             and (m.by_code or m.by_text_code or m.code_prefix
                  or ($3 <> '' and (o.name ilike '%' || $3 || '%' or similarity(o.name, $3) >= 0.25
                                    or o.article ilike $3 || '%')))
           order by "exact!" desc, m.code_prefix desc, similarity(o.name, $3) desc, o.name
           limit 20"#,
        user.branch_id,
        code,
        text,
        text_code
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| Hit {
                id: r.id,
                name: r.name,
                barcode: r.barcode,
                article: r.article,
                cost_tyiyn: r.cost_tyiyn,
                price_tyiyn: r.price_tyiyn,
                unit: r.unit,
                supplier: r.supplier,
                category: r.category,
                exact: r.exact,
                linked_product_id: r.linked,
            })
            .collect(),
    ))
}
