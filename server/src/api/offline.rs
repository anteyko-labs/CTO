//! Снимок каталога для работы кассы без сети (SPEC-09).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/offline/snapshot", routing::get(snapshot))
        .route("/sales/offline-rejected", routing::post(rejected))
        .route("/sales/by-op/{op_id}", routing::get(sale_by_op))
}

#[derive(Serialize)]
struct PostedSale {
    id: Uuid,
    number: i64,
}

/// Проведён ли уже чек с этим `op_id` (ADR-062). Касса спрашивает, когда сервер ответил
/// «op_id уже использован»: чек мог уйти от имени другого пользователя той же кассы (вход
/// сменился, пока чек лежал в очереди). Чек этого филиала видит любой его пользователь.
async fn sale_by_op(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(op_id): Path<Uuid>,
) -> AppResult<Json<PostedSale>> {
    let row = sqlx::query!(
        r#"select s.id, s.number from operations o
           join sales s on s.id = (o.response ->> 'id')::uuid
           where o.op_id = $1 and o.kind = 'sale.post' and s.branch_id = $2"#,
        op_id,
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(PostedSale {
        id: row.id,
        number: row.number,
    }))
}

#[derive(Serialize)]
struct SnapProduct {
    id: Uuid,
    name: String,
    brand: String,
    article: String,
    category_id: Uuid,
    /// Вид категории: касса без сети спрашивает «Это замена масла?» по маслу (SPEC-29 §18).
    category_kind: String,
    unit: String,
    container_ml: Option<i64>,
    barcodes: Vec<String>,
    sale_price_tyiyn: i64,
    pour_price_per_l_tyiyn: Option<i64>,
    stock_qty: i64,
}

/// Услуга для кассы без сети (SPEC-09): работа уже сделана, чек пробивается и без связи.
#[derive(Serialize)]
struct SnapService {
    id: Uuid,
    name: String,
    price_tyiyn: i64,
    master_fee_tyiyn: i64,
    active: bool,
}

#[derive(Serialize)]
struct SnapCategory {
    id: Uuid,
    name: String,
    kind: String,
}

#[derive(Serialize)]
struct SnapContact {
    id: Uuid,
    full_name: String,
    position: String,
    active: bool,
}

#[derive(Serialize)]
struct SnapVehicle {
    id: Uuid,
    plate: String,
    brand: String,
    model: String,
    comment: String,
    active: bool,
    mileage_unit: String,
}

/// Клиент для кассы без сети (SPEC-09, ADR-093): чтобы без связи выбрать клиента для долга и
/// машину для замены. Только действующие покупатели с долгом, авансом или недавними покупками.
#[derive(Serialize)]
struct SnapClient {
    id: Uuid,
    role: String,
    kind: String,
    name: String,
    phone: String,
    inn: String,
    comment: String,
    credit_limit_tyiyn: Option<i64>,
    due_days: Option<i32>,
    active: bool,
    balance_tyiyn: i64,
    overdue_tyiyn: i64,
    contacts: Vec<SnapContact>,
    vehicles: Vec<SnapVehicle>,
}

/// Сколько клиентов кладём в снимок: самые недавние покупатели и все должники.
const SNAP_CLIENTS: i64 = 1000;

#[derive(Serialize)]
struct SnapEmployee {
    id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
}

/// Безналичный счёт для выбора на кассе без сети (ADR-076) с отметками «по умолчанию для
/// карты» и «по умолчанию для перевода» (ADR-077).
#[derive(Serialize)]
struct SnapBank {
    id: Uuid,
    name: String,
    default_card: bool,
    default_transfer: bool,
    /// Ставка комиссии банка на сегодня, сотые доли процента: подпись у кнопок оплаты без сети.
    fee_rate_bp: i32,
}

#[derive(Serialize)]
struct Snapshot {
    version: String,
    server_time: DateTime<Utc>,
    products: Vec<SnapProduct>,
    employees: Vec<SnapEmployee>,
    services: Vec<SnapService>,
    categories: Vec<SnapCategory>,
    clients: Vec<SnapClient>,
    bank_accounts: Vec<SnapBank>,
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
        r#"select p.id, p.name, p.brand, p.article, p.category_id, c.kind as category_kind, p.unit,
                  p.container_ml as "container_ml?",
                  coalesce(bp.sale_price_tyiyn, 0) as "sale_price_tyiyn!",
                  coalesce(bp.stock_qty, 0) as "stock_qty!",
                  coalesce((select array_agg(b.code order by b.created_at)
                            from product_barcodes b where b.product_id = p.id), '{}') as "barcodes!: Vec<String>"
           from products p
           join categories c on c.id = p.category_id
           left join branch_products bp on bp.product_id = p.id and bp.branch_id = $1
           where not p.archived
           order by p.name"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| SnapProduct {
        // Касса без сети считает розлив по той же цене, что сервер (ADR-074).
        pour_price_per_l_tyiyn: crate::domain::money::pour_price_per_l(
            &r.unit,
            r.container_ml,
            r.sale_price_tyiyn,
        ),
        id: r.id,
        name: r.name,
        brand: r.brand,
        article: r.article,
        category_id: r.category_id,
        category_kind: r.category_kind,
        unit: r.unit,
        container_ml: r.container_ml,
        barcodes: r.barcodes,
        sale_price_tyiyn: r.sale_price_tyiyn,
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
    let services = sqlx::query_as!(
        SnapService,
        r#"select id, name, price_tyiyn, master_fee_tyiyn, active from services
           where branch_id = $1 and active order by name"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let categories = sqlx::query_as!(
        SnapCategory,
        "select id, name, kind from categories order by name"
    )
    .fetch_all(&mut *conn)
    .await?;
    let clients = snapshot_clients(&mut conn, user.branch_id).await?;
    // Счета карты и перевода: действующие; «только владелец» — только владельцу (ADR-076).
    crate::api::cash::ensure_accounts(&mut conn, user.branch_id).await?;
    let bank_accounts = sqlx::query_as!(
        SnapBank,
        r#"select id, name, default_card, default_transfer,
                  account_fee_on(id, (now() at time zone 'Asia/Bishkek')::date) as "fee_rate_bp!"
           from cash_accounts
           where branch_id = $1 and kind = 'bank' and active and (not owner_only or $2)
           order by default_card desc, default_transfer desc, name"#,
        user.branch_id,
        user.is_owner()
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
        services,
        categories,
        clients,
        bank_accounts,
        oil_change_master_fee_tyiyn: fee,
    };
    let payload = serde_json::to_string(&json!({
        "products": &body.products,
        "employees": &body.employees,
        "services": &body.services,
        "categories": &body.categories,
        "clients": &body.clients,
        "bank_accounts": &body.bank_accounts,
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

/// Клиенты для кассы без сети: должники и авансы, затем недавние покупатели, с машинами и людьми.
async fn snapshot_clients(
    conn: &mut sqlx::PgConnection,
    branch_id: Uuid,
) -> AppResult<Vec<SnapClient>> {
    let rows = sqlx::query!(
        r#"select p.id, p.role, p.kind, p.name, p.phone, p.inn, p.comment,
                  p.credit_limit_tyiyn as "credit_limit_tyiyn?", p.due_days as "due_days?",
                  p.active, p.balance_tyiyn
           from parties p
           left join lateral (select max(s.created_at) as last_at from sales s
                              where s.party_id = p.id and s.branch_id = $1) s on true
           where p.branch_id = $1 and p.role = 'customer' and p.active
             and (p.balance_tyiyn <> 0 or s.last_at > now() - interval '180 days'
                  or exists (select 1 from party_vehicles v where v.party_id = p.id and v.active))
           order by (p.balance_tyiyn <> 0) desc, s.last_at desc nulls last, p.name, p.id
           limit $2"#,
        branch_id,
        SNAP_CLIENTS
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut vehicles: std::collections::HashMap<Uuid, Vec<SnapVehicle>> =
        std::collections::HashMap::new();
    for v in sqlx::query!(
        r#"select id, party_id, plate, brand, model, comment, active, mileage_unit
           from party_vehicles where party_id = any($1) and active order by created_at"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        vehicles.entry(v.party_id).or_default().push(SnapVehicle {
            id: v.id,
            plate: v.plate,
            brand: v.brand,
            model: v.model,
            comment: v.comment,
            active: v.active,
            mileage_unit: v.mileage_unit,
        });
    }
    let mut contacts: std::collections::HashMap<Uuid, Vec<SnapContact>> =
        std::collections::HashMap::new();
    for c in sqlx::query!(
        r#"select id, party_id, full_name, position, active
           from party_contacts where party_id = any($1) and active order by full_name"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    {
        contacts.entry(c.party_id).or_default().push(SnapContact {
            id: c.id,
            full_name: c.full_name,
            position: c.position,
            active: c.active,
        });
    }
    Ok(rows
        .into_iter()
        .map(|r| SnapClient {
            vehicles: vehicles.remove(&r.id).unwrap_or_default(),
            contacts: contacts.remove(&r.id).unwrap_or_default(),
            id: r.id,
            role: r.role,
            kind: r.kind,
            name: r.name,
            phone: r.phone,
            inn: r.inn,
            comment: r.comment,
            credit_limit_tyiyn: r.credit_limit_tyiyn,
            due_days: r.due_days,
            active: r.active,
            balance_tyiyn: r.balance_tyiyn,
            // Просрочку касса без сети не показывает.
            overdue_tyiyn: 0,
        })
        .collect())
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
    const KIND: &str = "sale.offline_rejected";
    let mut tx = state.pool.begin().await?;
    // Об отказе уже сообщили — например, этот чек отправлял другой пользователь кассы: отчёт
    // принят, очередь устройства не встаёт (ADR-093).
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        req.op_id.to_string()
    )
    .execute(&mut *tx)
    .await?;
    let reported = sqlx::query_scalar!(
        "select response from operations where op_id = $1 and kind = $2",
        req.op_id,
        KIND
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(done) = reported {
        return Ok(Json(done));
    }
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    // Чек целиком — если он разумного размера; иначе хватит итога и числа строк.
    let body = if req.body.to_string().len() <= 64 * 1024 {
        req.body
    } else {
        json!({ "truncated": true, "lines": req.body.get("lines").and_then(|l| l.as_array()).map_or(0, Vec::len) })
    };
    let error: String = req.error.chars().take(500).collect();
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "sale",
        None,
        json!({ "op_id": req.op_id, "error": error, "body": body }),
    )
    .await?;
    let out = json!({ "ok": true });
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
