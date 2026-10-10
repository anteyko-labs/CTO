//! Оплата сотрудников: правила, начисления, выплаты (SPEC-07).

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
        .route("/payroll/accruals", routing::post(post_accrual))
        .route("/payroll/salary", routing::post(post_salary))
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
struct ReverseReq {
    op_id: Uuid,
    #[serde(default)]
    comment: String,
}

/// Сторно начисления владельцем: удержание недостачи, ошибочный бонус или оклад (SPEC-07).
/// Начисления из чека снимаются только возвратом чека.
async fn reverse_accrual(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<OkOut>> {
    const KIND: &str = "payroll.accrual_reverse";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
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
    lock_employee(&mut tx, ctx.user.branch_id, a.employee_id).await?;
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
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(a.employee_id),
        json!({ "accrual": id, "kind": a.kind, "amount_tyiyn": -a.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = OkOut { ok: true };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно выплаты владельцем: деньги возвращаются в ту кассу, откуда ушли.
async fn reverse_payout(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<PayoutOut>> {
    const KIND: &str = "payout.reverse";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let p = sqlx::query!(
        r#"select employee_id, amount_tyiyn, source, account_id, reversal_of
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
    lock_employee(&mut tx, branch_id, p.employee_id).await?;
    let number = ops::next_counter(&mut tx, branch_id, "payout").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into payouts (id, branch_id, number, employee_id, amount_tyiyn, source,
                                account_id, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
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
        ctx.device_id
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
        cash::require_open_shift(&mut tx, branch_id, acc).await?;
        cash::add_movement(
            &mut tx,
            &ctx,
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
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(p.employee_id),
        json!({ "payout": id, "amount_tyiyn": p.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = PayoutOut {
        id: rid,
        number,
        balance_tyiyn: balance,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Правила оплаты ----------

#[derive(Serialize)]
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
    if !matches!(
        req.kind.as_str(),
        "per_service" | "revenue_percent" | "per_shift" | "monthly_salary"
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
    let rule_id = new_id();
    let mut tx = state.pool.begin().await?;
    // Под блокировкой сотрудника два одновременных правила не останутся открытыми оба.
    lock_employee(&mut tx, ctx.user.branch_id, id).await?;
    // Прежнее правило того же вида закрывается сегодняшним днём: история начислений не меняется.
    sqlx::query!(
        r#"update employee_pay_rules
           set active_to = (now() at time zone 'Asia/Bishkek')::date
           where branch_id = $1 and employee_id = $2 and kind = $3 and active_to is null"#,
        ctx.user.branch_id,
        id,
        req.kind
    )
    .execute(&mut *tx)
    .await?;
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
        "payroll.rule",
        "employee",
        Some(id),
        json!({ "kind": req.kind, "amount_tyiyn": req.amount_tyiyn, "rate_bp": req.rate_bp }),
    )
    .await?;
    tx.commit().await?;
    let rows = list_rules(State(state), ctx.user.clone(), Path(id)).await?;
    rows.0
        .into_iter()
        .find(|r| r.id == rule_id)
        .map(Json)
        .ok_or(AppError::NotFound)
}

async fn close_rule(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, rule_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let done = sqlx::query!(
        r#"update employee_pay_rules
           set active_to = (now() at time zone 'Asia/Bishkek')::date
           where id = $1 and employee_id = $2 and branch_id = $3 and active_to is null
           returning id"#,
        rule_id,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?;
    if done.is_none() {
        return Err(AppError::NotFound);
    }
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
) -> AppResult<()> {
    if a.amount == 0 {
        return Ok(());
    }
    ops::check_amount(a.amount)?;
    sqlx::query!(
        r#"insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn,
                                         base_tyiyn, rule_id, doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, coalesce($4, (now() at time zone 'Asia/Bishkek')::date), $5, $6,
                   $7, $8, $9, $10, $11, $12, $13)"#,
        new_id(),
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
    Ok(())
}

/// Оплата кассира по умолчанию (SPEC-07, ADR-019, ADR-036): 2 % с валовой прибыли чека
/// и оклад 30 000 сом в месяц. Заводится, только если таких правил у него ещё нет;
/// дальше владелец правит их в карточке сотрудника.
pub const DEFAULT_CASHIER_PERCENT_BP: i32 = 200;
pub const DEFAULT_CASHIER_SALARY_TYIYN: i64 = 3_000_000;

pub async fn ensure_cashier_rules(
    conn: &mut PgConnection,
    ctx: &Ctx,
    employee_id: Uuid,
) -> AppResult<()> {
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
    Ok(())
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
    } = s;
    let Some((rule_id, _, rate)) = active_rule(
        conn,
        ctx.user.branch_id,
        cashier_id,
        "revenue_percent",
        business_date,
    )
    .await?
    else {
        return Ok(());
    };
    let rate = i64::from(rate.unwrap_or(0));
    if rate <= 0 || gross == 0 || total == 0 {
        return Ok(());
    }
    let paid = total - debt;
    let now_base = div_round(i128::from(gross) * i128::from(paid), i128::from(total)).unwrap_or(0);
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
    if debt > 0 {
        let pending_base = gross - now_base;
        if let Some(pid) = party_id {
            sqlx::query!(
                r#"insert into payroll_pending (id, branch_id, party_id, sale_id, employee_id, rule_id,
                                                rate_bp, gross_tyiyn, debt_total_tyiyn, debt_remaining_tyiyn)
                   values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)"#,
                new_id(),
                ctx.user.branch_id,
                pid,
                sale_id,
                cashier_id,
                rule_id,
                i32::try_from(rate).unwrap_or(0),
                pending_base,
                debt
            )
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(())
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
    pub full_return: bool,
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
    // процент ещё не получил — она снимается с ожидания ниже.
    let paid_back = r.total_back.saturating_sub(r.debt_back).max(0);
    let base_back = if r.total_back > 0 && r.gross_back > 0 {
        div_round(
            i128::from(r.gross_back) * i128::from(paid_back),
            i128::from(r.total_back),
        )
        .unwrap_or(0)
    } else {
        0
    };
    for row in rows {
        if row.net <= 0 {
            continue;
        }
        let back = match row.kind.as_str() {
            // Работу мастер сделал: ставка снимается только при возврате чека целиком (ADR-027).
            "service_fee" if r.full_return => row.net,
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
        amount -= take;
    }
    Ok(())
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
    other_tyiyn: i64,
    base_tyiyn: Option<i64>,
    paid_tyiyn: i64,
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
             coalesce((select sum(a.base_tyiyn) from payroll_accruals a
                       where a.employee_id = e.id and a.business_date = $2 and a.kind = 'revenue_percent'), 0)::bigint as "base!",
             coalesce((select sum(p.amount_tyiyn) from payouts p
                       where p.employee_id = e.id and (p.created_at at time zone 'Asia/Bishkek')::date = $2), 0)::bigint as "paid!"
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
        .map(|r| DayRow {
            employee_id: r.employee_id,
            full_name: r.full_name,
            opening_tyiyn: r.opening,
            accrued_tyiyn: r.accrued,
            service_fee_tyiyn: r.service_fee,
            percent_tyiyn: r.percent,
            other_tyiyn: r.accrued - r.service_fee - r.percent,
            // База процента раскрывает себестоимость, поэтому она только владельцу (ADR-036).
            base_tyiyn: owner.then_some(r.base),
            paid_tyiyn: r.paid,
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
struct MonthRow {
    employee_id: Uuid,
    full_name: String,
    is_cashier: bool,
    is_master: bool,
    opening_tyiyn: i64,
    salary_tyiyn: i64,
    percent_tyiyn: i64,
    service_fee_tyiyn: i64,
    shift_fee_tyiyn: i64,
    bonus_tyiyn: i64,
    penalty_tyiyn: i64,
    shortage_tyiyn: i64,
    accrued_tyiyn: i64,
    paid_tyiyn: i64,
    closing_tyiyn: i64,
    /// Оклад за этот месяц уже начислен (повторно — 409).
    salary_done: bool,
    /// Ставка оклада по действующему правилу — подставляется в форму начисления.
    salary_rule_tyiyn: Option<i64>,
}

#[derive(Serialize)]
struct MonthOut {
    from: NaiveDate,
    to: NaiveDate,
    rows: Vec<MonthRow>,
}

/// Расчёт за месяц: остаток на начало, начисления по видам, выплаты, остаток на конец (SPEC-07).
async fn month(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<MonthQuery>,
) -> AppResult<Json<MonthOut>> {
    let mut conn = state.pool.acquire().await?;
    let b = sqlx::query!(
        r#"select date_trunc('month', coalesce($1::date, (now() at time zone 'Asia/Bishkek')::date))::date as "from!",
                  (date_trunc('month', coalesce($1::date, (now() at time zone 'Asia/Bishkek')::date))
                   + interval '1 month - 1 day')::date as "to!""#,
        q.month
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
           having e.active or coalesce(sum(a.amount_tyiyn), 0) <> 0
           order by e.full_name"#,
        user.branch_id,
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
            closing_tyiyn: r.opening.saturating_add(r.accrued).saturating_sub(r.paid),
            salary_done: r.salary_done,
            salary_rule_tyiyn: r.salary_rule,
        })
        .collect();
    Ok(Json(MonthOut {
        from: b.from,
        to: b.to,
        rows,
    }))
}

#[derive(Deserialize)]
struct AccrualReq {
    op_id: Uuid,
    employee_id: Uuid,
    kind: String,
    amount_tyiyn: i64,
    business_date: Option<NaiveDate>,
    comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OkOut {
    pub ok: bool,
}

async fn post_accrual(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<AccrualReq>,
) -> AppResult<Json<OkOut>> {
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
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_employee(&mut tx, ctx.user.branch_id, req.employee_id).await?;
    let amount = if req.kind == "penalty" {
        -req.amount_tyiyn
    } else {
        req.amount_tyiyn
    };
    add_accrual(
        &mut tx,
        &ctx,
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
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({ "kind": req.kind, "amount_tyiyn": amount, "comment": req.comment.trim() }),
    )
    .await?;
    let out = OkOut { ok: true };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct SalaryReq {
    op_id: Uuid,
    employee_id: Uuid,
    /// Первый день месяца, за который платим.
    month: NaiveDate,
    amount_tyiyn: Option<i64>,
}

async fn post_salary(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SalaryReq>,
) -> AppResult<Json<OkOut>> {
    const KIND: &str = "payroll.salary";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_employee(&mut tx, ctx.user.branch_id, req.employee_id).await?;
    // Оклад попадает в последний день месяца, но не в будущее (SPEC-07).
    let d = sqlx::query!(
        r#"select least((date_trunc('month', $1::date) + interval '1 month - 1 day')::date,
                        (now() at time zone 'Asia/Bishkek')::date) as "d!",
                  date_trunc('month', $1::date) > date_trunc('month', now() at time zone 'Asia/Bishkek')
                    as "future!""#,
        req.month
    )
    .fetch_one(&mut *tx)
    .await?;
    // Иначе оклад «за ноябрь» лёг бы в октябрь и закрыл бы начисление настоящего октябрьского.
    if d.future {
        return Err(invalid(
            "оклад за месяц, который ещё не начался, не начисляется",
        ));
    }
    let date = d.d;
    let rule = active_rule(
        &mut tx,
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
    .fetch_one(&mut *tx)
    .await?;
    if exists > 0 {
        return Err(AppError::Conflict(
            "оклад за этот месяц уже начислен".into(),
        ));
    }
    add_accrual(
        &mut tx,
        &ctx,
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
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({ "amount_tyiyn": amount, "month": req.month }),
    )
    .await?;
    let out = OkOut { ok: true };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct PayoutReq {
    op_id: Uuid,
    employee_id: Uuid,
    amount_tyiyn: i64,
    source: Option<String>,
    account_id: Option<Uuid>,
    #[serde(default)]
    advance: bool,
    #[serde(default)]
    comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PayoutOut {
    pub id: Uuid,
    pub number: i64,
    pub balance_tyiyn: i64,
}

async fn post_payout(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<PayoutReq>,
) -> AppResult<Json<PayoutOut>> {
    const KIND: &str = "payout.post";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    let source = req.source.unwrap_or_else(|| "account".into());
    if !matches!(source.as_str(), "account" | "outside") {
        return Err(invalid("откуда платим: account или outside"));
    }
    let owner = ctx.user.is_owner();
    if source == "outside" && !owner {
        return Err(AppError::Forbidden);
    }
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    lock_employee(&mut tx, branch_id, req.employee_id).await?;
    let balance = sqlx::query_scalar!(
        r#"select coalesce((select sum(amount_tyiyn) from payroll_accruals where employee_id = $1), 0)::bigint
                - coalesce((select sum(amount_tyiyn) from payouts where employee_id = $1), 0)::bigint as "b!""#,
        req.employee_id
    )
    .fetch_one(&mut *tx)
    .await?;
    // Аванс сверх заработанного отмечает только владелец (ответ заказчика).
    if req.amount_tyiyn > balance && (!req.advance || !owner) {
        return Err(invalid(format!(
            "заработано {}: аванс сверх этого отмечает владелец",
            crate::domain::money::format_som(balance)
        )));
    }
    let mut account_id = None;
    if source == "account" {
        let id = match req.account_id {
            Some(id) => id,
            None => cash::default_account(&mut tx, branch_id).await?,
        };
        let acc = sqlx::query!(
            "select kind, owner_only from cash_accounts where id = $1 and branch_id = $2",
            id,
            branch_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("касса не найдена"))?;
        if !owner && (acc.kind != "register" || acc.owner_only) {
            return Err(AppError::Forbidden);
        }
        account_id = Some(id);
    }
    let number = ops::next_counter(&mut tx, branch_id, "payout").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into payouts (id, branch_id, number, employee_id, amount_tyiyn, source,
                                account_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        req.employee_id,
        req.amount_tyiyn,
        source,
        account_id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    if let Some(acc) = account_id {
        cash::require_open_shift(&mut tx, branch_id, acc).await?;
        cash::add_movement(
            &mut tx,
            &ctx,
            CashEntry {
                account_id: acc,
                kind: "payout",
                amount: -req.amount_tyiyn,
                doc_type: "payout",
                doc_id: Some(id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(req.employee_id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "advance": req.amount_tyiyn > balance }),
    )
    .await?;
    let out = PayoutOut {
        id,
        number,
        balance_tyiyn: balance
            .checked_sub(req.amount_tyiyn)
            .ok_or_else(crate::error::overflow)?,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
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
}

/// Начисления сотрудникам за дни учёта — для сводки владельца и отчёта смены.
pub async fn staff_pay(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
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
            }),
        }
    }
    Ok(out)
}
