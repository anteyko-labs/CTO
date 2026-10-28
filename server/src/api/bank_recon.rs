//! Сверка с выпиской банка Optima Business и настоящая комиссия (SPEC-31 §3, ADR-106).
//!
//! Выписка по POS и QR (`get-pos-operation-statement`) сохраняется в `bank_operations` —
//! повторная загрузка не дублирует. Сопоставление по дню: оплата по QR — по номеру транзакции
//! или операции банка, оплата картой — по сумме и времени (±15 минут, ближайшая). Для дней, где
//! сопоставилось всё, разница между комиссией банка и уже учтённой проводится поправкой —
//! движением «комиссия банка» на счёт и записью в `bank_fee_adjustments`; прибыль считает её
//! комиссией банка в день операции. Повторная сверка проводит только новую разницу.

use std::collections::{BTreeMap, HashSet};

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::api::cash::{self, CashEntry};
use crate::api::optima::{self, Optima};
use crate::auth::{Ctx, CurrentUser, Role};
use crate::domain::money::format_som;
use crate::error::{AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// Оплата картой и операция банка сходятся, если между ними не больше 15 минут.
pub const MATCH_MINUTES: i64 = 15;
/// Выписка банка — не больше 14 дней за раз.
pub const MAX_DAYS: i64 = 14;
/// Сверка сама — каждый день с 06:00 по времени точки.
const DAILY_HOUR: u32 = 6;

pub fn routes() -> Router<AppState> {
    Router::new().route("/bank/reconcile", routing::get(get_report).post(run))
}

// ---------- Сопоставление ----------

/// Сторона сверки для сопоставления: время, сумма и номера банка (у QR).
#[derive(Debug, Clone)]
pub struct Side {
    pub at: DateTime<Utc>,
    pub amount: i64,
    pub ids: Vec<String>,
}

/// Пары «наша оплата → операция банка». Сначала QR по номеру транзакции или операции, потом
/// остальное по сумме и времени: ближайшая операция банка той же суммы в пределах 15 минут.
pub fn match_pairs(ours: &[Side], bank: &[Side]) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; ours.len()];
    let mut taken = vec![false; bank.len()];
    for (i, o) in ours.iter().enumerate() {
        if o.ids.is_empty() {
            continue;
        }
        if let Some(j) = bank.iter().enumerate().position(|(j, b)| {
            !taken[j]
                && b.ids
                    .iter()
                    .any(|x| o.ids.iter().any(|y| !y.is_empty() && x == y))
        }) {
            taken[j] = true;
            out[i] = Some(j);
        }
    }
    let mut order: Vec<usize> = (0..ours.len()).filter(|i| out[*i].is_none()).collect();
    order.sort_by_key(|i| ours[*i].at);
    let window = Duration::minutes(MATCH_MINUTES);
    for i in order {
        let o = &ours[i];
        let best = bank
            .iter()
            .enumerate()
            .filter(|(j, b)| !taken[*j] && b.amount == o.amount)
            .map(|(j, b)| (j, (b.at - o.at).abs()))
            .filter(|(_, d)| *d <= window)
            .min_by_key(|(_, d)| *d);
        if let Some((j, _)) = best {
            taken[j] = true;
            out[i] = Some(j);
        }
    }
    out
}

fn day_of(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&optima::bishkek()).date_naive()
}

// ---------- Данные сверки ----------

/// Оплата в учёте: картой по чеку (в том числе по QR) или погашение долга картой.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OurItem {
    /// `card`, `qr` (чек по QR банка), `repayment` (погашение долга картой) или `transfer`
    /// (перенос из наличных в QR за смену, SPEC-37 §4).
    pub kind: String,
    pub at: DateTime<Utc>,
    pub amount_tyiyn: i64,
    /// Комиссия, учтённая по ставке счёта.
    pub fee_tyiyn: i64,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
    /// Перенос: номер смены.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shift_number: Option<i64>,
    #[serde(skip)]
    ids: Vec<String>,
    /// Перенос: окно смены — QR банка в нём, не сопоставленные с чеками, засчитываются в перенос.
    #[serde(skip)]
    window: Option<(DateTime<Utc>, DateTime<Utc>)>,
}

/// Операция из выписки банка.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BankItem {
    pub id: Uuid,
    /// `card` или `qr`.
    pub kind: String,
    pub at: DateTime<Utc>,
    pub amount_tyiyn: i64,
    pub fee_tyiyn: i64,
    pub auth_code: String,
    pub rrn: String,
    pub requisite: String,
    #[serde(skip)]
    ids: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconDay {
    pub date: NaiveDate,
    /// Выписка за этот день загружена.
    pub loaded: bool,
    pub ours_count: i64,
    pub ours_tyiyn: i64,
    pub bank_count: i64,
    pub bank_tyiyn: i64,
    /// У нас минус в банке.
    pub diff_tyiyn: i64,
    /// Несовпавших оплат с обеих сторон.
    pub unmatched: i64,
    /// Комиссия банка за день по выписке и учтённая по ставке (по сопоставленным операциям).
    pub bank_fee_tyiyn: i64,
    pub our_fee_tyiyn: i64,
    /// Уже проведённые поправки комиссии за день.
    pub adjusted_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReconOut {
    /// Ключ банка задан на сервере.
    pub enabled: bool,
    /// Расчётный счёт указан в настройке.
    pub configured: bool,
    pub account_id: Option<Uuid>,
    pub account_name: Option<String>,
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Дни, новые сверху.
    pub days: Vec<ReconDay>,
    /// «В учёте, нет в банке» — только за дни с загруженной выпиской.
    pub ours_unmatched: Vec<OurItem>,
    /// «В банке, нет в учёте».
    pub bank_unmatched: Vec<BankItem>,
    /// Что сделала эта сверка (у ответа на «Сверить с банком»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunSummary>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct RunSummary {
    /// Операций в выписке и сколько из них новых.
    pub operations: i64,
    pub new_operations: i64,
    /// Поправок комиссии проведено и на сколько всего.
    pub adjustments: i64,
    pub adjusted_tyiyn: i64,
}

async fn our_items(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<OurItem>> {
    // Время оплаты — время устройства, если оно правдоподобно (чек без сети уходит позже).
    // Исправленный чек и его сторно не входят: деньги один раз — по новому чеку, а у него время
    // и QR исходного (исправление проводят позже оплаты).
    let sales = sqlx::query!(
        r#"with p as (
             select p.amount_tyiyn, p.fee_tyiyn, s.id as sale_id, s.number,
                    case when t.client_time between t.created_at - interval '2 days'
                                                and t.created_at + interval '5 minutes'
                         then t.client_time else t.created_at end as at,
                    coalesce(q.transaction_id, oq.transaction_id) as tx,
                    coalesce(q.operation_id, oq.operation_id) as op
             from sale_payments p
             join sales s on s.id = p.sale_id
             left join doc_corrections fix on fix.kind = 'sale' and fix.new_id = s.id
             left join sales o on o.id = fix.original_id
             cross join lateral (select case when o.id is null then s.client_time else o.client_time end as client_time,
                                        coalesce(o.created_at, s.created_at) as created_at) t
             left join bank_qr q on q.id = p.bank_qr_id
             left join bank_qr oq on oq.sale_id = o.id
             where s.branch_id = $1 and p.method = 'card' and p.account_id = $2
               and t.created_at >= ($3::date - 3)::timestamp at time zone 'Asia/Bishkek'
               and not exists (select 1 from doc_corrections c
                               where c.kind = 'sale' and (c.original_id = s.id or c.reversal_id = s.id)))
           select amount_tyiyn as "amount!", fee_tyiyn as "fee!", sale_id as "sale_id!",
                  number as "number!", at as "at!", tx as "tx?", op as "op?"
           from p where (at at time zone 'Asia/Bishkek')::date between $3 and $4
           order by at"#,
        branch_id,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<OurItem> = sales
        .into_iter()
        .map(|r| {
            let ids: Vec<String> = [r.tx, r.op].into_iter().flatten().collect();
            OurItem {
                kind: if ids.is_empty() { "card" } else { "qr" }.into(),
                at: r.at,
                amount_tyiyn: r.amount,
                fee_tyiyn: r.fee,
                sale_id: Some(r.sale_id),
                sale_number: Some(r.number),
                shift_number: None,
                ids,
                window: None,
            }
        })
        .collect();
    // Погашения долга картой (не переводом и не «По счёту») на этот счёт, кроме отменённых.
    let repayments = sqlx::query!(
        r#"select m.amount_tyiyn, m.created_at,
                  coalesce((select -sum(f.amount_tyiyn) from cash_movements f
                            where f.doc_type = 'repayment' and f.doc_id = m.doc_id
                              and f.kind = 'bank_fee' and f.account_id = m.account_id), 0)::bigint as "fee!"
           from cash_movements m
           where m.branch_id = $1 and m.account_id = $2 and m.kind = 'debt_repayment'
             and m.doc_type = 'repayment' and m.amount_tyiyn > 0
             and (m.created_at at time zone 'Asia/Bishkek')::date between $3 and $4
             and not exists (select 1 from audit_log a
                             where a.branch_id = $1 and a.action = 'debt.repayment'
                               and a.data->>'repayment_id' = m.doc_id::text
                               and a.data->>'method' in ('transfer', 'invoice'))
             and not exists (select 1 from party_ledger l join party_ledger r on r.reversal_of = l.id
                             where l.doc_type = 'repayment' and l.doc_id = m.doc_id)"#,
        branch_id,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    out.extend(repayments.into_iter().map(|r| OurItem {
        kind: "repayment".into(),
        at: r.created_at,
        amount_tyiyn: r.amount_tyiyn,
        fee_tyiyn: r.fee,
        sale_id: None,
        sale_number: None,
        shift_number: None,
        ids: Vec::new(),
        window: None,
    }));
    // Переносы из наличных в QR за смену (SPEC-37 §4), кроме отменённых: одной суммой на день
    // смены, окно — от открытия до действующего закрытия (у открытой — до сейчас).
    let transfers = sqlx::query!(
        r#"select f.amount_tyiyn, f.fee_tyiyn, s.number, s.opened_at,
                  coalesce((select c.closed_at from shift_closes c
                            where c.shift_id = s.id
                              and not exists (select 1 from shift_reopens r where r.close_id = c.id)
                            order by c.seq desc limit 1), now()) as "until!"
           from pay_fixes f join shifts s on s.id = f.shift_id
           where f.branch_id = $1 and f.account_id = $2 and f.kind = 'shift' and f.reversal_of is null
             and not exists (select 1 from pay_fixes r where r.reversal_of = f.id)
             and s.business_date between $3 and $4"#,
        branch_id,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    out.extend(transfers.into_iter().map(|r| OurItem {
        kind: "transfer".into(),
        at: r.opened_at,
        amount_tyiyn: r.amount_tyiyn,
        fee_tyiyn: r.fee_tyiyn,
        sale_id: None,
        sale_number: None,
        shift_number: Some(r.number),
        ids: Vec::new(),
        window: Some((r.opened_at, r.until)),
    }));
    out.sort_by_key(|o| o.at);
    Ok(out)
}

/// Строки операции банка — по ним QR находится по номеру транзакции или операции.
fn strings_of(v: &Value) -> Vec<String> {
    match v {
        Value::Object(m) => m
            .values()
            .filter_map(|x| match x {
                Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

async fn bank_items(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<BankItem>> {
    let rows = sqlx::query!(
        r#"select id, kind, processed_at, amount_tyiyn, fee_tyiyn, auth_code, rrn, requisite, raw
           from bank_operations
           where branch_id = $1 and account_id = $2 and op_day between $3 and $4
           order by processed_at"#,
        branch_id,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| BankItem {
            ids: strings_of(&r.raw),
            id: r.id,
            kind: r.kind,
            at: r.processed_at,
            amount_tyiyn: r.amount_tyiyn,
            fee_tyiyn: r.fee_tyiyn,
            auth_code: r.auth_code,
            rrn: r.rrn,
            requisite: r.requisite,
        })
        .collect())
}

/// Дни, за которые выписка загружалась.
async fn loaded_days(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<HashSet<NaiveDate>> {
    let rows = sqlx::query!(
        r#"select distinct (data->>'from')::date as "from!", (data->>'to')::date as "to!"
           from audit_log
           where branch_id = $1 and action = 'bank_statement.load'
             and (data->>'from')::date <= $3 and (data->>'to')::date >= $2"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = HashSet::new();
    for r in rows {
        let mut d = r.from.max(from);
        while d <= r.to.min(to) {
            out.insert(d);
            d += Duration::days(1);
        }
    }
    Ok(out)
}

/// Сверка за дни по сохранённой выписке: дни, несовпавшие с обеих сторон.
async fn compute(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<(Vec<ReconDay>, Vec<OurItem>, Vec<BankItem>)> {
    let (transfers, ours): (Vec<OurItem>, Vec<OurItem>) =
        our_items(conn, branch_id, account_id, from, to)
            .await?
            .into_iter()
            .partition(|o| o.kind == "transfer");
    let bank = bank_items(conn, branch_id, account_id, from, to).await?;
    let loaded = loaded_days(conn, branch_id, from, to).await?;
    let adjusted: BTreeMap<NaiveDate, i64> = sqlx::query!(
        r#"select op_day, sum(amount_tyiyn)::bigint as "v!" from bank_fee_adjustments
           where branch_id = $1 and account_id = $2 and op_day between $3 and $4
           group by op_day"#,
        branch_id,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.op_day, r.v))
    .collect();
    let pairs = match_pairs(
        &ours
            .iter()
            .map(|o| Side {
                at: o.at,
                amount: o.amount_tyiyn,
                ids: o.ids.clone(),
            })
            .collect::<Vec<_>>(),
        &bank
            .iter()
            .map(|b| Side {
                at: b.at,
                amount: b.amount_tyiyn,
                ids: b.ids.clone(),
            })
            .collect::<Vec<_>>(),
    );
    let mut days: BTreeMap<NaiveDate, ReconDay> = BTreeMap::new();
    let mut d = from;
    while d <= to {
        days.insert(
            d,
            ReconDay {
                date: d,
                loaded: loaded.contains(&d),
                adjusted_tyiyn: adjusted.get(&d).copied().unwrap_or(0),
                ..ReconDay::default()
            },
        );
        d += Duration::days(1);
    }
    let mut bank_matched = vec![false; bank.len()];
    let mut ours_unmatched = Vec::new();
    for (i, o) in ours.iter().enumerate() {
        let own = day_of(o.at);
        if let Some(x) = days.get_mut(&own) {
            x.ours_count += 1;
            x.ours_tyiyn = x.ours_tyiyn.saturating_add(o.amount_tyiyn);
        }
        match pairs[i] {
            // Комиссия сопоставленной оплаты — в день операции банка.
            Some(j) => {
                bank_matched[j] = true;
                if let Some(x) = days.get_mut(&day_of(bank[j].at)) {
                    x.our_fee_tyiyn = x.our_fee_tyiyn.saturating_add(o.fee_tyiyn);
                }
            }
            None => {
                if let Some(x) = days.get_mut(&own) {
                    x.unmatched += 1;
                    if x.loaded {
                        ours_unmatched.push(o.clone());
                    }
                }
            }
        }
    }
    // Перенос из наличных в QR: QR банка в окне смены, не сопоставленные с чеками, по времени —
    // пока их сумма не дойдёт до суммы переноса. Сошлась точно — перенос сопоставлен.
    for t in &transfers {
        let own = day_of(t.at);
        if let Some(x) = days.get_mut(&own) {
            x.ours_count += 1;
            x.ours_tyiyn = x.ours_tyiyn.saturating_add(t.amount_tyiyn);
        }
        let (start, end) = t.window.unwrap_or((t.at, t.at));
        let mut taken = Vec::new();
        let mut sum: i64 = 0;
        for (j, b) in bank.iter().enumerate() {
            if sum >= t.amount_tyiyn {
                break;
            }
            if !bank_matched[j] && b.kind == "qr" && b.at >= start && b.at <= end {
                sum = sum.saturating_add(b.amount_tyiyn);
                taken.push(j);
            }
        }
        if sum == t.amount_tyiyn && !taken.is_empty() {
            for j in taken {
                bank_matched[j] = true;
            }
            if let Some(x) = days.get_mut(&own) {
                x.our_fee_tyiyn = x.our_fee_tyiyn.saturating_add(t.fee_tyiyn);
            }
        } else if let Some(x) = days.get_mut(&own) {
            x.unmatched += 1;
            if x.loaded {
                ours_unmatched.push(t.clone());
            }
        }
    }
    let mut bank_unmatched = Vec::new();
    for (j, b) in bank.iter().enumerate() {
        if let Some(x) = days.get_mut(&day_of(b.at)) {
            x.bank_count += 1;
            x.bank_tyiyn = x.bank_tyiyn.saturating_add(b.amount_tyiyn);
            x.bank_fee_tyiyn = x.bank_fee_tyiyn.saturating_add(b.fee_tyiyn);
            if !bank_matched[j] {
                x.unmatched += 1;
                bank_unmatched.push(b.clone());
            }
        }
    }
    for x in days.values_mut() {
        x.diff_tyiyn = x.ours_tyiyn.saturating_sub(x.bank_tyiyn);
    }
    Ok((
        days.into_values().rev().collect(),
        ours_unmatched,
        bank_unmatched,
    ))
}

async fn account_name(conn: &mut PgConnection, id: Uuid) -> AppResult<Option<String>> {
    Ok(
        sqlx::query_scalar!("select name from cash_accounts where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

#[derive(Deserialize)]
struct RangeReq {
    from: NaiveDate,
    to: NaiveDate,
}

fn check_range(from: NaiveDate, to: NaiveDate, today: NaiveDate) -> AppResult<()> {
    if to < from {
        return Err(invalid("конец периода раньше начала"));
    }
    if (to - from).num_days() >= MAX_DAYS {
        return Err(invalid("выписка банка — не больше 14 дней за раз"));
    }
    if to > today {
        return Err(invalid("выписку будущих дней банк не даёт"));
    }
    Ok(())
}

/// Экран «Сверка с банком»: по сохранённой выписке, без запроса в банк.
async fn get_report(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<RangeReq>,
) -> AppResult<Json<ReconOut>> {
    user.require_owner()?;
    if q.to < q.from || (q.to - q.from).num_days() > 92 {
        return Err(invalid("период — не больше 3 месяцев"));
    }
    let mut conn = state.pool.acquire().await?;
    let s = optima::load_settings(&mut conn, user.branch_id).await?;
    let account = optima::money_account(&mut conn, user.branch_id, &s).await?;
    let mut out = ReconOut {
        enabled: state.optima.is_some(),
        configured: s.statement_ready(),
        account_id: account,
        account_name: None,
        from: q.from,
        to: q.to,
        days: Vec::new(),
        ours_unmatched: Vec::new(),
        bank_unmatched: Vec::new(),
        run: None,
    };
    if let Some(acc) = account {
        out.account_name = account_name(&mut conn, acc).await?;
        let (days, ou, bu) = compute(&mut conn, user.branch_id, acc, q.from, q.to).await?;
        out.days = days;
        out.ours_unmatched = ou;
        out.bank_unmatched = bu;
    }
    Ok(Json(out))
}

/// «Сверить с банком»: выписка за дни (не больше 14), сопоставление, поправка комиссии.
async fn run(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<RangeReq>,
) -> AppResult<Json<ReconOut>> {
    ctx.user.require_owner()?;
    let bank = state
        .optima
        .as_ref()
        .ok_or_else(|| invalid("ключ Optima Business не задан на сервере"))?;
    let today = {
        let mut conn = state.pool.acquire().await?;
        crate::api::fees::today(&mut conn).await?
    };
    check_range(req.from, req.to, today)?;
    let summary = reconcile(&state.pool, bank, &ctx, req.from, req.to, today).await?;
    let mut conn = state.pool.acquire().await?;
    let s = optima::load_settings(&mut conn, ctx.user.branch_id).await?;
    let account = optima::money_account(&mut conn, ctx.user.branch_id, &s).await?;
    let (days, ou, bu) = match account {
        Some(acc) => compute(&mut conn, ctx.user.branch_id, acc, req.from, req.to).await?,
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    Ok(Json(ReconOut {
        enabled: true,
        configured: true,
        account_name: match account {
            Some(acc) => account_name(&mut conn, acc).await?,
            None => None,
        },
        account_id: account,
        from: req.from,
        to: req.to,
        days,
        ours_unmatched: ou,
        bank_unmatched: bu,
        run: Some(summary),
    }))
}

/// Загрузка выписки и сверка дней `from..=to`: одна операция — одна транзакция.
pub async fn reconcile(
    pool: &PgPool,
    bank: &Optima,
    ctx: &Ctx,
    from: NaiveDate,
    to: NaiveDate,
    today: NaiveDate,
) -> AppResult<RunSummary> {
    let branch_id = ctx.user.branch_id;
    let (s, account) = {
        let mut conn = pool.acquire().await?;
        let s = optima::load_settings(&mut conn, branch_id).await?;
        let account = optima::money_account(&mut conn, branch_id, &s).await?;
        (s, account)
    };
    if !s.statement_ready() {
        return Err(invalid(
            "укажите расчётный счёт в «Настройки» → «Optima Business»",
        ));
    }
    let account = account.ok_or_else(|| invalid("нет безналичного счёта для денег банка"))?;
    let ops_list = match bank.pos_statement(from, to, &s.account_number).await {
        Ok(o) => o,
        Err(e) => {
            optima::key_problem(pool, branch_id, &e).await;
            return Err(e.into());
        }
    };
    let mut tx = pool.begin().await?;
    // Две сверки одного филиала одновременно не проводят одну разницу дважды.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        format!("bank_recon:{branch_id}")
    )
    .execute(&mut *tx)
    .await?;
    let mut summary = RunSummary {
        operations: i64::try_from(ops_list.len()).unwrap_or(i64::MAX),
        ..RunSummary::default()
    };
    for o in &ops_list {
        let added = sqlx::query!(
            r#"insert into bank_operations (id, branch_id, account_id, op_key, kind, processed_at,
                                            op_day, amount_tyiyn, fee_tyiyn, transfer_tyiyn,
                                            auth_code, rrn, terminal, requisite, raw, user_id)
               values ($1, $2, $3, $4, $5, $6, ($6 at time zone 'Asia/Bishkek')::date, $7, $8, $9,
                       $10, $11, $12, $13, $14::text::jsonb, $15)
               on conflict (branch_id, op_key) do nothing"#,
            new_id(),
            branch_id,
            account,
            o.key,
            o.kind,
            o.processed_at,
            o.amount,
            o.fee,
            o.transfer,
            o.auth_code,
            o.rrn,
            o.terminal,
            o.requisite,
            o.raw,
            ctx.user.id
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        summary.new_operations += i64::try_from(added).unwrap_or(0);
    }
    ops::audit(
        &mut tx,
        ctx,
        "bank_statement.load",
        "bank_operations",
        None,
        json!({ "from": from, "to": to, "operations": summary.operations,
                "new": summary.new_operations }),
    )
    .await?;
    let (days, _, _) = compute(&mut tx, branch_id, account, from, to).await?;
    for d in &days {
        if !d.loaded {
            continue;
        }
        // Комиссия — только за дни, где сошлось всё: иначе разница — не комиссия, а пропуск.
        let all_matched = d.unmatched == 0 && (d.ours_count > 0 || d.bank_count > 0);
        let diff = d
            .bank_fee_tyiyn
            .saturating_sub(d.our_fee_tyiyn)
            .saturating_sub(d.adjusted_tyiyn);
        if all_matched && diff != 0 {
            adjust_fee(&mut tx, ctx, account, d, diff).await?;
            summary.adjustments += 1;
            summary.adjusted_tyiyn = summary.adjusted_tyiyn.saturating_add(diff);
        }
        // Расхождение прошедшего дня — владельцу, если такого же ещё не было.
        if d.unmatched > 0 && d.date < today {
            notify_mismatch(&mut tx, ctx, d).await?;
        }
    }
    tx.commit().await?;
    Ok(summary)
}

/// Поправка комиссии за день: движение «комиссия банка» на счёт и запись поправки.
async fn adjust_fee(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account: Uuid,
    d: &ReconDay,
    diff: i64,
) -> AppResult<()> {
    let id = new_id();
    let comment = format!(
        "Комиссия банка: поправка по выписке за {}",
        d.date.format("%d.%m.%Y")
    );
    let (movement_id, _) = cash::add_movement_id(
        conn,
        ctx,
        CashEntry {
            account_id: account,
            kind: "bank_fee",
            amount: -diff,
            doc_type: "bank_fee_adjust",
            doc_id: Some(id),
            comment: &comment,
        },
    )
    .await?;
    sqlx::query!(
        r#"insert into bank_fee_adjustments (id, branch_id, account_id, op_day, amount_tyiyn,
                                             bank_fee_tyiyn, our_fee_tyiyn, movement_id, user_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        ctx.user.branch_id,
        account,
        d.date,
        diff,
        d.bank_fee_tyiyn,
        d.our_fee_tyiyn.saturating_add(d.adjusted_tyiyn),
        movement_id,
        ctx.user.id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        "bank_fee.adjust",
        "bank_fee_adjustment",
        Some(id),
        json!({ "day": d.date, "amount": diff, "bank_fee": d.bank_fee_tyiyn,
                "our_fee": d.our_fee_tyiyn, "before": d.adjusted_tyiyn }),
    )
    .await
}

/// «Сверка с банком 21.10: в учёте 12 оплат на 18 400 с, в банке 11 на 17 600 с» — один раз
/// на одно и то же расхождение дня.
async fn notify_mismatch(conn: &mut PgConnection, ctx: &Ctx, d: &ReconDay) -> AppResult<()> {
    let data = json!({
        "day": d.date,
        "ours_count": d.ours_count, "ours": d.ours_tyiyn,
        "bank_count": d.bank_count, "bank": d.bank_tyiyn,
        "unmatched": d.unmatched,
    });
    let same = sqlx::query_scalar!(
        r#"select data from audit_log
           where branch_id = $1 and action = 'bank_recon.mismatch' and data->>'day' = $2
           order by created_at desc limit 1"#,
        ctx.user.branch_id,
        d.date.to_string()
    )
    .fetch_optional(&mut *conn)
    .await?
    .is_some_and(|prev| {
        ["ours_count", "ours", "bank_count", "bank", "unmatched"]
            .iter()
            .all(|k| prev.get(k) == data.get(k))
    });
    if same {
        return Ok(());
    }
    ops::audit(
        conn,
        ctx,
        "bank_recon.mismatch",
        "bank_operations",
        None,
        data,
    )
    .await
}

/// Описание расхождения для ленты и бота.
pub fn mismatch_text(data: &Value) -> (String, String) {
    let num = |k: &str| data.get(k).and_then(Value::as_i64).unwrap_or(0);
    let day = data
        .get("day")
        .and_then(Value::as_str)
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .map_or_else(String::new, |d| format!(" {}", d.format("%d.%m")));
    (
        format!(
            "Сверка с банком{day}: в учёте {} на {}, в банке {} на {}",
            payments_word(num("ours_count")),
            format_som(num("ours")),
            num("bank_count"),
            format_som(num("bank"))
        ),
        format!(
            "не совпало оплат: {} — «Безнал (банк)» → «Сверка с банком»",
            num("unmatched")
        ),
    )
}

/// «1 оплата», «2 оплаты», «12 оплат».
fn payments_word(n: i64) -> String {
    let (d10, d100) = (n.abs() % 10, n.abs() % 100);
    let w = if (11..=14).contains(&d100) {
        "оплат"
    } else if d10 == 1 {
        "оплата"
    } else if (2..=4).contains(&d10) {
        "оплаты"
    } else {
        "оплат"
    };
    format!("{n} {w}")
}

// ---------- Сверка каждый день ----------

/// Владелец филиала — от его имени сервер проводит ежедневную сверку.
async fn owner_ctx(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Option<Ctx>> {
    let row = sqlx::query!(
        // Владелец — всех филиалов (SPEC-33): первый действующий, записи — в этом филиале.
        r#"select id, login, full_name from users
           where role = 'owner' and active order by created_at limit 1"#
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| Ctx {
        user: CurrentUser {
            id: r.id,
            branch_id,
            login: r.login,
            full_name: r.full_name,
            role: Role::Owner,
        },
        device_id: optima::SERVER_DEVICE,
        bot_chat: None,
    }))
}

/// С 06:00 по времени точки — сверка трёх прошедших дней, раз в день по каждому филиалу с
/// расчётным счётом. Не вышло (нет связи) — повтор через 30 минут.
pub async fn daily(pool: &PgPool, bank: &Optima, now: DateTime<Utc>) {
    if let Err(e) = daily_inner(pool, bank, now).await {
        tracing::warn!(error = %e, "банк: сверка дня не прошла");
    }
}

async fn daily_inner(pool: &PgPool, bank: &Optima, now: DateTime<Utc>) -> AppResult<()> {
    let local = now.with_timezone(&optima::bishkek());
    if chrono::Timelike::hour(&local) < DAILY_HOUR {
        return Ok(());
    }
    let today = local.date_naive();
    let branches = sqlx::query!(
        r#"select o.branch_id, d.value as "last?"
           from settings o
           left join settings d on d.branch_id = o.branch_id and d.key = 'optima_daily'
           where o.key = 'optima'"#
    )
    .fetch_all(pool)
    .await?;
    for b in branches {
        let last = b.last.unwrap_or(Value::Null);
        let done_today = last.get("date").and_then(Value::as_str) == Some(&today.to_string());
        let ok = last.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let recent = last
            .get("at")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .is_some_and(|t| now - t.with_timezone(&Utc) < Duration::minutes(30));
        if done_today && (ok || recent) {
            continue;
        }
        let mut conn = pool.acquire().await?;
        let s = optima::load_settings(&mut conn, b.branch_id).await?;
        let ctx = owner_ctx(&mut conn, b.branch_id).await?;
        drop(conn);
        let (true, Some(ctx)) = (s.statement_ready(), ctx) else {
            continue;
        };
        let result = reconcile(
            pool,
            bank,
            &ctx,
            today - Duration::days(3),
            today - Duration::days(1),
            today,
        )
        .await;
        if let Err(e) = &result {
            tracing::warn!(error = %e, "банк: ежедневная сверка не прошла, повтор через 30 минут");
        }
        let value = json!({ "date": today, "ok": result.is_ok(), "at": now.to_rfc3339() });
        sqlx::query!(
            r#"insert into settings (branch_id, key, value) values ($1, 'optima_daily', $2)
               on conflict (branch_id, key) do update set value = excluded.value"#,
            b.branch_id,
            value
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(min: i64) -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_760_000_000 + min * 60, 0).unwrap_or_default()
    }

    fn side(min: i64, amount: i64, ids: &[&str]) -> Side {
        Side {
            at: at(min),
            amount,
            ids: ids.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn qr_by_id_card_by_amount_and_nearest_time() {
        let ours = vec![
            side(0, 99_000, &[]),
            side(1, 99_000, &[]),
            side(5, 50_000, &["tx-1", "op-1"]),
            side(40, 12_000, &[]),
        ];
        let bank = vec![
            // QR: другая сумма по времени ближе, но номер совпал.
            side(30, 50_000, &["op-1"]),
            side(2, 99_000, &[]),
            side(-1, 99_000, &[]),
            // Та же сумма, но больше 15 минут — не пара.
            side(60, 12_000, &[]),
            side(7, 77_700, &[]),
        ];
        let p = match_pairs(&ours, &bank);
        assert_eq!(p, vec![Some(2), Some(1), Some(0), None]);
    }

    #[test]
    fn payments_in_words() {
        assert_eq!(payments_word(1), "1 оплата");
        assert_eq!(payments_word(3), "3 оплаты");
        assert_eq!(payments_word(12), "12 оплат");
        assert_eq!(payments_word(21), "21 оплата");
        let (title, _) = mismatch_text(&json!({
            "day": "2026-10-21", "ours_count": 12, "ours": 1_840_000, "bank_count": 11, "bank": 1_760_000
        }));
        assert_eq!(
            title,
            "Сверка с банком 21.10: в учёте 12 оплат на 18 400 с, в банке 11 на 17 600 с"
        );
    }
}
