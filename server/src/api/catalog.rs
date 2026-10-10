//! Категории, товары, штрихкоды и цены (SPEC-02).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::domain::barcode;
use crate::domain::costing::{Pool, average};
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid, is_unique_violation};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/categories",
            routing::get(list_categories).post(create_category),
        )
        .route("/categories/{id}", routing::patch(update_category))
        .route(
            "/products",
            routing::get(search_products).post(create_product),
        )
        .route("/products/similar", routing::get(similar_products))
        .route(
            "/products/by-barcode/{code}",
            routing::get(product_by_barcode),
        )
        .route(
            "/products/{id}",
            routing::get(get_product).patch(update_product),
        )
        .route("/products/{id}/prices", routing::patch(update_prices))
        .route("/products/{id}/merge", routing::post(merge_product))
        .route("/products/{id}/barcodes", routing::post(add_barcode))
        .route(
            "/products/{id}/barcodes/{code}",
            routing::delete(remove_barcode),
        )
}

// ---------- Категории ----------

#[derive(Serialize, Deserialize, Clone)]
pub struct AttributeDef {
    pub key: String,
    pub label: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default)]
    pub filterable: bool,
}

#[derive(Serialize)]
struct CategoryOut {
    id: Uuid,
    name: String,
    kind: String,
    attributes: Value,
}

fn attr(key: &str, label: &str, kind: &str, options: &[&str]) -> AttributeDef {
    AttributeDef {
        key: key.into(),
        label: label.into(),
        kind: kind.into(),
        options: options.iter().map(|s| (*s).to_string()).collect(),
        filterable: true,
    }
}

fn default_attributes(kind: &str) -> Vec<AttributeDef> {
    match kind {
        "oil" => vec![
            attr(
                "viscosity",
                "Вязкость",
                "select",
                &[
                    "0W-20", "0W-30", "5W-30", "5W-40", "10W-40", "15W-40", "20W-50",
                ],
            ),
            attr(
                "base",
                "Основа",
                "select",
                &["синтетика", "полусинтетика", "минеральное"],
            ),
            attr("approvals", "Допуски", "text", &[]),
        ],
        "filter" => vec![
            attr(
                "filter_type",
                "Тип",
                "select",
                &["масляный", "воздушный", "салонный", "топливный"],
            ),
            attr("cross", "Кросс-номера", "text", &[]),
        ],
        "battery" => vec![
            attr("capacity_ah", "Ёмкость, А·ч", "number", &[]),
            attr("cranking_a", "Пусковой ток, А", "number", &[]),
            attr("polarity", "Полярность", "select", &["прямая", "обратная"]),
        ],
        _ => vec![],
    }
}

fn check_kind(kind: &str) -> AppResult<()> {
    match kind {
        "oil" | "filter" | "battery" | "other" => Ok(()),
        _ => Err(invalid("неизвестный вид категории")),
    }
}

fn check_attributes(defs: &[AttributeDef]) -> AppResult<()> {
    let mut seen = std::collections::HashSet::new();
    for d in defs {
        if d.key.is_empty() || !d.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(invalid("ключ характеристики: латиница, цифры, _"));
        }
        if !seen.insert(d.key.as_str()) {
            return Err(invalid("ключи характеристик повторяются"));
        }
        if !matches!(d.kind.as_str(), "text" | "number" | "select") {
            return Err(invalid("тип характеристики: text, number или select"));
        }
        if d.kind == "select" && d.options.is_empty() {
            return Err(invalid("у списка должны быть варианты"));
        }
    }
    Ok(())
}

fn to_json<T: Serialize>(v: &T) -> AppResult<Value> {
    serde_json::to_value(v).map_err(|e| AppError::Internal(e.to_string()))
}

async fn list_categories(
    State(state): State<AppState>,
    _user: CurrentUser,
) -> AppResult<Json<Vec<CategoryOut>>> {
    let rows = sqlx::query_as!(
        CategoryOut,
        "select id, name, kind, attributes from categories order by name"
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CategoryReq {
    name: String,
    kind: String,
    attributes: Option<Vec<AttributeDef>>,
}

async fn create_category(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CategoryReq>,
) -> AppResult<Json<CategoryOut>> {
    check_kind(&req.kind)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("название обязательно"));
    }
    let defs = req
        .attributes
        .unwrap_or_else(|| default_attributes(&req.kind));
    check_attributes(&defs)?;
    let attributes = to_json(&defs)?;
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into categories (id, name, kind, attributes) values ($1, $2, $3, $4)",
        id,
        name,
        req.kind,
        attributes
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "category.create",
        "category",
        Some(id),
        json!({ "name": name }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(CategoryOut {
        id,
        name,
        kind: req.kind,
        attributes,
    }))
}

#[derive(Deserialize)]
struct CategoryPatch {
    name: Option<String>,
    attributes: Option<Vec<AttributeDef>>,
}

async fn update_category(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CategoryPatch>,
) -> AppResult<Json<CategoryOut>> {
    let name = req.name.map(|s| s.trim().to_string());
    if name.as_deref() == Some("") {
        return Err(invalid("название обязательно"));
    }
    let attributes = match &req.attributes {
        Some(defs) => {
            check_attributes(defs)?;
            Some(to_json(defs)?)
        }
        None => None,
    };
    let mut tx = state.pool.begin().await?;
    let out = sqlx::query_as!(
        CategoryOut,
        r#"update categories set name = coalesce($2, name), attributes = coalesce($3, attributes)
           where id = $1 returning id, name, kind, attributes"#,
        id,
        name,
        attributes
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    ops::audit(
        &mut tx,
        &ctx,
        "category.update",
        "category",
        Some(id),
        json!({ "name": out.name }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Товары ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProductOut {
    pub id: Uuid,
    pub category_id: Uuid,
    pub category_kind: String,
    pub name: String,
    pub brand: String,
    pub article: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub attrs: Value,
    pub archived: bool,
    pub barcodes: Vec<String>,
    pub sale_price_tyiyn: i64,
    pub pour_price_per_l_tyiyn: Option<i64>,
    pub min_stock: i64,
    pub stock_qty: i64,
    pub needs_review: bool,
    /// Цена последнего прихода за штуку или канистру (ADR-008: закупочные цены видны обеим ролям).
    pub last_purchase_price_tyiyn: Option<i64>,
    /// Средняя себестоимость за штуку или канистру — только владельцу.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_cost_tyiyn: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stock_value_tyiyn: Option<i64>,
}

#[derive(Default)]
pub struct ProductFilter {
    pub q: Option<String>,
    pub category_id: Option<Uuid>,
    pub attrs: Option<Value>,
    pub ids: Option<Vec<Uuid>>,
    pub include_archived: bool,
    pub low_only: bool,
    pub limit: i64,
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Единый запрос товаров с остатками филиала; финансовые поля — по роли.
pub async fn query_products(
    conn: &mut PgConnection,
    user: &CurrentUser,
    f: ProductFilter,
) -> AppResult<Vec<ProductOut>> {
    let q =
        f.q.map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(|s| escape_like(&s));
    let rows = sqlx::query!(
        r#"select p.id, p.category_id, c.kind as category_kind, p.name, p.brand, p.article, p.unit,
                  p.container_ml, p.attrs, p.archived,
                  coalesce(bp.sale_price_tyiyn, 0) as "sale_price_tyiyn!",
                  bp.pour_price_per_l_tyiyn as "pour_price_per_l_tyiyn?",
                  coalesce(bp.min_stock, 0) as "min_stock!",
                  coalesce(bp.stock_qty, 0) as "stock_qty!",
                  coalesce(bp.stock_value_tyiyn, 0) as "stock_value_tyiyn!",
                  coalesce(bp.needs_review, false) as "needs_review!",
                  coalesce(bp.last_cost_qty, 0) as "last_cost_qty!",
                  coalesce(bp.last_cost_tyiyn, 0) as "last_cost_tyiyn!",
                  coalesce((select array_agg(b.code order by b.created_at) from product_barcodes b
                            where b.product_id = p.id), '{}') as "barcodes!: Vec<String>"
           from products p
           join categories c on c.id = p.category_id
           left join branch_products bp on bp.product_id = p.id and bp.branch_id = $1
           where ($2::text is null
                  or p.name ilike '%' || $2 || '%' or p.brand ilike '%' || $2 || '%'
                  or p.article ilike '%' || $2 || '%'
                  or exists (select 1 from product_barcodes b where b.product_id = p.id and b.code like $2 || '%')
                  -- по кросс-номеру в любом написании (SPEC-17)
                  or (norm_code($2) <> '' and exists (select 1 from product_cross_numbers x
                      where x.product_id = p.id and x.code_norm like norm_code($2) || '%')))
             and ($3::uuid is null or p.category_id = $3)
             and ($4::jsonb is null or p.attrs @> $4)
             and ($5::uuid[] is null or p.id = any($5))
             and (not p.archived or $6)
             and (not $7 or coalesce(bp.stock_qty, 0) < coalesce(bp.min_stock, 0))
           order by p.name
           limit $8"#,
        user.branch_id,
        q,
        f.category_id,
        f.attrs,
        f.ids.as_deref(),
        f.include_archived,
        f.low_only,
        f.limit.clamp(1, 500)
    )
    .fetch_all(&mut *conn)
    .await?;
    let owner = user.is_owner();
    Ok(rows
        .into_iter()
        .map(|r| {
            let per = r.container_ml.unwrap_or(1);
            let pool = Pool {
                qty: r.stock_qty,
                value: r.stock_value_tyiyn,
                last_qty: 0,
                last_cost: 0,
            };
            ProductOut {
                id: r.id,
                category_id: r.category_id,
                category_kind: r.category_kind,
                name: r.name,
                brand: r.brand,
                article: r.article,
                unit: r.unit,
                container_ml: r.container_ml,
                attrs: r.attrs,
                archived: r.archived,
                barcodes: r.barcodes,
                sale_price_tyiyn: r.sale_price_tyiyn,
                pour_price_per_l_tyiyn: r.pour_price_per_l_tyiyn,
                min_stock: r.min_stock,
                stock_qty: r.stock_qty,
                needs_review: r.needs_review,
                last_purchase_price_tyiyn: (r.last_cost_qty > 0)
                    .then(|| {
                        div_round(
                            i128::from(r.last_cost_tyiyn) * i128::from(per),
                            i128::from(r.last_cost_qty),
                        )
                    })
                    .flatten(),
                avg_cost_tyiyn: if owner { average(&pool, per) } else { None },
                stock_value_tyiyn: owner.then_some(r.stock_value_tyiyn),
            }
        })
        .collect())
}

pub async fn product_by_id(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
) -> AppResult<ProductOut> {
    let f = ProductFilter {
        ids: Some(vec![id]),
        include_archived: true,
        limit: 1,
        ..Default::default()
    };
    query_products(conn, user, f)
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)
}

fn attr_filter(params: &HashMap<String, String>) -> Option<Value> {
    let map: Map<String, Value> = params
        .iter()
        .filter_map(|(k, v)| {
            k.strip_prefix("attr.")
                .map(|k| (k.to_string(), Value::String(v.clone())))
        })
        .filter(|(_, v)| v.as_str().is_some_and(|s| !s.is_empty()))
        .collect();
    (!map.is_empty()).then_some(Value::Object(map))
}

async fn search_products(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<Vec<ProductOut>>> {
    let category_id = params
        .get("category_id")
        .filter(|s| !s.is_empty())
        .map(|s| Uuid::parse_str(s).map_err(|_| invalid("неверный category_id")))
        .transpose()?;
    let f = ProductFilter {
        q: params.get("q").cloned(),
        category_id,
        attrs: attr_filter(&params),
        ids: None,
        include_archived: params.get("archived").is_some_and(|v| v == "true"),
        low_only: params.get("low").is_some_and(|v| v == "true"),
        limit: params
            .get("limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(50),
    };
    let mut conn = state.pool.acquire().await?;
    Ok(Json(query_products(&mut conn, &user, f).await?))
}

async fn get_product(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ProductOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(product_by_id(&mut conn, &user, id).await?))
}

async fn product_by_barcode(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(code): Path<String>,
) -> AppResult<Json<ProductOut>> {
    let mut conn = state.pool.acquire().await?;
    let id = sqlx::query_scalar!(
        "select product_id from product_barcodes where code = $1",
        code.trim()
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(product_by_id(&mut conn, &user, id).await?))
}

#[derive(Deserialize)]
struct SimilarQuery {
    name: String,
}

async fn similar_products(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SimilarQuery>,
) -> AppResult<Json<Vec<ProductOut>>> {
    let name = q.name.trim();
    if name.chars().count() < 3 {
        return Ok(Json(vec![]));
    }
    let mut conn = state.pool.acquire().await?;
    let ids = sqlx::query_scalar!(
        r#"select id from products
           where not archived and (similarity(name, $1) >= 0.3 or name ilike '%' || $2 || '%')
           order by similarity(name, $1) desc limit 5"#,
        name,
        escape_like(name)
    )
    .fetch_all(&mut *conn)
    .await?;
    if ids.is_empty() {
        return Ok(Json(vec![]));
    }
    let mut found = query_products(
        &mut conn,
        &user,
        ProductFilter {
            ids: Some(ids.clone()),
            limit: 5,
            ..Default::default()
        },
    )
    .await?;
    found.sort_by_key(|p| ids.iter().position(|id| *id == p.id));
    Ok(Json(found))
}

async fn category_info(
    conn: &mut PgConnection,
    id: Uuid,
) -> AppResult<(String, Vec<AttributeDef>)> {
    let row = sqlx::query!("select kind, attributes from categories where id = $1", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("категория не найдена"))?;
    let defs: Vec<AttributeDef> = serde_json::from_value(row.attributes).unwrap_or_default();
    Ok((row.kind, defs))
}

/// Оставляет только известные характеристики со строковыми непустыми значениями.
fn clean_attrs(attrs: Option<Value>, defs: &[AttributeDef]) -> AppResult<Value> {
    let mut out = Map::new();
    let Some(attrs) = attrs else {
        return Ok(Value::Object(out));
    };
    let Value::Object(map) = attrs else {
        return Err(invalid("характеристики — объект"));
    };
    for (k, v) in map {
        let Some(def) = defs.iter().find(|d| d.key == k) else {
            return Err(invalid(format!("неизвестная характеристика {k}")));
        };
        let s = match v {
            Value::String(s) => s.trim().to_string(),
            Value::Number(n) => n.to_string(),
            Value::Null => continue,
            _ => return Err(invalid(format!("значение {k} — строка"))),
        };
        if s.is_empty() {
            continue;
        }
        if def.kind == "number" && s.parse::<i64>().is_err() {
            return Err(invalid(format!("{} — целое число", def.label)));
        }
        if def.kind == "select" && !def.options.contains(&s) {
            return Err(invalid(format!("{}: значение не из списка", def.label)));
        }
        out.insert(k, Value::String(s));
    }
    Ok(Value::Object(out))
}

#[derive(Deserialize)]
struct CreateProductReq {
    op_id: Uuid,
    category_id: Uuid,
    name: String,
    #[serde(default)]
    brand: String,
    #[serde(default)]
    article: String,
    container_ml: Option<i64>,
    attrs: Option<Value>,
    #[serde(default)]
    barcodes: Vec<String>,
    #[serde(default)]
    sale_price_tyiyn: i64,
    pour_price_per_l_tyiyn: Option<i64>,
    #[serde(default)]
    min_stock: i64,
}

pub(crate) async fn insert_barcode(
    conn: &mut PgConnection,
    product_id: Uuid,
    code: &str,
    internal: bool,
) -> AppResult<()> {
    let existing = sqlx::query!(
        "select p.name from product_barcodes b join products p on p.id = b.product_id where b.code = $1",
        code
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(e) = existing {
        return Err(AppError::Conflict(format!(
            "штрихкод {code} уже у товара «{}»",
            e.name
        )));
    }
    sqlx::query!(
        "insert into product_barcodes (code, product_id, internal) values ($1, $2, $3)",
        code,
        product_id,
        internal
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict(format!("штрихкод {code} уже занят"))
        } else {
            e.into()
        }
    })?;
    Ok(())
}

async fn create_product(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateProductReq>,
) -> AppResult<Json<ProductOut>> {
    const KIND: &str = "product.create";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let (kind, defs) = category_info(&mut tx, req.category_id).await?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("название обязательно"));
    }
    let (unit, container_ml) = if kind == "oil" {
        match req.container_ml {
            // Бочка 200 л — уже много; тысяча литров — предел, дальше явная ошибка ввода.
            Some(v) if (1..=1_000_000).contains(&v) => ("ml", Some(v)),
            Some(v) if v > 1_000_000 => return Err(invalid("объём тары не больше 1000 л")),
            _ => return Err(invalid("для масла укажите объём канистры")),
        }
    } else {
        ("piece", None)
    };
    if req.sale_price_tyiyn < 0
        || req.min_stock < 0
        || req.pour_price_per_l_tyiyn.is_some_and(|v| v < 0)
    {
        return Err(invalid("цены и минимальный остаток не отрицательны"));
    }
    ops::check_amount(req.sale_price_tyiyn)?;
    ops::check_amount(req.pour_price_per_l_tyiyn.unwrap_or(0))?;
    if req.pour_price_per_l_tyiyn.is_some() && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let pour = if unit == "ml" {
        req.pour_price_per_l_tyiyn
    } else {
        None
    };
    let attrs = clean_attrs(req.attrs, &defs)?;
    let id = new_id();
    sqlx::query!(
        r#"insert into products (id, category_id, name, brand, article, unit, container_ml, attrs)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        id,
        req.category_id,
        name,
        req.brand.trim(),
        req.article.trim(),
        unit,
        container_ml,
        attrs
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into branch_products (branch_id, product_id, sale_price_tyiyn, pour_price_per_l_tyiyn, min_stock)
           values ($1, $2, $3, $4, $5)"#,
        ctx.user.branch_id,
        id,
        req.sale_price_tyiyn,
        pour,
        req.min_stock
    )
    .execute(&mut *tx)
    .await?;
    let mut codes = 0usize;
    for code in &req.barcodes {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        if !barcode::is_acceptable(code) {
            return Err(invalid("недопустимый штрихкод"));
        }
        insert_barcode(&mut tx, id, code, false).await?;
        codes += 1;
    }
    if codes == 0 {
        // Товара без штрихкода не бывает: заводского кода нет — выдаём свой.
        let code = next_internal_code(&mut tx).await?;
        insert_barcode(&mut tx, id, &code, true).await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "product",
        Some(id),
        json!({ "name": name }),
    )
    .await?;
    let out = product_by_id(&mut tx, &ctx.user, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct UpdateProductReq {
    name: Option<String>,
    brand: Option<String>,
    article: Option<String>,
    category_id: Option<Uuid>,
    attrs: Option<Value>,
    archived: Option<bool>,
}

async fn update_product(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateProductReq>,
) -> AppResult<Json<ProductOut>> {
    let mut tx = state.pool.begin().await?;
    let cur = sqlx::query!(
        "select category_id, unit, archived, merged_into from products where id = $1 for update",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Архив меняет владелец; влитый дубль из архива не возвращается, карточку с остатком
    // в архив не убрать — остаток стал бы невидимым (ADR-044).
    if let Some(arch) = req.archived.filter(|a| *a != cur.archived) {
        if !ctx.user.is_owner() {
            return Err(AppError::Forbidden);
        }
        if !arch && cur.merged_into.is_some() {
            return Err(invalid(
                "карточка влита в другую и из архива не возвращается",
            ));
        }
        if arch {
            let stock = sqlx::query_scalar!(
                r#"select coalesce(sum(stock_qty), 0)::bigint as "q!" from branch_products where product_id = $1"#,
                id
            )
            .fetch_one(&mut *tx)
            .await?;
            if stock != 0 {
                return Err(invalid(
                    "на складе есть остаток: объедините карточку с основной или спишите его",
                ));
            }
        }
    }
    let category_id = req.category_id.unwrap_or(cur.category_id);
    let (kind, defs) = category_info(&mut tx, category_id).await?;
    if (kind == "oil") != (cur.unit == "ml") {
        return Err(invalid(
            "нельзя перенести товар между маслом и штучным товаром",
        ));
    }
    let attrs = match req.attrs {
        Some(a) => Some(clean_attrs(Some(a), &defs)?),
        None => None,
    };
    let name = req.name.map(|s| s.trim().to_string());
    if name.as_deref() == Some("") {
        return Err(invalid("название обязательно"));
    }
    sqlx::query!(
        r#"update products set name = coalesce($2, name), brand = coalesce($3, brand),
             article = coalesce($4, article), category_id = $5, attrs = coalesce($6, attrs),
             archived = coalesce($7, archived)
           where id = $1"#,
        id,
        name,
        req.brand.map(|s| s.trim().to_string()),
        req.article.map(|s| s.trim().to_string()),
        category_id,
        attrs,
        req.archived
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "product.update",
        "product",
        Some(id),
        json!({}),
    )
    .await?;
    let out = product_by_id(&mut tx, &ctx.user, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct PricesReq {
    sale_price_tyiyn: Option<i64>,
    pour_price_per_l_tyiyn: Option<i64>,
    min_stock: Option<i64>,
}

async fn update_prices(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PricesReq>,
) -> AppResult<Json<ProductOut>> {
    // Менять цену заведённого товара может только владелец (ADR-032).
    if (req.pour_price_per_l_tyiyn.is_some() || req.sale_price_tyiyn.is_some())
        && !ctx.user.is_owner()
    {
        return Err(AppError::Forbidden);
    }
    if [
        req.sale_price_tyiyn,
        req.pour_price_per_l_tyiyn,
        req.min_stock,
    ]
    .iter()
    .flatten()
    .any(|v| *v < 0)
    {
        return Err(invalid("значения не отрицательны"));
    }
    // Цена с потолком одной операции (ADR-045).
    for v in [req.sale_price_tyiyn, req.pour_price_per_l_tyiyn]
        .into_iter()
        .flatten()
    {
        ops::check_amount(v)?;
    }
    let mut tx = state.pool.begin().await?;
    let unit = sqlx::query_scalar!("select unit from products where id = $1", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    if req.pour_price_per_l_tyiyn.is_some() && unit != "ml" {
        return Err(invalid("цена розлива только для масла"));
    }
    ops::lock_pool(&mut tx, ctx.user.branch_id, id).await?;
    let old = sqlx::query!(
        "select sale_price_tyiyn, pour_price_per_l_tyiyn, min_stock from branch_products where branch_id = $1 and product_id = $2",
        ctx.user.branch_id,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"update branch_products set sale_price_tyiyn = coalesce($3, sale_price_tyiyn),
             pour_price_per_l_tyiyn = coalesce($4, pour_price_per_l_tyiyn), min_stock = coalesce($5, min_stock)
           where branch_id = $1 and product_id = $2"#,
        ctx.user.branch_id,
        id,
        req.sale_price_tyiyn,
        req.pour_price_per_l_tyiyn,
        req.min_stock
    )
    .execute(&mut *tx)
    .await?;
    let name = sqlx::query_scalar!("select name from products where id = $1", id)
        .fetch_one(&mut *tx)
        .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "product.prices",
        "product",
        Some(id),
        json!({
            "name": name,
            "old_sale_price_tyiyn": old.sale_price_tyiyn,
            "sale_price_tyiyn": req.sale_price_tyiyn.unwrap_or(old.sale_price_tyiyn),
            "old": { "sale": old.sale_price_tyiyn, "pour": old.pour_price_per_l_tyiyn, "min_stock": old.min_stock },
            "new": { "sale": req.sale_price_tyiyn, "pour": req.pour_price_per_l_tyiyn, "min_stock": req.min_stock },
        }),
    )
    .await?;
    let out = product_by_id(&mut tx, &ctx.user, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct BarcodeReq {
    code: Option<String>,
}

#[derive(Serialize)]
struct BarcodeOut {
    code: String,
    internal: bool,
}

/// Следующий внутренний код. Счётчик общий для всех филиалов: код уникален глобально.
pub(crate) async fn next_internal_code(tx: &mut sqlx::PgConnection) -> AppResult<String> {
    let seq = sqlx::query_scalar!("select nextval('internal_barcode_seq') as \"v!\"")
        .fetch_one(&mut *tx)
        .await?;
    barcode::internal_code(seq).ok_or_else(|| AppError::Internal("кончились коды".into()))
}

async fn add_barcode(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<BarcodeReq>,
) -> AppResult<Json<BarcodeOut>> {
    let mut tx = state.pool.begin().await?;
    let archived = sqlx::query_scalar!("select archived from products where id = $1", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    if archived {
        return Err(invalid(
            "товар в архиве: штрихкод привязывается к основной карточке",
        ));
    }
    let (code, internal) = match req
        .code
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
    {
        Some(c) => {
            if !barcode::is_acceptable(&c) {
                return Err(invalid("недопустимый штрихкод"));
            }
            (c, false)
        }
        None => (next_internal_code(&mut tx).await?, true),
    };
    insert_barcode(&mut tx, id, &code, internal).await?;
    ops::audit(
        &mut tx,
        &ctx,
        "product.barcode",
        "product",
        Some(id),
        json!({ "code": code }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(BarcodeOut { code, internal }))
}

async fn remove_barcode(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, code)): Path<(Uuid, String)>,
) -> AppResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    // Блокировка товара: два одновременных удаления не оставят его совсем без кода (ADR-026).
    sqlx::query_scalar!("select id from products where id = $1 for update", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let total = sqlx::query_scalar!(
        "select count(*) as \"n!\" from product_barcodes where product_id = $1",
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if total == 0 {
        return Err(AppError::NotFound);
    }
    if total == 1 {
        return Err(invalid(
            "это последний штрихкод товара, без кода товар не продать",
        ));
    }
    let removed = sqlx::query!(
        "delete from product_barcodes where product_id = $1 and code = $2",
        id,
        code
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if removed == 0 {
        return Err(AppError::NotFound);
    }
    ops::audit(
        &mut tx,
        &ctx,
        "product.barcode_remove",
        "product",
        Some(id),
        json!({ "code": code }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct MergeReq {
    op_id: Uuid,
    into_product_id: Uuid,
}

/// Объединение карточек-дублей: коды и остаток переходят на основной товар,
/// дубль уходит в архив, проведённые документы остаются за ним (SPEC-02).
async fn merge_product(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<MergeReq>,
) -> AppResult<Json<ProductOut>> {
    const KIND: &str = "product.merge";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if id == req.into_product_id {
        return Err(invalid("это один и тот же товар"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let branch_id = ctx.user.branch_id;
    // Обе карточки блокируются в порядке id до проверки архива: встречные объединения
    // не ждут друг друга вечно, а товар, который уже ушёл в архив, не примет остаток.
    let mut both = [id, req.into_product_id];
    both.sort();
    sqlx::query!(
        "select id from products where id = any($1) order by id for no key update",
        &both[..]
    )
    .fetch_all(&mut *tx)
    .await?;
    let dup = sqlx::query!(
        "select name, unit, container_ml, archived from products where id = $1",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let main = sqlx::query!(
        "select name, unit, container_ml, archived from products where id = $1",
        req.into_product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("основной товар не найден"))?;
    if dup.archived || main.archived {
        return Err(invalid("архивный товар не объединяется"));
    }
    if dup.unit != main.unit || dup.container_ml != main.container_ml {
        return Err(invalid("у товаров разные единицы учёта или объём тары"));
    }

    // Остаток и его стоимость переезжают движениями: история склада остаётся сходящейся.
    ops::lock_pool(&mut tx, branch_id, both[0]).await?;
    ops::lock_pool(&mut tx, branch_id, both[1]).await?;
    let pool = ops::lock_pool(&mut tx, branch_id, id).await?;
    let (qty, value) = (pool.qty, pool.value);
    if qty != 0 || value != 0 {
        ops::apply_movement(
            &mut tx,
            Movement {
                branch_id,
                product_id: id,
                qty_delta: -qty,
                value_delta: -value,
                doc_type: "merge",
                doc_id: req.into_product_id,
            },
        )
        .await?;
        ops::apply_movement(
            &mut tx,
            Movement {
                branch_id,
                product_id: req.into_product_id,
                qty_delta: qty,
                value_delta: value,
                doc_type: "merge",
                doc_id: id,
            },
        )
        .await?;
    }
    sqlx::query!(
        "update product_barcodes set product_id = $2 where product_id = $1",
        id,
        req.into_product_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "update products set archived = true, merged_into = $2 where id = $1",
        id,
        req.into_product_id
    )
    .execute(&mut *tx)
    .await?;
    // Подарки: правило дубля переходит основной карточке (если у неё своего нет), а сам дубль
    // в списках подарков заменяется основной — иначе чек с подарком отклонится.
    sqlx::query!(
        r#"update gift_rules set trigger_product_id = $2
           where trigger_product_id = $1
             and not exists (select 1 from gift_rules x where x.branch_id = gift_rules.branch_id and x.trigger_product_id = $2)"#,
        id,
        req.into_product_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "update gift_rules set active = false where trigger_product_id = $1",
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"delete from gift_rule_items i where i.gift_product_id = $1
             and exists (select 1 from gift_rule_items x where x.rule_id = i.rule_id and x.gift_product_id = $2)"#,
        id,
        req.into_product_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "update gift_rule_items set gift_product_id = $2 where gift_product_id = $1",
        id,
        req.into_product_id
    )
    .execute(&mut *tx)
    .await?;
    // Остаток дубля ушёл к основному товару: снимаем его отметку «проверить», а последнюю
    // закупочную цену отдаём основному, если своей у него ещё нет.
    sqlx::query!(
        r#"update branch_products m
           set last_cost_qty = d.last_cost_qty, last_cost_tyiyn = d.last_cost_tyiyn
           from branch_products d
           where m.branch_id = $1 and m.product_id = $2 and d.branch_id = $1 and d.product_id = $3
             and m.last_cost_qty = 0 and d.last_cost_qty > 0"#,
        branch_id,
        req.into_product_id,
        id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        "update branch_products set needs_review = false where branch_id = $1 and product_id = $2",
        branch_id,
        id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "product",
        Some(req.into_product_id),
        json!({ "from": id, "from_name": dup.name, "into_name": main.name, "qty": qty, "value": value }),
    )
    .await?;
    let out = product_by_id(&mut tx, &ctx.user, req.into_product_id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
