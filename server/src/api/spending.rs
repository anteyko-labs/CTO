//! «Расходы» — все траты, кроме оплаты поставщикам за товар (задача 003 Н12б, ADR-{f2.1}):
//! расходы по статьям (SPEC-06) и выплаты работникам (SPEC-07) одним списком, начисленные бонусы —
//! отдельной строкой по настройке. Выплата не становится расходом (ADR-023): прибыль считает
//! зарплату по начислению, здесь — выданные деньги.
//!
//! Права (I12): расходы — по праву `expenses` (администратору — как в `GET /expenses`: из кассы,
//! за неделю, без статей владельца); выплаты и бонусы — по праву `view_payroll`, кассиры — с
//! `payroll_view_all`, как в «Расчёте» (ADR-151). Ни себестоимости, ни прибыли здесь нет (I13).

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::paging::{self, PageQuery, Paged};
use crate::auth::CurrentUser;
use crate::error::{AppResult, invalid};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/expenses/all", routing::get(list))
        .route("/expenses/all/summary", routing::get(summary))
}

/// Строка списка: расход, выплата работнику или начисленный бонус.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpendingRow {
    /// `expense`, `payout`, `bonus_accrual`.
    pub kind: String,
    pub id: Uuid,
    /// Номер документа; у начисления номера нет.
    pub number: Option<i64>,
    /// Дата расхода, дата выплаты в расчёте (`pay_date`) или день начисления.
    pub date: NaiveDate,
    pub amount_tyiyn: i64,
    /// Статья расхода; у выплаты и бонуса — «Зарплата работникам».
    pub article: String,
    /// Вид выплаты (`salary`, `advance`, `percent`, `service_fee`, `bonus`, `general`); у бонуса —
    /// `bonus`; у расхода — пусто.
    pub purpose: Option<String>,
    /// Кому: «Кому» расхода или работник.
    pub payee: String,
    pub employee_id: Option<Uuid>,
    /// За что: комментарий.
    pub comment: String,
    pub source: Option<String>,
    pub account_name: Option<String>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub reversal_of: Option<Uuid>,
    pub reversed: bool,
}

#[derive(Deserialize, Default)]
pub struct SpendingQuery {
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
    /// `all` (по умолчанию), `expenses`, `payouts`.
    pub kind: Option<String>,
    /// Показать сторно и сторнированное (по умолчанию пары скрыты).
    pub reversals: Option<String>,
}

/// Что входит в отбор для этого пользователя.
struct Scope {
    owner: bool,
    expenses: bool,
    payouts: bool,
    bonuses: bool,
    cashiers: bool,
    reversals: bool,
}

async fn scope(conn: &mut PgConnection, user: &CurrentUser, q: &SpendingQuery) -> AppResult<Scope> {
    let kind = q.kind.as_deref().unwrap_or("all");
    if !matches!(kind, "all" | "expenses" | "payouts") {
        return Err(invalid("отбор: всё, расходы или выплаты работникам"));
    }
    let may_expenses = crate::auth::has_permission(&mut *conn, user, "expenses").await?;
    let may_payroll = crate::auth::has_permission(&mut *conn, user, "view_payroll").await?;
    // Отказ — с подписью права, которого не хватает (tier2): сначала «Расходы».
    match kind {
        "expenses" if !may_expenses => {
            crate::auth::require_permission(&mut *conn, user, "expenses").await?
        }
        "payouts" if !may_payroll => {
            crate::auth::require_permission(&mut *conn, user, "view_payroll").await?
        }
        _ if !may_expenses && !may_payroll => {
            crate::auth::require_permission(&mut *conn, user, "expenses").await?
        }
        _ => {}
    }
    let expenses = may_expenses && kind != "payouts";
    let payouts = may_payroll && kind != "expenses";
    let bonuses = payouts
        && crate::api::pnl::profit_settings(&mut *conn, user.branch_id)
            .await?
            .expenses_bonus_accruals;
    let cashiers = crate::auth::has_permission(&mut *conn, user, "payroll_view_all").await?;
    if let (Some(f), Some(t)) = (q.from, q.to)
        && f > t
    {
        return Err(invalid("начало периода позже конца"));
    }
    Ok(Scope {
        owner: user.is_owner(),
        expenses,
        payouts,
        bonuses,
        cashiers,
        reversals: matches!(q.reversals.as_deref(), Some("1" | "true")),
    })
}

/// `GET /expenses/all` — одной выборкой, страницей; число строк отбора — в `x-total-count`.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SpendingQuery>,
    Query(page): Query<PageQuery>,
) -> AppResult<Paged<SpendingRow>> {
    let mut conn = state.pool.acquire().await?;
    let s = scope(&mut conn, &user, &q).await?;
    let (rows, total) = rows(&mut conn, &user, &q, &s, page).await?;
    Ok(paging::paged(rows, total))
}

async fn rows(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: &SpendingQuery,
    s: &Scope,
    page: PageQuery,
) -> AppResult<(Vec<SpendingRow>, i64)> {
    let limit = page.size(paging::MAX_PAGE);
    let offset = page.skip();
    let found = sqlx::query!(
        r#"with r as (
             select 'expense'::text as kind, e.id, e.number::bigint as number, e.expense_date as date,
                    e.amount_tyiyn, a.name as article, null::text as purpose, e.payee,
                    null::uuid as employee_id, e.comment, e.source::text as source,
                    c.name::text as account_name, u.full_name as user_name, e.created_at,
                    e.reversal_of,
                    exists (select 1 from expenses x where x.reversal_of = e.id) as reversed
             from expenses e
             join expense_articles a on a.id = e.article_id
             join users u on u.id = e.user_id
             left join cash_accounts c on c.id = e.account_id
             where $5 and e.branch_id = $1
               and ($2::date is null or e.expense_date >= $2)
               and ($3::date is null or e.expense_date <= $3)
               -- Администратору — как в «Расходах»: из кассы, за неделю, без статей владельца.
               and ($4 or (not a.owner_only and e.source = 'account'
                           and c.kind = 'register' and not c.owner_only
                           and e.expense_date >= (now() at time zone 'Asia/Bishkek')::date - 7))
             union all
             select 'payout', p.id, p.number, p.pay_date, p.amount_tyiyn, 'Зарплата работникам',
                    p.purpose, em.full_name, em.id, p.comment, p.source,
                    -- Счёт владельца администратору не называется.
                    case when $4 or not coalesce(c.owner_only, false) then c.name end,
                    u.full_name, p.created_at, p.reversal_of,
                    exists (select 1 from payouts x where x.reversal_of = p.id)
             from payouts p
             join employees em on em.id = p.employee_id
             join users u on u.id = p.user_id
             left join cash_accounts c on c.id = p.account_id
             where $6 and p.branch_id = $1
               and ($2::date is null or p.pay_date >= $2)
               and ($3::date is null or p.pay_date <= $3)
               and ($8 or (em.is_master and not em.is_cashier))
             union all
             select 'bonus_accrual', b.id, null, b.business_date, b.amount_tyiyn,
                    'Зарплата работникам', 'bonus', em.full_name, em.id, b.comment, null, null,
                    u.full_name, b.created_at, b.reversal_of,
                    exists (select 1 from payroll_accruals x where x.reversal_of = b.id)
             from payroll_accruals b
             join employees em on em.id = b.employee_id
             join users u on u.id = b.user_id
             where $7 and b.branch_id = $1 and b.kind = 'bonus'
               and ($2::date is null or b.business_date >= $2)
               and ($3::date is null or b.business_date <= $3)
               and ($8 or (em.is_master and not em.is_cashier))
           ), f as (
             select * from r where $9 or (r.reversal_of is null and not r.reversed)
           )
           select f.kind as "kind!", f.id as "id!", f.number, f.date as "date!",
                  f.amount_tyiyn as "amount_tyiyn!", f.article as "article!", f.purpose,
                  f.payee as "payee!", f.employee_id, f.comment as "comment!", f.source,
                  f.account_name, f.user_name as "user_name!", f.created_at as "created_at!",
                  f.reversal_of, f.reversed as "reversed!",
                  count(*) over () as "total!"
           from f
           order by f.date desc, f.created_at desc, f.id
           limit $10 offset $11"#,
        user.branch_id,
        q.from,
        q.to,
        s.owner,
        s.expenses,
        s.payouts,
        s.bonuses,
        s.cashiers,
        s.reversals,
        limit,
        offset
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut total = found.first().map_or(0, |r| r.total);
    if found.is_empty() && offset > 0 {
        // Страница за концом отбора: число строк — отдельным счётом, чтобы полоса страниц не
        // показала пустой список.
        total = count(conn, user, q, s).await?;
    }
    let rows = found
        .into_iter()
        .map(|r| SpendingRow {
            kind: r.kind,
            id: r.id,
            number: r.number,
            date: r.date,
            amount_tyiyn: r.amount_tyiyn,
            article: r.article,
            purpose: r.purpose,
            payee: r.payee,
            employee_id: r.employee_id,
            comment: r.comment,
            source: r.source,
            account_name: r.account_name,
            user_name: r.user_name,
            created_at: r.created_at,
            reversal_of: r.reversal_of,
            reversed: r.reversed,
        })
        .collect();
    Ok((rows, total))
}

/// Число строк отбора — та же выборка без страницы.
async fn count(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: &SpendingQuery,
    s: &Scope,
) -> AppResult<i64> {
    let first = PageQuery {
        limit: Some(1),
        offset: Some(0),
    };
    Ok(Box::pin(rows(conn, user, q, s, first)).await?.1)
}

/// Итог по статьям: группа «Зарплата работникам» (выданные деньги), в т. ч. бонусы; начисленные
/// и не выданные бонусы — отдельно, в итог не входят (итог не задваивает).
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct SpendingSummary {
    /// Статьи расходов: пары «расход + сторно» гасятся.
    pub articles: Vec<ArticleSum>,
    pub expenses_tyiyn: i64,
    /// Выдано работникам за период (по дате выплаты) и по видам.
    pub payouts_tyiyn: i64,
    pub purposes: Vec<PurposeSum>,
    /// В т. ч. выплачено бонусом.
    pub bonus_paid_tyiyn: i64,
    /// Начислено бонусов за период и начислено, но не выдано (по работнику: начислено − выдано
    /// бонусом за период, не меньше нуля). Без настройки или без права — `None`.
    pub bonus_accrued_tyiyn: Option<i64>,
    pub bonus_unpaid_tyiyn: Option<i64>,
    /// Всего: расходы + выдано работникам.
    pub total_tyiyn: i64,
    /// Что вошло в отбор этому пользователю.
    pub expenses: bool,
    pub payouts: bool,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ArticleSum {
    pub article: String,
    pub amount_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct PurposeSum {
    pub purpose: String,
    pub amount_tyiyn: i64,
}

async fn summary(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<SpendingQuery>,
) -> AppResult<Json<SpendingSummary>> {
    let mut conn = state.pool.acquire().await?;
    let s = scope(&mut conn, &user, &q).await?;
    let b = user.branch_id;
    let mut out = SpendingSummary {
        expenses: s.expenses,
        payouts: s.payouts,
        ..SpendingSummary::default()
    };
    if s.expenses {
        out.articles = sqlx::query!(
            r#"select a.name as "article!", coalesce(sum(e.amount_tyiyn), 0)::bigint as "sum!"
               from expenses e
               join expense_articles a on a.id = e.article_id
               left join cash_accounts c on c.id = e.account_id
               where e.branch_id = $1
                 and ($2::date is null or e.expense_date >= $2)
                 and ($3::date is null or e.expense_date <= $3)
                 and ($4 or (not a.owner_only and e.source = 'account'
                             and c.kind = 'register' and not c.owner_only
                             and e.expense_date >= (now() at time zone 'Asia/Bishkek')::date - 7))
               group by a.name
               having coalesce(sum(e.amount_tyiyn), 0) <> 0
               order by 2 desc, 1"#,
            b,
            q.from,
            q.to,
            s.owner
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| ArticleSum {
            article: r.article,
            amount_tyiyn: r.sum,
        })
        .collect();
        out.expenses_tyiyn = sum(out.articles.iter().map(|a| a.amount_tyiyn))?;
    }
    if s.payouts {
        out.purposes = sqlx::query!(
            r#"select p.purpose as "purpose!", coalesce(sum(p.amount_tyiyn), 0)::bigint as "sum!"
               from payouts p join employees em on em.id = p.employee_id
               where p.branch_id = $1
                 and ($2::date is null or p.pay_date >= $2)
                 and ($3::date is null or p.pay_date <= $3)
                 and ($4 or (em.is_master and not em.is_cashier))
               group by p.purpose
               having coalesce(sum(p.amount_tyiyn), 0) <> 0
               order by 2 desc, 1"#,
            b,
            q.from,
            q.to,
            s.cashiers
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| PurposeSum {
            purpose: r.purpose,
            amount_tyiyn: r.sum,
        })
        .collect();
        out.payouts_tyiyn = sum(out.purposes.iter().map(|p| p.amount_tyiyn))?;
        out.bonus_paid_tyiyn = out
            .purposes
            .iter()
            .filter(|p| p.purpose == "bonus")
            .map(|p| p.amount_tyiyn)
            .sum();
    }
    if s.bonuses {
        let by = sqlx::query!(
            r#"select em.id,
                 coalesce((select sum(a.amount_tyiyn) from payroll_accruals a
                           where a.employee_id = em.id and a.kind = 'bonus'
                             and ($2::date is null or a.business_date >= $2)
                             and ($3::date is null or a.business_date <= $3)), 0)::bigint as "accrued!",
                 coalesce((select sum(p.amount_tyiyn) from payouts p
                           where p.employee_id = em.id and p.purpose = 'bonus'
                             and ($2::date is null or p.pay_date >= $2)
                             and ($3::date is null or p.pay_date <= $3)), 0)::bigint as "paid!"
               from employees em
               where em.branch_id = $1 and ($4 or (em.is_master and not em.is_cashier))"#,
            b,
            q.from,
            q.to,
            s.cashiers
        )
        .fetch_all(&mut *conn)
        .await?;
        let accrued = sum(by.iter().map(|r| r.accrued))?;
        let unpaid = sum(by.iter().map(|r| r.accrued.saturating_sub(r.paid).max(0)))?;
        out.bonus_accrued_tyiyn = Some(accrued);
        out.bonus_unpaid_tyiyn = Some(unpaid);
    }
    out.total_tyiyn = out
        .expenses_tyiyn
        .checked_add(out.payouts_tyiyn)
        .ok_or_else(crate::error::overflow)?;
    Ok(Json(out))
}

fn sum(mut it: impl Iterator<Item = i64>) -> AppResult<i64> {
    it.try_fold(0i64, |a, x| a.checked_add(x))
        .ok_or_else(crate::error::overflow)
}
