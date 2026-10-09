//! Бонусные баллы клиентов, подключённых к Телеграм-боту (SPEC-19, ADR-053).
//! 1 балл = 1 сом; начисление — процент от оплаченного деньгами, списание — оплата чека баллами.

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::sales::PaymentReq;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::{div_round, format_som};
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/loyalty/lookup", routing::get(lookup))
        .route("/parties/{id}/loyalty", routing::get(party_history))
        .route(
            "/settings/loyalty",
            routing::get(get_settings).put(put_settings),
        )
}

/// Процент начисления по умолчанию — 0,5 % (решение заказчика).
const DEFAULT_RATE_BP: i32 = 50;

pub async fn rate_bp(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i32> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'loyalty'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| v.get("rate_bp").and_then(serde_json::Value::as_i64))
            .and_then(|d| i32::try_from(d).ok())
            .filter(|d| (0..=1000).contains(d))
            .unwrap_or(DEFAULT_RATE_BP),
    )
}

pub async fn balance(conn: &mut PgConnection, party_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from loyalty_ledger where party_id = $1"#,
        party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Участник программы — клиент, подключившийся к боту своим номером.
pub async fn is_member(conn: &mut PgConnection, party_id: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from telegram_customers where party_id = $1) as "e!""#,
        party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

struct Entry<'a> {
    branch_id: Uuid,
    party_id: Uuid,
    kind: &'a str,
    amount: i64,
    rate_bp: Option<i32>,
    doc_type: &'a str,
    doc_id: Uuid,
}

async fn add(conn: &mut PgConnection, e: Entry<'_>) -> AppResult<()> {
    if e.amount == 0 {
        return Ok(());
    }
    sqlx::query!(
        r#"insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, rate_bp, doc_type, doc_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        new_id(),
        e.branch_id,
        e.party_id,
        e.kind,
        e.amount,
        e.rate_bp,
        e.doc_type,
        e.doc_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn lock_party(conn: &mut PgConnection, party_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 7))",
        party_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn sum_of(payments: &[PaymentReq], pred: impl Fn(&str) -> bool) -> AppResult<i64> {
    payments
        .iter()
        .filter(|p| pred(&p.method))
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)
}

fn is_money(method: &str) -> bool {
    matches!(method, "cash" | "card" | "transfer")
}

/// Чек: списание баллов (оплата «bonus») и начисление с оплаченного деньгами.
/// Долг и баллы не начисляют: начисление — только с реально полученных денег.
pub async fn on_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    party_id: Option<Uuid>,
    payments: &[PaymentReq],
    offline: bool,
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let redeem = sum_of(payments, |m| m == "bonus")?;
    let Some(party_id) = party_id else {
        if redeem > 0 {
            return Err(invalid("для оплаты баллами укажите клиента"));
        }
        return Ok(());
    };
    let member = is_member(conn, party_id).await?;
    if redeem > 0 {
        if !member {
            return Err(invalid("клиент не подключён к боту — баллов у него нет"));
        }
        lock_party(conn, party_id).await?;
        let have = balance(conn, party_id).await?;
        // Без сети чек уже состоялся: принимаем, владелец увидит минус.
        if redeem > have && !offline {
            return Err(invalid(format!(
                "у клиента {} баллов, списать {} нельзя",
                format_som(have.max(0)),
                format_som(redeem)
            )));
        }
        add(
            conn,
            Entry {
                branch_id,
                party_id,
                kind: "redeem",
                amount: -redeem,
                rate_bp: None,
                doc_type: "sale",
                doc_id: sale_id,
            },
        )
        .await?;
    }
    if member {
        let rate = rate_bp(conn, branch_id).await?;
        let money = sum_of(payments, is_money)?;
        let accrual =
            div_round(i128::from(money) * i128::from(rate), 10_000).ok_or_else(overflow)?;
        add(
            conn,
            Entry {
                branch_id,
                party_id,
                kind: "accrual",
                amount: accrual,
                rate_bp: Some(rate),
                doc_type: "sale",
                doc_id: sale_id,
            },
        )
        .await?;
    }
    Ok(())
}

/// Возврат: баллы, которыми платили, возвращаются на счёт; начисленное снимается с
/// возвращённых денег по той же ставке, но не больше начисленного по чеку.
pub async fn on_return(
    conn: &mut PgConnection,
    ctx: &Ctx,
    orig_id: Uuid,
    return_id: Uuid,
    party_id: Option<Uuid>,
    payments: &[PaymentReq],
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let back = sum_of(payments, |m| m == "bonus")?;
    let money = sum_of(payments, is_money)?;
    let Some(party_id) = party_id else {
        if back > 0 {
            return Err(invalid("по чеку не платили баллами"));
        }
        return Ok(());
    };
    lock_party(conn, party_id).await?;
    let done = sqlx::query!(
        r#"select coalesce(-sum(l.amount_tyiyn) filter (where l.kind = 'redeem'), 0)::bigint as "redeemed!",
                  coalesce(-sum(l.amount_tyiyn) filter (where l.kind = 'refund'), 0)::bigint as "refunded!",
                  coalesce(sum(l.amount_tyiyn) filter (where l.kind in ('accrual', 'accrual_back')), 0)::bigint as "accrued!",
                  max(l.rate_bp) filter (where l.kind = 'accrual') as rate
           from loyalty_ledger l
           where l.doc_id = $1 or l.doc_id in (select id from sales where reversal_of = $1)"#,
        orig_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let left = done.redeemed + done.refunded;
    if back > left.max(0) {
        return Err(invalid(format!(
            "баллами по этому чеку можно вернуть не больше {}",
            format_som(left.max(0))
        )));
    }
    add(
        conn,
        Entry {
            branch_id,
            party_id,
            kind: "refund",
            amount: back,
            rate_bp: None,
            doc_type: "sale_return",
            doc_id: return_id,
        },
    )
    .await?;
    if let Some(rate) = done.rate {
        let take = div_round(i128::from(money) * i128::from(rate), 10_000)
            .ok_or_else(overflow)?
            .min(done.accrued.max(0));
        add(
            conn,
            Entry {
                branch_id,
                party_id,
                kind: "accrual_back",
                amount: -take,
                rate_bp: Some(rate),
                doc_type: "sale_return",
                doc_id: return_id,
            },
        )
        .await?;
    }
    Ok(())
}

// ---------- Касса: найти клиента по номеру ----------

#[derive(Deserialize)]
struct LookupQuery {
    phone: String,
}

#[derive(Serialize)]
struct LookupOut {
    party_id: Uuid,
    name: String,
    phone: String,
    balance_tyiyn: i64,
}

async fn lookup(
    State(state): State<AppState>,
    _user: CurrentUser,
    Query(q): Query<LookupQuery>,
) -> AppResult<Json<LookupOut>> {
    let key = crate::api::telegram::phone_key(&q.phone)
        .ok_or_else(|| invalid("номер — не меньше 9 цифр"))?;
    let mut conn = state.pool.acquire().await?;
    let r = sqlx::query!(
        r#"select p.id, p.name, p.phone from parties p
           where p.role = 'customer' and p.active
             and length(regexp_replace(p.phone, '[^0-9]', '', 'g')) >= 9
             and right(regexp_replace(p.phone, '[^0-9]', '', 'g'), 9) = $1
             and exists (select 1 from telegram_customers t where t.party_id = p.id)
           order by p.created_at desc limit 1"#,
        key
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| {
        invalid("клиент с этим номером не подключён к боту: пусть напишет боту и поделится номером")
    })?;
    let balance_tyiyn = balance(&mut conn, r.id).await?;
    Ok(Json(LookupOut {
        party_id: r.id,
        name: r.name,
        phone: r.phone,
        balance_tyiyn,
    }))
}

// ---------- История баллов клиента ----------

#[derive(Serialize)]
pub struct HistoryRow {
    pub kind: String,
    pub amount_tyiyn: i64,
    pub sale_number: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct HistoryOut {
    pub member: bool,
    pub balance_tyiyn: i64,
    pub rows: Vec<HistoryRow>,
}

pub async fn history(conn: &mut PgConnection, party_id: Uuid, limit: i64) -> AppResult<HistoryOut> {
    let rows = sqlx::query_as!(
        HistoryRow,
        r#"select l.kind, l.amount_tyiyn, s.number as "sale_number?", l.created_at
           from loyalty_ledger l left join sales s on s.id = l.doc_id
           where l.party_id = $1 order by l.created_at desc limit $2"#,
        party_id,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(HistoryOut {
        member: is_member(conn, party_id).await?,
        balance_tyiyn: balance(conn, party_id).await?,
        rows,
    })
}

async fn party_history(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<HistoryOut>> {
    let mut conn = state.pool.acquire().await?;
    let found = sqlx::query_scalar!(
        r#"select exists (select 1 from parties where id = $1 and branch_id = $2) as "e!""#,
        id,
        user.branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !found {
        return Err(AppError::NotFound);
    }
    Ok(Json(history(&mut conn, id, 50).await?))
}

// ---------- Настройка ----------

#[derive(Serialize, Deserialize)]
struct Settings {
    rate_bp: i32,
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Settings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(Settings {
        rate_bp: rate_bp(&mut conn, user.branch_id).await?,
    }))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<Settings>,
) -> AppResult<Json<Settings>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !(0..=1000).contains(&req.rate_bp) {
        return Err(invalid("процент начисления от 0 до 10"));
    }
    let mut tx = state.pool.begin().await?;
    let value = json!({ "rate_bp": req.rate_bp });
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'loyalty', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(&mut tx, &ctx, "settings.loyalty", "settings", None, value).await?;
    tx.commit().await?;
    Ok(Json(req))
}
