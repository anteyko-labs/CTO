//! Список товаров из старой программы Умаг: загружается из Excel и подсказывает при заведении
//! товаров по штрихкоду или названию (SPEC-20, ADR-056). Строка Умага связывается с товаром
//! учёта, и все её коды становятся кодами товара — код Умага находит товар везде, где ищут по
//! коду, и без сети (SPEC-35, ADR-110). Список у филиала свой, товары и их коды общие (SPEC-33).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, escape_like, product_by_id};
use crate::api::paging::{MAX_PAGE, Paged, paged};
use crate::auth::{Ctx, CurrentUser, require_permission};
use crate::domain::barcode;
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
        .route("/old-catalog/pairs", routing::get(pairs))
        .route("/old-catalog/auto-link", routing::post(auto_link))
        .route("/old-catalog/{id}/link", routing::post(link))
        .route("/old-catalog/{id}/unlink", routing::post(unlink))
        .route("/old-catalog/{id}/reject", routing::post(reject))
}

/// Связывать, отвязывать и разбирать пары — право «Товары и штрихкоды» (ADR-097).
const RIGHT: &str = "products";

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

/// Связи строк Умага филиала меняет одна операция за раз (выгрузка, связывание, автосвязывание,
/// новый товар): они не перебивают друг друга. Берётся до записи кодов товара — иначе две
/// операции, переносящие один код, ждали бы друг друга.
pub(crate) async fn lock_branch(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended('old_catalog:' || $1::text, 0))",
        branch_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Строка прежнего списка, связанная с товаром: по ней новая выгрузка восстанавливает связь.
struct LinkedRow {
    barcode: String,
    /// Штрихкод и «доп. коды» — по ним узнаётся та же строка в новой выгрузке.
    keys: Vec<String>,
    /// Все коды строки — они уже были у товара.
    codes: Vec<String>,
    product_id: Uuid,
}

/// Строка новой выгрузки.
struct FreshRow {
    id: Uuid,
    barcode: String,
    keys: Vec<String>,
    codes: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Restored {
    row_id: Uuid,
    product_id: Uuid,
    /// Коды, которых у прежней строки не было: их переносим товару.
    new_codes: Vec<String>,
}

/// Связи новой выгрузки (SPEC-35 §1): строка с тем же штрихкодом получает прежний товар; без
/// него — строка с общим кодом (штрихкод или «доп. код»), если он ведёт к одному товару. Разные
/// товары — связи нет, решит человек. Переносятся только новые коды строки: коды, которые
/// владелец убрал из карточки товара, выгрузка не возвращает.
fn restore_links(old: &[LinkedRow], fresh: &[FreshRow]) -> Vec<Restored> {
    let mut by_barcode: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, o) in old.iter().enumerate() {
        if !o.barcode.is_empty() {
            by_barcode.entry(o.barcode.as_str()).or_default().push(i);
        }
        for k in &o.keys {
            by_key.entry(k.as_str()).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for n in fresh {
        let mut matched: Vec<usize> = by_barcode
            .get(n.barcode.as_str())
            .filter(|_| !n.barcode.is_empty())
            .cloned()
            .unwrap_or_default();
        if matched.is_empty() {
            matched = n
                .keys
                .iter()
                .filter_map(|k| by_key.get(k.as_str()))
                .flatten()
                .copied()
                .collect();
        }
        let products: HashSet<Uuid> = matched.iter().map(|&i| old[i].product_id).collect();
        let products: Vec<Uuid> = products.into_iter().collect();
        let &[product_id] = products.as_slice() else {
            continue;
        };
        let before: HashSet<&str> = matched
            .iter()
            .flat_map(|&i| old[i].codes.iter().map(String::as_str))
            .collect();
        out.push(Restored {
            row_id: n.id,
            product_id,
            new_codes: n
                .codes
                .iter()
                .filter(|c| !before.contains(c.as_str()))
                .cloned()
                .collect(),
        });
    }
    out
}

/// Заменить список целиком: новая выгрузка из старой программы вытесняет прежнюю, связи с
/// товарами сохраняются (SPEC-35 §1). Строки-пустышки («Новый продукт», без названия)
/// отбрасываются.
async fn import(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(rows): Json<Vec<ImportRow>>,
) -> AppResult<Json<Value>> {
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
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    lock_branch(&mut tx, branch_id).await?;
    let old: Vec<LinkedRow> = sqlx::query!(
        r#"select barcode, keys, codes, product_id as "product_id!"
           from old_catalog where branch_id = $1 and product_id is not null"#,
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| LinkedRow {
        barcode: r.barcode,
        keys: r.keys,
        codes: r.codes,
        product_id: r.product_id,
    })
    .collect();
    sqlx::query!("delete from old_catalog where branch_id = $1", branch_id)
        .execute(&mut *tx)
        .await?;
    let n = rows.len();
    let ids: Vec<Uuid> = rows.iter().map(|_| new_id()).collect();
    let text = |f: fn(&ImportRow) -> &str, max: usize| -> Vec<String> {
        rows.iter().map(|r| clean(f(r), max)).collect()
    };
    let money = |f: fn(&ImportRow) -> i64| -> Vec<i64> {
        rows.iter()
            .map(|r| f(r).clamp(0, ops::MAX_AMOUNT_TYIYN))
            .collect()
    };
    // Одним запросом: пять тысяч строк по одной — секунды под замком.
    sqlx::query!(
        r#"insert into old_catalog (id, branch_id, name, barcode, article, extra_code, cost_tyiyn,
                                    price_tyiyn, unit, supplier, category)
           select t.id, $1, t.name, t.barcode, t.article, t.extra_code, t.cost, t.price, t.unit,
                  t.supplier, t.category
           from unnest($2::uuid[], $3::text[], $4::text[], $5::text[], $6::text[], $7::int8[],
                       $8::int8[], $9::text[], $10::text[], $11::text[])
                as t(id, name, barcode, article, extra_code, cost, price, unit, supplier, category)"#,
        branch_id,
        &ids,
        &text(|r| &r.name, 300),
        &text(|r| &r.barcode, 64),
        &text(|r| &r.article, 100),
        &text(|r| &r.extra_code, 100),
        &money(|r| r.cost_tyiyn),
        &money(|r| r.price_tyiyn),
        &text(|r| &r.unit, 20),
        &text(|r| &r.supplier, 200),
        &text(|r| &r.category, 200),
    )
    .execute(&mut *tx)
    .await?;
    let fresh: Vec<FreshRow> = sqlx::query!(
        r#"select id, barcode, keys, codes from old_catalog where branch_id = $1 order by id"#,
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| FreshRow {
        id: r.id,
        barcode: r.barcode,
        keys: r.keys,
        codes: r.codes,
    })
    .collect();
    let plan = restore_links(&old, &fresh);
    let mut copied = Vec::new();
    if !plan.is_empty() {
        let (rows_ids, products): (Vec<Uuid>, Vec<Uuid>) =
            plan.iter().map(|p| (p.row_id, p.product_id)).unzip();
        sqlx::query!(
            r#"update old_catalog o set product_id = t.product_id
               from unnest($1::uuid[], $2::uuid[]) as t(id, product_id) where o.id = t.id"#,
            &rows_ids,
            &products
        )
        .execute(&mut *tx)
        .await?;
        let pairs: Vec<(String, Uuid)> = plan
            .iter()
            .flat_map(|p| p.new_codes.iter().map(|c| (c.clone(), p.product_id)))
            .collect();
        copied = copy_codes(&mut tx, &pairs).await?;
        refresh_conflicts(&mut tx, &rows_ids).await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "old_catalog.import",
        "settings",
        None,
        json!({ "rows": n, "linked": plan.len(), "copied": copied.len() }),
    )
    .await?;
    // Статистика списка — сразу: иначе до автоматического пересчёта база считает таблицу
    // пустой, и похожие пары сравниваются перебором (минута вместо долей секунды).
    sqlx::query!("analyze old_catalog")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({ "rows": n, "linked": plan.len(), "copied": copied.len() }),
    ))
}

#[derive(Serialize)]
struct Stats {
    rows: i64,
    imported_at: Option<DateTime<Utc>>,
    /// Строк, связанных с товаром учёта (SPEC-35 §3).
    linked: i64,
    unlinked: i64,
    /// Строк с кодом, занятым другим товаром.
    conflicts: i64,
}

async fn stats(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Stats>> {
    let r = sqlx::query!(
        r#"select count(*) as "rows!", max(imported_at) as imported_at,
                  count(product_id) as "linked!",
                  count(*) filter (where jsonb_array_length(code_conflicts) > 0) as "conflicts!"
           from old_catalog where branch_id = $1"#,
        user.branch_id
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(Stats {
        rows: r.rows,
        imported_at: r.imported_at,
        linked: r.linked,
        unlinked: r.rows - r.linked,
        conflicts: r.conflicts,
    }))
}

/// Код строки Умага, занятый другим товаром (SPEC-35 §1): не перенесён, виден при связывании.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CodeConflict {
    pub code: String,
    pub product_id: Uuid,
    pub product_name: String,
}

fn conflicts_of(v: Value) -> Vec<CodeConflict> {
    serde_json::from_value(v).unwrap_or_default()
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    barcode: String,
    #[serde(default)]
    q: String,
}

#[derive(Serialize, Clone)]
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
    /// Совпал код — это та самая позиция.
    exact: bool,
    /// Товар уже заведён: связанный со строкой, а без связи — товар с её кодом.
    linked_product_id: Option<Uuid>,
    /// Товар, с которым строка связана (SPEC-35).
    product_id: Option<Uuid>,
    product_name: Option<String>,
    /// Коды строки: штрихкод, артикул, похожий на код, «доп. коды».
    codes: Vec<String>,
    /// Коды, занятые другими товарами.
    conflicts: Vec<CodeConflict>,
}

/// Строки Умага в порядке `ids`. `code` и `text` — то, что искали: совпавший код — `exact`.
async fn load_hits(
    conn: &mut PgConnection,
    branch_id: Uuid,
    ids: &[Uuid],
    code: &str,
    text: &str,
) -> AppResult<Vec<Hit>> {
    let rows = sqlx::query!(
        r#"select o.id, o.name, o.barcode, o.article, o.cost_tyiyn, o.price_tyiyn, o.unit,
                  o.supplier, o.category, o.codes, o.code_conflicts, o.product_id,
                  p.name as "product_name?",
                  (($3 <> '' and (o.article = $3 or umag_has_code(o.codes, $3)))
                   or ($4 <> '' and umag_has_code(o.codes, $4))) as "exact!",
                  -- Без связи заведённой считается строка, чей штрихкод или «доп. код» уже у
                  -- товара (не выданный программой: свои коды магазина в Умаге могут совпасть с
                  -- нашими; артикул бывает общим у разных товаров).
                  coalesce(o.product_id,
                           (select x.product_id from unnest(o.keys) with ordinality as c(code, n)
                            cross join lateral code_products(c.code) x
                            where not x.internal
                            order by c.n, (x.code = c.code) desc limit 1)) as linked
           from old_catalog o
           left join products p on p.id = o.product_id
           where o.branch_id = $1 and o.id = any($2)"#,
        branch_id,
        ids,
        code,
        text
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by_id: HashMap<Uuid, Hit> = rows
        .into_iter()
        .map(|r| {
            (
                r.id,
                Hit {
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
                    product_id: r.product_id,
                    product_name: r.product_name,
                    codes: r.codes,
                    conflicts: conflicts_of(r.code_conflicts),
                },
            )
        })
        .collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// Подсказки: сначала точное совпадение кода (штрихкод, артикул, «доп. код», числовой код без
/// потерянных нулей), потом похожие названия; среди них не заведённые — выше: блок «Из старой
/// базы Умаг» показывает только их (SPEC-35 §2).
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
    let mut conn = state.pool.acquire().await?;
    let ids = sqlx::query_scalar!(
        r#"select o.id from old_catalog o
           where o.branch_id = $1
             and (($2 <> '' and (o.article = $2 or umag_has_code(o.codes, $2)))
                  or ($3 <> '' and (o.name ilike '%' || $3 || '%' or similarity(o.name, $3) >= 0.25
                                    or o.article ilike $3 || '%' or umag_has_code(o.codes, $3))))
           order by (($2 <> '' and (o.article = $2 or umag_has_code(o.codes, $2)))
                     or ($3 <> '' and umag_has_code(o.codes, $3))) desc,
                    (o.product_id is null
                     and not exists (select 1 from unnest(o.keys) c
                                     cross join lateral code_products(c) x where not x.internal)) desc,
                    similarity(o.name, $3) desc, o.name, o.id
           limit 20"#,
        user.branch_id,
        code,
        text
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(
        load_hits(&mut conn, user.branch_id, &ids, code, text).await?,
    ))
}

/// Переносит коды строк Умага связанным товарам (SPEC-35 §1): код становится кодом товара с
/// источником «Умаг», если его нет ни у одного товара и у другого товара нет того же числового
/// кода с нулями впереди. Занятый код остаётся отметкой конфликта у строки. Товар в архиве
/// кодов не получает. Возвращает перенесённые коды в порядке пар.
async fn copy_codes(conn: &mut PgConnection, pairs: &[(String, Uuid)]) -> AppResult<Vec<String>> {
    let (codes, products): (Vec<String>, Vec<Uuid>) = pairs
        .iter()
        .filter(|(c, _)| barcode::is_acceptable(c))
        .cloned()
        .unzip();
    if codes.is_empty() {
        return Ok(Vec::new());
    }
    let done: HashSet<String> = sqlx::query_scalar!(
        r#"insert into product_barcodes (code, product_id, internal, source)
           select distinct on (t.code) t.code, t.product_id, false, 'umag'
           from unnest($1::text[], $2::uuid[]) with ordinality as t(code, product_id, n)
           join products p on p.id = t.product_id and not p.archived
           where not exists (select 1 from product_barcodes b where b.code = t.code)
             and not exists (select 1 from code_products(t.code) x where x.product_id <> t.product_id)
           order by t.code, t.n
           on conflict (code) do nothing
           returning code"#,
        &codes,
        &products
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();
    let mut seen = HashSet::new();
    Ok(codes
        .into_iter()
        .filter(|c| done.contains(c) && seen.insert(c.clone()))
        .collect())
}

/// Отметки «конфликт кода» строк заново: коды строки, которые нашли бы другой товар.
async fn refresh_conflicts(conn: &mut PgConnection, ids: &[Uuid]) -> AppResult<()> {
    sqlx::query!(
        r#"update old_catalog o set code_conflicts = coalesce((
               select jsonb_agg(jsonb_build_object('code', c.code, 'product_id', x.product_id,
                                                   'product_name', p.name) order by c.n)
               from unnest(o.codes) with ordinality as c(code, n)
               cross join lateral (select y.product_id from code_products(c.code) y
                                   where y.product_id <> o.product_id
                                   order by (y.code = c.code) desc, y.code limit 1) x
               join products p on p.id = x.product_id
               where not exists (select 1 from product_barcodes s
                                 where s.code = c.code and s.product_id = o.product_id)), '[]'::jsonb)
           where o.id = any($1)"#,
        ids
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Ставит связи строк с товарами и переносит коды строк; отметки конфликтов — заново.
/// Возвращает перенесённые коды.
async fn link_rows(conn: &mut PgConnection, links: &[(Uuid, Uuid)]) -> AppResult<Vec<String>> {
    if links.is_empty() {
        return Ok(Vec::new());
    }
    let (rows, products): (Vec<Uuid>, Vec<Uuid>) = links.iter().copied().unzip();
    sqlx::query!(
        r#"update old_catalog o set product_id = t.product_id
           from unnest($1::uuid[], $2::uuid[]) as t(id, product_id) where o.id = t.id"#,
        &rows,
        &products
    )
    .execute(&mut *conn)
    .await?;
    let pairs: Vec<(String, Uuid)> = sqlx::query!(
        r#"select c.code as "code!", o.product_id as "product_id!"
           from unnest($1::uuid[]) with ordinality as r(id, n)
           join old_catalog o on o.id = r.id
           cross join lateral unnest(o.codes) with ordinality as c(code, k)
           where o.product_id is not null
           order by r.n, c.k"#,
        &rows
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.code, r.product_id))
    .collect();
    let copied = copy_codes(&mut *conn, &pairs).await?;
    refresh_conflicts(&mut *conn, &rows).await?;
    Ok(copied)
}

/// Итог связи одной строки.
struct Linked {
    copied: Vec<String>,
    conflicts: Vec<CodeConflict>,
}

/// Связывает строку Умага филиала с товаром и переносит её коды (SPEC-35 §1). Строка,
/// связанная с другим товаром, — 409: сначала «Отвязать». `how` — откуда связь, для журнала:
/// `manual` (выбрал человек), `created` (товар заведён из строки), `code` (код строки — только
/// у этого товара).
async fn link_row(
    conn: &mut PgConnection,
    ctx: &Ctx,
    row_id: Uuid,
    product_id: Uuid,
    how: &str,
) -> AppResult<Linked> {
    let row = sqlx::query!(
        r#"select o.name, o.barcode, o.product_id, umag_key(o.barcode, o.name) as "key!",
                  p.name as "linked_name?"
           from old_catalog o left join products p on p.id = o.product_id
           where o.id = $1 and o.branch_id = $2
           for update of o"#,
        row_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let product_id = ops::live_product(&mut *conn, product_id).await?;
    let product = sqlx::query!(
        "select name, archived from products where id = $1",
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    if product.archived {
        return Err(invalid(
            "товар в архиве: строку Умага связывают с действующей карточкой",
        ));
    }
    if row.product_id.is_some_and(|p| p != product_id) {
        return Err(AppError::Conflict(format!(
            "строка Умага уже связана с товаром «{}» — сначала отвяжите её",
            row.linked_name.unwrap_or_default()
        )));
    }
    // Связал сам — прежнее «Не то» для этой пары больше не действует.
    sqlx::query!(
        r#"delete from old_catalog_link_rejects
           where branch_id = $1 and umag_barcode = $2 and product_id = $3"#,
        ctx.user.branch_id,
        row.key,
        product_id
    )
    .execute(&mut *conn)
    .await?;
    let copied = link_rows(&mut *conn, &[(row_id, product_id)]).await?;
    let conflicts = sqlx::query_scalar!(
        "select code_conflicts from old_catalog where id = $1",
        row_id
    )
    .fetch_one(&mut *conn)
    .await
    .map(conflicts_of)?;
    ops::audit(
        &mut *conn,
        ctx,
        "old_catalog.link",
        "product",
        Some(product_id),
        json!({
            "umag_id": row_id,
            "umag_name": row.name,
            "barcode": row.barcode,
            "name": product.name,
            "how": how,
            "copied": copied,
            "conflicts": conflicts,
        }),
    )
    .await?;
    Ok(Linked { copied, conflicts })
}

/// Строки Умага, чей штрихкод или «доп. код» (не выданный программой) теперь только у этого
/// товара — то же правило, что автосвязывание по точному коду. Без отказов «Не то» для пары.
async fn rows_coded_for(
    conn: &mut PgConnection,
    ctx: &Ctx,
    product_id: Uuid,
) -> AppResult<Vec<Uuid>> {
    // Коды товара и числовые — без нулей впереди: строки Умага ищутся по индексу кодов.
    let mut keys: Vec<String> = Vec::new();
    for code in sqlx::query_scalar!(
        "select code from product_barcodes where product_id = $1 and not internal",
        product_id
    )
    .fetch_all(&mut *conn)
    .await?
    {
        let bare = code.trim_start_matches('0');
        if code.chars().all(|c| c.is_ascii_digit()) && !bare.is_empty() && bare != code {
            keys.push(bare.to_string());
        }
        keys.push(code);
    }
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar!(
        r#"select o.id from old_catalog o
           where o.branch_id = $1 and o.product_id is null and o.keys && $3
             and not exists (select 1 from unnest(o.keys) c
                             cross join lateral code_products(c) x
                             join products p on p.id = x.product_id
                             where x.product_id <> $2 and not p.archived and not x.internal)
             and not exists (select 1 from old_catalog_link_rejects r
                             where r.branch_id = $1 and r.umag_barcode = umag_key(o.barcode, o.name)
                               and r.product_id = $2)
           order by o.id"#,
        ctx.user.branch_id,
        product_id,
        &keys
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Товар получил коды — новый или привязан код (SPEC-35 §1–2): заведённый из строки Умага
/// (`from_row`) связывается с ней, а строки, чей код теперь только у этого товара, — тоже. Все
/// коды строк переносятся товару. Замок списка вызывающий берёт до записи кодов товара.
pub(crate) async fn link_product_rows(
    conn: &mut PgConnection,
    ctx: &Ctx,
    product_id: Uuid,
    from_row: Option<Uuid>,
) -> AppResult<()> {
    if let Some(row) = from_row {
        match link_row(&mut *conn, ctx, row, product_id, "created").await {
            Err(AppError::NotFound) => {
                return Err(invalid(
                    "строка старой базы Умаг не найдена — обновите поиск",
                ));
            }
            other => {
                other?;
            }
        }
    }
    for row in rows_coded_for(&mut *conn, ctx, product_id).await? {
        link_row(&mut *conn, ctx, row, product_id, "code").await?;
    }
    Ok(())
}

#[derive(Deserialize)]
struct LinkReq {
    op_id: Uuid,
    product_id: Uuid,
}

#[derive(Serialize, Deserialize)]
struct LinkOut {
    product: ProductOut,
    copied: Vec<String>,
    conflicts: Vec<CodeConflict>,
}

/// «Это уже есть — выбрать товар», «Связать» в похожих парах и в карточке товара (SPEC-35).
async fn link(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LinkReq>,
) -> AppResult<Json<LinkOut>> {
    const KIND: &str = "old_catalog.link";
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, RIGHT).await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_branch(&mut tx, ctx.user.branch_id).await?;
    let linked = link_row(&mut tx, &ctx, id, req.product_id, "manual").await?;
    let product_id = ops::live_product(&mut tx, req.product_id).await?;
    let out = LinkOut {
        product: product_by_id(&mut tx, &ctx.user, product_id).await?,
        copied: linked.copied,
        conflicts: linked.conflicts,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct OpReq {
    op_id: Uuid,
}

/// Строка Умага филиала для отвязки и «Не то»: название, штрихкод, ключ и связь.
struct RowRef {
    name: String,
    barcode: String,
    key: String,
    product_id: Option<Uuid>,
}

async fn row_ref(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<RowRef> {
    let r = sqlx::query!(
        r#"select name, barcode, umag_key(barcode, name) as "key!", product_id
           from old_catalog where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(RowRef {
        name: r.name,
        barcode: r.barcode,
        key: r.key,
        product_id: r.product_id,
    })
}

/// «Не то» для пары: больше не предлагается и автоматически не связывается.
async fn remember_reject(
    conn: &mut PgConnection,
    branch_id: Uuid,
    key: &str,
    product_id: Uuid,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into old_catalog_link_rejects (branch_id, umag_barcode, product_id)
           values ($1, $2, $3) on conflict do nothing"#,
        branch_id,
        key,
        product_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// «Отвязать» (SPEC-35 §3): коды остаются у товара, пока их не уберут в карточке. Пара
/// запоминается как «не то» — иначе автосвязывание по оставшимся кодам вернуло бы связь.
async fn unlink(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<OpReq>,
) -> AppResult<Json<Value>> {
    const KIND: &str = "old_catalog.unlink";
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, RIGHT).await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_branch(&mut tx, ctx.user.branch_id).await?;
    let row = row_ref(&mut tx, ctx.user.branch_id, id).await?;
    let Some(product_id) = row.product_id else {
        return Err(AppError::Conflict(
            "строка Умага не связана с товаром".into(),
        ));
    };
    sqlx::query!(
        "update old_catalog set product_id = null, code_conflicts = '[]' where id = $1",
        id
    )
    .execute(&mut *tx)
    .await?;
    remember_reject(&mut tx, ctx.user.branch_id, &row.key, product_id).await?;
    let name = sqlx::query_scalar!("select name from products where id = $1", product_id)
        .fetch_one(&mut *tx)
        .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "product",
        Some(product_id),
        json!({ "umag_id": id, "umag_name": row.name, "barcode": row.barcode, "name": name }),
    )
    .await?;
    let out = json!({ "ok": true });
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// «Не то» в похожих парах (SPEC-35 §3).
async fn reject(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LinkReq>,
) -> AppResult<Json<Value>> {
    const KIND: &str = "old_catalog.reject";
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, RIGHT).await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_branch(&mut tx, ctx.user.branch_id).await?;
    let row = row_ref(&mut tx, ctx.user.branch_id, id).await?;
    let product_id = ops::live_product(&mut tx, req.product_id).await?;
    let name = sqlx::query_scalar!("select name from products where id = $1", product_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("товар не найден"))?;
    remember_reject(&mut tx, ctx.user.branch_id, &row.key, product_id).await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "product",
        Some(product_id),
        json!({ "umag_id": id, "umag_name": row.name, "barcode": row.barcode, "name": name }),
    )
    .await?;
    let out = json!({ "ok": true });
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize, Deserialize)]
struct AutoLinkOut {
    linked: usize,
    by_code: usize,
    by_name: usize,
}

/// В журнале автосвязывания — пары, но не больше этого числа: запись не разрастается.
const AUDIT_PAIRS: usize = 500;

/// «Связать автоматически» (SPEC-35 §3): строка без связи получает товар, если её штрихкод или
/// «доп. код» (не выданный программой) есть ровно у одного действующего товара или её название
/// после нормализации (регистр, пробелы, кавычки, «ё»→«е») совпадает с названием ровно одного
/// товара. Пары «не то» не связываются. Коды строк переносятся товарам.
async fn auto_link(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OpReq>,
) -> AppResult<Json<AutoLinkOut>> {
    const KIND: &str = "old_catalog.auto_link";
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, RIGHT).await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let branch_id = ctx.user.branch_id;
    lock_branch(&mut tx, branch_id).await?;
    let by_code: Vec<(Uuid, Uuid)> = sqlx::query!(
        r#"select o.id as "id!", (array_agg(distinct x.product_id))[1] as "product_id!"
           from old_catalog o
           cross join lateral unnest(o.keys) c
           cross join lateral code_products(c) x
           join products p on p.id = x.product_id and not p.archived
           where o.branch_id = $1 and o.product_id is null and not x.internal
             and not exists (select 1 from old_catalog_link_rejects r
                             where r.branch_id = $1 and r.umag_barcode = umag_key(o.barcode, o.name)
                               and r.product_id = x.product_id)
           group by o.id
           having count(distinct x.product_id) = 1
           order by o.id"#,
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| (r.id, r.product_id))
    .collect();
    let coded: Vec<Uuid> = by_code.iter().map(|(r, _)| *r).collect();
    // По названию — только строки, чьих штрихкода и «доп. кодов» нет ни у одного действующего
    // товара: строка с кодом другого товара — его, решит человек.
    let by_name: Vec<(Uuid, Uuid)> = sqlx::query!(
        r#"with names as (
               select p.id, umag_norm_name(p.name) as k from products p where not p.archived
               union
               select p.id, umag_norm_name(p.brand || ' ' || p.name) from products p
               where not p.archived and btrim(p.brand) <> ''
           )
           select o.id as "id!", (array_agg(distinct n.id))[1] as "product_id!"
           from old_catalog o
           join names n on n.k = umag_norm_name(o.name) and n.k <> ''
           where o.branch_id = $1 and o.product_id is null and not (o.id = any($2))
             and not exists (select 1 from unnest(o.keys) c
                             cross join lateral code_products(c) x
                             join products q on q.id = x.product_id
                             where not q.archived and not x.internal)
             and not exists (select 1 from old_catalog_link_rejects r
                             where r.branch_id = $1 and r.umag_barcode = umag_key(o.barcode, o.name)
                               and r.product_id = n.id)
           group by o.id
           having count(distinct n.id) = 1
           order by o.id"#,
        branch_id,
        &coded
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| (r.id, r.product_id))
    .collect();
    let all: Vec<(Uuid, Uuid)> = by_code.iter().chain(by_name.iter()).copied().collect();
    let copied = link_rows(&mut tx, &all).await?;
    let out = AutoLinkOut {
        linked: all.len(),
        by_code: by_code.len(),
        by_name: by_name.len(),
    };
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "old_catalog",
        None,
        json!({
            "linked": out.linked,
            "by_code": out.by_code,
            "by_name": out.by_name,
            "copied": copied.len(),
            "pairs": all.iter().take(AUDIT_PAIRS)
                .map(|(r, p)| json!({ "umag_id": r, "product_id": p }))
                .collect::<Vec<_>>(),
        }),
    )
    .await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Похожие пары ----------

/// Признаки названия, по которым похожие пары не должны расходиться (SPEC-35 §3).
#[derive(Debug, Default, PartialEq, Eq)]
struct NameMarks {
    /// Объём тары, мл: «4Л», «0,5 л», «325ml».
    volume_ml: Option<i64>,
    /// Вес, г: «5 кг», «450г».
    weight_g: Option<i64>,
    /// Вязкость: «5W-30», «5w30», «15/40» — «5w30», «15w40».
    viscosity: Option<String>,
}

/// Нижний регистр и «ё»→«е».
fn norm_text(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .collect()
}

enum Unit {
    Liter,
    Milliliter,
    Kilogram,
    Gram,
}

fn unit_of(word: &str) -> Option<Unit> {
    match word {
        "л" | "литр" | "литра" | "литров" | "l" | "lt" | "ltr" | "lit" | "liter" | "litre"
        | "liters" | "litres" => Some(Unit::Liter),
        "мл" | "ml" => Some(Unit::Milliliter),
        "кг" | "kg" => Some(Unit::Kilogram),
        "г" | "гр" | "g" | "gr" => Some(Unit::Gram),
        _ => None,
    }
}

/// Целая часть и до трёх знаков дробной — в тысячных: «0,5» → 500, «4» → 4000.
fn thousandths(int: &str, frac: &str) -> Option<i64> {
    let whole: i64 = int.parse().ok()?;
    let frac: String = frac.chars().chain(std::iter::repeat('0')).take(3).collect();
    whole.checked_mul(1000)?.checked_add(frac.parse().ok()?)
}

/// Вязкость сразу за числом `first` (1–2 цифры) с позиции `at`: «w» или «/», затем две цифры.
fn viscosity_at(s: &[char], at: usize, first: &str) -> Option<String> {
    let mut k = at;
    while s.get(k) == Some(&' ') {
        k += 1;
    }
    if !matches!(s.get(k), Some('w' | '/')) {
        return None;
    }
    k += 1;
    while matches!(s.get(k), Some(' ' | '-')) {
        k += 1;
    }
    let start = k;
    while s.get(k).is_some_and(char::is_ascii_digit) {
        k += 1;
    }
    if k - start != 2 {
        return None;
    }
    let a: u32 = first.parse().ok()?;
    let b: String = s[start..k].iter().collect();
    Some(format!("{a}w{b}"))
}

/// Объём, вес и вязкость из названия — первые найденные.
fn name_marks(name: &str) -> NameMarks {
    let s: Vec<char> = norm_text(name).chars().collect();
    let mut out = NameMarks::default();
    let mut i = 0;
    while i < s.len() {
        // Число начинается не внутри другого числа (не «12» из «712/75»).
        let inside = i > 0
            && (s[i - 1].is_ascii_digit()
                || (matches!(s[i - 1], ',' | '.') && i > 1 && s[i - 2].is_ascii_digit()));
        if !s[i].is_ascii_digit() || inside {
            i += 1;
            continue;
        }
        let mut j = i;
        while s.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
        let int: String = s[i..j].iter().collect();
        let mut k = j;
        let mut frac = String::new();
        if matches!(s.get(k), Some(',' | '.')) && s.get(k + 1).is_some_and(char::is_ascii_digit) {
            k += 1;
            while let Some(c) = s.get(k).filter(|c| c.is_ascii_digit()) {
                frac.push(*c);
                k += 1;
            }
        }
        if out.viscosity.is_none() && frac.is_empty() && int.len() <= 2 {
            out.viscosity = viscosity_at(&s, j, &int);
        }
        let mut u = k;
        while s.get(u) == Some(&' ') {
            u += 1;
        }
        let mut e = u;
        while s.get(e).is_some_and(|c| c.is_alphabetic()) {
            e += 1;
        }
        let word: String = s[u..e].iter().collect();
        match unit_of(&word) {
            Some(Unit::Liter) if out.volume_ml.is_none() => {
                out.volume_ml = thousandths(&int, &frac)
            }
            Some(Unit::Milliliter) if out.volume_ml.is_none() => out.volume_ml = int.parse().ok(),
            Some(Unit::Kilogram) if out.weight_g.is_none() => {
                out.weight_g = thousandths(&int, &frac)
            }
            Some(Unit::Gram) if out.weight_g.is_none() => out.weight_g = int.parse().ok(),
            _ => {}
        }
        i = k.max(i + 1);
    }
    out
}

/// Бренды из списка `known` (нижний регистр), которые есть в тексте отдельным словом.
fn brands_in<'a>(text: &str, known: &'a [String]) -> HashSet<&'a str> {
    let t = norm_text(text);
    known
        .iter()
        .filter(|b| {
            t.match_indices(b.as_str()).any(|(at, m)| {
                let before = t[..at].chars().next_back();
                let after = t[at + m.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
        })
        .map(String::as_str)
        .collect()
}

/// Пара не расходится: объём, вес, вязкость и бренд, если они есть в обоих названиях, совпадают.
fn compatible(product: &str, umag: &str, brands: &[String]) -> bool {
    let (a, b) = (name_marks(product), name_marks(umag));
    fn differ<T: PartialEq>(x: &Option<T>, y: &Option<T>) -> bool {
        matches!((x, y), (Some(x), Some(y)) if x != y)
    }
    if differ(&a.volume_ml, &b.volume_ml)
        || differ(&a.weight_g, &b.weight_g)
        || differ(&a.viscosity, &b.viscosity)
    {
        return false;
    }
    let (pa, pb) = (brands_in(product, brands), brands_in(umag, brands));
    pa.is_empty() || pb.is_empty() || !pa.is_disjoint(&pb)
}

/// Сходство названий для показа: тысячные доли → число 0…1 («0.734»). Сравнивают и сортируют
/// целые тысячные.
fn share(permille: i32) -> AppResult<serde_json::Number> {
    let p = permille.clamp(0, 1000);
    format!("{}.{:03}", p / 1000, p % 1000)
        .parse()
        .map_err(|_| AppError::Internal("сходство".into()))
}

/// Самое меньшее сходство названий пары (SPEC-35 §3).
const MIN_SIMILARITY: &str = "0.45";
/// Сколько похожих строк Умага показывать у одного товара.
const PER_PRODUCT: usize = 3;

#[derive(Deserialize)]
struct PairsQuery {
    #[serde(default)]
    q: String,
    /// Номер страницы с нуля и её размер; `limit`/`offset` — как у других списков (SPEC-28 §2).
    page: Option<i64>,
    size: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
}

#[derive(Serialize)]
struct PairProduct {
    id: Uuid,
    name: String,
    brand: String,
    article: String,
    sale_price_tyiyn: i64,
}

#[derive(Serialize)]
struct PairOut {
    product: PairProduct,
    umag: Hit,
    similarity: serde_json::Number,
}

/// Похожие пары (SPEC-35 §3): товары без связи и строки Умага без связи с похожими
/// названиями (сходство не меньше 0,45, объём, вес, вязкость и бренд не расходятся), без пар
/// «не то». У товара — до трёх самых похожих строк; сверху — самые похожие.
async fn pairs(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PairsQuery>,
) -> AppResult<Paged<PairOut>> {
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &user, RIGHT).await?;
    // Порог оператора «%» — для поиска похожих по индексу названий.
    sqlx::query_scalar!(
        "select set_config('pg_trgm.similarity_threshold', $1, true)",
        MIN_SIMILARITY
    )
    .fetch_one(&mut *tx)
    .await?;
    let text = q.q.trim().to_string();
    let rows = sqlx::query!(
        r#"with free as (
               select p.id, p.name, p.brand, p.article,
                      case when btrim(p.brand) <> '' and strpos(lower(p.name), lower(btrim(p.brand))) = 0
                           then btrim(p.brand) || ' ' || p.name else p.name end as full_name
               from products p
               where not p.archived
                 and not exists (select 1 from old_catalog l where l.branch_id = $1 and l.product_id = p.id)
                 and not exists (select 1 from product_barcodes b
                                 where b.product_id = p.id and not b.internal
                                   and exists (select 1 from old_catalog l
                                               where l.branch_id = $1 and l.keys @> array[b.code]))
           )
           select f.id as "product_id!", f.name as "product_name!", f.brand as "brand!",
                  f.article as "article!", f.full_name as "full_name!",
                  coalesce(bp.sale_price_tyiyn, p.sale_price_tyiyn) as "sale_price_tyiyn!",
                  o.id as "umag_id!", o.name as "umag_name!", o.sim as "sim!"
           from free f
           join products p on p.id = f.id
           left join branch_products bp on bp.product_id = f.id and bp.branch_id = $1
           -- Похожие строки каждого товара — по индексу названий: «offset 0» не даёт
           -- планировщику сравнить все пары подряд (тысячи товаров × тысячи строк — минута).
           cross join lateral (
               select o.id, o.name, o.barcode, o.keys, o.codes,
                      round(similarity(o.name, f.full_name) * 1000)::int as sim
               from old_catalog o
               where o.name % f.full_name and o.branch_id = $1 and o.product_id is null
                 and similarity(o.name, f.full_name) >= $4::text::real
               offset 0
           ) o
           where not exists (select 1 from product_barcodes b where b.code = any(o.keys) and not b.internal)
             and not exists (select 1 from old_catalog_link_rejects r
                             where r.branch_id = $1 and r.umag_barcode = umag_key(o.barcode, o.name)
                               and r.product_id = f.id)
             and ($2 = '' or f.name ilike '%' || $3 || '%' or f.brand ilike '%' || $3 || '%'
                  or f.article ilike '%' || $3 || '%' or o.name ilike '%' || $3 || '%'
                  or umag_has_code(o.codes, $2))"#,
        user.branch_id,
        text,
        escape_like(&text),
        MIN_SIMILARITY
    )
    .fetch_all(&mut *tx)
    .await?;
    let brands: Vec<String> = sqlx::query_scalar!(
        r#"select distinct lower(btrim(brand)) as "b!" from products
           where not archived and length(btrim(brand)) >= 2"#
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|b| norm_text(&b))
    .collect();
    struct Cand {
        product: PairProduct,
        umag_id: Uuid,
        umag_name: String,
        sim: i32,
    }
    let mut per: HashMap<Uuid, Vec<Cand>> = HashMap::new();
    for r in rows {
        if !compatible(&r.full_name, &r.umag_name, &brands) {
            continue;
        }
        per.entry(r.product_id).or_default().push(Cand {
            product: PairProduct {
                id: r.product_id,
                name: r.product_name,
                brand: r.brand,
                article: r.article,
                sale_price_tyiyn: r.sale_price_tyiyn,
            },
            umag_id: r.umag_id,
            umag_name: r.umag_name,
            sim: r.sim,
        });
    }
    let mut all: Vec<Cand> = per
        .into_values()
        .flat_map(|mut v| {
            v.sort_by(|a, b| b.sim.cmp(&a.sim).then(a.umag_name.cmp(&b.umag_name)));
            v.truncate(PER_PRODUCT);
            v
        })
        .collect();
    all.sort_by(|a, b| {
        b.sim
            .cmp(&a.sim)
            .then_with(|| {
                a.product
                    .name
                    .to_lowercase()
                    .cmp(&b.product.name.to_lowercase())
            })
            .then(a.product.id.cmp(&b.product.id))
            .then_with(|| a.umag_name.cmp(&b.umag_name))
            .then(a.umag_id.cmp(&b.umag_id))
    });
    let total = i64::try_from(all.len()).unwrap_or(i64::MAX);
    let size = q.limit.or(q.size).unwrap_or(50).clamp(1, MAX_PAGE);
    let offset = q
        .offset
        .or_else(|| q.page.map(|p| p.max(0).saturating_mul(size)))
        .unwrap_or(0)
        .max(0);
    let page: Vec<Cand> = all
        .into_iter()
        .skip(usize::try_from(offset).unwrap_or(usize::MAX))
        .take(usize::try_from(size).unwrap_or(0))
        .collect();
    let ids: Vec<Uuid> = page.iter().map(|c| c.umag_id).collect();
    let hits: HashMap<Uuid, Hit> = load_hits(&mut tx, user.branch_id, &ids, "", "")
        .await?
        .into_iter()
        .map(|h| (h.id, h))
        .collect();
    let mut out = Vec::with_capacity(page.len());
    for c in page {
        // Одна строка Умага может стоять у нескольких товаров: копия подсказки на каждую пару.
        let Some(umag) = hits.get(&c.umag_id).cloned() else {
            continue;
        };
        out.push(PairOut {
            product: c.product,
            umag,
            similarity: share(c.sim)?,
        });
    }
    tx.commit().await?;
    Ok(paged(out, total))
}

// ---------- Карточка товара ----------

/// Строка Умага, связанная с товаром в этом филиале (SPEC-35 §3): блок «Умаг» карточки.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ProductUmag {
    pub id: Uuid,
    pub name: String,
    pub barcode: String,
    pub codes: Vec<String>,
    pub price_tyiyn: i64,
    /// Закупка в Умаге — только владельцу (ADR-008).
    pub cost_tyiyn: Option<i64>,
}

pub(crate) async fn product_umag(
    conn: &mut PgConnection,
    user: &CurrentUser,
    product_id: Uuid,
) -> AppResult<Option<ProductUmag>> {
    let row = sqlx::query!(
        r#"select id, name, barcode, codes, price_tyiyn, cost_tyiyn from old_catalog
           where branch_id = $1 and product_id = $2 order by id limit 1"#,
        user.branch_id,
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| ProductUmag {
        id: r.id,
        name: r.name,
        barcode: r.barcode,
        codes: r.codes,
        price_tyiyn: r.price_tyiyn,
        cost_tyiyn: user.is_owner().then_some(r.cost_tyiyn),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_from_names() {
        let m = name_marks("CASTROL MAGNATEC 5W30 4Л");
        assert_eq!(m.viscosity.as_deref(), Some("5w30"));
        assert_eq!(m.volume_ml, Some(4000));
        let m = name_marks("Тормозная жидкость DOT-4 0,5 л");
        assert_eq!((m.volume_ml, m.viscosity), (Some(500), None));
        assert_eq!(
            name_marks("Lukoil Luxe 5/40 4l").viscosity.as_deref(),
            Some("5w40")
        );
        assert_eq!(
            name_marks("LUKOIL 15/40 SG/CD").viscosity.as_deref(),
            Some("15w40")
        );
        assert_eq!(
            name_marks("Castrol Transmax 75W-90 1 л").volume_ml,
            Some(1000)
        );
        // Номер фильтра не вязкость: «12/75» внутри «712/75».
        assert_eq!(
            name_marks("Фильтр масляный W 712/75 (VW)"),
            NameMarks::default()
        );
        assert_eq!(name_marks("Очиститель Hi-Gear 325ml").volume_ml, Some(325));
        assert_eq!(name_marks("ЛИТОЛ AGRINOL 20КГ").weight_g, Some(20_000));
        assert_eq!(
            name_marks("ТОРМОЗНАЯ ЖИДКОСТЬ DOT-4 450г").weight_g,
            Some(450)
        );
        assert_eq!(name_marks("AZMOL 5w40 200l").volume_ml, Some(200_000));
        // «А·ч» и «мин» — не единицы объёма.
        assert_eq!(
            name_marks("Аккумулятор Тюмень 55 А·ч L"),
            NameMarks::default()
        );
        assert_eq!(name_marks("Промывка двигателя 5 мин").volume_ml, None);
    }

    #[test]
    fn pairs_keep_volume_viscosity_and_brand() {
        let brands = vec!["castrol".to_string(), "mobil".to_string()];
        assert!(compatible(
            "Castrol Magnatec 5W-30",
            "CASTROL MAGNATEC 5W30 4Л",
            &brands
        ));
        assert!(!compatible("Idemitsu 5W-30", "IDEMITSU 5W40 4Л", &brands));
        assert!(!compatible(
            "Shell Helix 10W-40 1 л",
            "SHELL HELIX 10W40 4л",
            &brands
        ));
        assert!(!compatible("Mobil Super 5W-40", "CASTROL 5W40 4L", &brands));
        // Бренда нет в одном из названий — не мешает.
        assert!(compatible("Масло 5W-40", "MOBIL SUPER 5W40 4Л", &brands));
        // Вес и объём не сравниваются между собой.
        assert!(compatible(
            "Антифриз Felix зелёный 5 кг",
            "АНТИФРИЗ X-COOL ЗЕЛЁНЫЙ 5Л",
            &brands
        ));
        // Бренд — словом, а не частью слова.
        assert!(compatible("Automobil 5W-40", "CASTROL 5W40", &brands));
    }

    #[test]
    fn similarity_is_shown_as_a_share() {
        assert_eq!(share(734).unwrap().to_string(), "0.734");
        assert_eq!(share(1000).unwrap().to_string(), "1.0");
        assert_eq!(share(450).unwrap().to_string(), "0.45");
    }

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn new_upload_keeps_links() {
        let p1 = uid(1);
        let p2 = uid(2);
        let old = vec![
            LinkedRow {
                barcode: "2760488".into(),
                keys: vec!["2760488".into(), "1489".into()],
                codes: vec!["2760488".into(), "1489".into()],
                product_id: p1,
            },
            LinkedRow {
                barcode: "2760500".into(),
                keys: vec!["2760500".into(), "4600".into()],
                codes: vec!["2760500".into(), "4600".into()],
                product_id: p2,
            },
        ];
        let fresh = vec![
            // Тот же штрихкод, новый «доп. код» — переносится только он.
            FreshRow {
                id: uid(10),
                barcode: "2760488".into(),
                keys: vec!["2760488".into(), "1489".into(), "8809451090361".into()],
                codes: vec!["2760488".into(), "1489".into(), "8809451090361".into()],
            },
            // Новый штрихкод, но прежний «доп. код».
            FreshRow {
                id: uid(11),
                barcode: "2770000".into(),
                keys: vec!["2770000".into(), "4600".into()],
                codes: vec!["2770000".into(), "4600".into()],
            },
            // Коды двух разных товаров — связи нет.
            FreshRow {
                id: uid(12),
                barcode: "2780000".into(),
                keys: vec!["2780000".into(), "1489".into(), "4600".into()],
                codes: vec!["2780000".into(), "1489".into(), "4600".into()],
            },
            // Новая строка.
            FreshRow {
                id: uid(13),
                barcode: "2790000".into(),
                keys: vec!["2790000".into()],
                codes: vec!["2790000".into()],
            },
        ];
        assert_eq!(
            restore_links(&old, &fresh),
            vec![
                Restored {
                    row_id: uid(10),
                    product_id: p1,
                    new_codes: vec!["8809451090361".into()],
                },
                Restored {
                    row_id: uid(11),
                    product_id: p2,
                    new_codes: vec!["2770000".into()],
                },
            ]
        );
    }
}
