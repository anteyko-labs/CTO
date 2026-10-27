//! Оплата сотрудников: правила, начисления, выплаты (SPEC-07).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::{self, CashEntry};
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/payroll/day", routing::get(day))
        .route("/payroll/month", routing::get(month))
        .route("/payroll/entries", routing::get(entries))
        .route("/payroll/accruals", routing::post(post_accrual))
        .route("/payroll/salary", routing::post(post_salary))
        .route("/payroll/remains", routing::get(remains))
        .route("/payouts", routing::post(post_payout))
        .route("/payouts/{id}/reverse", routing::post(reverse_payout))
        .route(
            "/payroll/accruals/{id}/reverse",
            routing::post(reverse_accrual),
        )
        .route(
            "/employees/{id}/pay-rules",
            routing::get(list_rules).post(create_rule),
        )
        .route(
            "/employees/{id}/pay-rules/{rule_id}",
            routing::delete(close_rule),
        )
}

// ---------- Сторно ----------

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Сторно начисления владельцем: удержание недостачи, ошибочный бонус или оклад (SPEC-07).
/// Начисления из чека снимаются только возвратом чека.
async fn reverse_accrual(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<OkOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_accrual_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно начисления — одна транзакция вызывающего: экран и «Исправить» (SPEC-29 §24).
pub async fn reverse_accrual_tx(
    tx: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<OkOut> {
    const KIND: &str = "payroll.accrual_reverse";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    if let Some(done) = ops::begin_op(&mut *tx, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let a = sqlx::query!(
        r#"select employee_id, business_date, kind, amount_tyiyn, doc_type, reversal_of
           from payroll_accruals where id = $1 and branch_id = $2 for update"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if a.reversal_of.is_some() {
        return Err(invalid("это уже сторно"));
    }
    if matches!(a.doc_type.as_str(), "sale" | "sale_return") {
        return Err(AppError::Conflict(
            "начисление по чеку снимается возвратом чека".into(),
        ));
    }
    lock_employee(&mut *tx, ctx.user.branch_id, a.employee_id).await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn,
                                         doc_type, doc_id, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, 'reversal', $7, $8, $7, $9, $10)"#,
        rid,
        ctx.user.branch_id,
        a.employee_id,
        a.business_date,
        a.kind,
        -a.amount_tyiyn,
        id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("начисление уже сторнировано".into())
        } else {
            AppError::from(e)
        }
    })?;
    ops::audit(
        &mut *tx,
        ctx,
        KIND,
        "employee",
        Some(a.employee_id),
        json!({ "accrual": id, "kind": a.kind, "amount_tyiyn": -a.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = OkOut {
        ok: true,
        id: Some(rid),
    };
    ops::finish_op(&mut *tx, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Сторно выплаты владельцем: деньги возвращаются в ту кассу, откуда ушли.
async fn reverse_payout(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<PayoutOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_payout_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно выплаты — одна транзакция вызывающего: экран и «Исправить» (SPEC-29 §24).
pub async fn reverse_payout_tx(
    tx: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<PayoutOut> {
    const KIND: &str = "payout.reverse";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let branch_id = ctx.user.branch_id;
    if let Some(done) = ops::begin_op(&mut *tx, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let p = sqlx::query!(
        r#"select employee_id, amount_tyiyn, source, account_id, reversal_of, purpose
           from payouts where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.reversal_of.is_some() {
        return Err(invalid("это уже сторно"));
    }
    lock_employee(&mut *tx, branch_id, p.employee_id).await?;
    let number = ops::next_counter(&mut *tx, branch_id, "payout").await?;
    let rid = new_id();
    sqlx::query!(
        // Сторно — того же вида, что и выплата: остаток по виду возвращается (ADR-122).
        r#"insert into payouts (id, branch_id, number, employee_id, amount_tyiyn, source,
                                account_id, comment, reversal_of, user_id, device_id, purpose, op_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        rid,
        branch_id,
        number,
        p.employee_id,
        -p.amount_tyiyn,
        p.source,
        p.account_id,
        req.comment.trim(),
        id,
        ctx.user.id,
        ctx.device_id,
        p.purpose,
        req.op_id
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("выплата уже сторнирована".into())
        } else {
            AppError::from(e)
        }
    })?;
    // Деньги возвращаются в кассу смены — только в открытую смену (SPEC-07).
    if let Some(acc) = p.account_id {
        cash::require_open_shift(&mut *tx, branch_id, acc).await?;
        cash::add_movement(
            &mut *tx,
            ctx,
            CashEntry {
                account_id: acc,
                kind: "reversal",
                amount: p.amount_tyiyn,
                doc_type: "payout",
                doc_id: Some(rid),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    let balance = sqlx::query_scalar!(
        r#"select coalesce((select sum(amount_tyiyn) from payroll_accruals where employee_id = $1), 0)::bigint
                - coalesce((select sum(amount_tyiyn) from payouts where employee_id = $1), 0)::bigint as "b!""#,
        p.employee_id
    )
    .fetch_one(&mut *tx)
    .await?;
    ops::audit(
        &mut *tx,
        ctx,
        KIND,
        "employee",
        Some(p.employee_id),
        json!({ "payout": id, "amount_tyiyn": p.amount_tyiyn, "purpose": p.purpose,
                "comment": req.comment.trim() }),
    )
    .await?;
    let out = PayoutOut {
        id: rid,
        number,
        balance_tyiyn: balance,
        lines: Vec::new(),
    };
    ops::finish_op(&mut *tx, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

// ---------- Начисления и выплаты сотрудника ----------

#[derive(Deserialize)]
struct EntriesQuery {
    employee_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
}

#[derive(Serialize)]
pub(crate) struct AccrualRow {
    pub(crate) id: Uuid,
    pub(crate) business_date: NaiveDate,
    pub(crate) kind: String,
    pub(crate) amount_tyiyn: i64,
    pub(crate) comment: String,
    pub(crate) doc_type: String,
    pub(crate) reversal_of: Option<Uuid>,
    pub(crate) reversed: bool,
    pub(crate) user_name: String,
    pub(crate) created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct PayoutRow {
    id: Uuid,
    number: i64,
    amount_tyiyn: i64,
    /// За что выдано (ADR-122): `general` — прежние выплаты без вида.
    purpose: String,
    source: String,
    account_name: Option<String>,
    comment: String,
    reversal_of: Option<Uuid>,
    reversed: bool,
    user_name: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct EntriesOut {
    /// Начисления не из чеков: оклад, за смену, бонус, удержание, недостача и их сторно.
    /// Начисления из чеков снимаются только возвратом чека — здесь их нет.
    accruals: Vec<AccrualRow>,
    payouts: Vec<PayoutRow>,
}

/// Что владелец может отменить у сотрудника за период (SPEC-07): начисления и выплаты
/// с отметкой, какие уже сторнированы.
async fn entries(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<EntriesQuery>,
) -> AppResult<Json<EntriesOut>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if q.from > q.to {
        return Err(invalid("начало периода позже конца"));
    }
    if (q.to - q.from).num_days() > 366 {
        return Err(invalid("период не больше года"));
    }
    let mut conn = state.pool.acquire().await?;
    let accruals = accrual_rows(&mut conn, user.branch_id, q.employee_id, q.from, q.to).await?;
    let payouts = sqlx::query_as!(
        PayoutRow,
        r#"select p.id, p.number, p.amount_tyiyn, p.purpose, p.source, c.name as "account_name?", p.comment,
                  p.reversal_of as "reversal_of?",
                  exists (select 1 from payouts r where r.reversal_of = p.id) as "reversed!",
                  u.full_name as "user_name!", p.created_at
           from payouts p join users u on u.id = p.user_id
           left join cash_accounts c on c.id = p.account_id
           where p.branch_id = $1 and p.employee_id = $2
             and (p.created_at at time zone 'Asia/Bishkek')::date between $3 and $4
           order by p.created_at desc"#,
        user.branch_id,
        q.employee_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(EntriesOut { accruals, payouts }))
}

/// Начисления сотрудника не из чеков за период, со сторно и отметкой «отменено»: для экрана
/// «Расчёт» и для кабинета работника в боте (SPEC-07, SPEC-23).
pub(crate) async fn accrual_rows(
    conn: &mut PgConnection,
    branch_id: Uuid,
    employee_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<AccrualRow>> {
    Ok(sqlx::query_as!(
        AccrualRow,
        r#"select a.id, a.business_date, a.kind, a.amount_tyiyn, a.comment, a.doc_type,
                  a.reversal_of as "reversal_of?",
                  exists (select 1 from payroll_accruals r where r.reversal_of = a.id) as "reversed!",
                  u.full_name as "user_name!", a.created_at
           from payroll_accruals a join users u on u.id = a.user_id
           where a.branch_id = $1 and a.employee_id = $2 and a.business_date between $3 and $4
             and a.doc_type not in ('sale', 'sale_return')
           order by a.business_date desc, a.created_at desc"#,
        branch_id,
        employee_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Начислено сотруднику по учётным дням периода (со сторно); дни без начислений не попадают.
pub(crate) async fn employee_days(
    conn: &mut PgConnection,
    branch_id: Uuid,
    employee_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<(NaiveDate, i64)>> {
    Ok(sqlx::query!(
        r#"select business_date, sum(amount_tyiyn)::bigint as "sum!"
           from payroll_accruals
           where branch_id = $1 and employee_id = $2 and business_date between $3 and $4
           group by business_date
           having sum(amount_tyiyn) <> 0
           order by business_date"#,
        branch_id,
        employee_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.business_date, r.sum))
    .collect())
}

// ---------- Правила оплаты ----------

#[derive(Serialize, Deserialize)]
struct RuleOut {
    id: Uuid,
    kind: String,
    role: String,
    base: Option<String>,
    amount_tyiyn: Option<i64>,
    rate_bp: Option<i32>,
    active_from: NaiveDate,
    active_to: Option<NaiveDate>,
}

async fn list_rules(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<RuleOut>>> {
    // Ставки — настройка оплаты, её видит и меняет владелец (SPEC-07).
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let rows = sqlx::query_as!(
        RuleOut,
        r#"select id, kind, role, base as "base?", amount_tyiyn as "amount_tyiyn?",
                  rate_bp as "rate_bp?", active_from, active_to as "active_to?"
           from employee_pay_rules
           where branch_id = $1 and employee_id = $2
           order by active_to nulls first, created_at"#,
        user.branch_id,
        id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct RuleReq {
    /// Повтор формы после потерянного ответа не заводит второе правило (ADR-070).
    op_id: Uuid,
    kind: String,
    role: Option<String>,
    base: Option<String>,
    amount_tyiyn: Option<i64>,
    rate_bp: Option<i32>,
}

async fn create_rule(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<RuleReq>,
) -> AppResult<Json<RuleOut>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // Ставка мастера за замену — настройка филиала, зафиксированная в чеке (ADR-027). Правило
    // `per_service` нигде не считалось бы и только вводило бы в заблуждение (ADR-062).
    if req.kind == "per_service" {
        return Err(invalid(
            "ставка мастера за замену задаётся в настройках продаж, а не правилом сотрудника",
        ));
    }
    if !matches!(
        req.kind.as_str(),
        "revenue_percent" | "per_shift" | "monthly_salary"
    ) {
        return Err(invalid("неизвестный вид оплаты"));
    }
    let role = req.role.unwrap_or_else(|| "cashier".into());
    if !matches!(role.as_str(), "cashier" | "master") {
        return Err(invalid("роль: кассир или мастер"));
    }
    let base = req
        .base
        .or_else(|| (req.kind == "revenue_percent").then(|| "gross".to_string()));
    // Процент считается только с валовой прибыли чека (ADR-036): другая база молча не работала бы.
    if base.as_deref().is_some_and(|b| b != "gross") {
        return Err(invalid("процент считается с валовой прибыли"));
    }
    if req.kind == "revenue_percent" {
        if req.rate_bp.is_none_or(|v| v <= 0) {
            return Err(invalid("укажите процент"));
        }
    } else if req.amount_tyiyn.is_none_or(|v| v <= 0) {
        return Err(invalid("укажите сумму"));
    }
    if let Some(a) = req.amount_tyiyn {
        ops::check_amount(a)?;
    }
    if req.rate_bp.is_some_and(|r| !(0..=10_000).contains(&r)) {
        return Err(invalid("процент от 0 до 100"));
    }
    const KIND: &str = "payroll.rule";
    let rule_id = new_id();
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    // Под блокировкой сотрудника два одновременных правила не останутся открытыми оба.
    lock_employee(&mut tx, ctx.user.branch_id, id).await?;
    // Прежнее правило того же вида закрывается сегодняшним днём: история начислений не меняется.
    let closed = sqlx::query!(
        r#"update employee_pay_rules
           set active_to = (now() at time zone 'Asia/Bishkek')::date
           where branch_id = $1 and employee_id = $2 and kind = $3 and active_to is null
           returning id, amount_tyiyn, rate_bp"#,
        ctx.user.branch_id,
        id,
        req.kind
    )
    .fetch_all(&mut *tx)
    .await?;
    let replaced: Vec<serde_json::Value> = closed
        .iter()
        .map(|r| json!({ "rule_id": r.id, "amount_tyiyn": r.amount_tyiyn, "rate_bp": r.rate_bp }))
        .collect();
    sqlx::query!(
        r#"insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base,
                                           amount_tyiyn, rate_bp, user_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        rule_id,
        ctx.user.branch_id,
        id,
        req.kind,
        role,
        base,
        req.amount_tyiyn,
        req.rate_bp,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(id),
        json!({
            "kind": req.kind,
            "amount_tyiyn": req.amount_tyiyn,
            "rate_bp": req.rate_bp,
            "replaced": replaced,
        }),
    )
    .await?;
    let out = sqlx::query_as!(
        RuleOut,
        r#"select id, kind, role, base as "base?", amount_tyiyn as "amount_tyiyn?",
                  rate_bp as "rate_bp?", active_from, active_to as "active_to?"
           from employee_pay_rules where id = $1"#,
        rule_id
    )
    .fetch_one(&mut *tx)
    .await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn close_rule(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, rule_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    let done = sqlx::query!(
        r#"update employee_pay_rules
           set active_to = (now() at time zone 'Asia/Bishkek')::date
           where id = $1 and employee_id = $2 and branch_id = $3 and active_to is null
           returning kind, role, amount_tyiyn, rate_bp, active_to"#,
        rule_id,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Кто и когда закрыл ставку — в журнал (инвариант 5): закрытие меняет оплату сотрудника.
    ops::audit(
        &mut tx,
        &ctx,
        "payroll.rule_close",
        "employee",
        Some(id),
        json!({
            "rule_id": rule_id,
            "kind": done.kind,
            "role": done.role,
            "amount_tyiyn": done.amount_tyiyn,
            "rate_bp": done.rate_bp,
            "active_to": done.active_to,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- Начисления ----------

pub struct Accrual<'a> {
    pub employee_id: Uuid,
    pub kind: &'a str,
    pub amount: i64,
    pub base: Option<i64>,
    pub rule_id: Option<Uuid>,
    pub doc_type: &'a str,
    pub doc_id: Option<Uuid>,
    pub comment: &'a str,
}

/// Начисление сотруднику. День берётся у документа, а не у момента записи (SPEC-08).
pub async fn add_accrual(
    conn: &mut PgConnection,
    ctx: &Ctx,
    business_date: Option<NaiveDate>,
    a: Accrual<'_>,
) -> AppResult<Option<Uuid>> {
    if a.amount == 0 {
        return Ok(None);
    }
    ops::check_amount(a.amount)?;
    let id = new_id();
    sqlx::query!(
        r#"insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn,
                                         base_tyiyn, rule_id, doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, coalesce($4, (now() at time zone 'Asia/Bishkek')::date), $5, $6,
                   $7, $8, $9, $10, $11, $12, $13)"#,
        id,
        ctx.user.branch_id,
        a.employee_id,
        business_date,
        a.kind,
        a.amount,
        a.base,
        a.rule_id,
        a.doc_type,
        a.doc_id,
        a.comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(Some(id))
}

/// Оплата кассира по умолчанию (SPEC-07, ADR-019, ADR-036): 2 % с валовой прибыли чека
/// и оклад 30 000 сом в месяц. Заводится, только если у сотрудника никогда не было правил
/// кассира: снятый и снова назначенный кассир не получает обратно ставки, которые владелец
/// закрыл (ADR-062). Дальше владелец правит их в карточке сотрудника. `true` — правила заведены.
pub const DEFAULT_CASHIER_PERCENT_BP: i32 = 200;
pub const DEFAULT_CASHIER_SALARY_TYIYN: i64 = 3_000_000;

pub async fn ensure_cashier_rules(
    conn: &mut PgConnection,
    ctx: &Ctx,
    employee_id: Uuid,
) -> AppResult<bool> {
    let had = sqlx::query_scalar!(
        r#"select exists (select 1 from employee_pay_rules
                          where branch_id = $1 and employee_id = $2 and role = 'cashier') as "e!""#,
        ctx.user.branch_id,
        employee_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if had {
        return Ok(false);
    }
    let mut any = false;
    for (kind, base, amount, rate) in [
        (
            "revenue_percent",
            Some("gross"),
            None,
            Some(DEFAULT_CASHIER_PERCENT_BP),
        ),
        (
            "monthly_salary",
            None,
            Some(DEFAULT_CASHIER_SALARY_TYIYN),
            None,
        ),
    ] {
        let id = new_id();
        let added = sqlx::query!(
            r#"insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base,
                                               amount_tyiyn, rate_bp, user_id)
               select $1, $2, $3, $4, 'cashier', $5, $6, $7, $8
               where not exists (select 1 from employee_pay_rules
                                 where employee_id = $3 and kind = $4 and active_to is null)"#,
            id,
            ctx.user.branch_id,
            employee_id,
            kind,
            base,
            amount,
            rate,
            ctx.user.id
        )
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if added > 0 {
            any = true;
            ops::audit(
                conn,
                ctx,
                "payroll.rule",
                "employee",
                Some(employee_id),
                json!({ "kind": kind, "amount_tyiyn": amount, "rate_bp": rate, "default": true }),
            )
            .await?;
        }
    }
    Ok(any)
}

/// Правило сотрудника нужного вида, действовавшее в день документа (SPEC-07): офлайн-чек
/// вчерашнего дня получает вчерашнюю ставку, а не ту, что владелец поставил сегодня.
pub async fn active_rule(
    conn: &mut PgConnection,
    branch_id: Uuid,
    employee_id: Uuid,
    kind: &str,
    on: NaiveDate,
) -> AppResult<Option<(Uuid, Option<i64>, Option<i32>)>> {
    let r = sqlx::query!(
        r#"select id, amount_tyiyn, rate_bp from employee_pay_rules
           where branch_id = $1 and employee_id = $2 and kind = $3
             and active_from <= $4 and (active_to is null or active_to > $4)
           order by created_at desc limit 1"#,
        branch_id,
        employee_id,
        kind,
        on
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(r.map(|r| (r.id, r.amount_tyiyn, r.rate_bp)))
}

/// Процент кассира с чека: с оплаченной части сразу, с долговой — при погашении (ADR-035, ADR-036).
pub struct SaleAccrual {
    pub sale_id: Uuid,
    pub business_date: NaiveDate,
    pub cashier_id: Uuid,
    pub party_id: Option<Uuid>,
    /// Валовая прибыль чека, сумма чека и сколько из неё ушло в долг.
    pub gross: i64,
    pub total: i64,
    pub debt: i64,
    /// Оплачено баллами: скидка точки, процент с неё не платится (ADR-065).
    pub bonus: i64,
    /// Сумма строк без цены закупки: на них приходится своя доля скидки баллами.
    pub unknown_amount: i64,
    /// Строки без цены закупки: в `gross` не входят, процент по ним ждёт цены (ADR-062).
    pub unknown: Vec<UnknownCost>,
}

/// Скидка баллами, приходящаяся на часть чека суммой `part` из `total` (ADR-065).
fn bonus_share(bonus: i64, part: i64, total: i64) -> i64 {
    if bonus <= 0 || total <= 0 || part <= 0 {
        return 0;
    }
    div_round(i128::from(bonus) * i128::from(part), i128::from(total)).unwrap_or(0)
}

/// Часть базы процента, за которую деньги пришли сразу: деньги чека — сумма без баллов,
/// долговая часть ждёт погашения (ADR-035).
fn paid_share(base: i64, money: i64, debt: i64) -> i64 {
    if base <= 0 || money <= 0 {
        return 0;
    }
    let paid = money.saturating_sub(debt).max(0);
    div_round(i128::from(base) * i128::from(paid), i128::from(money)).unwrap_or(0)
}

/// Строка чека, себестоимость которой неизвестна: сколько единиц не покрыто ценой.
pub struct UnknownCost {
    pub line_no: i32,
    pub product_id: Uuid,
    pub units: i64,
}

pub async fn accrue_for_sale(conn: &mut PgConnection, ctx: &Ctx, s: SaleAccrual) -> AppResult<()> {
    let SaleAccrual {
        sale_id,
        business_date,
        cashier_id,
        party_id,
        gross,
        total,
        debt,
        bonus,
        unknown_amount,
        unknown,
    } = s;
    let rule = active_rule(
        conn,
        ctx.user.branch_id,
        cashier_id,
        "revenue_percent",
        business_date,
    )
    .await?;
    // Ожидание цены заводится всегда, даже без правила процента: по нему владелец видит, у каких
    // товаров нет цены закупки, и не снимет отметку «проверить», пока её не укажет (ADR-062).
    for u in &unknown {
        sqlx::query!(
            r#"insert into payroll_cost_pending (id, branch_id, sale_id, line_no, product_id, employee_id,
                                                 rule_id, rate_bp, unknown_units)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
            new_id(),
            ctx.user.branch_id,
            sale_id,
            u.line_no,
            u.product_id,
            cashier_id,
            rule.map(|r| r.0),
            rule.and_then(|r| r.2).unwrap_or(0),
            u.units
        )
        .execute(&mut *conn)
        .await?;
    }
    let Some((rule_id, _, rate)) = rule else {
        return Ok(());
    };
    let rate = i64::from(rate.unwrap_or(0));
    if rate <= 0 || total == 0 {
        return Ok(());
    }
    // 2 % — только с денег: часть, оплаченная баллами, — скидка точки и из базы вычитается
    // (ADR-065). Скидка делится между строками по сумме: доля строк без цены вычтется, когда
    // владелец укажет цену.
    let gross = gross.saturating_sub(bonus_share(
        bonus,
        total.saturating_sub(unknown_amount),
        total,
    ));
    // Убыточный чек (подарок, распродажа) процента не даёт и с кассира не вычитается.
    // Ожидание долга всё же нужно, если в чеке есть строки без цены: их процент придёт позже.
    if gross <= 0 {
        if debt > 0
            && !unknown.is_empty()
            && let Some(pid) = party_id
        {
            insert_debt_pending(conn, ctx, pid, sale_id, cashier_id, rule_id, rate, 0, debt)
                .await?;
        }
        return Ok(());
    }
    let now_base = paid_share(gross, total.saturating_sub(bonus), debt);
    let amount = div_round(i128::from(now_base) * i128::from(rate), 10_000).unwrap_or(0);
    add_accrual(
        conn,
        ctx,
        Some(business_date),
        Accrual {
            employee_id: cashier_id,
            kind: "revenue_percent",
            amount,
            base: Some(now_base),
            rule_id: Some(rule_id),
            doc_type: "sale",
            doc_id: Some(sale_id),
            comment: "",
        },
    )
    .await?;
    if debt > 0
        && let Some(pid) = party_id
    {
        insert_debt_pending(
            conn,
            ctx,
            pid,
            sale_id,
            cashier_id,
            rule_id,
            rate,
            gross - now_base,
            debt,
        )
        .await?;
        // У клиента был аванс (переплата, деньги прежнего чека после его исправления): долг
        // этого чека им уже частично оплачен. Процент с этой части — сразу, ожидание — только на
        // настоящий долг (ADR-092). Долг чека уже в балансе клиента.
        let balance = sqlx::query_scalar!("select balance_tyiyn from parties where id = $1", pid)
            .fetch_one(&mut *conn)
            .await?;
        let covered = debt.saturating_sub(balance.max(0)).clamp(0, debt);
        if covered > 0 {
            let base = div_round(
                i128::from(gross - now_base) * i128::from(covered),
                i128::from(debt),
            )
            .unwrap_or(0);
            let accrued = div_round(i128::from(base) * i128::from(rate), 10_000).unwrap_or(0);
            add_accrual(
                conn,
                ctx,
                Some(business_date),
                Accrual {
                    employee_id: cashier_id,
                    kind: "revenue_percent",
                    amount: accrued,
                    base: Some(base),
                    rule_id: Some(rule_id),
                    doc_type: "sale",
                    doc_id: Some(sale_id),
                    comment: "долг оплачен авансом клиента",
                },
            )
            .await?;
            sqlx::query!(
                r#"update payroll_pending set debt_remaining_tyiyn = debt_remaining_tyiyn - $2
                   where sale_id = $1 and debt_remaining_tyiyn >= $2"#,
                sale_id,
                covered
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Долговая часть процента ждёт погашения (ADR-035).
#[allow(clippy::too_many_arguments)]
async fn insert_debt_pending(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    sale_id: Uuid,
    cashier_id: Uuid,
    rule_id: Uuid,
    rate: i64,
    base: i64,
    debt: i64,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into payroll_pending (id, branch_id, party_id, sale_id, employee_id, rule_id,
                                        rate_bp, gross_tyiyn, debt_total_tyiyn, debt_remaining_tyiyn)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)"#,
        new_id(),
        ctx.user.branch_id,
        party_id,
        sale_id,
        cashier_id,
        rule_id,
        i32::try_from(rate).unwrap_or(0),
        base,
        debt
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Валовая прибыль возвращаемой части строки без цены закупки (ADR-062): пока владелец цену не
/// указал, процент по строке не начислялся — снимать нечего (`None`); после — по его цене.
pub async fn unknown_line_gross_back(
    conn: &mut PgConnection,
    sale_id: Uuid,
    line_no: i32,
    amount_back: i64,
    qty_back: i64,
    line_qty: i64,
) -> AppResult<Option<i64>> {
    let resolved = sqlx::query_scalar!(
        r#"select resolved_cost_tyiyn from payroll_cost_pending
           where sale_id = $1 and line_no = $2 and resolved_at is not null"#,
        sale_id,
        line_no
    )
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let Some(cost) = resolved else {
        return Ok(None);
    };
    if line_qty <= 0 {
        return Ok(Some(0));
    }
    let cost_back = div_round(
        i128::from(cost) * i128::from(qty_back),
        i128::from(line_qty),
    )
    .unwrap_or(0);
    Ok(Some(amount_back.saturating_sub(cost_back)))
}

/// Итог указания цены закупки: сколько строк закрыто и сколько начислено кассирам.
#[derive(Serialize, Deserialize, Clone)]
pub struct CostResolved {
    pub lines: i64,
    pub accrued_tyiyn: i64,
}

/// Владелец указал цену закупки товара: строки чеков без себестоимости получают цену, и кассиру
/// начисляется его процент с них — в сегодняшний день, прошлые дни не меняются (ADR-062).
/// `price` — за `per` единиц учёта (за штуку, канистру или килограмм). Вызывается в транзакции.
pub async fn resolve_cost_pending(
    conn: &mut PgConnection,
    ctx: &Ctx,
    product_id: Uuid,
    price: i64,
    per: i64,
) -> AppResult<CostResolved> {
    let branch_id = ctx.user.branch_id;
    // Ожидания этой карточки и всех, что в неё влиты: продажа дубля тоже ждёт цены.
    let rows = sqlx::query!(
        r#"with recursive fam as (
             select $2::uuid as id
             union select p.id from products p join fam on p.merged_into = fam.id
           )
           select k.id, k.sale_id, k.line_no, k.employee_id, k.rule_id, k.rate_bp, k.unknown_units,
                  s.number, s.party_id as "party_id?",
                  l.qty, l.amount_tyiyn, l.cost_tyiyn, l.gift,
                  coalesce((select sum(-r.qty) from sale_lines r join sales x on x.id = r.sale_id
                            where x.reversal_of = s.id and r.line_no = l.line_no), 0)::bigint as "ret_qty!",
                  coalesce((select sum(-r.amount_tyiyn) from sale_lines r join sales x on x.id = r.sale_id
                            where x.reversal_of = s.id and r.line_no = l.line_no), 0)::bigint as "ret_amount!",
                  s.total_tyiyn,
                  coalesce((select sum(p.amount_tyiyn) from sale_payments p
                            where p.sale_id = s.id and p.method = 'debt'), 0)::bigint as "debt!",
                  coalesce((select sum(p.amount_tyiyn) from sale_payments p
                            where p.sale_id = s.id and p.method = 'bonus'), 0)::bigint as "bonus!"
           from payroll_cost_pending k
           join sales s on s.id = k.sale_id
           join sale_lines l on l.sale_id = k.sale_id and l.line_no = k.line_no
           where k.branch_id = $1 and k.resolved_at is null and k.product_id in (select id from fam)
           order by k.created_at
           for update of k"#,
        branch_id,
        product_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = CostResolved {
        lines: 0,
        accrued_tyiyn: 0,
    };
    for r in rows {
        let unknown_cost = div_round(
            i128::from(r.unknown_units) * i128::from(price),
            i128::from(per.max(1)),
        )
        .ok_or_else(crate::error::overflow)?;
        let line_cost = r
            .cost_tyiyn
            .checked_add(unknown_cost)
            .ok_or_else(crate::error::overflow)?;
        sqlx::query!(
            r#"update payroll_cost_pending
               set resolved_cost_tyiyn = $2, resolved_at = now(), resolved_by = $3
               where id = $1"#,
            r.id,
            line_cost,
            ctx.user.id
        )
        .execute(&mut *conn)
        .await?;
        out.lines += 1;
        // Что от строки осталось после возвратов — с того и процент.
        let left_qty = r.qty - r.ret_qty;
        if r.gift || r.rate_bp <= 0 || left_qty <= 0 || r.qty <= 0 || r.total_tyiyn <= 0 {
            continue;
        }
        let left_cost = div_round(
            i128::from(line_cost) * i128::from(left_qty),
            i128::from(r.qty),
        )
        .unwrap_or(0);
        // Доля скидки баллами на эту строку — как у остального чека (ADR-065).
        let left_amount = r.amount_tyiyn - r.ret_amount;
        let gross = left_amount
            .saturating_sub(left_cost)
            .saturating_sub(bonus_share(r.bonus, left_amount, r.total_tyiyn));
        if gross <= 0 {
            continue;
        }
        let rate = i128::from(r.rate_bp);
        // Оплаченная часть чека — процент сейчас; долговая — по мере погашения, как у всего чека.
        let now_base = paid_share(gross, r.total_tyiyn.saturating_sub(r.bonus), r.debt);
        let debt_base = gross - now_base;
        let mut base = now_base;
        if debt_base > 0 {
            let pending = sqlx::query!(
                r#"select id, debt_total_tyiyn, debt_remaining_tyiyn from payroll_pending
                   where sale_id = $1 and employee_id = $2 and branch_id = $3
                   order by created_at limit 1
                   for update"#,
                r.sale_id,
                r.employee_id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            // Ожидания нет (долг уже прощён или списан возвратом) — денег по долговой части
            // не будет, процента с неё тоже.
            if let Some(p) = pending {
                // Погашенная часть долга уже дала деньги — её процент сейчас; на остаток
                // долга база ложится в ожидание, погашения возьмут её пропорционально.
                let repaid = p.debt_total_tyiyn - p.debt_remaining_tyiyn;
                base += div_round(
                    i128::from(debt_base) * i128::from(repaid.max(0)),
                    i128::from(p.debt_total_tyiyn.max(1)),
                )
                .unwrap_or(0);
                sqlx::query!(
                    "update payroll_pending set gross_tyiyn = gross_tyiyn + $2 where id = $1",
                    p.id,
                    debt_base
                )
                .execute(&mut *conn)
                .await?;
            }
        }
        let amount = div_round(i128::from(base) * rate, 10_000).unwrap_or(0);
        add_accrual(
            conn,
            ctx,
            None,
            Accrual {
                employee_id: r.employee_id,
                kind: "revenue_percent",
                amount,
                base: Some(base),
                rule_id: r.rule_id,
                doc_type: "sale",
                doc_id: Some(r.sale_id),
                comment: &format!("цена закупки указана: чек № {}", r.number),
            },
        )
        .await?;
        out.accrued_tyiyn = out.accrued_tyiyn.saturating_add(amount);
    }
    Ok(out)
}

/// Возврат по чеку: обратные начисления в день возврата (SPEC-07: «такие же строки с минусом»).
pub struct ReturnAccrual {
    pub orig_sale_id: Uuid,
    pub return_id: Uuid,
    pub business_date: NaiveDate,
    /// Валовая прибыль возвращённых строк, сумма возврата и сколько из неё списано с долга.
    pub gross_back: i64,
    pub total_back: i64,
    pub debt_back: i64,
    /// Сколько вернули баллами и на какую сумму строк, вошедших в процент (без строк,
    /// ждущих цену закупки): база снимается так же, как считалась при продаже (ADR-065).
    pub bonus_back: i64,
    pub counted_back: i64,
    pub full_return: bool,
    /// Сколько ставок мастера снимает возврат: доли возвращённых строк услуг (ADR-096), при
    /// возврате целиком — и ставка за чек (ADR-027).
    pub fee_back: i64,
}

pub async fn reverse_for_return(
    conn: &mut PgConnection,
    ctx: &Ctx,
    r: ReturnAccrual,
) -> AppResult<()> {
    // Начисления по чеку и по прежним возвратам к нему.
    let rows = sqlx::query!(
        r#"select a.employee_id, a.kind, a.rule_id,
                  coalesce(sum(a.amount_tyiyn), 0)::bigint as "net!",
                  coalesce(sum(a.amount_tyiyn) filter (where a.amount_tyiyn > 0), 0)::bigint as "accrued!",
                  coalesce(sum(a.base_tyiyn) filter (where a.amount_tyiyn > 0), 0)::bigint as "base!"
           from payroll_accruals a
           where a.branch_id = $1 and a.kind in ('service_fee', 'revenue_percent')
             and ((a.doc_type = 'sale' and a.doc_id = $2)
                  or (a.doc_type = 'sale_return'
                      and a.doc_id in (select id from sales where reversal_of = $2)))
           group by a.employee_id, a.kind, a.rule_id"#,
        ctx.user.branch_id,
        r.orig_sale_id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Процент снимается с той части возврата, что отдана деньгами; долговую часть
    // процент ещё не получил — она снимается с ожидания ниже. Баллы — скидка: их доля
    // вычитается из базы, как при продаже (ADR-065).
    let gross_back =
        r.gross_back
            .saturating_sub(bonus_share(r.bonus_back, r.counted_back, r.total_back));
    let base_back = paid_share(
        gross_back,
        r.total_back.saturating_sub(r.bonus_back),
        r.debt_back,
    );
    for row in rows {
        if row.net <= 0 {
            continue;
        }
        let back = match row.kind.as_str() {
            // Работу мастер сделал: возврат товара ставку не снимает. Ставка услуги уходит с
            // возвращённой строкой услуги (ADR-096), ставка за чек — при возврате целиком (ADR-027).
            "service_fee" if r.full_return => row.net,
            "service_fee" => r.fee_back.clamp(0, row.net),
            "revenue_percent" if row.base > 0 && base_back > 0 => div_round(
                i128::from(row.accrued) * i128::from(base_back),
                i128::from(row.base),
            )
            .unwrap_or(0)
            .min(row.net),
            _ => 0,
        };
        add_accrual(
            conn,
            ctx,
            Some(r.business_date),
            Accrual {
                employee_id: row.employee_id,
                kind: &row.kind,
                amount: -back,
                base: (row.kind == "revenue_percent").then_some(-base_back),
                rule_id: row.rule_id,
                doc_type: "sale_return",
                doc_id: Some(r.return_id),
                comment: "возврат по чеку",
            },
        )
        .await?;
    }
    // Долг уменьшился на возврат: процент с этой части уже никогда не придёт.
    if r.debt_back > 0 {
        let pending = sqlx::query!(
            r#"select id, gross_tyiyn, debt_total_tyiyn, debt_remaining_tyiyn from payroll_pending
               where sale_id = $1 and branch_id = $2 and debt_remaining_tyiyn > 0
               for update"#,
            r.orig_sale_id,
            ctx.user.branch_id
        )
        .fetch_all(&mut *conn)
        .await?;
        for p in pending {
            let d = r.debt_back.min(p.debt_remaining_tyiyn);
            let gross_cut = div_round(
                i128::from(p.gross_tyiyn) * i128::from(d),
                i128::from(p.debt_total_tyiyn),
            )
            .unwrap_or(0);
            // Остаток базы делится на остаток долга в той же пропорции, что и раньше.
            let left_total = (p.debt_total_tyiyn - d).max(1);
            sqlx::query!(
                r#"update payroll_pending
                   set gross_tyiyn = $2, debt_total_tyiyn = $3, debt_remaining_tyiyn = $4
                   where id = $1"#,
                p.id,
                p.gross_tyiyn - gross_cut,
                left_total,
                p.debt_remaining_tyiyn - d
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
}

/// Погашение сторнировали — процент, начисленный за него, снимается, а долговая часть чеков
/// снова ждёт оплаты. Идём от последних погашенных чеков к первым (обратный порядок).
pub async fn reverse_repayment(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    mut amount: i64,
) -> AppResult<()> {
    let rows = sqlx::query!(
        r#"select id, sale_id, employee_id, rule_id, rate_bp, gross_tyiyn,
                  debt_total_tyiyn, debt_remaining_tyiyn
           from payroll_pending
           where party_id = $1 and branch_id = $2 and debt_remaining_tyiyn < debt_total_tyiyn
           order by created_at desc
           for update"#,
        party_id,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        if amount <= 0 {
            break;
        }
        let give = amount.min(r.debt_total_tyiyn - r.debt_remaining_tyiyn);
        let base = div_round(
            i128::from(r.gross_tyiyn) * i128::from(give),
            i128::from(r.debt_total_tyiyn),
        )
        .unwrap_or(0);
        let back = div_round(i128::from(base) * i128::from(r.rate_bp), 10_000).unwrap_or(0);
        add_accrual(
            conn,
            ctx,
            None,
            Accrual {
                employee_id: r.employee_id,
                kind: "revenue_percent",
                amount: -back,
                base: Some(-base),
                rule_id: r.rule_id,
                doc_type: "sale",
                doc_id: Some(r.sale_id),
                comment: "сторно погашения долга",
            },
        )
        .await?;
        sqlx::query!(
            "update payroll_pending set debt_remaining_tyiyn = debt_remaining_tyiyn + $2 where id = $1",
            r.id,
            give
        )
        .execute(&mut *conn)
        .await?;
        amount -= give;
    }
    Ok(())
}

/// Долг погасили — догоняем процент по тем чекам, что ждали (ADR-035).
pub async fn accrue_on_repayment(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    mut amount: i64,
    ledger_id: Uuid,
) -> AppResult<()> {
    let rows = sqlx::query!(
        r#"select id, sale_id, employee_id, rule_id, rate_bp, gross_tyiyn,
                  debt_total_tyiyn, debt_remaining_tyiyn
           from payroll_pending
           where party_id = $1 and branch_id = $2 and debt_remaining_tyiyn > 0
           order by created_at"#,
        party_id,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        if amount <= 0 {
            break;
        }
        let take = amount.min(r.debt_remaining_tyiyn);
        let base = div_round(
            i128::from(r.gross_tyiyn) * i128::from(take),
            i128::from(r.debt_total_tyiyn),
        )
        .unwrap_or(0);
        let accrued = div_round(i128::from(base) * i128::from(r.rate_bp), 10_000).unwrap_or(0);
        add_accrual(
            conn,
            ctx,
            None,
            Accrual {
                employee_id: r.employee_id,
                kind: "revenue_percent",
                amount: accrued,
                base: Some(base),
                rule_id: r.rule_id,
                doc_type: "sale",
                doc_id: Some(r.sale_id),
                comment: "после погашения долга",
            },
        )
        .await?;
        sqlx::query!(
            "update payroll_pending set debt_remaining_tyiyn = debt_remaining_tyiyn - $2 where id = $1",
            r.id,
            take
        )
        .execute(&mut *conn)
        .await?;
        record_take(conn, ctx, r.id, ledger_id, take, base, accrued).await?;
        amount -= take;
    }
    Ok(())
}

/// Владелец простил долг — по этим чекам денег не будет, и процент ждать не нужно.
/// Ожидание гасится по тем же правилам очереди, что и погашение, но без начисления.
pub async fn forgive_pending(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    mut amount: i64,
    ledger_id: Uuid,
) -> AppResult<()> {
    let rows = sqlx::query!(
        r#"select id, debt_remaining_tyiyn from payroll_pending
           where party_id = $1 and branch_id = $2 and debt_remaining_tyiyn > 0
           order by created_at"#,
        party_id,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in rows {
        if amount <= 0 {
            break;
        }
        let take = amount.min(r.debt_remaining_tyiyn);
        sqlx::query!(
            "update payroll_pending set debt_remaining_tyiyn = debt_remaining_tyiyn - $2 where id = $1",
            r.id,
            take
        )
        .execute(&mut *conn)
        .await?;
        record_take(conn, ctx, r.id, ledger_id, take, 0, 0).await?;
        amount -= take;
    }
    Ok(())
}

/// Сколько записи журнала долга сняли с ожидания процента (ADR-088).
async fn record_take(
    conn: &mut PgConnection,
    ctx: &Ctx,
    pending_id: Uuid,
    ledger_id: Uuid,
    take: i64,
    base: i64,
    accrued: i64,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into payroll_pending_takes (id, branch_id, pending_id, ledger_id, take_tyiyn,
                                              base_tyiyn, accrued_tyiyn)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        Uuid::now_v7(),
        ctx.user.branch_id,
        pending_id,
        ledger_id,
        take,
        base,
        accrued
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Сторно погашения или прощения долга: ожидание процента возвращается ровно на то, что эта
/// запись сняла, и начисленный за неё процент снимается — с того же чека (ADR-088). Записи,
/// проведённые до учёта по записям, — прежним порядком: погашение — от последних чеков,
/// прощение — ничего. Возвращает `false`, если у записи нет учёта.
pub async fn reverse_takes(conn: &mut PgConnection, ctx: &Ctx, ledger_id: Uuid) -> AppResult<bool> {
    let rows = sqlx::query!(
        r#"select t.pending_id, p.sale_id, p.employee_id, p.rule_id,
                  sum(t.take_tyiyn)::bigint as "take!", sum(t.base_tyiyn)::bigint as "base!",
                  sum(t.accrued_tyiyn)::bigint as "accrued!"
           from payroll_pending_takes t join payroll_pending p on p.id = t.pending_id
           where t.ledger_id = $1 and t.branch_id = $2
           group by t.pending_id, p.sale_id, p.employee_id, p.rule_id
           order by t.pending_id"#,
        ledger_id,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.is_empty() {
        return Ok(false);
    }
    for r in rows {
        if r.take == 0 && r.accrued == 0 {
            continue;
        }
        sqlx::query!(
            "select id from payroll_pending where id = $1 for update",
            r.pending_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if r.accrued != 0 {
            add_accrual(
                conn,
                ctx,
                None,
                Accrual {
                    employee_id: r.employee_id,
                    kind: "revenue_percent",
                    amount: -r.accrued,
                    base: Some(-r.base),
                    rule_id: r.rule_id,
                    doc_type: "sale",
                    doc_id: Some(r.sale_id),
                    comment: "сторно погашения долга",
                },
            )
            .await?;
        }
        sqlx::query!(
            "update payroll_pending set debt_remaining_tyiyn = debt_remaining_tyiyn + $2 where id = $1",
            r.pending_id,
            r.take
        )
        .execute(&mut *conn)
        .await?;
        record_take(
            conn,
            ctx,
            r.pending_id,
            ledger_id,
            -r.take,
            -r.base,
            -r.accrued,
        )
        .await?;
    }
    Ok(true)
}

/// Блокирует сотрудника филиала: выплаты и оклады по нему идут по очереди,
/// и остаток не уходит в минус от двух одновременных выплат.
async fn lock_employee(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<()> {
    sqlx::query_scalar!(
        "select id from employees where id = $1 and branch_id = $2 for update",
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("сотрудник не найден"))?;
    Ok(())
}

/// Закрытие смены: оплата за смену и удержание недостачи с кассира (ADR-020).
pub async fn accrue_for_shift_close(
    conn: &mut PgConnection,
    ctx: &Ctx,
    shift_id: Uuid,
    business_date: NaiveDate,
    cashier_id: Uuid,
    diff: i64,
) -> AppResult<()> {
    let already = sqlx::query_scalar!(
        r#"select count(*) as "n!" from payroll_accruals
           where doc_id = $1 and kind = 'shift_fee'"#,
        shift_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if already == 0
        && let Some((rule_id, amount, _)) = active_rule(
            conn,
            ctx.user.branch_id,
            cashier_id,
            "per_shift",
            business_date,
        )
        .await?
    {
        add_accrual(
            conn,
            ctx,
            Some(business_date),
            Accrual {
                employee_id: cashier_id,
                kind: "shift_fee",
                amount: amount.unwrap_or(0),
                base: None,
                rule_id: Some(rule_id),
                doc_type: "shift",
                doc_id: Some(shift_id),
                comment: "",
            },
        )
        .await?;
    }
    if diff < 0 {
        add_accrual(
            conn,
            ctx,
            Some(business_date),
            Accrual {
                employee_id: cashier_id,
                kind: "shortage",
                amount: diff,
                base: None,
                rule_id: None,
                doc_type: "shift",
                doc_id: Some(shift_id),
                comment: "недостача по смене",
            },
        )
        .await?;
    }
    Ok(())
}

// ---------- Отчёт и выплаты ----------

#[derive(Serialize)]
struct DayRow {
    employee_id: Uuid,
    full_name: String,
    opening_tyiyn: i64,
    accrued_tyiyn: i64,
    service_fee_tyiyn: i64,
    percent_tyiyn: i64,
    /// Всё, кроме процента и услуг: оклад, бонус и удержания вместе (прежний «Прочее»).
    other_tyiyn: i64,
    /// Оклад за месяц и за смену (ADR-122).
    salary_tyiyn: i64,
    bonus_tyiyn: i64,
    /// Удержания и недостача — с минусом.
    withheld_tyiyn: i64,
    base_tyiyn: Option<i64>,
    /// Выдано за день, авансы тоже.
    paid_tyiyn: i64,
    /// В том числе авансом.
    advance_tyiyn: i64,
    balance_tyiyn: i64,
}

#[derive(Deserialize)]
struct DayQuery {
    date: Option<NaiveDate>,
}

async fn day(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<DayQuery>,
) -> AppResult<Json<Vec<DayRow>>> {
    let mut conn = state.pool.acquire().await?;
    let date = match q.date {
        Some(d) => d,
        None => {
            sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                .fetch_one(&mut *conn)
                .await?
        }
    };
    let owner = user.is_owner();
    let rows = sqlx::query!(
        r#"select e.id as "employee_id!", e.full_name as "full_name!",
             e.is_master as "is_master!", e.is_cashier as "is_cashier!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date < $2), 0)::bigint
             - coalesce((select sum(p.amount_tyiyn) from payouts p
                         where p.employee_id = e.id and (p.created_at at time zone 'Asia/Bishkek')::date < $2), 0)::bigint
               as "opening!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2), 0)::bigint as "accrued!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2 and a.kind = 'service_fee'), 0)::bigint as "service_fee!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2 and a.kind = 'revenue_percent'), 0)::bigint as "percent!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2
                         and a.kind in ('monthly_salary', 'shift_fee')), 0)::bigint as "salary!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2 and a.kind = 'bonus'), 0)::bigint as "bonus!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2
                         and a.kind in ('penalty', 'shortage')), 0)::bigint as "withheld!",
             coalesce((select sum(a.base_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2 and a.kind = 'revenue_percent'), 0)::bigint as "base!",
             coalesce((select sum(p.amount_tyiyn) from payouts p
                       where p.employee_id = e.id and (p.created_at at time zone 'Asia/Bishkek')::date = $2), 0)::bigint as "paid!",
             coalesce((select sum(p.amount_tyiyn) from payouts p
                       where p.employee_id = e.id and p.purpose = 'advance'
                         and (p.created_at at time zone 'Asia/Bishkek')::date = $2), 0)::bigint as "advance!"
           from employees e
           where e.branch_id = $1
           order by e.full_name"#,
        user.branch_id,
        date
    )
    .fetch_all(&mut *conn)
    .await?;
    let out = rows
        .into_iter()
        // Администратор выплачивает только мастерам (SPEC-30 §6): к выплате видит только их.
        .filter(|r| owner || (r.is_master && !r.is_cashier))
        .map(|r| DayRow {
            employee_id: r.employee_id,
            full_name: r.full_name,
            opening_tyiyn: r.opening,
            accrued_tyiyn: r.accrued,
            service_fee_tyiyn: r.service_fee,
            percent_tyiyn: r.percent,
            other_tyiyn: r.accrued - r.service_fee - r.percent,
            salary_tyiyn: r.salary,
            bonus_tyiyn: r.bonus,
            withheld_tyiyn: r.withheld,
            // База процента раскрывает себестоимость, поэтому она только владельцу (ADR-036).
            base_tyiyn: owner.then_some(r.base),
            paid_tyiyn: r.paid,
            advance_tyiyn: r.advance,
            balance_tyiyn: r.opening + r.accrued - r.paid,
        })
        .filter(|r| r.opening_tyiyn != 0 || r.accrued_tyiyn != 0 || r.paid_tyiyn != 0)
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
struct MonthQuery {
    /// Любой день месяца; по умолчанию — текущий месяц по Бишкеку.
    month: Option<NaiveDate>,
}

#[derive(Serialize)]
pub(crate) struct MonthRow {
    pub(crate) employee_id: Uuid,
    pub(crate) full_name: String,
    pub(crate) is_cashier: bool,
    pub(crate) is_master: bool,
    pub(crate) opening_tyiyn: i64,
    pub(crate) salary_tyiyn: i64,
    pub(crate) percent_tyiyn: i64,
    pub(crate) service_fee_tyiyn: i64,
    pub(crate) shift_fee_tyiyn: i64,
    pub(crate) bonus_tyiyn: i64,
    pub(crate) penalty_tyiyn: i64,
    pub(crate) shortage_tyiyn: i64,
    pub(crate) accrued_tyiyn: i64,
    /// Выдано за месяц, авансы тоже.
    pub(crate) paid_tyiyn: i64,
    /// В том числе авансом (ADR-122).
    pub(crate) advance_tyiyn: i64,
    pub(crate) closing_tyiyn: i64,
    /// Оклад за этот месяц уже начислен (повторно — 409).
    pub(crate) salary_done: bool,
    /// Ставка оклада по действующему правилу — подставляется в форму начисления.
    pub(crate) salary_rule_tyiyn: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct MonthOut {
    pub(crate) from: NaiveDate,
    pub(crate) to: NaiveDate,
    pub(crate) rows: Vec<MonthRow>,
}

/// Расчёт за месяц: остаток на начало, начисления по видам, выплаты, остаток на конец (SPEC-07).
async fn month(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<MonthQuery>,
    Query(scope): Query<crate::api::branches::BranchQuery>,
) -> AppResult<Json<serde_json::Value>> {
    use crate::api::branches;
    let mut conn = state.pool.acquire().await?;
    let month = q.month;
    Ok(Json(
        branches::report(&mut conn, &user, &scope, |c, u| {
            Box::pin(async move { branches::to_json(&month_data(c, u.branch_id, month).await?) })
        })
        .await?,
    ))
}

/// Расчёт за месяц, где `month` — любой день месяца: для экрана и для вопросов боту (SPEC-21).
pub(crate) async fn month_data(
    conn: &mut PgConnection,
    branch_id: Uuid,
    month: Option<NaiveDate>,
) -> AppResult<MonthOut> {
    let b = sqlx::query!(
        r#"select date_trunc('month', coalesce($1::date, (now() at time zone 'Asia/Bishkek')::date))::date as "from!",
                  (date_trunc('month', coalesce($1::date, (now() at time zone 'Asia/Bishkek')::date))
                   + interval '1 month - 1 day')::date as "to!""#,
        month
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"select e.id as "employee_id!", e.full_name as "full_name!", e.is_cashier as "is_cashier!",
             e.is_master as "is_master!",
             coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date < $2), 0)::bigint
             - coalesce((select sum(p.amount_tyiyn) from payouts p
                         where p.employee_id = e.id and (p.created_at at time zone 'Asia/Bishkek')::date < $2), 0)::bigint
               as "opening!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'monthly_salary'), 0)::bigint as "salary!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'revenue_percent'), 0)::bigint as "percent!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'service_fee'), 0)::bigint as "service_fee!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'shift_fee'), 0)::bigint as "shift_fee!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'bonus'), 0)::bigint as "bonus!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'penalty'), 0)::bigint as "penalty!",
             coalesce(sum(a.amount_tyiyn) filter (where a.kind = 'shortage'), 0)::bigint as "shortage!",
             coalesce(sum(a.amount_tyiyn), 0)::bigint as "accrued!",
             coalesce((select sum(p.amount_tyiyn) from payouts p
                       where p.employee_id = e.id
                         and (p.created_at at time zone 'Asia/Bishkek')::date between $2 and $3), 0)::bigint as "paid!",
             coalesce((select sum(p.amount_tyiyn) from payouts p
                       where p.employee_id = e.id and p.purpose = 'advance'
                         and (p.created_at at time zone 'Asia/Bishkek')::date between $2 and $3), 0)::bigint as "advance!",
             exists (select 1 from payroll_accruals s
                     where s.employee_id = e.id and s.kind = 'monthly_salary' and s.reversal_of is null
                       and s.business_date between $2 and $3
                       and not exists (select 1 from payroll_accruals r where r.reversal_of = s.id)) as "salary_done!",
             (select r.amount_tyiyn from employee_pay_rules r
               where r.employee_id = e.id and r.kind = 'monthly_salary'
                 and r.active_from <= $3 and (r.active_to is null or r.active_to > $3)
               order by r.created_at desc limit 1) as "salary_rule?"
           from employees e
           left join payroll_accruals a on a.employee_id = e.id and a.business_date between $2 and $3
           where e.branch_id = $1
           group by e.id, e.full_name, e.is_cashier, e.is_master, e.active
           -- Отключённый сотрудник виден, пока с ним есть расчёт: начисления или выплаты в месяце,
           -- долг на начало или на конец.
           having e.active or coalesce(sum(a.amount_tyiyn), 0) <> 0
               or exists (select 1 from payouts p
                          where p.employee_id = e.id
                            and (p.created_at at time zone 'Asia/Bishkek')::date between $2 and $3)
               or coalesce((select sum(x.amount_tyiyn) from payroll_accruals x
                            where x.employee_id = e.id and x.business_date <= $3), 0)
                  <> coalesce((select sum(p.amount_tyiyn) from payouts p
                               where p.employee_id = e.id
                                 and (p.created_at at time zone 'Asia/Bishkek')::date <= $3), 0)
               or coalesce((select sum(x.amount_tyiyn) from payroll_accruals x
                            where x.employee_id = e.id and x.business_date < $2), 0)
                  <> coalesce((select sum(p.amount_tyiyn) from payouts p
                               where p.employee_id = e.id
                                 and (p.created_at at time zone 'Asia/Bishkek')::date < $2), 0)
           order by e.full_name"#,
        branch_id,
        b.from,
        b.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let rows = rows
        .into_iter()
        .map(|r| MonthRow {
            employee_id: r.employee_id,
            full_name: r.full_name,
            is_cashier: r.is_cashier,
            is_master: r.is_master,
            opening_tyiyn: r.opening,
            salary_tyiyn: r.salary,
            percent_tyiyn: r.percent,
            service_fee_tyiyn: r.service_fee,
            shift_fee_tyiyn: r.shift_fee,
            bonus_tyiyn: r.bonus,
            penalty_tyiyn: r.penalty,
            shortage_tyiyn: r.shortage,
            accrued_tyiyn: r.accrued,
            paid_tyiyn: r.paid,
            advance_tyiyn: r.advance,
            closing_tyiyn: r.opening.saturating_add(r.accrued).saturating_sub(r.paid),
            salary_done: r.salary_done,
            salary_rule_tyiyn: r.salary_rule,
        })
        .collect();
    Ok(MonthOut {
        from: b.from,
        to: b.to,
        rows,
    })
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AccrualReq {
    pub op_id: Uuid,
    pub employee_id: Uuid,
    pub kind: String,
    pub amount_tyiyn: i64,
    pub business_date: Option<NaiveDate>,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OkOut {
    pub ok: bool,
    /// Проведённое начисление: премия, удержание, оклад или сторно.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Uuid>,
}

async fn post_accrual(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<AccrualReq>,
) -> AppResult<Json<OkOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_accrual_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Премия или удержание.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_accrual_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: AccrualReq,
) -> AppResult<OkOut> {
    const KIND: &str = "payroll.accrual";
    // Бонус, удержание и аванс отмечает только владелец (SPEC-07).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !matches!(req.kind.as_str(), "bonus" | "penalty") {
        return Err(invalid("вид: бонус или удержание"));
    }
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    lock_employee(&mut *conn, ctx.user.branch_id, req.employee_id).await?;
    let amount = if req.kind == "penalty" {
        -req.amount_tyiyn
    } else {
        req.amount_tyiyn
    };
    let accrual_id = add_accrual(
        &mut *conn,
        ctx,
        req.business_date,
        Accrual {
            employee_id: req.employee_id,
            kind: &req.kind,
            amount,
            base: None,
            rule_id: None,
            doc_type: "manual",
            doc_id: None,
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({ "kind": req.kind, "amount_tyiyn": amount, "comment": req.comment.trim() }),
    )
    .await?;
    let out = OkOut {
        ok: true,
        id: accrual_id,
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SalaryReq {
    pub op_id: Uuid,
    pub employee_id: Uuid,
    /// Первый день месяца, за который платим.
    pub month: NaiveDate,
    pub amount_tyiyn: Option<i64>,
}

async fn post_salary(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SalaryReq>,
) -> AppResult<Json<OkOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_salary_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Оклад за месяц.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_salary_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: SalaryReq,
) -> AppResult<OkOut> {
    const KIND: &str = "payroll.salary";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    lock_employee(&mut *conn, ctx.user.branch_id, req.employee_id).await?;
    // Оклад попадает в последний день месяца, но не в будущее (SPEC-07).
    let d = sqlx::query!(
        r#"select least((date_trunc('month', $1::date) + interval '1 month - 1 day')::date,
                        (now() at time zone 'Asia/Bishkek')::date) as "d!",
                  date_trunc('month', $1::date) > date_trunc('month', now() at time zone 'Asia/Bishkek')
                    as "future!""#,
        req.month
    )
    .fetch_one(&mut *conn)
    .await?;
    // Иначе оклад «за ноябрь» лёг бы в октябрь и закрыл бы начисление настоящего октябрьского.
    if d.future {
        return Err(invalid(
            "оклад за месяц, который ещё не начался, не начисляется",
        ));
    }
    let date = d.d;
    let rule = active_rule(
        &mut *conn,
        ctx.user.branch_id,
        req.employee_id,
        "monthly_salary",
        date,
    )
    .await?;
    let amount = req
        .amount_tyiyn
        .or(rule.as_ref().and_then(|r| r.1))
        .ok_or_else(|| invalid("укажите сумму оклада"))?;
    if amount <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    let exists = sqlx::query_scalar!(
        r#"select count(*) as "n!" from payroll_accruals a
           where a.employee_id = $1 and a.kind = 'monthly_salary' and a.reversal_of is null
             and date_trunc('month', a.business_date) = date_trunc('month', $2::date)
             -- сторнированный оклад можно начислить заново
             and not exists (select 1 from payroll_accruals r where r.reversal_of = a.id)"#,
        req.employee_id,
        req.month
    )
    .fetch_one(&mut *conn)
    .await?;
    if exists > 0 {
        return Err(AppError::Conflict(
            "оклад за этот месяц уже начислен".into(),
        ));
    }
    let accrual_id = add_accrual(
        &mut *conn,
        ctx,
        Some(date),
        Accrual {
            employee_id: req.employee_id,
            kind: "monthly_salary",
            amount,
            base: None,
            rule_id: rule.map(|r| r.0),
            doc_type: "salary",
            doc_id: None,
            comment: "",
        },
    )
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({ "amount_tyiyn": amount, "month": req.month }),
    )
    .await?;
    let out = OkOut {
        ok: true,
        id: accrual_id,
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

// ---------- Выплаты по видам (SPEC-36 §11, ADR-122) ----------

/// Виды выплаты в строках окна «Выплата», в порядке показа. `general` — прежние выплаты без вида
/// и выплата одной суммой.
pub const PAYOUT_PURPOSES: [&str; 5] = ["percent", "service_fee", "bonus", "salary", "advance"];

/// Как вид называется в ответах сервера и в боте.
pub fn purpose_label(purpose: &str) -> &'static str {
    match purpose {
        "percent" => "проценты",
        "service_fee" => "мастеру за услуги",
        "bonus" => "бонус",
        "salary" => "оклад",
        "advance" => "аванс",
        _ => "выплата",
    }
}

/// Начислено и выдано сотруднику по видам — всё, что нужно для остатков.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayTotals {
    /// Процент с чеков (`revenue_percent`).
    pub percent: i64,
    pub service_fee: i64,
    pub bonus: i64,
    /// Оклад за месяц и за смену.
    pub salary: i64,
    /// Удержания и недостача — с минусом.
    pub withheld: i64,
    pub paid_general: i64,
    pub paid_percent: i64,
    pub paid_service_fee: i64,
    pub paid_bonus: i64,
    pub paid_salary: i64,
    pub paid_advance: i64,
}

/// Остаток к выдаче по видам. Сумма всех полей — прежний баланс «начислено − выдано».
#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Remains {
    pub percent_tyiyn: i64,
    pub service_fee_tyiyn: i64,
    pub bonus_tyiyn: i64,
    /// Оклад за месяц и за смену минус авансы — с минусом, пока аванс не закрыт окладом.
    pub salary_tyiyn: i64,
    /// Удержание, которое ещё не из чего вычесть (с минусом).
    pub withheld_tyiyn: i64,
}

impl Remains {
    pub fn total(&self) -> i64 {
        self.percent_tyiyn
            .saturating_add(self.service_fee_tyiyn)
            .saturating_add(self.bonus_tyiyn)
            .saturating_add(self.salary_tyiyn)
            .saturating_add(self.withheld_tyiyn)
    }

    /// Остаток вида выплаты; аванс берётся из оклада или, у мастера без оклада, из услуг.
    pub fn bucket(&mut self, purpose: &str, advance_to_salary: bool) -> Option<&mut i64> {
        match purpose {
            "percent" => Some(&mut self.percent_tyiyn),
            "service_fee" => Some(&mut self.service_fee_tyiyn),
            "bonus" => Some(&mut self.bonus_tyiyn),
            "salary" => Some(&mut self.salary_tyiyn),
            "advance" if advance_to_salary => Some(&mut self.salary_tyiyn),
            "advance" => Some(&mut self.service_fee_tyiyn),
            _ => None,
        }
    }
}

/// Остатки по видам (ADR-122):
/// - вид = начислено по виду − выдано этим видом; аванс уменьшает оклад (у мастера без оклада —
///   услуги), проценты он не трогает;
/// - удержания и недостача вычитаются из положительных остатков по порядку
///   проценты → мастеру → бонусы → оклад; что не из чего вычесть — остаётся «удержано»;
/// - прежние выплаты без вида гасят положительные остатки в том же порядке, лишнее — из оклада.
pub fn split_remains(t: &PayTotals, advance_to_salary: bool) -> Remains {
    let mut b = [
        t.percent.saturating_sub(t.paid_percent),
        t.service_fee.saturating_sub(t.paid_service_fee),
        t.bonus.saturating_sub(t.paid_bonus),
        t.salary.saturating_sub(t.paid_salary),
    ];
    let target = if advance_to_salary { 3 } else { 1 };
    b[target] = b[target].saturating_sub(t.paid_advance);
    // Удержание; отменённое сверх удержанного (плюс) — к окладу.
    let mut withheld = t.withheld;
    if withheld > 0 {
        b[3] = b[3].saturating_add(withheld);
        withheld = 0;
    }
    for x in &mut b {
        if withheld < 0 && *x > 0 {
            let take = (*x).min(withheld.saturating_neg());
            *x -= take;
            withheld += take;
        }
    }
    // Выплаты без вида; отрицательная сумма (сторно сверх выплат) — обратно в оклад.
    let mut general = t.paid_general;
    if general < 0 {
        b[3] = b[3].saturating_sub(general);
        general = 0;
    }
    for x in &mut b {
        if general > 0 && *x > 0 {
            let take = (*x).min(general);
            *x -= take;
            general -= take;
        }
    }
    b[3] = b[3].saturating_sub(general);
    Remains {
        percent_tyiyn: b[0],
        service_fee_tyiyn: b[1],
        bonus_tyiyn: b[2],
        salary_tyiyn: b[3],
        withheld_tyiyn: withheld,
    }
}

/// Остатки сотрудников филиала по видам на конец дня `upto` (без дня — сейчас).
/// `employee` — только один сотрудник. Третье поле — аванс в счёт оклада (иначе — услуг).
pub(crate) async fn remains_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    employee: Option<Uuid>,
    upto: Option<NaiveDate>,
) -> AppResult<Vec<(Uuid, Remains, bool)>> {
    let rows = sqlx::query!(
        r#"select e.id as "employee_id!",
                  (e.is_cashier or exists (
                      select 1 from employee_pay_rules r
                      where r.employee_id = e.id and r.kind = 'monthly_salary'
                        and (r.active_to is null or r.active_to > (now() at time zone 'Asia/Bishkek')::date)
                  )) as "to_salary!",
                  a.percent as "percent!", a.service_fee as "service_fee!", a.bonus as "bonus!",
                  a.salary as "salary!", a.withheld as "withheld!",
                  p.general as "paid_general!", p.percent as "paid_percent!",
                  p.service_fee as "paid_service_fee!", p.bonus as "paid_bonus!",
                  p.salary as "paid_salary!", p.advance as "paid_advance!"
           from employees e
           cross join lateral (
               select coalesce(sum(amount_tyiyn) filter (where kind = 'revenue_percent'), 0)::bigint as percent,
                      coalesce(sum(amount_tyiyn) filter (where kind = 'service_fee'), 0)::bigint as service_fee,
                      coalesce(sum(amount_tyiyn) filter (where kind = 'bonus'), 0)::bigint as bonus,
                      coalesce(sum(amount_tyiyn) filter (where kind in ('monthly_salary', 'shift_fee')), 0)::bigint as salary,
                      coalesce(sum(amount_tyiyn) filter (where kind in ('penalty', 'shortage')), 0)::bigint as withheld
               from payroll_accruals
               where employee_id = e.id and ($3::date is null or business_date <= $3)
           ) a
           cross join lateral (
               select coalesce(sum(amount_tyiyn) filter (where purpose = 'general'), 0)::bigint as general,
                      coalesce(sum(amount_tyiyn) filter (where purpose = 'percent'), 0)::bigint as percent,
                      coalesce(sum(amount_tyiyn) filter (where purpose = 'service_fee'), 0)::bigint as service_fee,
                      coalesce(sum(amount_tyiyn) filter (where purpose = 'bonus'), 0)::bigint as bonus,
                      coalesce(sum(amount_tyiyn) filter (where purpose = 'salary'), 0)::bigint as salary,
                      coalesce(sum(amount_tyiyn) filter (where purpose = 'advance'), 0)::bigint as advance
               from payouts
               where employee_id = e.id
                 and ($3::date is null or (created_at at time zone 'Asia/Bishkek')::date <= $3)
           ) p
           where e.branch_id = $1 and ($2::uuid is null or e.id = $2)"#,
        branch_id,
        employee,
        upto
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let t = PayTotals {
                percent: r.percent,
                service_fee: r.service_fee,
                bonus: r.bonus,
                salary: r.salary,
                withheld: r.withheld,
                paid_general: r.paid_general,
                paid_percent: r.paid_percent,
                paid_service_fee: r.paid_service_fee,
                paid_bonus: r.paid_bonus,
                paid_salary: r.paid_salary,
                paid_advance: r.paid_advance,
            };
            (r.employee_id, split_remains(&t, r.to_salary), r.to_salary)
        })
        .collect())
}

#[derive(Deserialize)]
struct RemainsQuery {
    employee_id: Uuid,
}

#[derive(Serialize)]
struct RemainsOut {
    employee_id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
    remains: Remains,
    /// Сколько должны всего: сумма остатков по видам.
    balance_tyiyn: i64,
    /// Аванс в счёт чего: `salary` или `service_fee`.
    advance_to: &'static str,
}

/// Остатки сотрудника по видам сейчас — для окна «Выплата» (ADR-122). Администратор видит
/// только тех, кому может выплатить: мастеров не кассиров (SPEC-30 §6).
async fn remains(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<RemainsQuery>,
) -> AppResult<Json<RemainsOut>> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "payroll_pay").await?;
    let e = sqlx::query!(
        "select full_name, is_cashier, is_master from employees where id = $1 and branch_id = $2",
        q.employee_id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if !user.is_owner() && (e.is_cashier || !e.is_master) {
        return Err(AppError::ForbiddenWith(
            "Кассиру выплачивает владелец".into(),
        ));
    }
    let (_, r, to_salary) = remains_of(&mut conn, user.branch_id, Some(q.employee_id), None)
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    Ok(Json(RemainsOut {
        employee_id: q.employee_id,
        full_name: e.full_name,
        is_cashier: e.is_cashier,
        is_master: e.is_master,
        balance_tyiyn: r.total(),
        remains: r,
        advance_to: if to_salary { "salary" } else { "service_fee" },
    }))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PayoutReq {
    pub op_id: Uuid,
    pub employee_id: Uuid,
    /// Одна сумма; при строках по видам не нужна.
    #[serde(default)]
    pub amount_tyiyn: i64,
    pub source: Option<String>,
    pub account_id: Option<Uuid>,
    /// Одна сумма без вида — это аванс. С `purpose = general` — владелец выдаёт сверх
    /// заработанного (исправление прежней выплаты).
    #[serde(default)]
    pub advance: bool,
    #[serde(default)]
    pub comment: String,
}

/// Строка выплаты: вид и сумма.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct PayoutLine {
    pub purpose: String,
    pub amount_tyiyn: i64,
}

/// Выплата по видам (ADR-122): строки `lines` одной операцией; без строк — одна сумма
/// `amount_tyiyn` видом `purpose` (по умолчанию `general`, с `advance` — аванс).
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PayoutsReq {
    #[serde(flatten)]
    pub base: PayoutReq,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<PayoutLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

impl From<PayoutReq> for PayoutsReq {
    fn from(base: PayoutReq) -> Self {
        Self {
            base,
            lines: Vec::new(),
            purpose: None,
        }
    }
}

/// Проведённая строка выплаты.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PayoutLineOut {
    pub id: Uuid,
    pub number: i64,
    pub purpose: String,
    pub amount_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PayoutOut {
    /// Первая строка выплаты.
    pub id: Uuid,
    pub number: i64,
    pub balance_tyiyn: i64,
    /// Все строки по видам; у прежних ответов — пусто.
    #[serde(default)]
    pub lines: Vec<PayoutLineOut>,
}

async fn post_payout(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<PayoutsReq>,
) -> AppResult<Json<PayoutOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_payouts_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Строк в одной выплате — не больше, чем видов.
const MAX_PAYOUT_LINES: usize = PAYOUT_PURPOSES.len();

/// Выплата или аванс сотруднику по видам (ADR-122): по записи на каждый вид, одна операция.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_payouts_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: PayoutsReq,
) -> AppResult<PayoutOut> {
    const KIND: &str = "payout.post";
    let PayoutsReq {
        base: req,
        lines,
        purpose,
    } = req;
    let mut lines = if lines.is_empty() {
        let purpose =
            purpose.unwrap_or_else(|| if req.advance { "advance" } else { "general" }.to_string());
        vec![PayoutLine {
            purpose,
            amount_tyiyn: req.amount_tyiyn,
        }]
    } else {
        if purpose.is_some() {
            return Err(invalid("вид — в каждой строке"));
        }
        if lines.len() > MAX_PAYOUT_LINES {
            return Err(invalid("строк больше, чем видов выплаты"));
        }
        lines
    };
    let single = lines.len() == 1;
    let mut seen = std::collections::HashSet::new();
    for l in &lines {
        let known = PAYOUT_PURPOSES.contains(&l.purpose.as_str());
        if !(known || single && l.purpose == "general") {
            return Err(invalid(
                "вид выплаты: проценты, мастеру, бонус, оклад или аванс",
            ));
        }
        if !seen.insert(l.purpose.clone()) {
            return Err(invalid("каждый вид — одной строкой"));
        }
        if l.amount_tyiyn <= 0 {
            return Err(invalid("сумма больше нуля"));
        }
        crate::ops::check_amount(l.amount_tyiyn)?;
    }
    let total = lines
        .iter()
        .try_fold(0i64, |a, l| a.checked_add(l.amount_tyiyn))
        .ok_or_else(crate::error::overflow)?;
    crate::ops::check_amount(total)?;
    if !single && req.amount_tyiyn != 0 && req.amount_tyiyn != total {
        return Err(invalid("сумма не равна сумме строк"));
    }
    // Аванс проверяется последним: сначала выдаётся заработанное.
    lines.sort_by_key(|l| {
        PAYOUT_PURPOSES
            .iter()
            .position(|p| *p == l.purpose)
            .unwrap_or(0)
    });
    let source = req.source.unwrap_or_else(|| "account".into());
    if source == "outside" {
        return Err(invalid(crate::api::receipts::NO_OUTSIDE));
    }
    if source != "account" {
        return Err(invalid("откуда платим: из кассы или со счёта"));
    }
    let owner = ctx.user.is_owner();
    let branch_id = ctx.user.branch_id;
    // Выплаты — право администратора `payroll_pay`, аванс — ещё и `payroll_advance` (ADR-097, ADR-122).
    crate::auth::require_permission(&mut *conn, &ctx.user, "payroll_pay").await?;
    if lines.iter().any(|l| l.purpose == "advance") {
        crate::auth::require_permission(&mut *conn, &ctx.user, "payroll_advance").await?;
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    lock_employee(&mut *conn, branch_id, req.employee_id).await?;
    // Администратор выдаёт деньги только мастерам — за услуги и, с правом, аванс; кассиру
    // и остальным — владелец (SPEC-30 §6, ADR-122).
    if !owner {
        let e = sqlx::query!(
            "select is_master, is_cashier from employees where id = $1",
            req.employee_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if e.is_cashier {
            return Err(AppError::ForbiddenWith(
                "Кассиру выплачивает владелец".into(),
            ));
        }
        if !e.is_master {
            return Err(AppError::ForbiddenWith(
                "Выплачивает владелец: администратор выдаёт деньги только мастерам".into(),
            ));
        }
        if let Some(l) = lines
            .iter()
            .find(|l| !matches!(l.purpose.as_str(), "general" | "service_fee" | "advance"))
        {
            return Err(AppError::ForbiddenWith(format!(
                "Выплачивает владелец: {} — администратор выдаёт мастеру только за услуги и аванс",
                purpose_label(&l.purpose)
            )));
        }
    }
    let (_, mut left, to_salary) = remains_of(&mut *conn, branch_id, Some(req.employee_id), None)
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    let balance = left.total();
    for l in &lines {
        if l.purpose == "general" {
            // Одна сумма без вида — в пределах всего долга; сверх — аванс владельца.
            if l.amount_tyiyn > balance && (!req.advance || !owner) {
                return Err(invalid(format!(
                    "заработано {}: аванс сверх этого отмечает владелец",
                    crate::domain::money::format_som(balance)
                )));
            }
            continue;
        }
        let Some(bucket) = left.bucket(&l.purpose, to_salary) else {
            continue;
        };
        // Аванс владельца — сверх заработанного; остальное — не больше остатка своего вида.
        if l.amount_tyiyn > *bucket && !(owner && l.purpose == "advance") {
            return Err(invalid(format!(
                "{}: к выдаче {}, а в строке {}",
                purpose_label(&l.purpose),
                crate::domain::money::format_som((*bucket).max(0)),
                crate::domain::money::format_som(l.amount_tyiyn)
            )));
        }
        *bucket = bucket.saturating_sub(l.amount_tyiyn);
    }
    let id = match req.account_id {
        Some(id) => id,
        None => cash::default_account(&mut *conn, branch_id).await?,
    };
    // Администратор платит только из кассы смены: счета и сейфы — владельцу; архивный
    // счёт не выбирается (ADR-037, ADR-076).
    cash::check_pay_account(&mut *conn, &ctx.user, id).await?;
    let mut out_lines = Vec::with_capacity(lines.len());
    for l in &lines {
        let number = ops::next_counter(&mut *conn, branch_id, "payout").await?;
        let pid = new_id();
        sqlx::query!(
            r#"insert into payouts (id, branch_id, number, employee_id, amount_tyiyn, source,
                                    account_id, comment, user_id, device_id, purpose, op_id)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
            pid,
            branch_id,
            number,
            req.employee_id,
            l.amount_tyiyn,
            source,
            id,
            req.comment.trim(),
            ctx.user.id,
            ctx.device_id,
            l.purpose,
            req.op_id
        )
        .execute(&mut *conn)
        .await?;
        out_lines.push(PayoutLineOut {
            id: pid,
            number,
            purpose: l.purpose.clone(),
            amount_tyiyn: l.amount_tyiyn,
        });
    }
    // Из кассы — только в открытую смену (SPEC-07); по движению на каждую строку.
    cash::require_open_shift(&mut *conn, branch_id, id).await?;
    for l in &out_lines {
        cash::add_movement(
            &mut *conn,
            ctx,
            CashEntry {
                account_id: id,
                kind: "payout",
                amount: -l.amount_tyiyn,
                doc_type: "payout",
                doc_id: Some(l.id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    let advance = lines
        .iter()
        .any(|l| l.purpose == "advance" || l.purpose == "general" && l.amount_tyiyn > balance);
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({
            "amount_tyiyn": total,
            "advance": advance,
            // Сколько выдано сверх всего заработанного — для ленты владельца.
            "over_tyiyn": total.saturating_sub(balance.max(0)).max(0),
            "account_id": id,
            "lines": lines,
        }),
    )
    .await?;
    let first = out_lines
        .first()
        .ok_or_else(|| AppError::Internal("выплата без строк".into()))?;
    let out = PayoutOut {
        id: first.id,
        number: first.number,
        balance_tyiyn: balance
            .checked_sub(total)
            .ok_or_else(crate::error::overflow)?,
        lines: out_lines,
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Serialize)]
pub struct HistoryRow {
    pub at: DateTime<Utc>,
    pub kind: String,
    pub amount_tyiyn: i64,
    pub comment: String,
}

/// Строка «кто сколько заработал»: вид начисления, сколько раз и на какую сумму.
#[derive(Serialize, Deserialize, Clone)]
pub struct PayItem {
    pub kind: String,
    pub count: i64,
    pub amount_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct StaffPay {
    pub employee_id: Uuid,
    pub name: String,
    pub items: Vec<PayItem>,
    pub total_tyiyn: i64,
    /// Выдано за те же дни.
    #[serde(default)]
    pub paid_tyiyn: i64,
    /// Сколько ему должны сейчас: всё начисленное минус всё выданное.
    #[serde(default)]
    pub owed_tyiyn: i64,
}

/// Начисления сотрудникам за дни учёта — для сводки владельца и отчёта смены.
pub async fn staff_pay(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<StaffPay>> {
    staff_pay_with(conn, branch_id, from, to, &mut HashMap::new()).await
}

/// То же, что `staff_pay`; `owed` — уже посчитанный долг по работникам (он не зависит от
/// периода): список смен считает долг каждого работника один раз, а не в каждой смене.
pub async fn staff_pay_with(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
    owed: &mut HashMap<Uuid, i64>,
) -> AppResult<Vec<StaffPay>> {
    let rows = sqlx::query!(
        r#"select e.id, e.full_name, a.kind,
                  (count(*) filter (where a.amount_tyiyn > 0) - count(*) filter (where a.amount_tyiyn < 0)) as "count!",
                  sum(a.amount_tyiyn)::bigint as "sum!"
           from payroll_accruals a join employees e on e.id = a.employee_id
           where a.branch_id = $1 and a.business_date between $2 and $3
           group by e.id, e.full_name, a.kind
           having sum(a.amount_tyiyn) <> 0
           order by e.full_name, a.kind"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<StaffPay> = Vec::new();
    for r in rows {
        let item = PayItem {
            kind: r.kind,
            count: r.count,
            amount_tyiyn: r.sum,
        };
        match out.iter_mut().find(|x| x.employee_id == r.id) {
            Some(x) => {
                x.total_tyiyn = x.total_tyiyn.saturating_add(item.amount_tyiyn);
                x.items.push(item);
            }
            None => out.push(StaffPay {
                employee_id: r.id,
                name: r.full_name,
                total_tyiyn: item.amount_tyiyn,
                items: vec![item],
                paid_tyiyn: 0,
                owed_tyiyn: 0,
            }),
        }
    }
    for p in &mut out {
        p.paid_tyiyn = sqlx::query_scalar!(
            r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "paid!" from payouts
               where employee_id = $1
                 and (created_at at time zone 'Asia/Bishkek')::date between $2 and $3"#,
            p.employee_id,
            from,
            to
        )
        .fetch_one(&mut *conn)
        .await?;
        p.owed_tyiyn = match owed.get(&p.employee_id) {
            Some(v) => *v,
            None => {
                let v = sqlx::query_scalar!(
                    r#"select (coalesce((select sum(amount_tyiyn) from payroll_accruals where employee_id = $1), 0)
                              - coalesce((select sum(amount_tyiyn) from payouts where employee_id = $1), 0))::bigint as "owed!""#,
                    p.employee_id
                )
                .fetch_one(&mut *conn)
                .await?;
                owed.insert(p.employee_id, v);
                v
            }
        };
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{PayTotals, Remains, split_remains};

    #[test]
    fn remains_sum_to_the_balance() {
        // Сумма остатков по видам — прежний баланс «начислено − выдано» (ADR-122).
        let t = PayTotals {
            percent: 50_000,
            service_fee: 3_000,
            bonus: 100_000,
            salary: 3_000_000,
            withheld: -20_000,
            paid_general: 70_000,
            paid_percent: 0,
            paid_service_fee: 0,
            paid_bonus: 10_000,
            paid_salary: 0,
            paid_advance: 1_000_000,
        };
        let r = split_remains(&t, true);
        let accrued = 50_000 + 3_000 + 100_000 + 3_000_000 - 20_000;
        let paid = 70_000 + 10_000 + 1_000_000;
        assert_eq!(r.total(), accrued - paid);
        // Удержание — из процентов; выплата без вида — из остатка процентов, мастеру, бонуса.
        assert_eq!(
            r,
            Remains {
                percent_tyiyn: 0,
                service_fee_tyiyn: 0,
                bonus_tyiyn: 53_000,
                salary_tyiyn: 2_000_000,
                withheld_tyiyn: 0,
            }
        );
    }

    #[test]
    fn advance_of_master_without_salary_counts_against_services() {
        let t = PayTotals {
            service_fee: 3_000,
            paid_advance: 5_000,
            ..PayTotals::default()
        };
        let r = split_remains(&t, false);
        assert_eq!(r.service_fee_tyiyn, -2_000);
        assert_eq!(r.salary_tyiyn, 0);
    }

    #[test]
    fn general_over_everything_goes_to_salary() {
        // Прежний аванс сверх заработанного одной суммой — минус в окладе.
        let t = PayTotals {
            percent: 1_000,
            paid_general: 4_000,
            ..PayTotals::default()
        };
        let r = split_remains(&t, true);
        assert_eq!((r.percent_tyiyn, r.salary_tyiyn), (0, -3_000));
        // Удержать не из чего — остаётся «удержано».
        let t = PayTotals {
            withheld: -500,
            ..PayTotals::default()
        };
        assert_eq!(split_remains(&t, true).withheld_tyiyn, -500);
    }
}
