//! Накладные поставщика в долг: оплачено, осталось, статус и история платежей (SPEC-27 §5,
//! SPEC-10, ADR-073). Что чем закрыто, хранит `supplier_allocations`; распределяет функция базы
//! `supplier_allocate` — одна и для проведения, и для миграции старых оплат.
//!
//! Старый долг поставщику из тетради (задача 003 Н15, ADR-{g1.1}) — `POST /debts/old-supplier`:
//! запись долга с датой долга и номером накладной поставщика; гасится оплатой, как накладная.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::parties::{OLD_DEBT_DOC, old_debt_text};
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/parties/{id}/invoices", routing::get(invoices))
        .route("/debts/old-supplier", routing::post(post_old_debt))
}

/// Распределяет долг поставщика: снятые записи теряют свои распределения, открытые оплаты
/// ложатся на самые старые долги. В транзакции записи долга, под блокировкой контрагента.
pub async fn allocate(conn: &mut PgConnection, party_id: Uuid) -> AppResult<()> {
    sqlx::query_scalar!(r#"select supplier_allocate($1) as "n!""#, party_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(())
}

/// Оплата выбранной накладной: сумма ложится на неё (проверена вызывающим: не больше остатка).
pub async fn allocate_to(
    conn: &mut PgConnection,
    ctx: &Ctx,
    party_id: Uuid,
    credit_id: Uuid,
    debit_id: Uuid,
    amount: i64,
) -> AppResult<()> {
    if amount <= 0 {
        return Err(invalid("сумма оплаты больше нуля"));
    }
    sqlx::query!(
        r#"insert into supplier_allocations (id, branch_id, party_id, credit_id, debit_id, amount_tyiyn)
           values ($1, $2, $3, $4, $5, $6)"#,
        new_id(),
        ctx.user.branch_id,
        party_id,
        credit_id,
        debit_id,
        amount
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Остаток долга по накладной.
pub struct InvoiceOpen {
    pub ledger_id: Uuid,
    pub number: i64,
    pub open_tyiyn: i64,
}

/// Сколько осталось заплатить по накладной в долг этого поставщика.
pub async fn invoice_open(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Uuid,
    receipt_id: Uuid,
) -> AppResult<InvoiceOpen> {
    let row = sqlx::query!(
        r#"select l.id, r.number,
                  (-l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                               where s.debit_id = l.id), 0))::bigint as "open!"
           from party_ledger l join receipts r on r.id = l.doc_id
           where l.branch_id = $1 and l.party_id = $2 and l.doc_type = 'receipt' and l.doc_id = $3
             and l.amount_tyiyn < 0 and supplier_ledger_live(l)"#,
        branch_id,
        party_id,
        receipt_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| {
        invalid("накладная не в долг у этого поставщика или отменена — оплачивать нечего")
    })?;
    if row.open <= 0 {
        return Err(invalid(format!("накладная № {} уже оплачена", row.number)));
    }
    Ok(InvoiceOpen {
        ledger_id: row.id,
        number: row.number,
        open_tyiyn: row.open,
    })
}

/// Статус накладной по оплате.
pub fn status(total: i64, paid: i64) -> &'static str {
    if paid <= 0 {
        "unpaid"
    } else if paid >= total {
        "paid"
    } else {
        "partial"
    }
}

/// Долг перед поставщиком: накладная в долг, старый долг из тетради или правка владельца.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InvoiceOut {
    pub ledger_id: Uuid,
    /// `receipt` — накладная в долг, `old_debt` — старый долг из тетради (задача 003 Н15),
    /// `adjust` — правка владельца.
    #[serde(default)]
    pub kind: String,
    /// Кто занёс старый долг; у накладной и правки — пусто.
    #[serde(default)]
    pub user_name: String,
    pub receipt_id: Option<Uuid>,
    pub number: Option<i64>,
    pub supplier_doc: String,
    pub business_date: NaiveDate,
    pub created_at: DateTime<Utc>,
    pub total_tyiyn: i64,
    pub paid_tyiyn: i64,
    pub left_tyiyn: i64,
    /// `unpaid` — не оплачена, `partial` — частично, `paid` — оплачена.
    pub status: String,
    pub comment: String,
}

/// На какой долг легла часть платежа.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Applied {
    pub ledger_id: Uuid,
    pub receipt_id: Option<Uuid>,
    pub number: Option<i64>,
    /// Сколько лежит сейчас (после отмен).
    pub amount_tyiyn: i64,
    /// Сколько ложилось всего, до отмен: у сторнированной оплаты видно, что она закрывала.
    pub gross_tyiyn: i64,
}

/// Платёж поставщику или зачёт: кто, когда, сколько и откуда (SPEC-27).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PaymentOut {
    pub ledger_id: Uuid,
    pub at: DateTime<Utc>,
    pub user_name: String,
    /// `payment` — оплата, `defect` — зачёт брака скидкой, `return` — возврат товара в счёт
    /// долга (SPEC-29 §9), `adjust` — правка владельца.
    pub kind: String,
    pub amount_tyiyn: i64,
    /// Касса, из которой платили; `outside` — мимо касс (до ADR-079); пусто — зачёт, возврат или правка.
    pub source: String,
    /// Платёж сторнирован: его распределения отменены.
    pub reversed: bool,
    pub comment: String,
    pub applied: Vec<Applied>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SupplierInvoices {
    /// Долги поставщику (накладные и правки), от старых к новым.
    pub invoices: Vec<InvoiceOut>,
    /// Осталось заплатить по всем долгам.
    pub debt_tyiyn: i64,
    /// Аванс: заплачено или зачтено больше, чем есть долгов, — ляжет на следующие накладные.
    pub advance_tyiyn: i64,
    /// Платежи и зачёты, новые сверху.
    pub payments: Vec<PaymentOut>,
}

async fn invoices(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<SupplierInvoices>> {
    let mut conn = state.pool.acquire().await?;
    // Накладные поставщика — раздел «Поставщики» или оплата поставщику (окно оплаты долга),
    // задача 003 Н8, ADR-151.
    if !crate::auth::has_permission(&mut conn, &user, "view_suppliers").await? {
        crate::auth::require_permission(&mut conn, &user, "receipts").await?;
    }
    let role = sqlx::query_scalar!("select role from parties where id = $1", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound)?;
    if role != "supplier" {
        return Err(invalid("накладные есть только у поставщика"));
    }
    Ok(Json(
        supplier_invoices(&mut conn, user.branch_id, id).await?,
    ))
}

/// Накладные поставщика с остатком долга и история платежей — карточка поставщика, накладная, бот.
pub async fn supplier_invoices(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Uuid,
) -> AppResult<SupplierInvoices> {
    let debts = sqlx::query!(
        r#"select l.id, l.doc_type, l.business_date, l.created_at, l.comment, l.supplier_doc as ledger_doc,
                  u.full_name as user_name,
                  -l.amount_tyiyn as "total!",
                  coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                            where s.debit_id = l.id), 0)::bigint as "paid!",
                  r.id as "receipt_id?", r.number as "number?", r.supplier_doc as "supplier_doc?"
           from party_ledger l
           join users u on u.id = l.user_id
           left join receipts r on l.doc_type = 'receipt' and r.id = l.doc_id
           where l.branch_id = $1 and l.party_id = $2 and l.amount_tyiyn < 0
             and supplier_ledger_live(l)
           order by l.business_date, l.created_at, l.id"#,
        branch_id,
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut debt: i64 = 0;
    let invoices: Vec<InvoiceOut> = debts
        .into_iter()
        .map(|d| {
            let left = d.total.saturating_sub(d.paid);
            debt = debt.saturating_add(left);
            let old = d.doc_type == OLD_DEBT_DOC;
            InvoiceOut {
                ledger_id: d.id,
                kind: match d.doc_type.as_str() {
                    "receipt" => "receipt",
                    OLD_DEBT_DOC => OLD_DEBT_DOC,
                    _ => "adjust",
                }
                .into(),
                user_name: if old { d.user_name } else { String::new() },
                receipt_id: d.receipt_id,
                number: d.number,
                // Номер накладной поставщика: у прихода — из прихода, у старого долга — свой.
                supplier_doc: d.supplier_doc.unwrap_or(d.ledger_doc),
                business_date: d.business_date,
                created_at: d.created_at,
                total_tyiyn: d.total,
                paid_tyiyn: d.paid,
                left_tyiyn: left,
                status: status(d.total, d.paid).into(),
                comment: if old {
                    old_debt_text(&d.comment).to_string()
                } else {
                    d.comment
                },
            }
        })
        .collect();
    let allocs = sqlx::query!(
        r#"select s.credit_id, s.debit_id, r.id as "receipt_id?", r.number as "number?",
                  sum(s.amount_tyiyn)::bigint as "amount!",
                  coalesce(sum(s.amount_tyiyn) filter (where s.amount_tyiyn > 0), 0)::bigint as "gross!",
                  min(s.created_at) as "first!"
           from supplier_allocations s
           join party_ledger d on d.id = s.debit_id
           left join receipts r on d.doc_type = 'receipt' and r.id = d.doc_id
           where s.branch_id = $1 and s.party_id = $2
           group by s.credit_id, s.debit_id, r.id, r.number
           order by min(s.created_at)"#,
        branch_id,
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by_credit: HashMap<Uuid, Vec<Applied>> = HashMap::new();
    for a in allocs {
        by_credit.entry(a.credit_id).or_default().push(Applied {
            ledger_id: a.debit_id,
            receipt_id: a.receipt_id,
            number: a.number,
            amount_tyiyn: a.amount,
            gross_tyiyn: a.gross,
        });
    }
    let credits = sqlx::query!(
        r#"select l.id, l.doc_type, l.amount_tyiyn, l.created_at, l.comment, u.full_name as user_name,
                  (select a.name from cash_movements m join cash_accounts a on a.id = m.account_id
                    where l.doc_type = 'repayment' and m.doc_type = 'repayment' and m.doc_id = l.doc_id
                      and m.kind = 'supplier_payment'
                    order by m.created_at limit 1) as "account?",
                  supplier_ledger_live(l) as "live!",
                  (l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                              where s.credit_id = l.id), 0))::bigint as "open!"
           from party_ledger l join users u on u.id = l.user_id
           where l.branch_id = $1 and l.party_id = $2 and l.amount_tyiyn > 0 and l.reversal_of is null
             and l.doc_type in ('repayment', 'defect', 'adjust', 'supplier_return')
           order by l.created_at desc, l.id desc"#,
        branch_id,
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut advance: i64 = 0;
    let payments = credits
        .into_iter()
        .map(|c| {
            if c.live {
                advance = advance.saturating_add(c.open);
            }
            let kind = credit_kind(&c.doc_type);
            PaymentOut {
                ledger_id: c.id,
                at: c.created_at,
                user_name: c.user_name,
                kind: kind.into(),
                amount_tyiyn: c.amount_tyiyn,
                source: credit_source(kind, c.account),
                reversed: !c.live,
                comment: c.comment,
                applied: by_credit.remove(&c.id).unwrap_or_default(),
            }
        })
        .collect();
    Ok(SupplierInvoices {
        invoices,
        debt_tyiyn: debt,
        advance_tyiyn: advance,
        payments,
    })
}

/// Оплата накладной в долг: сколько заплачено, сколько осталось и кто платил (SPEC-27).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReceiptDebt {
    pub total_tyiyn: i64,
    pub paid_tyiyn: i64,
    pub left_tyiyn: i64,
    pub status: String,
    pub payments: Vec<InvoicePayment>,
}

/// Платёж по одной накладной: сколько из него легло на неё.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InvoicePayment {
    pub ledger_id: Uuid,
    pub at: DateTime<Utc>,
    pub user_name: String,
    pub kind: String,
    pub source: String,
    /// Сколько легло на эту накладную (у сторнированного — сколько ложилось до сторно).
    pub amount_tyiyn: i64,
    /// Весь платёж: он мог закрыть и другие накладные.
    pub payment_tyiyn: i64,
    pub reversed: bool,
    pub comment: String,
}

/// Вид закрывающей записи: оплата, зачёт брака, возврат поставщику или правка.
fn credit_kind(doc_type: &str) -> &'static str {
    match doc_type {
        "repayment" => "payment",
        "defect" => "defect",
        "supplier_return" => "return",
        _ => "adjust",
    }
}

/// Откуда оплата: название кассы, «outside» — из своих; у зачётов и правок — пусто.
fn credit_source(kind: &str, account: Option<String>) -> String {
    match (kind, account) {
        ("payment", Some(a)) => a,
        ("payment", None) => "outside".into(),
        _ => String::new(),
    }
}

/// Оплата накладной: `None`, если она не в долг (оплачена сразу) или снята сторно. Читает только
/// эту накладную и легшие на неё платежи, а не всю историю поставщика.
pub async fn receipt_debt(
    conn: &mut PgConnection,
    branch_id: Uuid,
    receipt_id: Uuid,
) -> AppResult<Option<ReceiptDebt>> {
    let Some(inv) = sqlx::query!(
        r#"select l.id, -l.amount_tyiyn as "total!",
                  coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                            where s.debit_id = l.id), 0)::bigint as "paid!"
           from party_ledger l
           join receipts r on r.id = l.doc_id
           where l.branch_id = $1 and l.doc_type = 'receipt' and l.doc_id = $2
             and l.amount_tyiyn < 0 and supplier_ledger_live(l)
           order by l.business_date, l.created_at, l.id
           limit 1"#,
        branch_id,
        receipt_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    let credits = sqlx::query!(
        r#"with a as (
             select s.credit_id, sum(s.amount_tyiyn)::bigint as amount,
                    coalesce(sum(s.amount_tyiyn) filter (where s.amount_tyiyn > 0), 0)::bigint as gross
             from supplier_allocations s where s.debit_id = $2
             group by s.credit_id
           )
           select l.id, l.doc_type, l.amount_tyiyn, l.created_at, l.comment, u.full_name as user_name,
                  (select a.name from cash_movements m join cash_accounts a on a.id = m.account_id
                    where l.doc_type = 'repayment' and m.doc_type = 'repayment' and m.doc_id = l.doc_id
                      and m.kind = 'supplier_payment'
                    order by m.created_at limit 1) as "account?",
                  supplier_ledger_live(l) as "live!",
                  a.amount as "applied!", a.gross as "gross!"
           from a
           join party_ledger l on l.id = a.credit_id
           join users u on u.id = l.user_id
           where l.branch_id = $1 and l.amount_tyiyn > 0 and l.reversal_of is null
             and l.doc_type in ('repayment', 'defect', 'adjust', 'supplier_return')
           order by l.created_at desc, l.id desc"#,
        branch_id,
        inv.id
    )
    .fetch_all(&mut *conn)
    .await?;
    let payments = credits
        .into_iter()
        .filter_map(|c| {
            let reversed = !c.live;
            let amount = if reversed { c.gross } else { c.applied };
            let kind = credit_kind(&c.doc_type);
            (amount > 0).then(|| InvoicePayment {
                ledger_id: c.id,
                at: c.created_at,
                user_name: c.user_name,
                kind: kind.into(),
                source: credit_source(kind, c.account),
                amount_tyiyn: amount,
                payment_tyiyn: c.amount_tyiyn,
                reversed,
                comment: c.comment,
            })
        })
        .collect();
    let left = inv.total.saturating_sub(inv.paid);
    Ok(Some(ReceiptDebt {
        total_tyiyn: inv.total,
        paid_tyiyn: inv.paid,
        left_tyiyn: left,
        status: status(inv.total, inv.paid).into(),
        payments,
    }))
}

// ---------- Старый долг поставщику из тетради (задача 003 Н15, ADR-{g1.1}) ----------

/// Номер накладной поставщика у старого долга — не длиннее (проверка базы — та же).
const SUPPLIER_DOC_MAX: usize = 100;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct SupplierOldDebtReq {
    pub op_id: Uuid,
    pub party_id: Uuid,
    /// Сколько мы должны поставщику, больше нуля.
    pub amount_tyiyn: i64,
    /// Когда остался долг: учётный день записи — от него идёт очередь погашения.
    pub date: NaiveDate,
    /// Номер накладной поставщика, по желанию.
    #[serde(default)]
    pub supplier_doc: String,
    /// Что брали, страница тетради.
    #[serde(default)]
    pub comment: String,
    /// Такой же старый долг (сумма и дата) у поставщика в этом филиале уже есть — занести ещё раз.
    #[serde(default)]
    pub confirm_duplicate: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SupplierOldDebtOut {
    pub party_id: Uuid,
    pub ledger_id: Uuid,
    /// Баланс поставщика в филиале после записи: меньше нуля — мы должны.
    pub balance_tyiyn: i64,
}

async fn post_old_debt(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SupplierOldDebtReq>,
) -> AppResult<Json<SupplierOldDebtOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_supplier_old_debt_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Старый долг поставщику из тетради: запись «долг» с минусом (мы должны) в филиале, учётный
/// день — дата долга. Деньги, склад, себестоимость и прибыль не меняются: товар получен до
/// программы. Оплата поставщику гасит его, как накладную, — самые старые долги первыми
/// (`supplier_allocate`). Экран, «Исправить» и бот владельца проводят одной функцией.
pub async fn post_supplier_old_debt_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: SupplierOldDebtReq,
) -> AppResult<SupplierOldDebtOut> {
    const KIND: &str = "debt.old_supplier";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма долга больше нуля"));
    }
    ops::check_amount(req.amount_tyiyn)?;
    let text = req.comment.trim();
    if text.chars().count() > 500 {
        return Err(invalid("комментарий — не длиннее 500 знаков"));
    }
    let doc = crate::api::bot_drafts::one_line(req.supplier_doc.trim());
    if doc.chars().count() > SUPPLIER_DOC_MAX {
        return Err(invalid("номер накладной — не длиннее 100 знаков"));
    }
    let today = crate::api::fees::today(&mut *conn).await?;
    crate::api::parties::check_old_debt_date(req.date, today)?;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    crate::auth::require_permission(&mut *conn, &ctx.user, "old_debts").await?;
    let p = sqlx::query!(
        "select name, role from parties where id = $1 for update",
        req.party_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("поставщик не найден"))?;
    if p.role != "supplier" {
        return Err(invalid(
            "долг поставщику заносится поставщику; старый долг клиента — «Занести старый долг» на «Долгах»",
        ));
    }
    // Долг поставщику у каждого филиала свой (SPEC-33): повтор ищется в этом филиале.
    let amount = -req.amount_tyiyn;
    if !req.confirm_duplicate
        && let Some(same) = crate::api::parties::old_debt_duplicate(
            &mut *conn,
            req.party_id,
            amount,
            req.date,
            Some(ctx.user.branch_id),
        )
        .await?
    {
        return Err(AppError::Conflict(format!("{same}. Занести ещё раз?")));
    }
    let (_, ledger_id) = crate::api::parties::add_ledger_at(
        &mut *conn,
        ctx,
        crate::api::parties::LedgerEntry {
            party_id: req.party_id,
            kind: "debt",
            amount,
            doc_type: OLD_DEBT_DOC,
            doc_id: None,
            comment: &crate::api::parties::old_debt_comment(text),
        },
        None,
        Some(req.date),
        &doc,
    )
    .await?;
    let balance = branch_balance(&mut *conn, ctx.user.branch_id, req.party_id).await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "party",
        Some(req.party_id),
        json!({
            "name": p.name,
            "amount_tyiyn": req.amount_tyiyn,
            "date": req.date,
            "supplier_doc": doc,
            "comment": text,
            "balance_tyiyn": balance,
            "ledger_id": ledger_id,
        }),
    )
    .await?;
    let out = SupplierOldDebtOut {
        party_id: req.party_id,
        ledger_id,
        balance_tyiyn: balance,
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Баланс поставщика в филиале: меньше нуля — мы должны (SPEC-33 §2).
pub async fn branch_balance(
    conn: &mut PgConnection,
    branch_id: Uuid,
    party_id: Uuid,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce((select balance_tyiyn from party_balances
                            where party_id = $1 and branch_id = $2), 0)::bigint as "v!""#,
        party_id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::status;

    #[test]
    fn invoice_status_by_paid_part() {
        assert_eq!(status(1000, 0), "unpaid");
        assert_eq!(status(1000, 1), "partial");
        assert_eq!(status(1000, 999), "partial");
        assert_eq!(status(1000, 1000), "paid");
    }
}
