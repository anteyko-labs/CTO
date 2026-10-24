//! Филиалы (SPEC-33, ADR-108): список, новый филиал, переименование, закрытие; филиал отчёта
//! («Все филиалы» — только просмотр владельцем).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid, is_unique_violation};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/branches", routing::get(list).post(create))
        .route("/branches/{id}", routing::patch(update))
        .route("/branches/settlements", routing::get(settlements))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchOut {
    pub id: Uuid,
    pub name: String,
    pub address: String,
    pub phone: String,
    pub archived: bool,
    pub is_main: bool,
}

/// Все филиалы: главный первым, закрытые в конце.
pub async fn all(conn: &mut PgConnection) -> AppResult<Vec<BranchOut>> {
    Ok(sqlx::query_as!(
        BranchOut,
        r#"select id, name, address, phone, archived, is_main from branches
           order by is_main desc, archived, created_at"#
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Филиалы, которые видит пользователь: владелец — все, администратор — только свой.
pub async fn visible(conn: &mut PgConnection, user: &CurrentUser) -> AppResult<Vec<BranchOut>> {
    let mut list = all(conn).await?;
    if !user.is_owner() {
        list.retain(|b| b.id == user.branch_id);
    }
    Ok(list)
}

/// Название филиала для чеков, ленты и отчётов.
pub async fn name(conn: &mut PgConnection, id: Uuid) -> AppResult<String> {
    Ok(
        sqlx::query_scalar!("select name from branches where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?
            .unwrap_or_default(),
    )
}

/// Филиалов больше одного: только тогда экраны, чеки и лента показывают название филиала.
pub async fn several(conn: &mut PgConnection) -> AppResult<bool> {
    Ok(
        sqlx::query_scalar!(r#"select count(*) > 1 as "x!" from branches"#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Главный филиал: из него новый филиал копирует настройки.
pub async fn main_branch(conn: &mut PgConnection) -> AppResult<Uuid> {
    sqlx::query_scalar!("select id from branches order by is_main desc, created_at limit 1")
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| AppError::Internal("нет ни одного филиала".into()))
}

/// Филиалы отчёта (SPEC-33 §3): выбранный — один; «Все филиалы» (`all`) — только владельцу,
/// администратору — 404, как чужие данные.
pub async fn report_scope(
    conn: &mut PgConnection,
    user: &CurrentUser,
    all_branches: bool,
) -> AppResult<Vec<BranchOut>> {
    if all_branches {
        if !user.is_owner() {
            return Err(AppError::NotFound);
        }
        return all(conn).await;
    }
    let list = all(conn).await?;
    Ok(list
        .into_iter()
        .filter(|b| b.id == user.branch_id)
        .collect())
}

/// Параметр отчёта `?branch=all` — «Все филиалы» (SPEC-33 §3).
#[derive(Debug, Default, Deserialize)]
pub struct BranchQuery {
    pub branch: Option<String>,
}

impl BranchQuery {
    pub fn all(&self) -> AppResult<bool> {
        match self.branch.as_deref() {
            None | Some("") | Some("current") => Ok(false),
            Some("all") => Ok(true),
            Some(_) => Err(invalid("филиал отчёта: all или текущий")),
        }
    }
}

async fn list(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<Vec<BranchOut>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(visible(&mut conn, &user).await?))
}

#[derive(Deserialize)]
struct CreateReq {
    op_id: Uuid,
    name: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    phone: String,
}

fn clean(name: &str, address: &str, phone: &str) -> AppResult<(String, String, String)> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("название филиала обязательно"));
    }
    if name.chars().count() > 100 || address.chars().count() > 300 || phone.chars().count() > 50 {
        return Err(invalid("слишком длинное название, адрес или телефон"));
    }
    Ok((name, address.trim().to_string(), phone.trim().to_string()))
}

fn name_taken(e: sqlx::Error) -> AppError {
    if is_unique_violation(&e) {
        AppError::Conflict("филиал с таким названием уже есть".into())
    } else {
        e.into()
    }
}

/// Настройки, которые новый филиал получает копией главного (SPEC-33 §1). Не копируются
/// точка и касса банка (Optima — у каждого филиала свои) и отметки «просмотрено».
const COPIED_SETTINGS: [&str; 8] = [
    "batteries",
    "client_messages",
    "debt_docs",
    "favorites",
    "labels",
    "loyalty",
    "sales",
    "stock",
];

/// Новый филиал: свои кассы и счета («Касса», «Счёт», сейфы), статьи расходов, настройки копией
/// главного, строки остатков всех товаров с общей ценой. Нумерация документов у филиала своя
/// с 1 (счётчики по филиалу). Всё — в транзакции создания.
pub async fn init_branch(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    let main = main_branch(conn).await?;
    crate::api::cash::ensure_accounts(conn, branch_id).await?;
    crate::api::expenses::ensure_articles(conn, branch_id).await?;
    let keys: Vec<String> = COPIED_SETTINGS.iter().map(|k| (*k).to_string()).collect();
    sqlx::query!(
        r#"insert into settings (branch_id, key, value)
           select $1, key, value from settings where branch_id = $2 and key = any($3)
           on conflict do nothing"#,
        branch_id,
        main,
        &keys
    )
    .execute(&mut *conn)
    .await?;
    // Мин. остаток — как в главном; цена — общая (триггер строки).
    sqlx::query!(
        r#"insert into branch_products (branch_id, product_id, min_stock)
           select $1, p.id, coalesce(m.min_stock, 0)
           from products p left join branch_products m on m.product_id = p.id and m.branch_id = $2
           where p.merged_into is null
           on conflict do nothing"#,
        branch_id,
        main
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<BranchOut>> {
    const KIND: &str = "branch.create";
    ctx.user.require_owner()?;
    let (name, address, phone) = clean(&req.name, &req.address, &req.phone)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let id = new_id();
    sqlx::query!(
        "insert into branches (id, name, address, phone) values ($1, $2, $3, $4)",
        id,
        name,
        address,
        phone
    )
    .execute(&mut *tx)
    .await
    .map_err(name_taken)?;
    init_branch(&mut tx, id).await?;
    let out = BranchOut {
        id,
        name,
        address,
        phone,
        archived: false,
        is_main: false,
    };
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "branch",
        Some(id),
        json!({ "name": out.name, "address": out.address, "phone": out.phone }),
    )
    .await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct UpdateReq {
    name: Option<String>,
    address: Option<String>,
    phone: Option<String>,
    archived: Option<bool>,
}

async fn update(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateReq>,
) -> AppResult<Json<BranchOut>> {
    ctx.user.require_owner()?;
    let mut tx = state.pool.begin().await?;
    let before = sqlx::query_as!(
        BranchOut,
        r#"select id, name, address, phone, archived, is_main from branches
           where id = $1 for update"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let (name, address, phone) = clean(
        req.name.as_deref().unwrap_or(&before.name),
        req.address.as_deref().unwrap_or(&before.address),
        req.phone.as_deref().unwrap_or(&before.phone),
    )?;
    let archived = req.archived.unwrap_or(before.archived);
    if archived && before.is_main {
        return Err(invalid(
            "главный филиал не закрывается: в нём настройки для новых филиалов",
        ));
    }
    let out = sqlx::query_as!(
        BranchOut,
        r#"update branches set name = $2, address = $3, phone = $4, archived = $5 where id = $1
           returning id, name, address, phone, archived, is_main"#,
        id,
        name,
        address,
        phone,
        archived
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(name_taken)?;
    let action = match (before.archived, out.archived) {
        (false, true) => "branch.close",
        (true, false) => "branch.reopen",
        _ => "branch.update",
    };
    let changes = ops::changes(
        &json!({ "name": before.name, "address": before.address, "phone": before.phone }),
        &json!({ "name": out.name, "address": out.address, "phone": out.phone }),
    );
    ops::audit(
        &mut tx,
        &ctx,
        action,
        "branch",
        Some(id),
        json!({ "name": out.name, "changes": changes }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct SettlementPair {
    /// Филиал, принявший деньги за чужой долг: он должен.
    branch_id: Uuid,
    branch_name: String,
    /// Филиал, чей долг закрыт: ему должны.
    creditor_branch_id: Uuid,
    creditor_name: String,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct SettlementEntry {
    id: Uuid,
    created_at: chrono::DateTime<chrono::Utc>,
    business_date: chrono::NaiveDate,
    branch_name: String,
    creditor_name: String,
    party_id: Uuid,
    party_name: String,
    amount_tyiyn: i64,
    reversal: bool,
    user_name: String,
}

/// «Расчёты между филиалами» (SPEC-33 §2): кто кому должен за погашенные в чужом филиале долги
/// клиентов — итог по парам и последние записи. Только владельцу: это деньги филиалов.
async fn settlements(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<serde_json::Value>> {
    user.require_owner()?;
    let mut conn = state.pool.acquire().await?;
    let items = sqlx::query_as!(
        SettlementPair,
        r#"select s.branch_id, b.name as branch_name, s.creditor_branch_id, c.name as creditor_name,
                  sum(s.amount_tyiyn)::bigint as "amount_tyiyn!"
           from branch_settlements s
           join branches b on b.id = s.branch_id
           join branches c on c.id = s.creditor_branch_id
           group by s.branch_id, b.name, s.creditor_branch_id, c.name
           having sum(s.amount_tyiyn) <> 0
           order by b.name, c.name"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let entries = sqlx::query_as!(
        SettlementEntry,
        r#"select s.id, s.created_at, s.business_date, b.name as branch_name, c.name as creditor_name,
                  s.party_id, p.name as party_name, s.amount_tyiyn,
                  s.reversal_of is not null as "reversal!", u.full_name as user_name
           from branch_settlements s
           join branches b on b.id = s.branch_id
           join branches c on c.id = s.creditor_branch_id
           join parties p on p.id = s.party_id
           join users u on u.id = s.user_id
           order by s.created_at desc limit 200"#
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(json!({ "items": items, "entries": entries })))
}

/// Отчёт «Все филиалы» (SPEC-33 §3): сумма по филиалам и разбивка `branches` —
/// `[{branch_id, name, archived, report}]`. Суммируются деньги, количества и счётчики; цены,
/// доли, ставки, номера и даты берутся из первого филиала; доли (`margin_bp`, средний чек)
/// пересчитываются по сумме. Списки строк сводятся по ключу строки (день, товар, способ,
/// название), строки без ключа идут подряд.
pub fn merge_reports(parts: Vec<(BranchOut, serde_json::Value)>) -> serde_json::Value {
    use serde_json::Value;
    let mut total = Value::Null;
    let mut branches = Vec::with_capacity(parts.len());
    for (b, report) in parts {
        total = if total.is_null() {
            report.clone()
        } else {
            merge_value(total, &report, "")
        };
        branches.push(json!({
            "branch_id": b.id,
            "name": b.name,
            "archived": b.archived,
            "report": report,
        }));
    }
    recompute_ratios(&mut total);
    match total {
        Value::Object(mut m) => {
            m.insert("branches".into(), Value::Array(branches));
            m.insert("all_branches".into(), Value::Bool(true));
            Value::Object(m)
        }
        Value::Array(items) => {
            json!({ "items": items, "branches": branches, "all_branches": true })
        }
        other => json!({ "value": other, "branches": branches, "all_branches": true }),
    }
}

/// Число под этим ключом не складывается: цена, доля, ставка, номер, час, срок, средние.
fn not_additive(key: &str) -> bool {
    key.ends_with("_bp")
        || key.contains("permille")
        || key.contains("price")
        || key.contains("avg")
        || key.contains("average")
        || key.contains("rate")
        || key.ends_with("_days")
        || matches!(
            key,
            "hour"
                | "number"
                | "days"
                | "limit"
                | "page"
                | "per_page"
                | "container_ml"
                | "year"
                | "month"
        )
}

/// Значения, общие для всех филиалов (клиенты и баллы общие): берутся один раз.
fn shared_key(key: &str) -> bool {
    matches!(key, "points")
}

/// Ключ строки списка: строки с одинаковым ключом складываются.
fn row_key(v: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 15] = [
        "id",
        "employee_id",
        "product_id",
        "service_id",
        "party_id",
        "date",
        "day",
        "hour",
        "article_id",
        "category_id",
        "method",
        "key",
        "name",
        "kind",
        "slot",
    ];
    let obj = v.as_object()?;
    // Ячейка тепловой карты — день недели и час вместе.
    if let (Some(d), Some(h)) = (obj.get("dow"), obj.get("hour")) {
        return Some(format!("dow={d},hour={h}"));
    }
    KEYS.iter().find_map(|k| {
        obj.get(*k)
            .filter(|x| !x.is_null())
            .map(|x| format!("{k}={x}"))
    })
}

fn merge_value(a: serde_json::Value, b: &serde_json::Value, key: &str) -> serde_json::Value {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(mut am), Value::Object(bm)) => {
            for (k, bv) in bm {
                let merged = match am.remove(k) {
                    Some(av) if shared_key(k) => av,
                    Some(av) => merge_value(av, bv, k),
                    None => bv.clone(),
                };
                am.insert(k.clone(), merged);
            }
            Value::Object(am)
        }
        (Value::Number(an), Value::Number(bn)) => {
            if not_additive(key) {
                return Value::Number(an);
            }
            match (an.as_i64(), bn.as_i64()) {
                (Some(x), Some(y)) => Value::from(x.saturating_add(y)),
                _ => Value::Number(an),
            }
        }
        (Value::Null, other) => other.clone(),
        (Value::Bool(x), Value::Bool(y)) => Value::Bool(x || *y),
        (Value::Array(mut items), Value::Array(more)) => {
            for m in more {
                let k = row_key(m);
                let at = k
                    .as_ref()
                    .and_then(|k| items.iter().position(|i| row_key(i).as_ref() == Some(k)));
                match at {
                    Some(i) => {
                        let cur = std::mem::take(&mut items[i]);
                        items[i] = merge_value(cur, m, key);
                    }
                    None if m.is_string() && items.contains(m) => {}
                    None => items.push(m.clone()),
                }
            }
            Value::Array(items)
        }
        (a, _) => a,
    }
}

/// Доли и средние после сложения: маржа — от суммы валовой и выручки, средний чек — от суммы
/// выручки и числа чеков.
fn recompute_ratios(v: &mut serde_json::Value) {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let get =
                |m: &serde_json::Map<String, Value>, k: &str| m.get(k).and_then(Value::as_i64);
            if m.contains_key("margin_bp") {
                let revenue = get(m, "goods_tyiyn")
                    .zip(get(m, "services_tyiyn"))
                    .map(|(g, s)| g.saturating_add(s))
                    .or_else(|| get(m, "revenue_tyiyn"));
                let margin = revenue.zip(get(m, "gross_tyiyn")).and_then(|(r, g)| {
                    crate::domain::money::div_round(i128::from(g) * 10_000, i128::from(r))
                });
                m.insert("margin_bp".into(), margin.map_or(Value::Null, Value::from));
            }
            if m.contains_key("average_check_tyiyn") {
                let totals = m.get("totals").cloned().unwrap_or(Value::Null);
                let revenue = totals
                    .get("goods_tyiyn")
                    .and_then(Value::as_i64)
                    .zip(totals.get("services_tyiyn").and_then(Value::as_i64))
                    .map(|(g, s)| g.saturating_add(s));
                let count = totals.get("sales_count").and_then(Value::as_i64);
                if let (Some(r), Some(c)) = (revenue, count) {
                    let avg =
                        crate::domain::money::div_round(i128::from(r), i128::from(c)).unwrap_or(0);
                    m.insert("average_check_tyiyn".into(), Value::from(avg));
                }
            }
            for (_, x) in m.iter_mut() {
                recompute_ratios(x);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(recompute_ratios),
        _ => {}
    }
}

/// Будущее отчёта одного филиала: соединение и пользователь в этом филиале.
pub type ReportFuture<'c> =
    std::pin::Pin<Box<dyn std::future::Future<Output = AppResult<serde_json::Value>> + Send + 'c>>;

/// Отчёт по выбранному филиалу или «Все филиалы» (SPEC-33 §3): `f` строит отчёт одного
/// филиала; для «Все» — по каждому филиалу (и закрытым: история остаётся) и сводит
/// [`merge_reports`]. «Все» — только владельцу.
pub async fn report<F>(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: &BranchQuery,
    mut f: F,
) -> AppResult<serde_json::Value>
where
    F: for<'c> FnMut(&'c mut PgConnection, CurrentUser) -> ReportFuture<'c>,
{
    if !q.all()? {
        return f(conn, user.clone()).await;
    }
    let mut parts = Vec::new();
    for b in report_scope(conn, user, true).await? {
        let v = f(&mut *conn, user.in_branch(b.id)).await?;
        parts.push((b, v));
    }
    Ok(merge_reports(parts))
}

/// Ответ отчёта как JSON-значение.
pub fn to_json<T: Serialize>(v: &T) -> AppResult<serde_json::Value> {
    serde_json::to_value(v).map_err(|e| AppError::Internal(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str) -> BranchOut {
        BranchOut {
            id: Uuid::now_v7(),
            name: name.into(),
            address: String::new(),
            phone: String::new(),
            archived: false,
            is_main: false,
        }
    }

    #[test]
    fn all_branches_report_is_the_sum_of_branches() {
        let a = json!({
            "from": "2026-10-01",
            "totals": { "goods_tyiyn": 1000, "services_tyiyn": 0, "gross_tyiyn": 300, "margin_bp": 3000, "sales_count": 2 },
            "days": [{ "date": "2026-10-01", "revenue_tyiyn": 1000 }],
            "items": [{ "product_id": "p1", "qty": 2, "sale_price_tyiyn": 500 }],
            "warnings": ["одно"],
            "points": { "clients": 3, "points_tyiyn": 900 },
            "shift_open": false,
        });
        let c = json!({
            "from": "2026-10-01",
            "totals": { "goods_tyiyn": 3000, "services_tyiyn": 0, "gross_tyiyn": 1700, "margin_bp": 5667, "sales_count": 3 },
            "days": [{ "date": "2026-10-01", "revenue_tyiyn": 3000 }, { "date": "2026-10-02", "revenue_tyiyn": 5 }],
            "items": [{ "product_id": "p1", "qty": 1, "sale_price_tyiyn": 700 }, { "product_id": "p2", "qty": 4 }],
            "warnings": ["одно", "другое"],
            "points": { "clients": 3, "points_tyiyn": 900 },
            "shift_open": true,
        });
        let m = merge_reports(vec![(b("А"), a), (b("Б"), c)]);
        assert_eq!(m["totals"]["goods_tyiyn"], 4000);
        assert_eq!(m["totals"]["gross_tyiyn"], 2000);
        assert_eq!(m["totals"]["margin_bp"], 5000);
        assert_eq!(m["totals"]["sales_count"], 5);
        assert_eq!(m["days"][0]["revenue_tyiyn"], 4000);
        assert_eq!(m["days"].as_array().unwrap().len(), 2);
        assert_eq!(m["items"][0]["qty"], 3);
        assert_eq!(m["items"][0]["sale_price_tyiyn"], 500);
        assert_eq!(m["warnings"], json!(["одно", "другое"]));
        // Баллы общие — не удваиваются.
        assert_eq!(m["points"]["points_tyiyn"], 900);
        assert_eq!(m["shift_open"], true);
        assert_eq!(m["from"], "2026-10-01");
        assert_eq!(m["branches"].as_array().unwrap().len(), 2);
        assert_eq!(m["branches"][1]["report"]["totals"]["goods_tyiyn"], 3000);
        assert_eq!(m["all_branches"], true);
    }
}
