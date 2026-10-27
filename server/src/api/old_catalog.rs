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
/// Предел «доп. кода» строки: в нём бывает и двадцать EAN через «;».
const MAX_EXTRA_CODE: usize = 2_000;

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
    /// Ключ строки (`umag_key`): штрихкод, без него — название; так же записаны отказы «Не то».
    key: String,
    /// Штрихкод и «доп. коды» — по ним узнаётся та же строка в новой выгрузке.
    keys: Vec<String>,
    /// Коды строки, которые уже были у товара: переносимые, кроме отмеченных конфликтом — код,
    /// занятый тогда другим товаром, товару не переходил, и новая выгрузка перенесёт его, если он
    /// освободился.
    codes: Vec<String>,
    product_id: Uuid,
}

/// Строка новой выгрузки.
struct FreshRow {
    id: Uuid,
    barcode: String,
    key: String,
    keys: Vec<String>,
    /// Коды, которые переходят товару (`umag_product_codes`).
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
/// него — строка с общим кодом (штрихкод или «доп. код»), если он ведёт к одному товару, а строка
/// без штрихкода и «доп. кодов» — по названию. Разные товары — связи нет, решит человек. Пара,
/// отвязанная или отвергнутая («Не то», `rejects`: ключ строки и товар), не возвращается.
/// Переносятся только новые коды строки: коды, которые владелец убрал из карточки товара,
/// выгрузка не возвращает.
fn restore_links(
    old: &[LinkedRow],
    fresh: &[FreshRow],
    rejects: &HashSet<(String, Uuid)>,
) -> Vec<Restored> {
    let mut by_barcode: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, o) in old.iter().enumerate() {
        if o.barcode.is_empty() {
            by_name.entry(o.key.as_str()).or_default().push(i);
        } else {
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
        if matched.is_empty() && n.barcode.is_empty() {
            matched = by_name.get(n.key.as_str()).cloned().unwrap_or_default();
        }
        let products: HashSet<Uuid> = matched.iter().map(|&i| old[i].product_id).collect();
        let products: Vec<Uuid> = products.into_iter().collect();
        let &[product_id] = products.as_slice() else {
            continue;
        };
        if rejects.contains(&(n.key.clone(), product_id)) {
            continue;
        }
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
        r#"select barcode, umag_key(barcode, name) as "key!", keys,
                  array(select c from unnest(umag_product_codes(codes, unit)) c
                        where not exists (select 1 from jsonb_array_elements(code_conflicts) j
                                          where j ->> 'code' = c)) as "codes!",
                  product_id as "product_id!"
           from old_catalog where branch_id = $1 and product_id is not null"#,
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| LinkedRow {
        barcode: r.barcode,
        key: r.key,
        keys: r.keys,
        codes: r.codes,
        product_id: r.product_id,
    })
    .collect();
    let rejects: HashSet<(String, Uuid)> = sqlx::query!(
        "select umag_barcode, product_id from old_catalog_link_rejects where branch_id = $1",
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| (r.umag_barcode, r.product_id))
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
        // «Доп. код» — несколько кодов подряд: обрезанный на 100 знаках, он оставлял обрывок
        // последнего кода (проверка 26.10.2026); каждый кусок — не длиннее кода (`umag_codes`).
        &text(|r| &r.extra_code, MAX_EXTRA_CODE),
        &money(|r| r.cost_tyiyn),
        &money(|r| r.price_tyiyn),
        &text(|r| &r.unit, 20),
        &text(|r| &r.supplier, 200),
        &text(|r| &r.category, 200),
    )
    .execute(&mut *tx)
    .await?;
    let fresh: Vec<FreshRow> = sqlx::query!(
        r#"select id, barcode, umag_key(barcode, name) as "key!", keys,
                  umag_product_codes(codes, unit) as "codes!"
           from old_catalog where branch_id = $1 order by id"#,
        branch_id
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| FreshRow {
        id: r.id,
        barcode: r.barcode,
        key: r.key,
        keys: r.keys,
        codes: r.codes,
    })
    .collect();
    let plan = restore_links(&old, &fresh, &rejects);
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
    /// Закупка в Умаге — тем, кто заводит карточку с закупкой: владелец, права «Приход»,
    /// «Ревизия», «Товары и штрихкоды» (ADR-008, ADR-113); остальным — `null`.
    cost_tyiyn: Option<i64>,
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

/// Права, с которыми карточку заводят с закупкой: приход (закупка строки), ревизия и «Сканер
/// товаров» (по ней оценивается найденный остаток). Им видна закупка в Умаге (ADR-113).
const COST_RIGHTS: [&str; 3] = ["receipts", "revision", "products"];

/// Видна ли пользователю закупка в Умаге в подсказках.
async fn sees_umag_cost(conn: &mut PgConnection, user: &CurrentUser) -> AppResult<bool> {
    if user.is_owner() {
        return Ok(true);
    }
    let rights: Vec<String> = COST_RIGHTS.iter().map(|r| r.to_string()).collect();
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from user_permissions
                          where user_id = $1 and branch_id = $2 and permission = any($3)) as "e!""#,
        user.id,
        user.branch_id,
        &rights
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Строки Умага в порядке `ids`; `exact` — строки, чей код совпал с искомым. `cost` — показать
/// ли закупку (`sees_umag_cost`).
async fn load_hits(
    conn: &mut PgConnection,
    branch_id: Uuid,
    ids: &[Uuid],
    exact: &HashSet<Uuid>,
    cost: bool,
) -> AppResult<Vec<Hit>> {
    let rows = sqlx::query!(
        r#"select o.id, o.name, o.barcode, o.article, o.cost_tyiyn, o.price_tyiyn, o.unit,
                  o.supplier, o.category, o.codes, o.code_conflicts, o.product_id,
                  p.name as "product_name?",
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
        ids
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
                    cost_tyiyn: cost.then_some(r.cost_tyiyn),
                    price_tyiyn: r.price_tyiyn,
                    unit: r.unit,
                    supplier: r.supplier,
                    category: r.category,
                    exact: exact.contains(&r.id),
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

/// Набранный текст, похожий на код (латиница, цифры и дефис, есть цифра, от трёх знаков): он
/// ищется и среди кодов строк Умага — артикул без учёта регистра, а от четырёх знаков и по началу
/// кода (замечание заказчика 26.10.2026: «в приходе по коду тоже чтобы искал»). Иначе — пусто.
fn typed_code(text: &str) -> &str {
    let code_like = text.len() >= 3
        && text.chars().any(|c| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if code_like { text } else { "" }
}

/// Подсказки: сначала точное совпадение кода (штрихкод, артикул, «доп. код», числовой код без
/// потерянных нулей), потом коды, начинающиеся с набранного, потом похожие названия; среди них не
/// заведённые — выше: блок «Из старой базы Умаг» показывает только их (SPEC-35 §2).
async fn search(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SearchQuery>,
) -> AppResult<Json<Vec<Hit>>> {
    let code = q.barcode.trim();
    let text = q.q.trim();
    check_search(code, text)?;
    if code.is_empty() && text.chars().count() < 2 {
        return Ok(Json(vec![]));
    }
    let mut conn = state.pool.acquire().await?;
    let found = sqlx::query!(
        r#"select o.id, m.exact as "exact!"
           from old_catalog o
           cross join lateral (
               select
                 -- Код совпал: штрихкод, артикул, «доп. код» (и числовой без потерянных нулей).
                 (($2 <> '' and (o.article = $2 or umag_has_code(o.codes, $2)))
                  or ($3 <> '' and umag_has_code(o.codes, $3))
                  or ($4 <> '' and lower(o.article) = lower($4))) as exact,
                 -- Код строки начинается с набранного: штрихкод или «доп. код» набирают по
                 -- цифрам, список Умага подсказывает, не дожидаясь Enter.
                 ($4 <> '' and length($4) >= 4
                  and exists (select 1 from unnest(o.codes) c
                              where starts_with(lower(c), lower($4)))) as code_prefix
           ) m
           where o.branch_id = $1
             and (m.exact or m.code_prefix
                  or ($3 <> '' and (o.name ilike '%' || $3 || '%' or similarity(o.name, $3) >= 0.25
                                    or o.article ilike $3 || '%')))
           order by m.exact desc, m.code_prefix desc,
                    (o.product_id is null
                     and not exists (select 1 from unnest(o.keys) c
                                     cross join lateral code_products(c) x where not x.internal)) desc,
                    similarity(o.name, $3) desc, o.name, o.id
           limit 20"#,
        user.branch_id,
        code,
        text,
        typed_code(text)
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = found.iter().map(|r| r.id).collect();
    let exact: HashSet<Uuid> = found.iter().filter(|r| r.exact).map(|r| r.id).collect();
    let cost = sees_umag_cost(&mut conn, &user).await?;
    Ok(Json(
        load_hits(&mut conn, user.branch_id, &ids, &exact, cost).await?,
    ))
}

/// Предел текста поиска по Умагу (ADR-090): сходство названий с длинным текстом считается секунды.
const MAX_SEARCH_TEXT: usize = 100;

/// Поиск по Умагу — код не длиннее кода товара, текст — не длиннее 100 знаков.
fn check_search(code: &str, text: &str) -> AppResult<()> {
    if code.chars().count() > 64 {
        return Err(invalid("код для поиска — не длиннее 64 знаков"));
    }
    if text.chars().count() > MAX_SEARCH_TEXT {
        return Err(invalid(format!(
            "текст для поиска — не длиннее {MAX_SEARCH_TEXT} знаков"
        )));
    }
    Ok(())
}

/// Переносит коды строк Умага связанным товарам (SPEC-35 §1): код становится кодом товара с
/// источником «Умаг», если его нет ни у одного товара и у другого товара нет того же числового
/// кода с нулями впереди. Один код (и тот же числовой с другим числом нулей впереди) в одной
/// пачке получает первый товар по порядку пар, строка другого — отметку конфликта. Занятый код
/// остаётся отметкой конфликта у строки. Товар в архиве кодов не получает. Возвращает
/// перенесённые коды с товаром в порядке пар.
async fn copy_codes(
    conn: &mut PgConnection,
    pairs: &[(String, Uuid)],
) -> AppResult<Vec<(String, Uuid)>> {
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
           select distinct on (umag_bare(t.code)) t.code, t.product_id, false, 'umag'
           from unnest($1::text[], $2::uuid[]) with ordinality as t(code, product_id, n)
           join products p on p.id = t.product_id and not p.archived
           where not exists (select 1 from product_barcodes b where b.code = t.code)
             and not exists (select 1 from code_products(t.code) x where x.product_id <> t.product_id)
           order by umag_bare(t.code), t.n
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
        .zip(products)
        .filter(|(c, _)| done.contains(c) && seen.insert(c.clone()))
        .collect())
}

/// Отметки «конфликт кода» строк заново: переносимые коды строки, которые нашли бы другой товар.
async fn refresh_conflicts(conn: &mut PgConnection, ids: &[Uuid]) -> AppResult<()> {
    sqlx::query!(
        r#"update old_catalog o set code_conflicts = coalesce((
               select jsonb_agg(jsonb_build_object('code', c.code, 'product_id', x.product_id,
                                                   'product_name', p.name) order by c.n)
               from unnest(umag_product_codes(o.codes, o.unit)) with ordinality as c(code, n)
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

/// У товара убрали код (SPEC-35 §1): строки Умага, чей код был занят этим товаром (отметка
/// конфликта), получают освободившийся код своему товару; отметки — заново. Без этого код
/// пропадал молча: отметка устаревала, а новая выгрузка считала код уже перенесённым.
/// Возвращает перенесённые коды с товаром.
pub(crate) async fn codes_freed(
    conn: &mut PgConnection,
    from_product: Uuid,
) -> AppResult<Vec<(String, Uuid)>> {
    let rows = sqlx::query!(
        r#"select o.id, o.product_id as "product_id!", j ->> 'code' as "code!"
           from old_catalog o cross join lateral jsonb_array_elements(o.code_conflicts) j
           where o.product_id is not null and j ->> 'product_id' = $1::text
           order by o.id"#,
        from_product.to_string()
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let pairs: Vec<(String, Uuid)> = rows
        .iter()
        .map(|r| (r.code.clone(), r.product_id))
        .collect();
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let copied = copy_codes(&mut *conn, &pairs).await?;
    refresh_conflicts(&mut *conn, &ids).await?;
    Ok(copied)
}

/// Отметки конфликтов строк, связанных с этими товарами или отмеченных их кодами, — заново
/// (объединение карточек переносит коды и связи дубля на основную).
pub(crate) async fn refresh_conflicts_of(
    conn: &mut PgConnection,
    products: &[Uuid],
) -> AppResult<()> {
    let ids: Vec<String> = products.iter().map(Uuid::to_string).collect();
    let rows = sqlx::query_scalar!(
        r#"select o.id from old_catalog o
           where o.product_id = any($1::text[]::uuid[])
              or exists (select 1 from jsonb_array_elements(o.code_conflicts) j
                         where j ->> 'product_id' = any($1))"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    refresh_conflicts(&mut *conn, &rows).await
}

/// Ставит связи строк с товарами и переносит коды строк; отметки конфликтов — заново.
/// Возвращает перенесённые коды с товаром.
async fn link_rows(
    conn: &mut PgConnection,
    links: &[(Uuid, Uuid)],
) -> AppResult<Vec<(String, Uuid)>> {
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
           cross join lateral unnest(umag_product_codes(o.codes, o.unit)) with ordinality as c(code, k)
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
    let copied: Vec<String> = link_rows(&mut *conn, &[(row_id, product_id)])
        .await?
        .into_iter()
        .map(|(c, _)| c)
        .collect();
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
/// товара — то же правило, что автосвязывание по точному коду: числовой код сравнивается без
/// нулей впереди с обеих сторон (код товара «0R12» и строка «00R12» — один код). Без отказов
/// «Не то» для пары.
async fn rows_coded_for(
    conn: &mut PgConnection,
    ctx: &Ctx,
    product_id: Uuid,
) -> AppResult<Vec<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"select o.id from old_catalog o
           where o.branch_id = $1 and o.product_id is null
             and o.keys_bare && array(select umag_bare(b.code) from product_barcodes b
                                      where b.product_id = $2 and not b.internal)
             and not exists (select 1 from unnest(o.keys) c
                             cross join lateral code_products(c) x
                             join products p on p.id = x.product_id
                             where x.product_id <> $2 and not p.archived and not x.internal)
             and not exists (select 1 from old_catalog_link_rejects r
                             where r.branch_id = $1 and r.umag_barcode = umag_key(o.barcode, o.name)
                               and r.product_id = $2)
           order by o.id"#,
        ctx.user.branch_id,
        product_id
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

/// Пар в одной записи журнала автосвязывания: больше — несколько записей (часть 1 из N), чтобы
/// запись не разрасталась, а по журналу восстанавливалась каждая связь.
const AUDIT_PAIRS: usize = 500;

/// «Связать автоматически» (SPEC-35 §3): строка без связи получает товар, если её штрихкод или
/// «доп. код» (не выданный программой) есть ровно у одного действующего товара или её название
/// после нормализации (регистр, пробелы, кавычки, «ё»→«е») совпадает с названием ровно одного
/// товара, а в списке филиала такое название — только у неё (одинаковые названия у разных
/// строк Умага — разные товары, решит человек). Пары «не то» не связываются. Коды строк
/// переносятся товарам.
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
           ), single as (
               select name_norm from old_catalog where branch_id = $1
               group by name_norm having count(*) = 1
           )
           select o.id as "id!", (array_agg(distinct n.id))[1] as "product_id!"
           from old_catalog o
           join single s on s.name_norm = o.name_norm
           join names n on n.k = o.name_norm and n.k <> ''
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
    // Журнал: каждая пара — строка (id и штрихкод: id меняются с новой выгрузкой), товар и
    // перенесённые ему коды этой строки.
    let ids: Vec<Uuid> = all.iter().map(|(r, _)| *r).collect();
    let rows: HashMap<Uuid, (String, Vec<String>)> = sqlx::query!(
        r#"select id, barcode, umag_product_codes(codes, unit) as "codes!"
           from old_catalog where id = any($1)"#,
        &ids
    )
    .fetch_all(&mut *tx)
    .await?
    .into_iter()
    .map(|r| (r.id, (r.barcode, r.codes)))
    .collect();
    let copied_to: HashSet<(&str, Uuid)> = copied.iter().map(|(c, p)| (c.as_str(), *p)).collect();
    let pairs: Vec<Value> = all
        .iter()
        .map(|(r, p)| {
            let (barcode, codes) = rows.get(r).cloned().unwrap_or_default();
            let moved: Vec<String> = codes
                .into_iter()
                .filter(|c| copied_to.contains(&(c.as_str(), *p)))
                .collect();
            json!({ "umag_id": r, "barcode": barcode, "product_id": p, "copied": moved })
        })
        .collect();
    let chunks: Vec<&[Value]> = if pairs.is_empty() {
        vec![&[]]
    } else {
        pairs.chunks(AUDIT_PAIRS).collect()
    };
    let parts = chunks.len();
    for (i, chunk) in chunks.into_iter().enumerate() {
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
                "part": i + 1,
                "parts": parts,
                "pairs": chunk,
            }),
        )
        .await?;
    }
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

/// Пара не расходится: объём, вес, вязкость и бренд, если они есть в обоих названиях, совпадают;
/// килограммы и литры (антифриз «5 кг» и «5 л») — с допуском в четверть.
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
    let far = |x: i64, y: i64| x.abs_diff(y).saturating_mul(4) > x.max(y).unsigned_abs();
    if let (Some(v), None, None, Some(w)) | (None, Some(w), Some(v), None) =
        (a.volume_ml, a.weight_g, b.volume_ml, b.weight_g)
        && far(v, w)
    {
        return false;
    }
    let (pa, pb) = (brands_in(product, brands), brands_in(umag, brands));
    pa.is_empty() || pb.is_empty() || !pa.is_disjoint(&pb)
}

/// Куски названия для сравнения пар: слова, номера деталей, вязкость; объём и вес («4л», «5 кг»,
/// «0,5 л») пропускаются — их сравнивает `compatible`.
#[derive(Debug, Default, PartialEq, Eq)]
struct NameTokens {
    /// Слова из трёх и больше букв и куски с буквами и цифрами («hx7», «x7», «5w30»), без единиц
    /// меры и служебных слов.
    words: Vec<String>,
    /// Номера деталей: куски с цифрами, кроме вязкости («fc158», «71275», «3000»).
    parts: Vec<String>,
    /// Латинские слова из трёх и больше букв («castrol», «felix»).
    latin: Vec<String>,
}

/// Вязкость отдельным куском: «5w30», «10w-40», «15/40».
fn is_viscosity(tok: &str) -> bool {
    let c: Vec<char> = tok.chars().collect();
    let lead = c.iter().take_while(|x| x.is_ascii_digit()).count();
    if !(1..=2).contains(&lead) || !matches!(c.get(lead), Some('w' | '/')) {
        return false;
    }
    let rest: String = c[lead + 1..].iter().filter(|x| **x != '-').collect();
    rest.len() == 2 && rest.chars().all(|x| x.is_ascii_digit())
}

/// Слова, номера деталей и латинские слова названия (`NameTokens`).
fn name_tokens(name: &str) -> NameTokens {
    const STOP: [&str; 10] = [
        "для", "без", "под", "над", "при", "или", "the", "and", "for", "with",
    ];
    let t = norm_text(name);
    // Запятая и точка между цифрами — дробь («0,5 л»), на краях куска — разделитель.
    let raw: Vec<&str> = t
        .split(|c: char| c.is_whitespace() || "()[];«»\"'+!?:*".contains(c))
        .map(|x| x.trim_matches(|c: char| c == ',' || c == '.' || c == '-'))
        .filter(|x| !x.is_empty())
        .collect();
    let mut out = NameTokens::default();
    let push = |list: &mut Vec<String>, x: &str| {
        if !list.iter().any(|y| y == x) {
            list.push(x.to_string());
        }
    };
    for (i, tok) in raw.iter().enumerate() {
        let has_digit = tok.chars().any(|c| c.is_ascii_digit());
        // Число с единицей — слитно («4л», «0,5l») или следующим словом («5 кг») — мера.
        let num_end = tok
            .find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '.'))
            .unwrap_or(tok.len());
        if has_digit && num_end > 0 {
            let unit = &tok[num_end..];
            let next_is_unit = raw
                .get(i + 1)
                .is_some_and(|n| unit_of(n.trim_end_matches('.')).is_some());
            if (unit.is_empty() && next_is_unit) || (!unit.is_empty() && unit_of(unit).is_some()) {
                continue;
            }
        }
        let joined: String = tok.chars().filter(|c| c.is_alphanumeric()).collect();
        if joined.is_empty() {
            continue;
        }
        let has_letter = joined.chars().any(char::is_alphabetic);
        if has_digit {
            let viscosity = is_viscosity(tok);
            if !viscosity {
                push(&mut out.parts, &joined);
            }
            if has_letter && joined.chars().count() >= 2 {
                push(&mut out.words, &joined);
            }
        } else if joined.chars().count() >= 3
            && unit_of(&joined).is_none()
            && !STOP.contains(&joined.as_str())
        {
            push(&mut out.words, &joined);
            if joined.chars().all(|c| c.is_ascii_alphabetic()) {
                push(&mut out.latin, &joined);
            }
        }
    }
    out
}

/// Одно слово: то же или с другим окончанием («каркас» — «каркасная», «щетка» — «щеткой»); куски
/// с цифрами — только те же.
fn same_word(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    if a.chars().chain(b.chars()).any(|c| c.is_ascii_digit()) {
        return false;
    }
    let shorter = a.chars().count().min(b.chars().count());
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count();
    shorter >= 4 && common >= 4.max(shorter.saturating_sub(2))
}

/// Сколько слов `a` есть в `b` (каждое слово `b` — один раз).
fn common_words(a: &[String], b: &[String]) -> usize {
    let mut used = vec![false; b.len()];
    let mut n = 0;
    for x in a {
        if let Some(j) = (0..b.len()).find(|&j| !used[j] && same_word(x, &b[j])) {
            used[j] = true;
            n += 1;
        }
    }
    n
}

/// Номер детали тот же: совпадает или один содержит другой («w71275» и «71275»).
fn same_part(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    a == b || (short.len() >= 3 && long.contains(short))
}

/// Пара похожа на правду (SPEC-35 §3, проверка 26.10.2026): признаки названий не расходятся
/// (`compatible`), общих слов — не меньше двух («фильтр хай» — не «фильтр вехчай»), номера
/// деталей, если они есть в обоих названиях, совпадают («FC-158» — не «ST385»), латинские слова
/// обоих названий пересекаются («Felix» — не «X-COOL»).
fn plausible(product: &str, umag: &str, brands: &[String]) -> bool {
    if !compatible(product, umag, brands) {
        return false;
    }
    let (a, b) = (name_tokens(product), name_tokens(umag));
    if common_words(&a.words, &b.words) < 2 {
        return false;
    }
    if !a.parts.is_empty()
        && !b.parts.is_empty()
        && !a
            .parts
            .iter()
            .any(|x| b.parts.iter().any(|y| same_part(x, y)))
    {
        return false;
    }
    a.latin.is_empty() || b.latin.is_empty() || common_words(&a.latin, &b.latin) > 0
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
/// Сколько похожих строк Умага показывать у одного товара и сколько товаров — у одной строки.
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
    check_search("", &text)?;
    let rows = sqlx::query!(
        r#"with free as (
               select p.id, p.name, p.brand, p.article,
                      case when btrim(p.brand) <> '' and strpos(lower(p.name), lower(btrim(p.brand))) = 0
                           then btrim(p.brand) || ' ' || p.name else p.name end as full_name
               from products p
               where not p.archived
                 and not exists (select 1 from old_catalog l where l.branch_id = $1 and l.product_id = p.id)
                 -- Код товара — штрихкод или «доп. код» строки Умага (и с другим числом нулей
                 -- впереди): строка уже его, она не «похожая».
                 and not exists (select 1 from product_barcodes b
                                 where b.product_id = p.id and not b.internal
                                   and exists (select 1 from old_catalog l
                                               where l.branch_id = $1
                                                 and l.keys_bare @> array[umag_bare(b.code)]))
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
           where not exists (select 1 from unnest(o.keys) c
                             cross join lateral code_products(c) x where not x.internal)
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
        if !plausible(&r.full_name, &r.umag_name, &brands) {
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
    // У товара — до трёх самых похожих строк, и у строки Умага — до трёх товаров: одну строку
    // («АРОМАТИЗАТОР») не предлагают десяткам товаров.
    let mut per_umag: HashMap<Uuid, Vec<Cand>> = HashMap::new();
    for mut v in per.into_values() {
        v.sort_by(|a, b| b.sim.cmp(&a.sim).then(a.umag_name.cmp(&b.umag_name)));
        v.truncate(PER_PRODUCT);
        for c in v {
            per_umag.entry(c.umag_id).or_default().push(c);
        }
    }
    let mut all: Vec<Cand> = per_umag
        .into_values()
        .flat_map(|mut v| {
            v.sort_by(|a, b| {
                b.sim
                    .cmp(&a.sim)
                    .then_with(|| a.product.name.cmp(&b.product.name))
                    .then(a.product.id.cmp(&b.product.id))
            });
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
    let cost = sees_umag_cost(&mut tx, &user).await?;
    let hits: HashMap<Uuid, Hit> = load_hits(&mut tx, user.branch_id, &ids, &HashSet::new(), cost)
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
    /// Коды меры (строка «литр», «кг», «метр»: 276…, PLU): товару не переходят — на кассе они
    /// добавили бы всю тару (ADR-113).
    pub measure_codes: Vec<String>,
    pub unit: String,
    pub price_tyiyn: i64,
    /// Закупка в Умаге — только владельцу (ADR-008).
    pub cost_tyiyn: Option<i64>,
}

/// Строки Умага филиала, связанные с товаром, — первая по порядку и остальные: у товара их
/// бывает несколько (канистра и литр, дубли в Умаге), лишнюю видно и её можно отвязать.
pub(crate) async fn product_umag(
    conn: &mut PgConnection,
    user: &CurrentUser,
    product_id: Uuid,
) -> AppResult<Vec<ProductUmag>> {
    let rows = sqlx::query!(
        r#"select id, name, barcode, codes, unit, price_tyiyn, cost_tyiyn,
                  array(select c from unnest(codes) c
                        where not (c = any(umag_product_codes(codes, unit)))) as "measure!"
           from old_catalog
           where branch_id = $1 and product_id = $2 order by id"#,
        user.branch_id,
        product_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| ProductUmag {
            id: r.id,
            name: r.name,
            barcode: r.barcode,
            codes: r.codes,
            measure_codes: r.measure,
            unit: r.unit,
            price_tyiyn: r.price_tyiyn,
            cost_tyiyn: user.is_owner().then_some(r.cost_tyiyn),
        })
        .collect())
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
        // Килограммы и литры — с допуском в четверть: 5 кг антифриза — не 1 л и не 10 л.
        assert!(compatible(
            "Антифриз Felix зелёный 5 кг",
            "АНТИФРИЗ X-COOL ЗЕЛЁНЫЙ 5Л",
            &brands
        ));
        assert!(!compatible(
            "Антифриз Felix 5 кг",
            "АНТИФРИЗ зелёный 1Л",
            &brands
        ));
        assert!(!compatible("Sintec G12 5 кг", "VERTEX G12+ 10Л", &brands));
        // Бренд — словом, а не частью слова.
        assert!(compatible("Automobil 5W-40", "CASTROL 5W40", &brands));
    }

    #[test]
    fn false_pairs_from_the_point_are_dropped() {
        // Ложные пары рабочей базы 26.10.2026 (проверка похожих пар).
        let brands = vec!["aykos".to_string()];
        for (product, umag) in [
            // Номера деталей разные.
            ("FC-158 (Toyota)", "SCT ST385 TOYOTA"),
            ("C-224 (Nissan)", "SCT SM 148 (W 610/83)"),
            // Марки латиницей разные: Felix — не X-COOL, Sintec — не Vertex.
            ("Антифриз Felix зелёный 5 кг", "АНТИФРИЗ X-COOL ЗЕЛЁНЫЙ 5Л"),
            ("Антифриз Sintec G12 5 кг", "АНТИФРИЗ VERTEX G12+ 5Л"),
            // Объём и вес не сходятся.
            ("Антифриз Felix 5 кг", "АНТИФРИЗ зелёный 1Л"),
            // Одно общее слово — не пара.
            ("фильтр хай", "фильтр вехчай"),
            ("Ароматизатор 5", "АРОМАТИЗАТОР ОЛЕНЬ"),
            ("Ароматизатор Ёлочка", "АРОМАТИЗАТОР w"),
        ] {
            assert!(!plausible(product, umag, &brands), "{product} ↔ {umag}");
        }
        // Настоящие пары остаются.
        for (product, umag) in [
            ("Castrol Magnatec 5W-30 4 л", "CASTROL MAGNATEC 5W30 4Л"),
            ("Idemitsu 5W-30", "IDEMITSU 5W30 4Л"),
            ("ZIC X7 5W-30", "ZIC X7 5W30 4Л"),
            ("Shell Helix HX7 10W-40", "SHELL HX7 10W40 4Л EURO"),
            ("Тормозная жидкость DOT-4 0,5 л", "Дот тормозная жидкость"),
            ("Фильтр общий", "Фильтр общий"),
            (
                "Щётка стеклоочистителя каркасная 12345678",
                "Щетка стеклоочистителя каркас 12345678",
            ),
            ("Фильтр масляный W 712/75 (VW)", "ФИЛЬТР МАСЛЯНЫЙ W712/75"),
            (
                "Тормозная жидкость DOT-4 0,5 л",
                "ТОРМОЗНАЯ ЖИДКОСТЬ DOT-4 0,5Л",
            ),
            ("Масло Ромашка 5W-30 4л", "Масло Ромашка 5W-30 4л тара"),
        ] {
            assert!(plausible(product, umag, &brands), "{product} ↔ {umag}");
        }
    }

    #[test]
    fn name_tokens_skip_volume_and_weight() {
        let parts = |s: &str| name_tokens(s).parts;
        assert_eq!(parts("FC-158 (Toyota)"), ["fc158"]);
        assert_eq!(parts("SCT SM 148 (W 610/83)"), ["148", "61083"]);
        assert!(parts("CASTROL MAGNATEC 5W30 4Л").is_empty());
        assert!(parts("Масло 10W-40 4 л").is_empty());
        assert!(parts("LUKOIL 15/40 0,5 l").is_empty());
        assert!(parts("Антифриз 5 кг").is_empty());
        assert_eq!(parts("АНТИФРИЗ VERTEX G12+ 10Л"), ["g12"]);
        let t = name_tokens("Shell Helix HX7 10W-40 4 л");
        assert_eq!(t.words, ["shell", "helix", "hx7", "10w40"]);
        assert_eq!(t.parts, ["hx7"]);
        assert_eq!(t.latin, ["shell", "helix"]);
        assert!(same_part("w71275", "71275"));
        assert!(!same_part("fc158", "st385"));
        assert!(same_word("каркас", "каркасная"));
        assert!(same_word("щетка", "щеткой"));
        assert!(!same_word("литол", "литий"));
        assert!(!same_word("хай", "хайвей"));
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
        let p3 = uid(3);
        let row = |barcode: &str, keys: &[&str], codes: &[&str], product_id| LinkedRow {
            barcode: barcode.into(),
            key: if barcode.is_empty() {
                "name:масло без кода".into()
            } else {
                barcode.into()
            },
            keys: keys.iter().map(|k| k.to_string()).collect(),
            codes: codes.iter().map(|k| k.to_string()).collect(),
            product_id,
        };
        let fresh = |id, barcode: &str, key: &str, keys: &[&str]| FreshRow {
            id: uid(id),
            barcode: barcode.into(),
            key: key.into(),
            keys: keys.iter().map(|k| k.to_string()).collect(),
            codes: keys.iter().map(|k| k.to_string()).collect(),
        };
        let old = vec![
            row("2760488", &["2760488", "1489"], &["2760488", "1489"], p1),
            // «4601» был занят другим товаром (конфликт): товару он не переходил.
            row(
                "2760500",
                &["2760500", "4600", "4601"],
                &["2760500", "4600"],
                p2,
            ),
            // Строка без штрихкода и «доп. кода» — узнаётся по названию.
            row("", &[], &["ART-0001"], p3),
        ];
        let fresh_rows = vec![
            // Тот же штрихкод, новый «доп. код» — переносится только он.
            fresh(
                10,
                "2760488",
                "2760488",
                &["2760488", "1489", "8809451090361"],
            ),
            // Новый штрихкод, но прежний «доп. код»; освободившийся «4601» переносится.
            fresh(11, "2770000", "2770000", &["2770000", "4600", "4601"]),
            // Коды двух разных товаров — связи нет.
            fresh(12, "2780000", "2780000", &["2780000", "1489", "4600"]),
            // Новая строка.
            fresh(13, "2790000", "2790000", &["2790000"]),
            // Та же строка без штрихкода — по названию.
            fresh(14, "", "name:масло без кода", &[]),
            // Отвязанная пара («Не то») — не возвращается.
            fresh(15, "2760501", "2760501", &["2760501", "4600"]),
        ];
        let rejects: HashSet<(String, Uuid)> = [("2760501".to_string(), p2)].into();
        assert_eq!(
            restore_links(&old, &fresh_rows, &rejects),
            vec![
                Restored {
                    row_id: uid(10),
                    product_id: p1,
                    new_codes: vec!["8809451090361".into()],
                },
                Restored {
                    row_id: uid(11),
                    product_id: p2,
                    new_codes: vec!["2770000".into(), "4601".into()],
                },
                Restored {
                    row_id: uid(14),
                    product_id: p3,
                    new_codes: vec![],
                },
            ]
        );
    }
}
