//! Деньги по кассам и способам оплаты, экран «Счёт» и проданное по накладным (SPEC-27 §6).
//!
//! Всё считается из тех же записей, что остальные отчёты: оплаты чеков (`sale_payments`) по
//! учётному дню — как прибыль (SPEC-08), движения касс (`cash_movements`) по времени — как экран
//! кассы (SPEC-05). Проданное по накладной — оценка «первым продаётся пришедшее раньше», как
//! поставки поставщика (SPEC-26): склад ведётся по средней без партий (инвариант 3).

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::MovementOut;
use crate::api::reports::{TopProduct, TopQuery, resolve_period};
use crate::auth::CurrentUser;
use crate::domain::money::div_round;
use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/cash/bank", routing::get(bank))
        .route("/reports/income", routing::get(income))
}

#[derive(Deserialize)]
struct Period {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Deserialize)]
struct BankQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    /// Безналичный счёт; без него — счёт по умолчанию для карты (ADR-076, ADR-077).
    account_id: Option<Uuid>,
}

/// Способы оплаты чека в постоянном порядке — строки отчёта есть всегда, даже нулевые.
pub const METHODS: [&str; 7] = [
    "cash", "card", "transfer", "invoice", "debt", "bonus", "battery",
];

// ---------- Оплаты чеков по способам ----------

/// Оплаты чеков одним способом за период: продажи, возвраты, итог и комиссия банка.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MethodRow {
    pub method: String,
    /// Оплачено в чеках продажи.
    pub sales_tyiyn: i64,
    /// Возвращено по чекам возврата (положительное число).
    pub returns_tyiyn: i64,
    /// Продажи минус возвраты.
    pub net_tyiyn: i64,
    /// Комиссия банка с этого способа (карта, перевод).
    pub fee_tyiyn: i64,
}

/// Оплаты чеков по способам за учётные дни периода. Сумма `net_tyiyn` по всем способам равна
/// выручке отчёта о прибыли: каждый чек оплачен ровно на свою сумму.
pub async fn methods_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<MethodRow>> {
    let rows = sqlx::query!(
        r#"select p.method as "method!",
                  coalesce(sum(p.amount_tyiyn) filter (where s.kind = 'sale'), 0)::bigint as "sales!",
                  coalesce(-sum(p.amount_tyiyn) filter (where s.kind = 'return'), 0)::bigint as "returns!",
                  coalesce(sum(p.amount_tyiyn), 0)::bigint as "net!",
                  coalesce(sum(p.fee_tyiyn), 0)::bigint as "fee!"
           from sale_payments p join sales s on s.id = p.sale_id
           where s.branch_id = $1 and s.business_date between $2 and $3
           group by p.method"#,
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by: HashMap<String, MethodRow> = rows
        .into_iter()
        .map(|r| {
            (
                r.method.clone(),
                MethodRow {
                    method: r.method,
                    sales_tyiyn: r.sales,
                    returns_tyiyn: r.returns,
                    net_tyiyn: r.net,
                    fee_tyiyn: r.fee,
                },
            )
        })
        .collect();
    let mut out: Vec<MethodRow> = METHODS
        .iter()
        .map(|m| {
            by.remove(*m).unwrap_or_else(|| MethodRow {
                method: (*m).to_string(),
                ..MethodRow::default()
            })
        })
        .collect();
    // Способ, которого нет в списке (появится позже), не теряется.
    let mut rest: Vec<MethodRow> = by.into_values().collect();
    rest.sort_by(|a, b| a.method.cmp(&b.method));
    out.extend(rest);
    Ok(out)
}

/// «Сколько по карте ушло»: безнал чеков за период — сводка и закрытие смены (SPEC-27 §6).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct Noncash {
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    /// «По счёту» — перечисления юрлиц (ADR-095).
    #[serde(default)]
    pub invoice_tyiyn: i64,
    pub total_tyiyn: i64,
}

/// Безнал чеков за период вместе с разбивкой по безналичным счетам (ADR-076).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct NoncashSplit {
    #[serde(flatten)]
    pub total: Noncash,
    pub by_account: Vec<BankSplit>,
}

pub async fn noncash_split_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<NoncashSplit> {
    Ok(NoncashSplit {
        total: noncash_for(&mut *conn, branch_id, from, to).await?,
        by_account: bank_split(&mut *conn, branch_id, Some((from, to)), None, None).await?,
    })
}

/// Безнал чеков одного безналичного счёта (ADR-076): картой, переводом, «По счёту» (ADR-095) и
/// комиссия банка.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BankSplit {
    pub account_id: Uuid,
    pub name: String,
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    #[serde(default)]
    pub invoice_tyiyn: i64,
    pub fee_tyiyn: i64,
    pub total_tyiyn: i64,
}

/// Безнал чеков по счетам: за учётные дни (`days`) — сводка, или за время смены (`since`..`until`)
/// — закрытие смены. Сумма по счетам равна карте и переводу всего: у каждой такой оплаты есть счёт.
pub async fn bank_split(
    conn: &mut PgConnection,
    branch_id: Uuid,
    days: Option<(NaiveDate, NaiveDate)>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
) -> AppResult<Vec<BankSplit>> {
    // Два запроса без необязательных условий: от чеков дня или смены к их оплатам (индексы
    // чеков по дню и по времени), а не перебор всех безналичных оплат (ADR-094).
    let rows: Vec<(Uuid, String, i64, i64, i64, i64)> = if let Some((from, to)) = days {
        sqlx::query!(
            r#"with x as materialized (
                 select p.account_id,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as card,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as transfer,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'invoice'), 0)::bigint as invoice,
                        coalesce(sum(p.fee_tyiyn), 0)::bigint as fee
                 from sales s
                 join sale_payments p on p.sale_id = s.id
                 where p.account_id is not null and s.branch_id = $1 and s.business_date between $2 and $3
                 and p.method in ('card', 'transfer', 'invoice')
                 group by p.account_id
               )
               select x.account_id as "account_id!", a.name, x.card as "card!",
                      x.transfer as "transfer!", x.invoice as "invoice!", x.fee as "fee!"
               from x join cash_accounts a on a.id = x.account_id
               order by a.default_card desc, a.name"#,
            branch_id,
            from,
            to
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| (r.account_id, r.name, r.card, r.transfer, r.invoice, r.fee))
        .collect()
    } else if let Some(since) = since {
        sqlx::query!(
            r#"with x as materialized (
                 select p.account_id,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as card,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as transfer,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'invoice'), 0)::bigint as invoice,
                        coalesce(sum(p.fee_tyiyn), 0)::bigint as fee
                 from sales s
                 join sale_payments p on p.sale_id = s.id
                 where p.account_id is not null and s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
                 and p.method in ('card', 'transfer', 'invoice')
                 group by p.account_id
               )
               select x.account_id as "account_id!", a.name, x.card as "card!",
                      x.transfer as "transfer!", x.invoice as "invoice!", x.fee as "fee!"
               from x join cash_accounts a on a.id = x.account_id
               order by a.default_card desc, a.name"#,
            branch_id,
            since,
            until
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| (r.account_id, r.name, r.card, r.transfer, r.invoice, r.fee))
        .collect()
    } else {
        sqlx::query!(
            r#"with x as materialized (
                 select p.account_id,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as card,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as transfer,
                        coalesce(sum(p.amount_tyiyn) filter (where p.method = 'invoice'), 0)::bigint as invoice,
                        coalesce(sum(p.fee_tyiyn), 0)::bigint as fee
                 from sales s
                 join sale_payments p on p.sale_id = s.id
                 where p.account_id is not null and s.branch_id = $1 and p.method in ('card', 'transfer', 'invoice')
                 group by p.account_id
               )
               select x.account_id as "account_id!", a.name, x.card as "card!",
                      x.transfer as "transfer!", x.invoice as "invoice!", x.fee as "fee!"
               from x join cash_accounts a on a.id = x.account_id
               order by a.default_card desc, a.name"#,
            branch_id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| (r.account_id, r.name, r.card, r.transfer, r.invoice, r.fee))
        .collect()
    };
    Ok(rows
        .into_iter()
        .map(
            |(account_id, name, card, transfer, invoice, fee)| BankSplit {
                account_id,
                name,
                card_tyiyn: card,
                transfer_tyiyn: transfer,
                invoice_tyiyn: invoice,
                fee_tyiyn: fee,
                total_tyiyn: card.saturating_add(transfer).saturating_add(invoice),
            },
        )
        .collect())
}

pub async fn noncash_for(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Noncash> {
    let methods = methods_for(conn, branch_id, from, to).await?;
    let net = |m: &str| {
        methods
            .iter()
            .find(|r| r.method == m)
            .map_or(0, |r| r.net_tyiyn)
    };
    let (card, transfer, invoice) = (net("card"), net("transfer"), net("invoice"));
    Ok(Noncash {
        card_tyiyn: card,
        transfer_tyiyn: transfer,
        invoice_tyiyn: invoice,
        total_tyiyn: card.saturating_add(transfer).saturating_add(invoice),
    })
}

// ---------- Деньги по кассам ----------

/// Движения одного вида в кассе за период: пришло и ушло (ушло — положительным числом).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct KindRow {
    pub kind: String,
    pub in_tyiyn: i64,
    pub out_tyiyn: i64,
}

/// Касса за период: на начало, по видам движений, на конец. На конец = на начало + пришло − ушло.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountFlow {
    pub account_id: Uuid,
    pub name: String,
    pub kind: String,
    pub opening_tyiyn: i64,
    pub in_tyiyn: i64,
    pub out_tyiyn: i64,
    pub closing_tyiyn: i64,
    pub kinds: Vec<KindRow>,
}

/// Кассы за период по движениям. Сейф владельца — только владельцу (ADR-037), как и на экране
/// касс: администратор видит кассы, которые видит в «Смене» и «Деньгах».
pub async fn account_flows(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<AccountFlow>> {
    flows_of(conn, user, from, to, None).await
}

/// Кассы за период; `only` — одна касса (история в «Обзоре счетов», SPEC-28 §5). Архивный счёт
/// попадает в отчёт, если за период по нему были движения: история не теряется (ADR-076).
pub async fn flows_of(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
    only: Option<Uuid>,
) -> AppResult<Vec<AccountFlow>> {
    crate::api::cash::ensure_accounts(&mut *conn, user.branch_id).await?;
    let accounts = sqlx::query!(
        r#"select a.id, a.name, a.kind,
                  coalesce((select sum(m.amount_tyiyn) from cash_movements m
                            where m.account_id = a.id
                              and m.created_at < ($3::date)::timestamp at time zone 'Asia/Bishkek'), 0)::bigint as "opening!"
           from cash_accounts a
           where a.branch_id = $1 and (not a.owner_only or $2)
             and ($5::uuid is null or a.id = $5)
             and (a.active or $5::uuid is not null
                  or exists (select 1 from cash_movements m
                             where m.account_id = a.id
                               and m.created_at >= ($3::date)::timestamp at time zone 'Asia/Bishkek'
                               and m.created_at < ($4::date + 1)::timestamp at time zone 'Asia/Bishkek'))
           order by a.active desc, a.kind = 'bank', a.is_default desc, a.default_card desc,
                    a.default_transfer desc, a.kind, a.name"#,
        user.branch_id,
        user.is_owner(),
        from,
        to,
        only
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = accounts.iter().map(|a| a.id).collect();
    let kinds = sqlx::query!(
        r#"select m.account_id, m.kind as "kind!",
                  coalesce(sum(m.amount_tyiyn) filter (where m.amount_tyiyn > 0), 0)::bigint as "inc!",
                  coalesce(-sum(m.amount_tyiyn) filter (where m.amount_tyiyn < 0), 0)::bigint as "outg!"
           from cash_movements m
           where m.account_id = any($1)
             and m.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek'
             and m.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek'
           group by m.account_id, m.kind
           order by m.account_id, m.kind"#,
        &ids,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by: HashMap<Uuid, Vec<KindRow>> = HashMap::new();
    for k in kinds {
        by.entry(k.account_id).or_default().push(KindRow {
            kind: k.kind,
            in_tyiyn: k.inc,
            out_tyiyn: k.outg,
        });
    }
    Ok(accounts
        .into_iter()
        .map(|a| {
            let kinds = by.remove(&a.id).unwrap_or_default();
            let inc = kinds.iter().fold(0i64, |s, k| s.saturating_add(k.in_tyiyn));
            let out = kinds
                .iter()
                .fold(0i64, |s, k| s.saturating_add(k.out_tyiyn));
            AccountFlow {
                account_id: a.id,
                name: a.name,
                kind: a.kind,
                opening_tyiyn: a.opening,
                in_tyiyn: inc,
                out_tyiyn: out,
                closing_tyiyn: a.opening.saturating_add(inc).saturating_sub(out),
                kinds,
            }
        })
        .collect())
}

// ---------- Экран «Счёт» ----------

/// Поступления на счёт одним способом: из чеков (за вычетом возвратов) и погашения долга.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
pub struct BankIncome {
    pub sales_tyiyn: i64,
    pub repayments_tyiyn: i64,
    pub total_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct BankDay {
    pub date: NaiveDate,
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    /// «По счёту» (ADR-095).
    #[serde(default)]
    pub invoice_tyiyn: i64,
    pub fee_tyiyn: i64,
    /// Прочее: перемещения, оплаты поставщикам, расходы, сторно — со знаком.
    pub other_tyiyn: i64,
    /// Изменение остатка за день.
    pub net_tyiyn: i64,
}

#[derive(Serialize)]
pub(crate) struct BankOut {
    pub(crate) account_id: Uuid,
    pub(crate) name: String,
    pub(crate) from: NaiveDate,
    pub to: NaiveDate,
    /// Остаток сейчас.
    pub balance_tyiyn: i64,
    pub opening_tyiyn: i64,
    pub closing_tyiyn: i64,
    pub card: BankIncome,
    pub transfer: BankIncome,
    /// «По счёту» — перечисления юрлиц по чекам (ADR-095); погашений долга этим способом нет.
    pub invoice: BankIncome,
    /// Комиссия банка (положительное число — сколько удержал банк).
    pub fee_tyiyn: i64,
    /// Прочие поступления: перемещения в счёт, внесения, возмещения.
    pub other_in_tyiyn: i64,
    /// Списания: перемещения со счёта, оплаты поставщикам, расходы, сторно.
    pub outgoing_tyiyn: i64,
    /// Все виды движений за период — сверка с движениями кассы «Счёт».
    pub kinds: Vec<KindRow>,
    pub days: Vec<BankDay>,
    /// Движения за период, новые сверху (не больше 500).
    pub movements: Vec<MovementOut>,
    pub movements_count: i64,
}

async fn bank(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<BankQuery>,
    Query(scope): Query<crate::api::branches::BranchQuery>,
) -> AppResult<Json<serde_json::Value>> {
    use crate::api::branches;
    let mut conn = state.pool.acquire().await?;
    // Счёт — у филиала свой: в «Все филиалы» — счёт по умолчанию каждого филиала.
    let account = if scope.all()? { None } else { q.account_id };
    let (from, to) = (q.from, q.to);
    Ok(Json(
        branches::report(&mut conn, &user, &scope, |c, u| {
            Box::pin(async move { branches::to_json(&bank_data(c, &u, from, to, account).await?) })
        })
        .await?,
    ))
}

/// Экран «Счёт» (SPEC-27 §6): только владельцу. Каждая сумма — из движений кассы «Счёт» за
/// период; карта и перевод различаются по оплатам чека и по способу погашения.
pub(crate) async fn bank_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    account: Option<Uuid>,
) -> AppResult<BankOut> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let (from, to) = resolve_period(&mut *conn, from, to).await?;
    let branch_id = user.branch_id;
    // Любой безналичный счёт, и архивный — его история остаётся (ADR-076); без выбора — счёт по
    // умолчанию для карты (ADR-077).
    let account_id = match account {
        Some(id) => {
            let kind = sqlx::query_scalar!(
                "select kind from cash_accounts where id = $1 and branch_id = $2",
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if kind != "bank" {
                return Err(invalid("это не безналичный счёт"));
            }
            id
        }
        None => crate::api::cash::bank_account(&mut *conn, branch_id, "card")
            .await?
            .ok_or_else(|| invalid("нет безналичного счёта"))?,
    };
    let acc = sqlx::query!(
        r#"select name, balance_tyiyn,
                  coalesce((select sum(m.amount_tyiyn) from cash_movements m
                            where m.account_id = $1
                              and m.created_at < ($2::date)::timestamp at time zone 'Asia/Bishkek'), 0)::bigint as "opening!"
           from cash_accounts where id = $1"#,
        account_id,
        from
    )
    .fetch_one(&mut *conn)
    .await?;
    let rows = sqlx::query!(
        r#"select (m.created_at at time zone 'Asia/Bishkek')::date as "day!", m.kind, m.doc_type,
                  m.doc_id, m.amount_tyiyn
           from cash_movements m
           where m.account_id = $1
             and m.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek'
             and m.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek'
           order by m.created_at"#,
        account_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?;
    // Карта и «По счёту» в чеках: оплаты чеков, чьи деньги пришли на счёт в этот период.
    let sale_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.doc_type == "sale" && matches!(r.kind.as_str(), "sale" | "sale_return"))
        .filter_map(|r| r.doc_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let card_of_sale: HashMap<Uuid, (i64, i64)> = sqlx::query!(
        r#"select sale_id,
                  coalesce(sum(amount_tyiyn) filter (where method = 'card'), 0)::bigint as "card!",
                  coalesce(sum(amount_tyiyn) filter (where method = 'invoice'), 0)::bigint as "invoice!"
           from sale_payments where sale_id = any($1) and method in ('card', 'invoice') and account_id = $2
           group by sale_id"#,
        &sale_ids,
        account_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.sale_id, (r.card, r.invoice)))
    .collect();
    // Способ погашения долга записан в журнале операции (SPEC-10).
    let repay_ids: Vec<Uuid> = rows
        .iter()
        .filter(|r| r.doc_type == "repayment" && r.kind == "debt_repayment")
        .filter_map(|r| r.doc_id)
        .collect();
    let repay_ids_text: Vec<String> = repay_ids.iter().map(Uuid::to_string).collect();
    let transfer_repay: HashSet<Uuid> = sqlx::query_scalar!(
        r#"select (data->>'repayment_id')::uuid as "id!" from audit_log
           where branch_id = $1 and action = 'debt.repayment'
             and data->>'repayment_id' = any($2) and data->>'method' = 'transfer'"#,
        branch_id,
        &repay_ids_text
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .collect();

    let mut card = BankIncome::default();
    let mut transfer = BankIncome::default();
    let mut invoice = BankIncome::default();
    let (mut fee, mut other_in, mut outgoing) = (0i64, 0i64, 0i64);
    let mut kinds: BTreeMap<String, KindRow> = BTreeMap::new();
    let mut days: BTreeMap<NaiveDate, BankDay> = BTreeMap::new();
    // Сумма движений чека на счёт: часть картой и «По счёту» — по оплатам чека, остальное —
    // переводом.
    let mut sale_sums: HashMap<Uuid, (NaiveDate, i64)> = HashMap::new();
    for r in &rows {
        let k = kinds.entry(r.kind.clone()).or_insert_with(|| KindRow {
            kind: r.kind.clone(),
            ..KindRow::default()
        });
        if r.amount_tyiyn >= 0 {
            k.in_tyiyn = k.in_tyiyn.saturating_add(r.amount_tyiyn);
        } else {
            k.out_tyiyn = k.out_tyiyn.saturating_sub(r.amount_tyiyn);
        }
        let d = days.entry(r.day).or_insert_with(|| BankDay {
            date: r.day,
            ..BankDay::default()
        });
        d.net_tyiyn = d.net_tyiyn.saturating_add(r.amount_tyiyn);
        let sale_doc = r.doc_type == "sale" && matches!(r.kind.as_str(), "sale" | "sale_return");
        match (r.kind.as_str(), sale_doc, r.doc_id) {
            (_, true, Some(id)) => {
                let e = sale_sums.entry(id).or_insert((r.day, 0));
                e.1 = e.1.saturating_add(r.amount_tyiyn);
            }
            ("debt_repayment", _, Some(id)) if r.doc_type == "repayment" => {
                if transfer_repay.contains(&id) {
                    transfer.repayments_tyiyn =
                        transfer.repayments_tyiyn.saturating_add(r.amount_tyiyn);
                    d.transfer_tyiyn = d.transfer_tyiyn.saturating_add(r.amount_tyiyn);
                } else {
                    card.repayments_tyiyn = card.repayments_tyiyn.saturating_add(r.amount_tyiyn);
                    d.card_tyiyn = d.card_tyiyn.saturating_add(r.amount_tyiyn);
                }
            }
            ("bank_fee", _, _) => {
                fee = fee.saturating_sub(r.amount_tyiyn);
                d.fee_tyiyn = d.fee_tyiyn.saturating_sub(r.amount_tyiyn);
            }
            _ => {
                if r.amount_tyiyn >= 0 {
                    other_in = other_in.saturating_add(r.amount_tyiyn);
                } else {
                    outgoing = outgoing.saturating_sub(r.amount_tyiyn);
                }
                d.other_tyiyn = d.other_tyiyn.saturating_add(r.amount_tyiyn);
            }
        }
    }
    for (id, (day, sum)) in sale_sums {
        let (by_card, by_invoice) = card_of_sale.get(&id).copied().unwrap_or((0, 0));
        let by_transfer = sum.saturating_sub(by_card).saturating_sub(by_invoice);
        card.sales_tyiyn = card.sales_tyiyn.saturating_add(by_card);
        invoice.sales_tyiyn = invoice.sales_tyiyn.saturating_add(by_invoice);
        transfer.sales_tyiyn = transfer.sales_tyiyn.saturating_add(by_transfer);
        if let Some(d) = days.get_mut(&day) {
            d.card_tyiyn = d.card_tyiyn.saturating_add(by_card);
            d.invoice_tyiyn = d.invoice_tyiyn.saturating_add(by_invoice);
            d.transfer_tyiyn = d.transfer_tyiyn.saturating_add(by_transfer);
        }
    }
    card.total_tyiyn = card.sales_tyiyn.saturating_add(card.repayments_tyiyn);
    transfer.total_tyiyn = transfer
        .sales_tyiyn
        .saturating_add(transfer.repayments_tyiyn);
    invoice.total_tyiyn = invoice.sales_tyiyn;
    let change = rows
        .iter()
        .fold(0i64, |s, r| s.saturating_add(r.amount_tyiyn));
    let movements =
        crate::api::cash::movements_between(&mut *conn, user, account_id, Some((from, to)), 500)
            .await?;
    Ok(BankOut {
        account_id,
        name: acc.name,
        from,
        to,
        balance_tyiyn: acc.balance_tyiyn,
        opening_tyiyn: acc.opening,
        closing_tyiyn: acc.opening.saturating_add(change),
        card,
        transfer,
        invoice,
        fee_tyiyn: fee,
        other_in_tyiyn: other_in,
        outgoing_tyiyn: outgoing,
        kinds: kinds.into_values().collect(),
        days: days.into_values().rev().collect(),
        movements,
        movements_count: i64::try_from(rows.len()).unwrap_or(i64::MAX),
    })
}

// ---------- Проданное по накладным ----------

/// Товар накладной: сколько привезли, сколько из этого продано и осталось (оценка).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReceiptItem {
    pub product_id: Uuid,
    pub name: String,
    pub unit: String,
    pub container_ml: Option<i64>,
    pub qty: i64,
    pub sold_qty: i64,
    pub left_qty: i64,
    /// Закупка по накладной.
    pub cost_tyiyn: i64,
    /// Закупка того, что осталось (доля строки), и проданного (остальное).
    pub left_cost_tyiyn: i64,
    pub sold_cost_tyiyn: i64,
    /// Выручка с проданного: по средней цене продаж товара после прихода.
    pub revenue_tyiyn: i64,
    /// Заработано: выручка с проданного минус его закупка.
    pub earned_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReceiptSold {
    pub receipt_id: Uuid,
    pub number: i64,
    pub created_at: DateTime<Utc>,
    pub supplier_name: Option<String>,
    pub cost_tyiyn: i64,
    pub sold_cost_tyiyn: i64,
    pub left_cost_tyiyn: i64,
    pub revenue_tyiyn: i64,
    pub earned_tyiyn: i64,
    pub items: Vec<ReceiptItem>,
}

/// Накладных в отчёте — не больше (новые сверху).
const RECEIPTS_LIMIT: i64 = 100;

/// Накладные за период: что из привезённого продано, что осталось и сколько заработано —
/// оценка на сейчас (SPEC-27 §6). Себестоимость и заработок — только владельцу (инвариант 13).
pub async fn receipts_sold(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<ReceiptSold>> {
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let branch_id = user.branch_id;
    // Строки проведённых накладных без сторнированных; влитая карточка — за основную (ADR-070).
    let lines = sqlx::query!(
        r#"with recursive fam (id, root, depth) as (
             select id, id, 0 from products where merged_into is null
             union all
             select p.id, fam.root, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           ), recent as (
             select r.id from receipts r
             where r.branch_id = $1 and r.reversal_of is null
               and not exists (select 1 from receipts x where x.reversal_of = r.id)
               and r.created_at >= ($2::date)::timestamp at time zone 'Asia/Bishkek'
               and r.created_at < ($3::date + 1)::timestamp at time zone 'Asia/Bishkek'
             order by r.created_at desc limit $4
           )
           select r.id as receipt_id, r.number, r.created_at, s.name as "supplier_name?",
                  fam.root as "product_id!", p.name, p.unit, p.container_ml,
                  sum(l.qty)::bigint as "qty!", sum(l.cost_tyiyn)::bigint as "cost!"
           from receipt_lines l
           join receipts r on r.id = l.receipt_id
           join recent on recent.id = r.id
           left join suppliers s on s.id = r.supplier_id
           join fam on fam.id = l.product_id
           join products p on p.id = fam.root
           where l.qty > 0
           group by r.id, r.number, r.created_at, s.name, fam.root, p.name, p.unit, p.container_ml
           order by r.created_at desc, p.name"#,
        branch_id,
        from,
        to,
        RECEIPTS_LIMIT
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = lines
        .iter()
        .map(|l| l.product_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let stock: HashMap<Uuid, i64> = sqlx::query!(
        "select product_id, stock_qty from branch_products where branch_id = $1 and product_id = any($2)",
        branch_id,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.product_id, r.stock_qty))
    .collect();
    let inflows = crate::api::deliveries::inflows(&mut *conn, branch_id, &ids).await?;

    // Продажи товара после прихода — одним запросом на все строки: сумма и единицы за вычетом
    // возвратов и без подарков, влитые карточки — вместе с основной (ADR-070).
    let roots: Vec<Uuid> = lines.iter().map(|l| l.product_id).collect();
    let since: Vec<DateTime<Utc>> = lines.iter().map(|l| l.created_at).collect();
    let sold_rows = sqlx::query!(
        r#"with recursive fam (id, root, depth) as (
             select p.id, p.id, 0 from products p where p.id = any($2)
             union all
             select p.id, fam.root, fam.depth + 1 from products p join fam on p.merged_into = fam.id
             where fam.depth < 8
           )
           select x.i as "i!", coalesce(t.amount, 0)::bigint as "amount!",
                  coalesce(t.units, 0)::bigint as "units!"
           from unnest($2::uuid[], $3::timestamptz[]) with ordinality as x(root, since, i)
           left join lateral (
             select sum(l.amount_tyiyn) as amount,
                    sum(case when s.kind = 'return' then -l.units else l.units end) as units
             from fam
             join sale_lines l on l.product_id = fam.id
             join sales s on s.id = l.sale_id
             where fam.root = x.root and s.branch_id = $1 and not l.gift
               and s.created_at >= x.since
           ) t on true"#,
        branch_id,
        &roots,
        &since
    )
    .fetch_all(&mut *conn)
    .await?;
    let sold_of: HashMap<i64, (i64, i64)> = sold_rows
        .into_iter()
        .map(|r| (r.i, (r.amount, r.units)))
        .collect();

    let mut out: Vec<ReceiptSold> = Vec::new();
    for (n, l) in lines.into_iter().enumerate() {
        let (sold_amount, sold_units) = i64::try_from(n)
            .ok()
            .and_then(|n| sold_of.get(&(n + 1)))
            .copied()
            .unwrap_or((0, 0));
        let tagged: Vec<(bool, i64)> = inflows
            .get(&l.product_id)
            .map(|v| {
                v.iter()
                    .map(|f| (f.receipt_id == Some(l.receipt_id), f.qty))
                    .collect()
            })
            .unwrap_or_default();
        let left_qty = crate::api::deliveries::fifo_estimate(
            stock.get(&l.product_id).copied().unwrap_or(0),
            &tagged,
            l.qty,
        );
        let sold_qty = l.qty.saturating_sub(left_qty);
        let left_cost =
            div_round(i128::from(l.cost) * i128::from(left_qty), i128::from(l.qty)).unwrap_or(0);
        let sold_cost = l.cost.saturating_sub(left_cost);
        let revenue = if sold_units > 0 {
            div_round(
                i128::from(sold_amount) * i128::from(sold_qty),
                i128::from(sold_units),
            )
            .unwrap_or(0)
        } else {
            0
        };
        let item = ReceiptItem {
            product_id: l.product_id,
            name: l.name,
            unit: l.unit,
            container_ml: l.container_ml,
            qty: l.qty,
            sold_qty,
            left_qty,
            cost_tyiyn: l.cost,
            left_cost_tyiyn: left_cost,
            sold_cost_tyiyn: sold_cost,
            revenue_tyiyn: revenue,
            earned_tyiyn: revenue.saturating_sub(sold_cost),
        };
        match out.last_mut() {
            Some(r) if r.receipt_id == l.receipt_id => r.items.push(item),
            _ => out.push(ReceiptSold {
                receipt_id: l.receipt_id,
                number: l.number,
                created_at: l.created_at,
                supplier_name: l.supplier_name,
                cost_tyiyn: 0,
                sold_cost_tyiyn: 0,
                left_cost_tyiyn: 0,
                revenue_tyiyn: 0,
                earned_tyiyn: 0,
                items: vec![item],
            }),
        }
    }
    for r in &mut out {
        for i in &r.items {
            r.cost_tyiyn = r.cost_tyiyn.saturating_add(i.cost_tyiyn);
            r.left_cost_tyiyn = r.left_cost_tyiyn.saturating_add(i.left_cost_tyiyn);
            r.revenue_tyiyn = r.revenue_tyiyn.saturating_add(i.revenue_tyiyn);
            r.earned_tyiyn = r.earned_tyiyn.saturating_add(i.earned_tyiyn);
        }
        r.sold_cost_tyiyn = r.cost_tyiyn.saturating_sub(r.left_cost_tyiyn);
    }
    Ok(out)
}

// ---------- «Куда сколько поступило» ----------

/// Выручка категории — без себестоимости: отчёт видит и администратор.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CategoryRevenue {
    pub name: String,
    pub revenue_tyiyn: i64,
}

#[derive(Serialize)]
pub struct IncomeOut {
    pub from: NaiveDate,
    pub to: NaiveDate,
    /// Оплаты чеков по способам: наличные, карта, перевод, долг, баллы, аккумулятор.
    pub methods: Vec<MethodRow>,
    /// Выручка (товары + работы) — тот же расчёт, что в отчёте о прибыли; равна сумме способов.
    pub revenue_tyiyn: i64,
    pub goods_tyiyn: i64,
    pub services_tyiyn: i64,
    /// Итоги строк способов: оплачено в продажах, возвращено, комиссия банка.
    pub sales_tyiyn: i64,
    pub returns_tyiyn: i64,
    pub fee_tyiyn: i64,
    /// Деньги по кассам: на начало, пришло и ушло по видам, на конец.
    pub accounts: Vec<AccountFlow>,
    pub categories: Vec<CategoryRevenue>,
    pub products: Vec<TopProduct>,
    /// По накладным — только владельцу.
    pub receipts: Option<Vec<ReceiptSold>>,
}

async fn income(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<Period>,
    Query(scope): Query<crate::api::branches::BranchQuery>,
) -> AppResult<Json<serde_json::Value>> {
    use crate::api::branches;
    let mut conn = state.pool.acquire().await?;
    let (from, to) = (q.from, q.to);
    Ok(Json(
        branches::report(&mut conn, &user, &scope, |c, u| {
            Box::pin(async move { branches::to_json(&income_data(c, &u, from, to).await?) })
        })
        .await?,
    ))
}

/// Отчёт «Куда сколько поступило» — экран и бот владельца (SPEC-27 §6). Администратору — без
/// сейфа владельца и без накладных (там себестоимость и заработок).
pub(crate) async fn income_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<IncomeOut> {
    let (from, to) = resolve_period(&mut *conn, from, to).await?;
    let branch_id = user.branch_id;
    let methods = methods_for(&mut *conn, branch_id, from, to).await?;
    let totals = crate::api::reports::totals_for(&mut *conn, branch_id, from, to).await?;
    let accounts = account_flows(&mut *conn, user, from, to).await?;
    let categories = crate::api::reports::categories_for(&mut *conn, branch_id, from, to)
        .await?
        .into_iter()
        .map(|c| CategoryRevenue {
            name: c.name,
            revenue_tyiyn: c.revenue_tyiyn,
        })
        .collect();
    let products = crate::api::reports::top_products_data(
        &mut *conn,
        user,
        TopQuery {
            from: Some(from),
            to: Some(to),
            limit: Some(100),
            ..TopQuery::default()
        },
    )
    .await?
    .items;
    let receipts = if user.is_owner() {
        Some(receipts_sold(&mut *conn, user, from, to).await?)
    } else {
        None
    };
    Ok(IncomeOut {
        from,
        to,
        sales_tyiyn: methods
            .iter()
            .fold(0i64, |s, m| s.saturating_add(m.sales_tyiyn)),
        returns_tyiyn: methods
            .iter()
            .fold(0i64, |s, m| s.saturating_add(m.returns_tyiyn)),
        fee_tyiyn: methods
            .iter()
            .fold(0i64, |s, m| s.saturating_add(m.fee_tyiyn)),
        methods,
        revenue_tyiyn: totals.goods_tyiyn.saturating_add(totals.services_tyiyn),
        goods_tyiyn: totals.goods_tyiyn,
        services_tyiyn: totals.services_tyiyn,
        accounts,
        categories,
        products,
        receipts,
    })
}
