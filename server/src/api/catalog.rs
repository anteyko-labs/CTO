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
use crate::error::{AppError, AppResult, invalid, is_unique_violation};
use crate::ops::{self, new_id};
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
        .route("/products/{id}/barcodes", routing::post(add_barcode))
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
                  coalesce((select array_agg(b.code order by b.created_at) from product_barcodes b
                            where b.product_id = p.id), '{}') as "barcodes!: Vec<String>"
           from products p
           join categories c on c.id = p.category_id
           left join branch_products bp on bp.product_id = p.id and bp.branch_id = $1
           where ($2::text is null
                  or p.name ilike '%' || $2 || '%' or p.brand ilike '%' || $2 || '%'
                  or p.article ilike '%' || $2 || '%'
                  or exists (select 1 from product_barcodes b where b.product_id = p.id and b.code like $2 || '%'))
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
           where not archived and (similarity(name, $1) >= 0.3 or name ilike '%' || $1 || '%')
           order by similarity(name, $1) desc limit 5"#,
        name
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

async fn insert_barcode(
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
            Some(v) if v > 0 => ("ml", Some(v)),
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
    for code in &req.barcodes {
        let code = code.trim();
        if !barcode::is_acceptable(code) {
            return Err(invalid("недопустимый штрихкод"));
        }
        insert_barcode(&mut tx, id, code, false).await?;
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
        "select category_id, unit from products where id = $1 for update",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
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
    if req.pour_price_per_l_tyiyn.is_some() && !ctx.user.is_owner() {
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
    ops::audit(
        &mut tx,
        &ctx,
        "product.prices",
        "product",
        Some(id),
        json!({
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

async fn add_barcode(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<BarcodeReq>,
) -> AppResult<Json<BarcodeOut>> {
    let mut tx = state.pool.begin().await?;
    sqlx::query_scalar!("select id from products where id = $1", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
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
        None => {
            // Счётчик общий для всех филиалов: код уникален глобально.
            let seq = sqlx::query_scalar!("select nextval('internal_barcode_seq') as \"v!\"")
                .fetch_one(&mut *tx)
                .await?;
            (
                barcode::internal_code(seq)
                    .ok_or_else(|| AppError::Internal("кончились коды".into()))?,
                true,
            )
        }
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
