//! «Прибыль и убытки» по образцу заказчика (задача 003 Н12, SPEC-08, ADR-157): раскрытие
//! строк отчёта о прибыли до документов и настройки отчёта. Только владелец (инвариант 13).
//!
//! Раскрытие приходит в `GET /reports/profit?detail=1` полем `detail` и ничего не пересчитывает:
//! итоги — те же `totals` (`gross_tyiyn`, `net_tyiyn`), раскрытие каждой строки складывается в
//! её итог (проверяется тестом `server/tests/pnl.rs`). У строк-документов есть `id`, поэтому
//! «Все филиалы» (`branches::merge_reports`) кладёт их подряд, а не складывает.

use axum::extract::State;
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/settings/profit-report",
        routing::get(get_settings).put(put_settings),
    )
}

// ---------- Настройки «Отчёт о прибыли» ----------

/// Настройки отчёта (`settings`, ключ `profit_report`): правит только владелец, каждое
/// изменение — в журнал (I5). Без записи — значения по умолчанию из задачи 003.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ProfitSettings {
    /// Период при открытии: `month` — этот месяц (как у заказчика), `today`, `week` — 7 дней.
    #[serde(default = "default_period")]
    pub default_period: String,
    /// Поле «Кому» в расходе: `off` — не спрашивать, `optional` — необязательно, `required`.
    #[serde(default = "default_payee")]
    pub expense_payee: String,
    /// Показывать сторно и исправления в раскрытии сразу (иначе — по переключателю).
    #[serde(default)]
    pub show_reversals: bool,
    /// «Расходы»: начисленные бонусы — отдельной строкой «начислен бонус» и итогом «начислено,
    /// не выдано» (задача 003 Н12б, ADR-161). По умолчанию да.
    #[serde(default = "yes")]
    pub expenses_bonus_accruals: bool,
}

fn yes() -> bool {
    true
}

fn default_period() -> String {
    "month".into()
}

fn default_payee() -> String {
    "optional".into()
}

impl Default for ProfitSettings {
    fn default() -> Self {
        Self {
            default_period: default_period(),
            expense_payee: default_payee(),
            show_reversals: false,
            expenses_bonus_accruals: true,
        }
    }
}

/// Настройки филиала; повреждённая или старая запись — значения по умолчанию.
pub async fn profit_settings(
    conn: &mut PgConnection,
    branch_id: Uuid,
) -> AppResult<ProfitSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'profit_report'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<ProfitSettings>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut conn = state.pool.acquire().await?;
    Ok(Json(profit_settings(&mut conn, user.branch_id).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ProfitSettings>,
) -> AppResult<Json<ProfitSettings>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !matches!(req.default_period.as_str(), "month" | "today" | "week") {
        return Err(invalid(
            "период по умолчанию: этот месяц, сегодня или 7 дней",
        ));
    }
    if !matches!(req.expense_payee.as_str(), "off" | "optional" | "required") {
        return Err(invalid(
            "поле «Кому»: не спрашивать, необязательно или обязательно",
        ));
    }
    // Запись и журнал — одна транзакция (инвариант 7).
    let mut tx = state.pool.begin().await?;
    let before = profit_settings(&mut tx, ctx.user.branch_id).await?;
    let value = json!(req);
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'profit_report', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "settings.profit_report",
        "settings",
        None,
        json!({ "changes": ops::changes(&json!(before), &value) }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(req))
}

// ---------- Раскрытие отчёта ----------

/// Товар за период: выручка и себестоимость проданного (возвраты — со своим знаком).
#[derive(Serialize)]
pub struct ProductRow {
    pub product_id: Option<Uuid>,
    pub name: String,
    pub category: String,
    /// Единица товара (`piece`, `ml`…); `units` — в ней, за вычетом возвратов.
    pub unit: String,
    pub units: i64,
    pub revenue_tyiyn: i64,
    pub cost_tyiyn: i64,
}

/// Работа (услуга) за период.
#[derive(Serialize)]
pub struct ServiceRow {
    pub service_id: Option<Uuid>,
    pub name: String,
    pub count: i64,
    pub revenue_tyiyn: i64,
    /// Начислено мастерам за эти работы.
    pub master_fee_tyiyn: i64,
}

/// Ревизия: недостача (плюс — уменьшает прибыль) или излишек (минус).
#[derive(Serialize)]
pub struct RevisionRow {
    pub id: Uuid,
    pub number: Option<i64>,
    pub date: NaiveDate,
    pub user_name: Option<String>,
    pub amount_tyiyn: i64,
    pub branch: String,
}

/// Переоценка остатков (ADR-044, ADR-087) за день по товару.
#[derive(Serialize)]
pub struct RevaluationRow {
    /// День и товар: строки разных филиалов по одному товару за день складываются.
    pub id: String,
    pub date: NaiveDate,
    pub product_id: Option<Uuid>,
    pub name: String,
    pub amount_tyiyn: i64,
}

/// Строка расхода (SPEC-06) — всё, что видно в «Расходах», и «Кому».
#[derive(Serialize)]
pub struct ExpenseRow {
    pub id: Uuid,
    pub number: i64,
    pub article: String,
    pub date: NaiveDate,
    pub amount_tyiyn: i64,
    pub payee: String,
    pub comment: String,
    pub account_name: Option<String>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub reversal_of: Option<Uuid>,
    pub reversed: bool,
    pub branch: String,
}

/// Выплата работнику за период: деньги ушли, прибыль не меняется (она — по начислению, ADR-023).
#[derive(Serialize)]
pub struct PayoutRow {
    pub id: Uuid,
    pub number: i64,
    pub employee_id: Uuid,
    pub employee_name: String,
    pub date: NaiveDate,
    pub amount_tyiyn: i64,
    /// Вид выплаты: `general`, `percent`, `service_fee`, `bonus`, `salary`, `advance`.
    pub purpose: String,
    pub account_name: Option<String>,
    pub comment: String,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub reversal_of: Option<Uuid>,
    pub reversed: bool,
    pub branch: String,
}

/// Комиссия банка по счёту — те же четыре источника, что в `totals_for`.
#[derive(Serialize)]
pub struct FeeRow {
    pub id: Option<Uuid>,
    pub name: String,
    pub amount_tyiyn: i64,
    pub branch: String,
}

/// Недостача (минус) или излишек (плюс) кассы по смене (ADR-065).
#[derive(Serialize)]
pub struct CashDiffRow {
    pub id: Uuid,
    pub number: i64,
    pub date: NaiveDate,
    pub cashier: Option<String>,
    pub amount_tyiyn: i64,
    pub branch: String,
}

/// Решение по браку (SPEC-25): списание и разница возмещения.
#[derive(Serialize)]
pub struct DefectRow {
    pub id: Uuid,
    pub number: i64,
    pub date: NaiveDate,
    pub product: String,
    pub kind: String,
    pub writeoff_tyiyn: i64,
    pub diff_tyiyn: i64,
    pub reversal_of: Option<Uuid>,
    pub user_name: String,
    pub branch: String,
}

/// Возврат поставщику (SPEC-29 §9): разница суммы и себестоимости.
#[derive(Serialize)]
pub struct SupplierReturnRow {
    pub id: Uuid,
    pub number: i64,
    pub date: NaiveDate,
    pub supplier: Option<String>,
    pub diff_tyiyn: i64,
    pub reversal_of: Option<Uuid>,
    pub user_name: String,
    pub branch: String,
}

/// Движение денег, которое прибыль не меняет (SPEC-08): оплата поставщику, изъятие, перевод.
#[derive(Serialize)]
pub struct MoneyRow {
    pub id: Uuid,
    /// `supplier_payment`, `cash_out`, `transfer_out`.
    pub kind: String,
    pub date: NaiveDate,
    /// Ушло со счёта — с минусом, как в движениях кассы.
    pub amount_tyiyn: i64,
    pub account_name: String,
    /// Поставщик накладной, если это оплата накладной.
    pub payee: Option<String>,
    pub comment: String,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub reversed: bool,
    pub branch: String,
}

/// Раскрытие отчёта о прибыли за период одного филиала.
#[derive(Serialize)]
pub struct PnlDetail {
    pub products: Vec<ProductRow>,
    pub services: Vec<ServiceRow>,
    /// Возвраты покупателей за период (уже вошли в выручку со знаком, справочно).
    pub returns_tyiyn: i64,
    /// Себестоимость: ревизия и переоценка отдельно; вместе — `totals.revaluation_tyiyn`.
    pub revision_tyiyn: i64,
    pub revaluation_only_tyiyn: i64,
    pub revisions: Vec<RevisionRow>,
    pub revaluations: Vec<RevaluationRow>,
    pub expenses: Vec<ExpenseRow>,
    pub payouts: Vec<PayoutRow>,
    pub fees: Vec<FeeRow>,
    pub cash_diffs: Vec<CashDiffRow>,
    pub defects: Vec<DefectRow>,
    pub supplier_returns: Vec<SupplierReturnRow>,
    pub not_in_profit: Vec<MoneyRow>,
    /// Оклад за незаконченный период без начисления — справочно, в чистую не входит
    /// (задача 003 Н12б).
    pub salary_expected: Vec<crate::api::payroll::SalaryExpected>,
}

/// Раскрытие строк отчёта. Вызывается после `reports::profit_data` (он проверил владельца и
/// период); проверка владельца повторяется — функция открыта и для бота.
pub async fn detail(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<PnlDetail> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let b = user.branch_id;
    let branch = sqlx::query_scalar!("select name from branches where id = $1", b)
        .fetch_optional(&mut *conn)
        .await?
        .unwrap_or_default();

    let products = sqlx::query!(
        r#"select p.id as "product_id?", coalesce(p.name, 'Без товара') as "name!",
                  coalesce(c.name, 'Без категории') as "category!", coalesce(p.unit, 'piece') as "unit!",
                  coalesce(sum(case when s.kind = 'return' then -l.units else l.units end), 0)::bigint as "units!",
                  coalesce(sum(l.amount_tyiyn), 0)::bigint as "revenue!",
                  coalesce(sum(l.cost_tyiyn), 0)::bigint as "cost!"
           from sale_lines l
           join sales s on s.id = l.sale_id
           left join products p on p.id = l.product_id
           left join categories c on c.id = p.category_id
           where s.branch_id = $1 and s.business_date between $2 and $3 and l.kind <> 'service'
           group by p.id, p.name, c.name, p.unit
           order by 6 desc, 2"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| ProductRow {
        product_id: r.product_id,
        name: r.name,
        category: r.category,
        unit: r.unit,
        units: r.units,
        revenue_tyiyn: r.revenue,
        cost_tyiyn: r.cost,
    })
    .collect();

    let services = sqlx::query!(
        r#"select sv.id as "service_id?", coalesce(sv.name, 'Работа') as "name!",
                  (count(*) filter (where s.kind <> 'return') - count(*) filter (where s.kind = 'return')) as "count!",
                  coalesce(sum(l.amount_tyiyn), 0)::bigint as "revenue!",
                  coalesce(sum(case when s.kind = 'return' then -abs(l.master_fee_tyiyn)
                                    else l.master_fee_tyiyn end), 0)::bigint as "fee!"
           from sale_lines l
           join sales s on s.id = l.sale_id
           left join services sv on sv.id = l.service_id
           where s.branch_id = $1 and s.business_date between $2 and $3 and l.kind = 'service'
           group by sv.id, sv.name
           order by 4 desc, 2"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| ServiceRow {
        service_id: r.service_id,
        name: r.name,
        count: r.count,
        revenue_tyiyn: r.revenue,
        master_fee_tyiyn: r.fee,
    })
    .collect();

    let returns_tyiyn = sqlx::query_scalar!(
        r#"select coalesce(sum(-total_tyiyn), 0)::bigint as "v!" from sales
           where branch_id = $1 and business_date between $2 and $3 and kind = 'return'"#,
        b,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;

    let revisions: Vec<RevisionRow> = sqlx::query!(
        r#"select m.doc_id as "id!", r.number as "number?",
                  (coalesce(r.posted_at, max(m.created_at)) at time zone 'Asia/Bishkek')::date as "date!",
                  u.full_name as "user_name?",
                  (-sum(m.value_delta_tyiyn))::bigint as "amount!"
           from stock_movements m
           left join revisions r on r.id = m.doc_id
           left join users u on u.id = coalesce(r.posted_by, r.user_id)
           where m.branch_id = $1 and m.doc_type = 'revision' and m.doc_id is not null
             and (m.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
           group by m.doc_id, r.number, r.posted_at, u.full_name
           having sum(m.value_delta_tyiyn) <> 0
           order by 3, 2"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| RevisionRow {
        id: r.id,
        number: r.number,
        date: r.date,
        user_name: r.user_name,
        amount_tyiyn: r.amount,
        branch: branch.clone(),
    })
    .collect();
    // Движения ревизии без документа (их нет, но сумма должна сойтись с итогом) — в итог.
    let revision_tyiyn = sqlx::query_scalar!(
        r#"select coalesce(-sum(value_delta_tyiyn), 0)::bigint as "v!" from stock_movements
           where branch_id = $1 and doc_type = 'revision'
             and (created_at at time zone 'Asia/Bishkek')::date between $2 and $3"#,
        b,
        from,
        to
    )
    .fetch_one(&mut *conn)
    .await?;

    let revaluations: Vec<RevaluationRow> = sqlx::query!(
        r#"select (m.created_at at time zone 'Asia/Bishkek')::date as "date!", p.id as "product_id?",
                  coalesce(p.name, 'Без товара') as "name!",
                  (-sum(m.value_delta_tyiyn))::bigint as "amount!"
           from stock_movements m
           left join products p on p.id = m.product_id
           where m.branch_id = $1 and m.doc_type = 'revaluation'
             and (m.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
           group by 1, p.id, p.name
           having sum(m.value_delta_tyiyn) <> 0
           order by 1, 3"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| RevaluationRow {
        id: format!(
            "{}:{}",
            r.date,
            r.product_id.map(|x| x.to_string()).unwrap_or_default()
        ),
        date: r.date,
        product_id: r.product_id,
        name: r.name,
        amount_tyiyn: r.amount,
    })
    .collect();
    let revaluation_only_tyiyn = revaluations
        .iter()
        .try_fold(0i64, |a, r| a.checked_add(r.amount_tyiyn))
        .ok_or_else(crate::error::overflow)?;

    // Все расходы периода, с личными (отчёт — только владельцу), без потолка строк: сумма
    // раскрытия должна совпасть с `expenses_tyiyn`.
    let expenses = sqlx::query!(
        r#"select e.id, e.number, a.name as article, e.expense_date, e.amount_tyiyn, e.payee,
                  e.comment, c.name as "account_name?", u.full_name as user_name, e.created_at,
                  e.reversal_of,
                  exists (select 1 from expenses x where x.reversal_of = e.id) as "reversed!"
           from expenses e
           join expense_articles a on a.id = e.article_id
           join users u on u.id = e.user_id
           left join cash_accounts c on c.id = e.account_id
           where e.branch_id = $1 and e.expense_date between $2 and $3
           order by a.name, e.expense_date, e.created_at"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| ExpenseRow {
        id: r.id,
        number: r.number,
        article: r.article,
        date: r.expense_date,
        amount_tyiyn: r.amount_tyiyn,
        payee: r.payee,
        comment: r.comment,
        account_name: r.account_name,
        user_name: r.user_name,
        created_at: r.created_at,
        reversal_of: r.reversal_of,
        reversed: r.reversed,
        branch: branch.clone(),
    })
    .collect();

    let payouts = sqlx::query!(
        r#"select p.id, p.number, p.employee_id, e.full_name as employee_name,
                  p.pay_date as "date!",
                  p.amount_tyiyn, p.purpose, c.name as "account_name?", p.comment,
                  u.full_name as user_name, p.created_at, p.reversal_of,
                  exists (select 1 from payouts x where x.reversal_of = p.id) as "reversed!"
           from payouts p
           join employees e on e.id = p.employee_id
           join users u on u.id = p.user_id
           left join cash_accounts c on c.id = p.account_id
           where p.branch_id = $1
             and p.pay_date between $2 and $3
           order by e.full_name, p.pay_date, p.created_at"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| PayoutRow {
        id: r.id,
        number: r.number,
        employee_id: r.employee_id,
        employee_name: r.employee_name,
        date: r.date,
        amount_tyiyn: r.amount_tyiyn,
        purpose: r.purpose,
        account_name: r.account_name,
        comment: r.comment,
        user_name: r.user_name,
        created_at: r.created_at,
        reversal_of: r.reversal_of,
        reversed: r.reversed,
        branch: branch.clone(),
    })
    .collect();

    let fees = sqlx::query!(
        r#"with f as (
             select p.account_id, p.fee_tyiyn as v
             from sale_payments p join sales s on s.id = p.sale_id
             where s.branch_id = $1 and s.business_date between $2 and $3 and p.fee_tyiyn <> 0
             union all
             select m.account_id, -m.amount_tyiyn from cash_movements m
             where m.branch_id = $1 and m.kind = 'bank_fee' and m.doc_type = 'repayment'
               and (m.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
             union all
             select a.account_id, a.amount_tyiyn from bank_fee_adjustments a
             where a.branch_id = $1 and a.op_day between $2 and $3
             union all
             select m.account_id, -m.amount_tyiyn from cash_movements m
             join pay_fixes fx on fx.id = m.doc_id join shifts sh on sh.id = fx.shift_id
             where m.branch_id = $1 and m.kind = 'bank_fee' and m.doc_type = 'pay_fix'
               and sh.business_date between $2 and $3
           )
           select f.account_id as "id?", coalesce(c.name, 'Без счёта') as "name!",
                  coalesce(sum(f.v), 0)::bigint as "amount!"
           from f left join cash_accounts c on c.id = f.account_id
           group by f.account_id, c.name
           having coalesce(sum(f.v), 0) <> 0
           order by 3 desc"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| FeeRow {
        id: r.id,
        name: r.name,
        amount_tyiyn: r.amount,
        branch: branch.clone(),
    })
    .collect();

    let cash_diffs = sqlx::query!(
        r#"select s.id, s.number, s.business_date, e.full_name as "cashier?",
                  coalesce(sum(m.amount_tyiyn), 0)::bigint as "amount!"
           from cash_movements m
           join shifts s on s.id = m.doc_id
           left join employees e on e.id = s.cashier_employee_id
           where m.branch_id = $1 and m.kind = 'count_diff' and m.doc_type = 'shift'
             and s.business_date between $2 and $3
           group by s.id, s.number, s.business_date, e.full_name
           having coalesce(sum(m.amount_tyiyn), 0) <> 0
           order by s.business_date, s.number"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| CashDiffRow {
        id: r.id,
        number: r.number,
        date: r.business_date,
        cashier: r.cashier,
        amount_tyiyn: r.amount,
        branch: branch.clone(),
    })
    .collect();

    let defects = sqlx::query!(
        r#"select x.id, d.number, (x.created_at at time zone 'Asia/Bishkek')::date as "date!",
                  p.name as product, x.kind, x.writeoff_tyiyn, x.diff_tyiyn, x.reversal_of,
                  u.full_name as user_name
           from defect_resolutions x
           join defects d on d.id = x.defect_id
           join products p on p.id = d.product_id
           join users u on u.id = x.user_id
           where x.branch_id = $1
             and (x.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
             and (x.writeoff_tyiyn <> 0 or x.diff_tyiyn <> 0)
           order by x.created_at"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| DefectRow {
        id: r.id,
        number: r.number,
        date: r.date,
        product: r.product,
        kind: r.kind,
        writeoff_tyiyn: r.writeoff_tyiyn,
        diff_tyiyn: r.diff_tyiyn,
        reversal_of: r.reversal_of,
        user_name: r.user_name,
        branch: branch.clone(),
    })
    .collect();

    let supplier_returns = sqlx::query!(
        r#"select r.id, r.number, (r.created_at at time zone 'Asia/Bishkek')::date as "date!",
                  sp.name as "supplier?", r.diff_tyiyn, r.reversal_of, u.full_name as user_name
           from supplier_returns r
           left join suppliers sp on sp.id = r.supplier_id
           join users u on u.id = r.user_id
           where r.branch_id = $1 and r.diff_tyiyn <> 0
             and (r.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
           order by r.created_at"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| SupplierReturnRow {
        id: r.id,
        number: r.number,
        date: r.date,
        supplier: r.supplier,
        diff_tyiyn: r.diff_tyiyn,
        reversal_of: r.reversal_of,
        user_name: r.user_name,
        branch: branch.clone(),
    })
    .collect();

    let not_in_profit = sqlx::query!(
        r#"select m.id, m.kind, (m.created_at at time zone 'Asia/Bishkek')::date as "date!",
                  m.amount_tyiyn, c.name as account_name, sup.name as "payee?", m.comment,
                  u.full_name as user_name, m.created_at,
                  exists (select 1 from cash_movements x where x.kind = 'reversal'
                            and x.doc_type = 'cash_movement' and x.doc_id = m.id) as "reversed!"
           from cash_movements m
           join cash_accounts c on c.id = m.account_id
           join users u on u.id = m.user_id
           left join receipts r on m.doc_type = 'receipt' and r.id = m.doc_id
           left join suppliers sup on sup.id = r.supplier_id
           where m.branch_id = $1 and m.kind in ('supplier_payment', 'cash_out', 'transfer_out')
             and (m.created_at at time zone 'Asia/Bishkek')::date between $2 and $3
           order by m.kind, m.created_at"#,
        b,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| MoneyRow {
        id: r.id,
        kind: r.kind,
        date: r.date,
        amount_tyiyn: r.amount_tyiyn,
        account_name: r.account_name,
        payee: r.payee,
        comment: r.comment,
        user_name: r.user_name,
        created_at: r.created_at,
        reversed: r.reversed,
        branch: branch.clone(),
    })
    .collect();

    let salary_expected = crate::api::payroll::expected_salaries(&mut *conn, b, from, to).await?;

    Ok(PnlDetail {
        products,
        services,
        returns_tyiyn,
        revision_tyiyn,
        revaluation_only_tyiyn,
        revisions,
        revaluations,
        expenses,
        payouts,
        fees,
        cash_diffs,
        defects,
        supplier_returns,
        not_in_profit,
        salary_expected,
    })
}
