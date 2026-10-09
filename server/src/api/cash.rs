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
        .route(
            "/cash/movements/{id}/reverse",
            routing::post(reverse_movement),
        )
        .route("/cash/transfers", routing::post(post_transfer))
        .route(
            "/cash/transfers/{id}/reverse",
            routing::post(reverse_transfer),
        )
        .route("/shifts", routing::get(list_shifts))
        .route("/shifts/open", routing::post(open_shift))
        .route("/shifts/current", routing::get(current_shift))
        .route("/shifts/{id}", routing::get(get_shift))
        .route("/shifts/{id}/close", routing::post(close_shift))
        .route("/shifts/{id}/reopen", routing::post(reopen_shift))
        .route("/shifts/{id}/handover", routing::post(handover_shift))
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
    .fetch_all(&mut *conn)
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
               values ($1, $2, $3, $4, $5, $6)
               on conflict (branch_id, name) do nothing"#,
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
        "select id from cash_accounts where branch_id = $1 and is_default and active order by created_at limit 1",
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
        "select id from cash_accounts where branch_id = $1 and kind = 'bank' and active order by created_at limit 1",
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
    // Сумма каждой операции проверена на её входе; здесь — предел остатка кассы.
    ops::check_balance(e.amount)?;
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
    ops::check_balance(next)?;
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

/// Деньги через кассу с наличными — только в открытой смене: иначе их не с чем сверить
/// при закрытии (SPEC-05, SPEC-06, SPEC-07). Продажи и сдача кассы этим не ограничены (ADR-021).
pub async fn require_open_shift(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
) -> AppResult<()> {
    let kind = sqlx::query_scalar!(
        "select kind from cash_accounts where id = $1 and branch_id = $2",
        account_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if kind == "register" && open_shift_id(conn, branch_id, account_id).await?.is_none() {
        return Err(AppError::Conflict(
            "смена не открыта: откройте её на экране «Смена»".into(),
        ));
    }
    Ok(())
}

/// Блокирует строки касс в порядке id: два встречных перемещения не ждут друг друга вечно.
async fn lock_accounts(conn: &mut PgConnection, branch_id: Uuid, ids: &[Uuid]) -> AppResult<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    let locked = sqlx::query_scalar!(
        "select id from cash_accounts where branch_id = $1 and id = any($2) order by id for update",
        branch_id,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    if locked.len() != ids.len() {
        return Err(invalid("касса не найдена"));
    }
    Ok(())
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
    /// Открытая смена — сколько должно быть в кассе сейчас; закрытая — сколько ждали при закрытии.
    pub expected_tyiyn: i64,
    pub closed_at: Option<DateTime<Utc>>,
    pub counted_tyiyn: Option<i64>,
    pub diff_tyiyn: Option<i64>,
    pub breakdown: Value,
    pub cash_sales_tyiyn: i64,
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    pub debt_tyiyn: i64,
    #[serde(default)]
    pub bank_fee_tyiyn: i64,
    /// Смена закрыта, а что сделали с деньгами (сейф или размен) ещё не отмечено.
    #[serde(default)]
    pub handover_pending: bool,
    #[serde(default)]
    pub to_safe_tyiyn: Option<i64>,
    #[serde(default)]
    pub left_tyiyn: Option<i64>,
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
        r#"select c.closed_at, c.expected_tyiyn, c.counted_tyiyn, c.diff_tyiyn,
                  h.to_safe_tyiyn as "to_safe?", h.left_tyiyn as "left?"
           from shift_closes c
           left join shift_handovers h on h.close_id = c.id
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    // Разбивка наличных: только касса смены, без счёта и сейфов.
    let breakdown: Vec<BreakdownRow> = sqlx::query!(
        r#"select kind as "kind!", sum(amount_tyiyn)::bigint as "sum!"
           from cash_movements where shift_id = $1 and account_id = $2
           group by kind order by kind"#,
        id,
        h.account_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| BreakdownRow {
        kind: r.kind,
        sum_tyiyn: r.sum,
    })
    .collect();
    // Сверка с терминалом и банком: чеки, проведённые, пока смена была открыта.
    // Берём по времени, а не по движениям кассы: чек картой или в долг наличных не трогает.
    let pays = sqlx::query!(
        r#"select
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'cash'), 0)::bigint as "cash!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as "card!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as "transfer!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
             coalesce(sum(p.fee_tyiyn), 0)::bigint as "fee!"
           from sale_payments p
           join sales s on s.id = p.sale_id
           where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())"#,
        branch_id,
        h.opened_at,
        close.as_ref().map(|c| c.closed_at)
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
        expected_tyiyn: close.as_ref().map_or(h.balance_tyiyn, |c| c.expected_tyiyn),
        closed_at: close.as_ref().map(|c| c.closed_at),
        counted_tyiyn: close.as_ref().map(|c| c.counted_tyiyn),
        diff_tyiyn: close.as_ref().map(|c| c.diff_tyiyn),
        breakdown: json!(breakdown),
        cash_sales_tyiyn: pays.cash,
        card_tyiyn: pays.card,
        transfer_tyiyn: pays.transfer,
        debt_tyiyn: pays.debt,
        bank_fee_tyiyn: pays.fee,
        handover_pending: close.as_ref().is_some_and(|c| c.to_safe.is_none()),
        to_safe_tyiyn: close.as_ref().and_then(|c| c.to_safe),
        left_tyiyn: close.as_ref().and_then(|c| c.left),
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

async fn get_shift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ShiftOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_shift(&mut conn, user.branch_id, id).await?))
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
    // Администратору — только последние 7 дней (SPEC-05, права).
    let earliest = if user.is_owner() {
        None
    } else {
        Some(
            sqlx::query_scalar!(
                r#"select ((now() at time zone 'Asia/Bishkek')::date - 6) as "d!""#
            )
            .fetch_one(&mut *conn)
            .await?,
        )
    };
    let from = match (q.from, earliest) {
        (Some(f), Some(e)) => Some(f.max(e)),
        (f, e) => f.or(e),
    };
    let ids = sqlx::query_scalar!(
        r#"select id from shifts
           where branch_id = $1
             and ($2::date is null or business_date >= $2)
             and ($3::date is null or business_date <= $3)
           order by business_date desc limit 60"#,
        user.branch_id,
        from,
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
    if req.counted_tyiyn.is_some_and(|c| c < 0) {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    if let Some(c) = req.counted_tyiyn {
        ops::check_balance(c)?;
    }
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
            "смена за сегодня уже была: переоткрыть её может владелец".into(),
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
        let diff = counted
            .checked_sub(acc.balance_tyiyn)
            .ok_or_else(crate::error::overflow)?;
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
        json!({ "number": number, "expected": acc.balance_tyiyn, "counted": req.counted_tyiyn }),
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
    if req.counted_tyiyn < 0 {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    ops::check_balance(req.counted_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let shift = sqlx::query!(
        "select account_id, cashier_employee_id, business_date from shifts where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Сначала блокировка кассы, потом проверка: два устройства не закроют смену дважды.
    let expected = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1 for update",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if open_shift_id(&mut tx, branch_id, shift.account_id).await? != Some(id) {
        return Err(AppError::Conflict("смена уже закрыта".into()));
    }
    let diff = req
        .counted_tyiyn
        .checked_sub(expected)
        .ok_or_else(crate::error::overflow)?;
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
    crate::api::payroll::accrue_for_shift_close(
        &mut tx,
        &ctx,
        id,
        shift.business_date,
        shift.cashier_employee_id,
        diff,
    )
    .await?;
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
    let account_id = sqlx::query_scalar!(
        "select account_id from shifts where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    lock_accounts(&mut tx, ctx.user.branch_id, &[account_id]).await?;
    // Пока открыта другая смена этой кассы, старую не переоткрыть: открытой бывает одна.
    if open_shift_id(&mut tx, ctx.user.branch_id, account_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("сначала закройте открытую смену".into()));
    }
    let close = sqlx::query!(
        r#"select c.id from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
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
struct HandoverReq {
    op_id: Uuid,
    /// Сколько переложили в сейф; 0 — всё осталось в кассе.
    amount_tyiyn: i64,
    to_account_id: Option<Uuid>,
}

/// После закрытия: перевод выручки в сейф или отметка, что деньги остались в кассе (SPEC-05).
async fn handover_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<HandoverReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.handover";
    let branch_id = ctx.user.branch_id;
    if req.amount_tyiyn < 0 {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    ops::check_amount(req.amount_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let shift = sqlx::query!(
        "select number, account_id from shifts where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Сейф магазина по умолчанию: в сейф владельца деньги перекладывает он сам (ADR-037).
    let safe = match req.to_account_id {
        Some(a) => a,
        None => sqlx::query_scalar!(
            r#"select id from cash_accounts
               where branch_id = $1 and kind = 'safe' and not owner_only and active
               order by created_at limit 1"#,
            branch_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("нет сейфа магазина"))?,
    };
    let safe_kind = sqlx::query_scalar!(
        "select kind from cash_accounts where id = $1 and branch_id = $2",
        safe,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if safe_kind != "safe" {
        return Err(invalid("кассу сдают в сейф"));
    }
    lock_accounts(&mut tx, branch_id, &[shift.account_id, safe]).await?;
    let close = sqlx::query!(
        r#"select c.id, c.expected_tyiyn, c.counted_tyiyn, c.diff_tyiyn,
                  exists (select 1 from shift_handovers h where h.close_id = c.id) as "done!"
           from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::Conflict("смена не закрыта".into()))?;
    if close.done {
        return Err(AppError::Conflict("касса по этой смене уже сдана".into()));
    }
    let transfer_id = if req.amount_tyiyn > 0 {
        let comment = format!("сдача кассы, смена № {}", shift.number);
        let (tid, _) = transfer_tx(
            &mut tx,
            &ctx,
            TransferArgs {
                from: shift.account_id,
                to: safe,
                amount: req.amount_tyiyn,
                comment: &comment,
                reversal_of: None,
            },
        )
        .await?;
        Some(tid)
    } else {
        None
    };
    let left = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into shift_handovers (id, shift_id, close_id, transfer_id, to_safe_tyiyn,
                                        left_tyiyn, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        new_id(),
        id,
        close.id,
        transfer_id,
        req.amount_tyiyn,
        left,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    // Владелец узнаёт итог смены из уведомления, не открывая её (SPEC-13).
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({
            "number": shift.number,
            "expected": close.expected_tyiyn,
            "counted": close.counted_tyiyn,
            "diff": close.diff_tyiyn,
            "to_safe": req.amount_tyiyn,
            "left": left,
        }),
    )
    .await?;
    let out = load_shift(&mut tx, branch_id, id).await?;
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

/// Кассы владельца трогает только он (ADR-037).
async fn check_account_access(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account_id: Uuid,
) -> AppResult<()> {
    let owner_only = sqlx::query_scalar!(
        "select owner_only from cash_accounts where id = $1 and branch_id = $2",
        account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if owner_only && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    Ok(())
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
    crate::ops::check_amount(req.amount_tyiyn)?;
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
    check_account_access(&mut tx, &ctx, account_id).await?;
    // Из сейфа и со счёта деньги двигает владелец (ADR-037): администратор — только касса смены.
    if !ctx.user.is_owner() {
        let kind = sqlx::query_scalar!("select kind from cash_accounts where id = $1", account_id)
            .fetch_one(&mut *tx)
            .await?;
        if kind != "register" {
            return Err(AppError::Forbidden);
        }
    }
    require_open_shift(&mut tx, ctx.user.branch_id, account_id).await?;
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
struct ReverseReq {
    op_id: Uuid,
    #[serde(default)]
    comment: String,
}

/// Администратор отменяет только то, что сделано в открытой смене (или вне смены, пока
/// новую не открыли); владелец — что угодно.
async fn check_reverse_window(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account_id: Uuid,
    shift_id: Option<Uuid>,
) -> AppResult<()> {
    if ctx.user.is_owner() {
        return Ok(());
    }
    let open = open_shift_id(conn, ctx.user.branch_id, account_id).await?;
    if shift_id != open {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn reverse_movement(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<BalanceOut>> {
    const KIND: &str = "cash.reverse";
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let m = sqlx::query!(
        r#"select account_id, shift_id, kind, amount_tyiyn, doc_type from cash_movements
           where id = $1 and branch_id = $2"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Чеки, расходы и выплаты отменяются своим документом, иначе разойдутся с учётом.
    if m.doc_type != "manual" {
        return Err(invalid(
            "это движение отменяется своим документом: чеком, расходом или выплатой",
        ));
    }
    check_account_access(&mut tx, &ctx, m.account_id).await?;
    lock_accounts(&mut tx, ctx.user.branch_id, &[m.account_id]).await?;
    check_reverse_window(&mut tx, &ctx, m.account_id, m.shift_id).await?;
    let done = sqlx::query_scalar!(
        r#"select exists (select 1 from cash_movements
                          where kind = 'reversal' and doc_type = 'cash_movement' and doc_id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if done {
        return Err(AppError::Conflict("движение уже отменено".into()));
    }
    let balance = add_movement(
        &mut tx,
        &ctx,
        CashEntry {
            account_id: m.account_id,
            kind: "reversal",
            amount: -m.amount_tyiyn,
            doc_type: "cash_movement",
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
        json!({ "kind": m.kind, "amount_tyiyn": -m.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = BalanceOut {
        account_id: m.account_id,
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

struct TransferArgs<'a> {
    from: Uuid,
    to: Uuid,
    amount: i64,
    comment: &'a str,
    reversal_of: Option<Uuid>,
}

/// Перемещение: запись и два движения с общим `doc_id`. Кассы должны быть заблокированы.
async fn transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    a: TransferArgs<'_>,
) -> AppResult<(Uuid, i64)> {
    let active = sqlx::query_scalar!(
        "select active from cash_accounts where id = $1 and branch_id = $2",
        a.to,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if !active {
        return Err(invalid("касса-получатель отключена"));
    }
    let number = ops::next_counter(conn, ctx.user.branch_id, "cash_transfer").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into cash_transfers (id, branch_id, number, from_account_id, to_account_id,
                                       amount_tyiyn, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        ctx.user.branch_id,
        number,
        a.from,
        a.to,
        a.amount,
        a.comment,
        a.reversal_of,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    let (out_kind, in_kind) = if a.reversal_of.is_some() {
        ("reversal", "reversal")
    } else {
        ("transfer_out", "transfer_in")
    };
    add_movement(
        conn,
        ctx,
        CashEntry {
            account_id: a.from,
            kind: out_kind,
            amount: -a.amount,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: a.comment,
        },
    )
    .await?;
    add_movement(
        conn,
        ctx,
        CashEntry {
            account_id: a.to,
            kind: in_kind,
            amount: a.amount,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: a.comment,
        },
    )
    .await?;
    Ok((id, number))
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
    crate::ops::check_amount(req.amount_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let from_kind = sqlx::query_scalar!(
        "select kind from cash_accounts where id = $1 and branch_id = $2",
        req.from_account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    // Администратор переводит только из кассы смены: из сейфов и со счёта деньги двигает
    // владелец (SPEC-05, права; ADR-037).
    if !ctx.user.is_owner() && from_kind != "register" {
        return Err(AppError::Forbidden);
    }
    lock_accounts(
        &mut tx,
        ctx.user.branch_id,
        &[req.from_account_id, req.to_account_id],
    )
    .await?;
    let (id, number) = transfer_tx(
        &mut tx,
        &ctx,
        TransferArgs {
            from: req.from_account_id,
            to: req.to_account_id,
            amount: req.amount_tyiyn,
            comment: req.comment.trim(),
            reversal_of: None,
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

async fn reverse_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<Value>> {
    const KIND: &str = "cash.transfer_reverse";
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let t = sqlx::query!(
        r#"select number, from_account_id, to_account_id, amount_tyiyn, reversal_of
           from cash_transfers where id = $1 and branch_id = $2"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if t.reversal_of.is_some() {
        return Err(invalid("это уже сторно перемещения"));
    }
    // Сторно забирает деньги из получателя: из кассы владельца — только он сам.
    check_account_access(&mut tx, &ctx, t.to_account_id).await?;
    lock_accounts(
        &mut tx,
        ctx.user.branch_id,
        &[t.from_account_id, t.to_account_id],
    )
    .await?;
    let out_shift = sqlx::query_scalar!(
        r#"select shift_id from cash_movements
           where doc_type = 'transfer' and doc_id = $1 and account_id = $2"#,
        id,
        t.from_account_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    check_reverse_window(&mut tx, &ctx, t.from_account_id, out_shift).await?;
    // Сдача кассы уже вошла в итог смены, который видел владелец: отменяет её только он.
    let handover = sqlx::query_scalar!(
        r#"select exists (select 1 from shift_handovers where transfer_id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if handover && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let done = sqlx::query_scalar!(
        r#"select exists (select 1 from cash_transfers where reversal_of = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if done {
        return Err(AppError::Conflict("перемещение уже отменено".into()));
    }
    let comment = format!("сторно перемещения № {}: {}", t.number, req.comment.trim());
    let (rid, number) = transfer_tx(
        &mut tx,
        &ctx,
        TransferArgs {
            from: t.to_account_id,
            to: t.from_account_id,
            amount: t.amount_tyiyn,
            comment: &comment,
            reversal_of: Some(id),
        },
    )
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "cash",
        Some(rid),
        json!({
            "amount_tyiyn": t.amount_tyiyn,
            "number": t.number,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    let out = json!({ "id": rid, "number": number });
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
    doc_type: String,
    doc_id: Option<Uuid>,
    user_name: String,
    created_at: DateTime<Utc>,
    /// Можно отменить сторно: внесение, изъятие или перемещение, ещё не отменённые.
    reversible: bool,
}

async fn account_movements(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<MovementOut>>> {
    // Движения сейфа владельца администратору не отдаём, как и его остаток (ADR-037).
    let owner_only = sqlx::query_scalar!(
        "select owner_only from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    if owner_only && !user.is_owner() {
        return Err(AppError::NotFound);
    }
    let rows = sqlx::query_as!(
        MovementOut,
        r#"select m.id, m.kind, m.amount_tyiyn, m.comment, m.doc_type, m.doc_id,
                  u.full_name as user_name, m.created_at,
                  case
                    when m.doc_type = 'manual' then not exists (
                      select 1 from cash_movements r
                      where r.kind = 'reversal' and r.doc_type = 'cash_movement' and r.doc_id = m.id)
                    when m.doc_type = 'transfer' and m.kind in ('transfer_in', 'transfer_out') then not exists (
                      select 1 from cash_transfers t where t.reversal_of = m.doc_id)
                    else false
                  end as "reversible!"
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
