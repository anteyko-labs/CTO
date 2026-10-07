//! Кассы, движения наличных и смена (SPEC-05).

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cash/accounts", routing::get(list_accounts))
        .route("/cash/accounts/{id}", routing::get(account_movements))
        .route("/cash/movements", routing::post(post_movement))
        .route("/cash/transfers", routing::post(post_transfer))
        .route("/shifts", routing::get(list_shifts))
        .route("/shifts/open", routing::post(open_shift))
        .route("/shifts/current", routing::get(current_shift))
        .route("/shifts/{id}/close", routing::post(close_shift))
        .route("/shifts/{id}/reopen", routing::post(reopen_shift))
}

#[derive(Serialize)]
pub struct AccountOut {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub owner_only: bool,
    pub is_default: bool,
    pub balance_tyiyn: i64,
}

async fn list_accounts(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<AccountOut>>> {
    let mut conn = state.pool.acquire().await?;
    ensure_accounts(&mut conn, user.branch_id).await?;
    // Сейф владельца администратору не показываем, перевести в него он может (SPEC-05).
    let rows = sqlx::query_as!(
        AccountOut,
        r#"select id, name, kind, owner_only, is_default, balance_tyiyn
           from cash_accounts
           where branch_id = $1 and active and (not owner_only or $2)
           order by is_default desc, kind, name"#,
        user.branch_id,
        user.is_owner()
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Новый филиал получает свои кассы при первом обращении: касса, счёт и два сейфа (ADR-037).
pub async fn ensure_accounts(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    let have = sqlx::query_scalar!(
        r#"select count(*) as "n!" from cash_accounts where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if have > 0 {
        return Ok(());
    }
    for (name, kind, owner_only, is_default) in [
        ("Касса", "register", false, true),
        ("Счёт", "bank", false, false),
        ("Сейф магазина", "safe", false, false),
        ("Сейф владельца", "safe", true, false),
    ] {
        sqlx::query!(
            r#"insert into cash_accounts (id, branch_id, name, kind, owner_only, is_default)
               values ($1, $2, $3, $4, $5, $6)"#,
            new_id(),
            branch_id,
            name,
            kind,
            owner_only,
            is_default
        )
        .execute(&mut *conn)
        .await?;
    }
    for method in ["card", "transfer"] {
        sqlx::query!(
            r#"insert into payment_method_fees (branch_id, method, rate_bp) values ($1, $2, 50)
               on conflict do nothing"#,
            branch_id,
            method
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Касса по умолчанию: в неё идут наличные чека.
pub async fn default_account(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Uuid> {
    ensure_accounts(conn, branch_id).await?;
    sqlx::query_scalar!(
        "select id from cash_accounts where branch_id = $1 and is_default and active limit 1",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| AppError::Internal("нет кассы по умолчанию".into()))
}

/// Счёт: туда приходят карта и перевод (ADR-024).
pub async fn bank_account(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Option<Uuid>> {
    ensure_accounts(conn, branch_id).await?;
    Ok(sqlx::query_scalar!(
        "select id from cash_accounts where branch_id = $1 and kind = 'bank' and active limit 1",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

pub struct CashEntry<'a> {
    pub account_id: Uuid,
    pub kind: &'a str,
    pub amount: i64,
    pub doc_type: &'a str,
    pub doc_id: Option<Uuid>,
    pub comment: &'a str,
}

/// Записывает движение денег и пересчитывает остаток кассы (инвариант 6).
/// Смена подставляется сама: открытая на этой кассе в момент проведения.
pub async fn add_movement(conn: &mut PgConnection, ctx: &Ctx, e: CashEntry<'_>) -> AppResult<i64> {
    let balance = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1 and branch_id = $2 for update",
        e.account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    let next = balance
        .checked_add(e.amount)
        .ok_or_else(|| AppError::Validation("слишком большая сумма".into()))?;
    if next < 0 {
        return Err(invalid("в кассе столько нет"));
    }
    let shift_id = open_shift_id(conn, ctx.user.branch_id, e.account_id).await?;
    sqlx::query!(
        r#"insert into cash_movements (id, branch_id, account_id, shift_id, kind, amount_tyiyn,
                                       doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        new_id(),
        ctx.user.branch_id,
        e.account_id,
        shift_id,
        e.kind,
        e.amount,
        e.doc_type,
        e.doc_id,
        e.comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "update cash_accounts set balance_tyiyn = $2 where id = $1",
        e.account_id,
        next
    )
    .execute(&mut *conn)
    .await?;
    Ok(next)
}

/// Открытая смена кассы: последнее закрытие переоткрыто или закрытий нет.
pub async fn open_shift_id(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"select s.id from shifts s
           where s.branch_id = $1 and s.account_id = $2
             and not exists (
               select 1 from shift_closes c
               where c.shift_id = s.id
                 and not exists (select 1 from shift_reopens r where r.close_id = c.id))
           order by s.opened_at desc limit 1"#,
        branch_id,
        account_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

#[derive(Serialize)]
struct BreakdownRow {
    kind: String,
    sum_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ShiftOut {
    pub id: Uuid,
    pub number: i64,
    pub business_date: NaiveDate,
    pub account_id: Uuid,
    pub account_name: String,
    pub cashier_name: String,
    pub opened_by: String,
    pub opened_at: DateTime<Utc>,
    pub opening_expected_tyiyn: i64,
    pub expected_tyiyn: i64,
    pub closed_at: Option<DateTime<Utc>>,
    pub counted_tyiyn: Option<i64>,
    pub diff_tyiyn: Option<i64>,
    pub breakdown: Value,
    pub cash_sales_tyiyn: i64,
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    pub debt_tyiyn: i64,
}

async fn load_shift(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<ShiftOut> {
    let h = sqlx::query!(
        r#"select s.id, s.number, s.business_date, s.account_id, a.name as account_name,
                  e.full_name as cashier_name, u.full_name as opened_by, s.opened_at,
                  s.opening_expected_tyiyn, a.balance_tyiyn
           from shifts s
           join cash_accounts a on a.id = s.account_id
           join employees e on e.id = s.cashier_employee_id
           join users u on u.id = s.opened_by
           where s.id = $1 and s.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let close = sqlx::query!(
        r#"select c.closed_at, c.counted_tyiyn, c.diff_tyiyn
           from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let breakdown: Vec<BreakdownRow> = sqlx::query!(
        r#"select kind as "kind!", sum(amount_tyiyn)::bigint as "sum!"
           from cash_movements where shift_id = $1 group by kind order by kind"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| BreakdownRow {
        kind: r.kind,
        sum_tyiyn: r.sum,
    })
    .collect();
    // Сверка с терминалом и банком: что прошло за смену по способам оплаты.
    let pays = sqlx::query!(
        r#"select
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'cash'), 0)::bigint as "cash!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as "card!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as "transfer!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!"
           from sale_payments p
           where p.sale_id in (select distinct m.doc_id from cash_movements m
                               where m.shift_id = $1 and m.doc_type = 'sale' and m.doc_id is not null)"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(ShiftOut {
        id: h.id,
        number: h.number,
        business_date: h.business_date,
        account_id: h.account_id,
        account_name: h.account_name,
        cashier_name: h.cashier_name,
        opened_by: h.opened_by,
        opened_at: h.opened_at,
        opening_expected_tyiyn: h.opening_expected_tyiyn,
        expected_tyiyn: h.balance_tyiyn,
        closed_at: close.as_ref().map(|c| c.closed_at),
        counted_tyiyn: close.as_ref().map(|c| c.counted_tyiyn),
        diff_tyiyn: close.as_ref().map(|c| c.diff_tyiyn),
        breakdown: json!(breakdown),
        cash_sales_tyiyn: pays.cash,
        card_tyiyn: pays.card,
        transfer_tyiyn: pays.transfer,
        debt_tyiyn: pays.debt,
    })
}

async fn current_shift(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Option<ShiftOut>>> {
    let mut conn = state.pool.acquire().await?;
    let account = default_account(&mut conn, user.branch_id).await?;
    match open_shift_id(&mut conn, user.branch_id, account).await? {
        Some(id) => Ok(Json(Some(load_shift(&mut conn, user.branch_id, id).await?))),
        None => Ok(Json(None)),
    }
}

#[derive(Deserialize)]
struct PeriodQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn list_shifts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ShiftOut>>> {
    let mut conn = state.pool.acquire().await?;
    let ids = sqlx::query_scalar!(
        r#"select id from shifts
           where branch_id = $1
             and ($2::date is null or business_date >= $2)
             and ($3::date is null or business_date <= $3)
           order by business_date desc limit 60"#,
        user.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(load_shift(&mut conn, user.branch_id, id).await?);
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct OpenReq {
    op_id: Uuid,
    account_id: Option<Uuid>,
    cashier_employee_id: Uuid,
    counted_tyiyn: Option<i64>,
}

async fn open_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OpenReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.open";
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let account_id = match req.account_id {
        Some(id) => id,
        None => default_account(&mut tx, branch_id).await?,
    };
    let acc = sqlx::query!(
        "select kind, balance_tyiyn from cash_accounts where id = $1 and branch_id = $2 for update",
        account_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if acc.kind != "register" {
        return Err(invalid("смена открывается только на кассе с наличными"));
    }
    if open_shift_id(&mut tx, branch_id, account_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("смена уже открыта".into()));
    }
    let cashier = sqlx::query!(
        "select is_cashier, active from employees where id = $1 and branch_id = $2",
        req.cashier_employee_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("кассир не найден"))?;
    if !cashier.active || !cashier.is_cashier {
        return Err(invalid("сотрудник не кассир или отключён"));
    }
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *tx)
        .await?;
    let exists = sqlx::query_scalar!(
        r#"select count(*) as "n!" from shifts
           where branch_id = $1 and account_id = $2 and business_date = $3"#,
        branch_id,
        account_id,
        today
    )
    .fetch_one(&mut *tx)
    .await?;
    if exists > 0 {
        return Err(AppError::Conflict(
            "смена за сегодня уже была: переоткройте её".into(),
        ));
    }
    let number = ops::next_counter(&mut tx, branch_id, "shift").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id,
                               opened_by, opened_device, opening_expected_tyiyn, opening_counted_tyiyn)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        today,
        account_id,
        req.cashier_employee_id,
        ctx.user.id,
        ctx.device_id,
        acc.balance_tyiyn,
        req.counted_tyiyn
    )
    .execute(&mut *tx)
    .await?;
    if let Some(counted) = req.counted_tyiyn {
        let diff = counted - acc.balance_tyiyn;
        if diff != 0 {
            add_movement(
                &mut tx,
                &ctx,
                CashEntry {
                    account_id,
                    kind: "count_diff",
                    amount: diff,
                    doc_type: "shift",
                    doc_id: Some(id),
                    comment: "пересчёт при открытии",
                },
            )
            .await?;
        }
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({ "number": number, "expected": acc.balance_tyiyn }),
    )
    .await?;
    let out = load_shift(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct CloseReq {
    op_id: Uuid,
    counted_tyiyn: i64,
    #[serde(default)]
    comment: String,
}

async fn close_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CloseReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.close";
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let shift = sqlx::query!(
        "select account_id from shifts where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if open_shift_id(&mut tx, branch_id, shift.account_id).await? != Some(id) {
        return Err(AppError::Conflict("смена не открыта".into()));
    }
    let expected = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1 for update",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let diff = req.counted_tyiyn - expected;
    if diff != 0 && req.comment.trim().is_empty() {
        return Err(invalid("при расхождении нужен комментарий"));
    }
    if diff != 0 {
        add_movement(
            &mut tx,
            &ctx,
            CashEntry {
                account_id: shift.account_id,
                kind: "count_diff",
                amount: diff,
                doc_type: "shift",
                doc_id: Some(id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    let seq = sqlx::query_scalar!(
        r#"select coalesce(max(seq), 0) + 1 as "n!" from shift_closes where shift_id = $1"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into shift_closes (id, shift_id, seq, expected_tyiyn, counted_tyiyn, diff_tyiyn,
                                     comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        new_id(),
        id,
        seq,
        expected,
        req.counted_tyiyn,
        diff,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({ "expected": expected, "counted": req.counted_tyiyn, "diff": diff }),
    )
    .await?;
    let out = load_shift(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ReopenReq {
    op_id: Uuid,
    reason: String,
}

async fn reopen_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReopenReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.reopen";
    // Переоткрывает только владелец (ADR-021).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.reason.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let close = sqlx::query!(
        r#"select c.id from shift_closes c
           join shifts s on s.id = c.shift_id
           where c.shift_id = $1 and s.branch_id = $2
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::Conflict("смена не закрыта".into()))?;
    sqlx::query!(
        r#"insert into shift_reopens (id, shift_id, close_id, reason, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        new_id(),
        id,
        close.id,
        req.reason.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({ "reason": req.reason.trim() }),
    )
    .await?;
    let out = load_shift(&mut tx, ctx.user.branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct MovementReq {
    op_id: Uuid,
    account_id: Option<Uuid>,
    kind: String,
    amount_tyiyn: i64,
    comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BalanceOut {
    pub account_id: Uuid,
    pub balance_tyiyn: i64,
}

async fn post_movement(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<MovementReq>,
) -> AppResult<Json<BalanceOut>> {
    const KIND: &str = "cash.movement";
    if !matches!(req.kind.as_str(), "cash_in" | "cash_out") {
        return Err(invalid("вид движения: внесение или изъятие"));
    }
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите, за что"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let account_id = match req.account_id {
        Some(id) => id,
        None => default_account(&mut tx, ctx.user.branch_id).await?,
    };
    let amount = if req.kind == "cash_in" {
        req.amount_tyiyn
    } else {
        -req.amount_tyiyn
    };
    let balance = add_movement(
        &mut tx,
        &ctx,
        CashEntry {
            account_id,
            kind: &req.kind,
            amount,
            doc_type: "manual",
            doc_id: None,
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "cash",
        Some(account_id),
        json!({ "kind": req.kind, "amount_tyiyn": amount, "comment": req.comment.trim() }),
    )
    .await?;
    let out = BalanceOut {
        account_id,
        balance_tyiyn: balance,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct TransferReq {
    op_id: Uuid,
    from_account_id: Uuid,
    to_account_id: Uuid,
    amount_tyiyn: i64,
    #[serde(default)]
    comment: String,
}

async fn post_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<TransferReq>,
) -> AppResult<Json<Value>> {
    const KIND: &str = "cash.transfer";
    if req.from_account_id == req.to_account_id {
        return Err(invalid("кассы должны быть разные"));
    }
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    // Из сейфа владельца деньги двигает только он (ADR-037).
    let from_owner_only = sqlx::query_scalar!(
        "select owner_only from cash_accounts where id = $1 and branch_id = $2",
        req.from_account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if from_owner_only && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let number = ops::next_counter(&mut tx, ctx.user.branch_id, "cash_transfer").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into cash_transfers (id, branch_id, number, from_account_id, to_account_id,
                                       amount_tyiyn, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        ctx.user.branch_id,
        number,
        req.from_account_id,
        req.to_account_id,
        req.amount_tyiyn,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    add_movement(
        &mut tx,
        &ctx,
        CashEntry {
            account_id: req.from_account_id,
            kind: "transfer_out",
            amount: -req.amount_tyiyn,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: req.comment.trim(),
        },
    )
    .await?;
    add_movement(
        &mut tx,
        &ctx,
        CashEntry {
            account_id: req.to_account_id,
            kind: "transfer_in",
            amount: req.amount_tyiyn,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "cash",
        Some(id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "number": number }),
    )
    .await?;
    let out = json!({ "id": id, "number": number });
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct MovementOut {
    id: Uuid,
    kind: String,
    amount_tyiyn: i64,
    comment: String,
    user_name: String,
    created_at: DateTime<Utc>,
}

async fn account_movements(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<MovementOut>>> {
    let rows = sqlx::query_as!(
        MovementOut,
        r#"select m.id, m.kind, m.amount_tyiyn, m.comment, u.full_name as user_name, m.created_at
           from cash_movements m join users u on u.id = m.user_id
           where m.account_id = $1 and m.branch_id = $2
           order by m.created_at desc limit 200"#,
        id,
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}
