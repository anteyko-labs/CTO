//! Кабинет поставщика (задача 028, ADR-188): тот же вход по ИНН, что у юрлица-клиента
//! (`cabinet.rs`), а разделы свои — накладные с оплатой и файлами, деньги, возвраты. Только
//! просмотр; единственная запись — файл к своей накладной. Себестоимость, остатки, продажи, цены
//! продажи, кассы и сотрудники точки поставщику не отдаются (инвариант 13).
//!
//! Здесь же — сообщение поставщику в Телеграм о принятой и отменённой накладной.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cabinet::Client;
use crate::api::client_messages::{Channels, som};
use crate::api::receipt_files::{MAX_FILE_BYTES, sniff};
use crate::api::supplier_debts::supplier_invoices;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::new_id;
use crate::state::AppState;

/// Сколько файлов у одной накладной — как у точки.
const MAX_FILES: i64 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/supplier-cabinet/invoices", routing::get(invoices))
        .route(
            "/supplier-cabinet/invoices/{id}/files",
            routing::post(upload).layer(axum::extract::DefaultBodyLimit::max(MAX_FILE_BYTES)),
        )
        .route(
            "/supplier-cabinet/invoices/{id}/files/{file_id}",
            routing::get(download),
        )
        .route("/supplier-cabinet/payments", routing::get(payments))
        .route("/supplier-cabinet/returns", routing::get(returns))
}

#[derive(Deserialize)]
struct PeriodQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

// ---------- Сводка ----------

#[derive(Serialize, Default)]
pub struct BranchSummary {
    pub branch_id: Uuid,
    pub branch_name: String,
    /// Наш долг поставщику: осталось заплатить по накладным.
    pub debt_tyiyn: i64,
    /// Аванс: заплачено или зачтено больше долгов.
    pub advance_tyiyn: i64,
    pub unpaid_count: i64,
    pub unpaid_tyiyn: i64,
}

#[derive(Serialize, Default)]
pub struct Summary {
    pub debt_tyiyn: i64,
    pub advance_tyiyn: i64,
    pub unpaid_count: i64,
    pub unpaid_tyiyn: i64,
    /// По филиалам: долг поставщику у каждого филиала свой (SPEC-33 §2).
    pub branches: Vec<BranchSummary>,
}

/// Филиалы, где у поставщика есть движения долга; нет ни одного — филиал, где его завели.
async fn branches_of(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Vec<(Uuid, String)>> {
    let rows = sqlx::query!(
        r#"select b.id, b.name from branches b
           where exists (select 1 from party_ledger l where l.party_id = $1 and l.branch_id = b.id)
              or b.id = (select branch_id from parties where id = $1)
           order by b.is_main desc, b.created_at"#,
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.name)).collect())
}

/// Наш долг поставщику и его аванс по филиалам и итог — тем же расчётом, что карточка поставщика.
pub async fn summary(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Summary> {
    let mut out = Summary::default();
    for (branch_id, branch_name) in branches_of(conn, party_id).await? {
        let inv = supplier_invoices(conn, branch_id, party_id).await?;
        let unpaid: Vec<_> = inv.invoices.iter().filter(|i| i.left_tyiyn > 0).collect();
        let b = BranchSummary {
            branch_id,
            branch_name,
            debt_tyiyn: inv.debt_tyiyn,
            advance_tyiyn: inv.advance_tyiyn,
            unpaid_count: i64::try_from(unpaid.len()).unwrap_or(i64::MAX),
            unpaid_tyiyn: unpaid.iter().map(|i| i.left_tyiyn).sum(),
        };
        out.debt_tyiyn = out.debt_tyiyn.saturating_add(b.debt_tyiyn);
        out.advance_tyiyn = out.advance_tyiyn.saturating_add(b.advance_tyiyn);
        out.unpaid_count = out.unpaid_count.saturating_add(b.unpaid_count);
        out.unpaid_tyiyn = out.unpaid_tyiyn.saturating_add(b.unpaid_tyiyn);
        out.branches.push(b);
    }
    Ok(out)
}

// ---------- Накладные ----------

#[derive(Serialize)]
struct InvoiceLine {
    name: String,
    qty: i64,
    unit: String,
    container_ml: Option<i64>,
    /// Сумма строки по цене поставщика.
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct InvoiceFile {
    id: Uuid,
    name: String,
    content_type: String,
    size_bytes: i32,
    /// `staff` — приложила точка, `supplier` — сам поставщик.
    source: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct Invoice {
    id: Uuid,
    number: i64,
    supplier_doc: String,
    business_date: NaiveDate,
    created_at: DateTime<Utc>,
    branch_name: String,
    total_tyiyn: i64,
    paid_tyiyn: i64,
    left_tyiyn: i64,
    /// `now` — оплачена сразу, `unpaid`, `partial`, `paid`.
    status: String,
    /// Накладная отменена (сторно).
    reversed: bool,
    lines: Vec<InvoiceLine>,
    files: Vec<InvoiceFile>,
}

fn default_from() -> NaiveDate {
    (Utc::now() + chrono::Duration::hours(6)).date_naive() - chrono::Duration::days(90)
}

async fn invoices(
    State(state): State<AppState>,
    client: Client,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<Invoice>>> {
    client.supplier()?;
    let mut conn = state.pool.acquire().await?;
    let heads = sqlx::query!(
        r#"select r.id, r.number, r.supplier_doc, r.created_at, r.total_tyiyn,
                  (r.created_at at time zone 'Asia/Bishkek')::date as "day!",
                  case when (select count(*) from branches) > 1 then b.name else '' end as "branch_name!",
                  exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!",
                  d.debt as "debt?", d.paid as "paid?"
           from receipts r
           join branches b on b.id = r.branch_id
           left join lateral (
             select -l.amount_tyiyn as debt,
                    coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                              where s.debit_id = l.id), 0)::bigint as paid
             from party_ledger l
             where l.doc_type = 'receipt' and l.doc_id = r.id and l.amount_tyiyn < 0
               and l.reversal_of is null
             order by l.created_at limit 1) d on true
           where r.supplier_id = $1 and r.reversal_of is null
             and (r.created_at at time zone 'Asia/Bishkek')::date >= $2
             and (r.created_at at time zone 'Asia/Bishkek')::date <= coalesce($3, (now() at time zone 'Asia/Bishkek')::date)
           order by r.created_at desc limit 500"#,
        client.party_id,
        q.from.unwrap_or_else(default_from),
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = heads.iter().map(|h| h.id).collect();
    let lines = sqlx::query!(
        r#"select l.receipt_id, p.name, l.qty, p.unit, p.container_ml as "container_ml?", l.cost_tyiyn
           from receipt_lines l join products p on p.id = l.product_id
           where l.receipt_id = any($1) order by l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let files = sqlx::query!(
        r#"select id, receipt_id, name, content_type, size_bytes, source, created_at
           from receipt_files where receipt_id = any($1) and hidden_at is null
           order by created_at, id"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by_lines: HashMap<Uuid, Vec<InvoiceLine>> = HashMap::new();
    for l in lines {
        by_lines.entry(l.receipt_id).or_default().push(InvoiceLine {
            name: l.name,
            qty: l.qty,
            unit: l.unit,
            container_ml: l.container_ml,
            amount_tyiyn: l.cost_tyiyn,
        });
    }
    let mut by_files: HashMap<Uuid, Vec<InvoiceFile>> = HashMap::new();
    for f in files {
        by_files.entry(f.receipt_id).or_default().push(InvoiceFile {
            id: f.id,
            name: f.name,
            content_type: f.content_type,
            size_bytes: f.size_bytes,
            source: f.source,
            created_at: f.created_at,
        });
    }
    let out = heads
        .into_iter()
        .map(|h| {
            // В долг ушла часть `debt`; остальное оплачено сразу при приходе (SPEC-29).
            let (paid, left, status) = match (h.debt, h.paid) {
                (Some(debt), Some(alloc)) => {
                    let left = debt.saturating_sub(alloc).max(0);
                    let paid = h.total_tyiyn.saturating_sub(left);
                    let status = if left == 0 {
                        "paid"
                    } else if paid > 0 {
                        "partial"
                    } else {
                        "unpaid"
                    };
                    (paid, left, status)
                }
                _ => (h.total_tyiyn, 0, "now"),
            };
            Invoice {
                id: h.id,
                number: h.number,
                supplier_doc: h.supplier_doc,
                business_date: h.day,
                created_at: h.created_at,
                branch_name: h.branch_name,
                total_tyiyn: h.total_tyiyn,
                paid_tyiyn: if h.reversed { 0 } else { paid },
                left_tyiyn: if h.reversed { 0 } else { left },
                status: status.into(),
                reversed: h.reversed,
                lines: by_lines.remove(&h.id).unwrap_or_default(),
                files: by_files.remove(&h.id).unwrap_or_default(),
            }
        })
        .collect();
    Ok(Json(out))
}

/// Своя накладная поставщика (не сторно); чужая — 404 без подсказок.
async fn own_receipt(
    conn: &mut PgConnection,
    party_id: Uuid,
    receipt_id: Uuid,
    lock: bool,
) -> AppResult<(Uuid, i64)> {
    let r = if lock {
        sqlx::query!(
            r#"select branch_id, number from receipts
               where id = $1 and supplier_id = $2 and reversal_of is null for update"#,
            receipt_id,
            party_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| (r.branch_id, r.number))
    } else {
        sqlx::query!(
            r#"select branch_id, number from receipts
               where id = $1 and supplier_id = $2 and reversal_of is null"#,
            receipt_id,
            party_id
        )
        .fetch_optional(&mut *conn)
        .await?
        .map(|r| (r.branch_id, r.number))
    };
    r.ok_or(AppError::NotFound)
}

async fn download(
    State(state): State<AppState>,
    client: Client,
    Path((id, file_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Response> {
    client.supplier()?;
    let mut conn = state.pool.acquire().await?;
    own_receipt(&mut conn, client.party_id, id, false).await?;
    let f = sqlx::query!(
        r#"select name, content_type, data from receipt_files
           where id = $1 and receipt_id = $2 and hidden_at is null"#,
        file_id,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let mut res = f.data.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&f.content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    // Файл, который владелец скроет, не должен остаться в кэше браузера поставщика.
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(res)
}

#[derive(Deserialize)]
struct UploadQuery {
    op_id: Uuid,
    #[serde(default)]
    name: String,
}

/// Поставщик прикладывает файл к своей накладной (свой экземпляр, счёт-фактура). Повтор `op_id` —
/// тот же файл. Править и удалять нельзя; скрыть может только владелец (ADR-080).
async fn upload(
    State(state): State<AppState>,
    client: Client,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Value>> {
    client.supplier()?;
    let declared = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    let mut tx = state.pool.begin().await?;
    let (branch_id, number) = own_receipt(&mut tx, client.party_id, id, true).await?;
    // Повтор после потерянного ответа — тот же файл (инвариант 4).
    if let Some(done) = sqlx::query!(
        "select id, receipt_id, party_id from receipt_files where op_id = $1",
        q.op_id
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        if done.receipt_id != id || done.party_id != Some(client.party_id) {
            return Err(AppError::Conflict("op_id уже использован".into()));
        }
        tx.commit().await?;
        return Ok(Json(json!({ "id": done.id })));
    }
    if body.is_empty() {
        return Err(invalid("файл пустой"));
    }
    let kind =
        sniff(&body).ok_or_else(|| invalid("файл — снимок JPEG, PNG, WEBP или документ PDF"))?;
    if !declared.is_empty() && declared != kind && declared != "application/octet-stream" {
        return Err(invalid("тип файла не совпадает с содержимым"));
    }
    let count = sqlx::query_scalar!(
        r#"select count(*) as "n!" from receipt_files where receipt_id = $1 and hidden_at is null"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if count >= MAX_FILES {
        return Err(invalid(format!(
            "у накладной уже {MAX_FILES} файлов — больше не нужно"
        )));
    }
    let name: String = q
        .name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    let size = i32::try_from(body.len()).map_err(|_| invalid("файл слишком большой"))?;
    let fid = new_id();
    sqlx::query!(
        r#"insert into receipt_files (id, branch_id, receipt_id, op_id, name, content_type, size_bytes,
                                      data, source, party_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, 'supplier', $9)"#,
        fid,
        branch_id,
        id,
        q.op_id,
        name,
        kind,
        size,
        &body[..],
        client.party_id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, 'receipt_file.add_supplier', 'receipt', $3, $4)"#,
        new_id(),
        branch_id,
        id,
        json!({ "number": number, "file": fid, "name": name, "size": size, "type": kind,
                "party_id": client.party_id })
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "id": fid })))
}

// ---------- Деньги ----------

#[derive(Serialize)]
struct AppliedOut {
    number: Option<i64>,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct MoneyOut {
    at: DateTime<Utc>,
    branch_name: String,
    /// `payment` — оплата, `defect` — зачёт брака, `return` — возврат товара, `adjust` — правка.
    kind: String,
    amount_tyiyn: i64,
    reversed: bool,
    /// Какие накладные закрыл (у сторнированного — что закрывал до отмены).
    applied: Vec<AppliedOut>,
}

/// Оплаты и зачёты поставщику по всем филиалам — без кассы, комментария и того, кто провёл.
async fn payments(
    State(state): State<AppState>,
    client: Client,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<MoneyOut>>> {
    client.supplier()?;
    let mut conn = state.pool.acquire().await?;
    let several = crate::api::branches::several(&mut conn).await?;
    let from = q.from.unwrap_or_else(default_from);
    let to = q.to.unwrap_or(NaiveDate::MAX);
    let mut out = Vec::new();
    for (branch_id, branch_name) in branches_of(&mut conn, client.party_id).await? {
        let inv = supplier_invoices(&mut conn, branch_id, client.party_id).await?;
        for p in inv.payments {
            let day = (p.at + chrono::Duration::hours(6)).date_naive();
            if day < from || day > to {
                continue;
            }
            out.push(MoneyOut {
                at: p.at,
                branch_name: if several {
                    branch_name.clone()
                } else {
                    String::new()
                },
                kind: p.kind,
                amount_tyiyn: p.amount_tyiyn,
                reversed: p.reversed,
                applied: p
                    .applied
                    .into_iter()
                    .map(|a| AppliedOut {
                        number: a.number,
                        amount_tyiyn: if p.reversed {
                            a.gross_tyiyn
                        } else {
                            a.amount_tyiyn
                        },
                    })
                    .collect(),
            });
        }
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.at));
    Ok(Json(out))
}

// ---------- Возвраты ----------

#[derive(Serialize)]
struct ReturnLine {
    name: String,
    qty: i64,
    unit: String,
    container_ml: Option<i64>,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct ReturnOut {
    at: DateTime<Utc>,
    branch_name: String,
    /// `return` — возврат товара, `defect` — брак.
    kind: String,
    number: i64,
    /// Возврат товара: `debt` — в счёт долга, `money` — деньгами. Брак: `replace` — заменой,
    /// `credit` — зачётом, `cash` — деньгами.
    settle: String,
    amount_tyiyn: i64,
    reversed: bool,
    lines: Vec<ReturnLine>,
}

/// Возвраты товара и брака этому поставщику: что, сколько и как рассчитались. Наша себестоимость
/// и разница возмещения не отдаются.
async fn returns(
    State(state): State<AppState>,
    client: Client,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ReturnOut>>> {
    client.supplier()?;
    let mut conn = state.pool.acquire().await?;
    let from = q.from.unwrap_or_else(default_from);
    let rets = sqlx::query!(
        r#"select r.id, r.number, r.created_at, r.settle, r.amount_tyiyn,
                  case when (select count(*) from branches) > 1 then b.name else '' end as "branch_name!",
                  exists (select 1 from supplier_returns x where x.reversal_of = r.id) as "reversed!"
           from supplier_returns r join branches b on b.id = r.branch_id
           where r.supplier_id = $1 and r.reversal_of is null
             and (r.created_at at time zone 'Asia/Bishkek')::date >= $2
             and (r.created_at at time zone 'Asia/Bishkek')::date <= coalesce($3, (now() at time zone 'Asia/Bishkek')::date)
           order by r.created_at desc limit 500"#,
        client.party_id,
        from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = rets.iter().map(|r| r.id).collect();
    let lines = sqlx::query!(
        r#"select l.return_id, p.name, l.qty, p.unit, p.container_ml as "container_ml?", l.amount_tyiyn
           from supplier_return_lines l join products p on p.id = l.product_id
           where l.return_id = any($1) order by l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut by_ret: HashMap<Uuid, Vec<ReturnLine>> = HashMap::new();
    for l in lines {
        by_ret.entry(l.return_id).or_default().push(ReturnLine {
            name: l.name,
            qty: l.qty,
            unit: l.unit,
            container_ml: l.container_ml,
            amount_tyiyn: l.amount_tyiyn,
        });
    }
    let mut out: Vec<ReturnOut> = rets
        .into_iter()
        .map(|r| ReturnOut {
            at: r.created_at,
            branch_name: r.branch_name,
            kind: "return".into(),
            number: r.number,
            settle: r.settle,
            amount_tyiyn: r.amount_tyiyn,
            reversed: r.reversed,
            lines: by_ret.remove(&r.id).unwrap_or_default(),
        })
        .collect();
    let defects = sqlx::query!(
        r#"select dr.created_at, dr.kind, d.number, p.name, d.qty, p.unit, p.container_ml as "container_ml?",
                  (dr.replace_tyiyn + dr.refund_tyiyn)::bigint as "amount!",
                  case when (select count(*) from branches) > 1 then b.name else '' end as "branch_name!",
                  exists (select 1 from defect_resolutions x where x.reversal_of = dr.id) as "reversed!"
           from defect_resolutions dr
           join defects d on d.id = dr.defect_id
           join products p on p.id = d.product_id
           join branches b on b.id = dr.branch_id
           where dr.supplier_id = $1 and dr.reversal_of is null and dr.kind <> 'write_off'
             and (dr.created_at at time zone 'Asia/Bishkek')::date >= $2
             and (dr.created_at at time zone 'Asia/Bishkek')::date <= coalesce($3, (now() at time zone 'Asia/Bishkek')::date)
           order by dr.created_at desc limit 500"#,
        client.party_id,
        from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    out.extend(defects.into_iter().map(|d| ReturnOut {
        at: d.created_at,
        branch_name: d.branch_name,
        kind: "defect".into(),
        number: d.number,
        settle: d.kind,
        amount_tyiyn: d.amount,
        reversed: d.reversed,
        lines: vec![ReturnLine {
            name: d.name,
            qty: d.qty,
            unit: d.unit,
            container_ml: d.container_ml,
            amount_tyiyn: d.amount,
        }],
    }));
    out.sort_by_key(|r| std::cmp::Reverse(r.at));
    Ok(Json(out))
}

// ---------- Сообщение о накладной ----------

/// Чаты поставщика в боте: его своя привязка и руководитель (`is_head`), чья карточка клиента
/// подключена к боту.
async fn supplier_chats(conn: &mut PgConnection, party_id: Uuid) -> AppResult<Vec<i64>> {
    let head_phone = sqlx::query_scalar!(
        r#"select phone from party_contacts
           where party_id = $1 and active and is_head order by created_at limit 1"#,
        party_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let key = head_phone.as_deref().and_then(crate::domain::phone::key);
    Ok(sqlx::query_scalar!(
        r#"select distinct t.chat_id as "chat_id!" from telegram_customers t
           join parties p on p.id = t.party_id
           where p.active
             and (t.party_id = $1 or ($2::text is not null and p.role = 'customer'
                                      and phone_norm(p.phone) = $2))"#,
        party_id,
        key
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Текст поставщику о накладной.
pub fn invoice_text(
    number: &str,
    total: i64,
    paid: i64,
    cancelled: bool,
    link: Option<&str>,
) -> String {
    let mut t = if cancelled {
        format!("Накладная № {number} на сумму {} отменена.", som(total))
    } else {
        format!(
            "Принята накладная № {number}, сумма {}, оплачено {}, осталось {}.",
            som(total),
            som(paid),
            som(total.saturating_sub(paid))
        )
    };
    if let Some(l) = link {
        t.push_str(&format!("\nЛичный кабинет: {l}"));
    }
    t
}

/// Новые накладные поставщиков и их сторно — сообщение в Телеграм (задача 028). Один повод — один
/// раз (`client_message_keys`); поставщик с флажком «Не присылать» — без сообщения; нет чата или
/// бота — строка журнала «не отправлено».
pub async fn plan_invoices(conn: &mut PgConnection, ch: &Channels) -> AppResult<usize> {
    let since =
        sqlx::query_scalar!("select value from bot_state where key = 'client_messages_since'")
            .fetch_optional(&mut *conn)
            .await?
            .and_then(|v| v.as_str().and_then(|s| s.parse::<DateTime<Utc>>().ok()))
            .unwrap_or_else(Utc::now);
    let rows = sqlx::query!(
        r#"select r.id, r.branch_id, r.number, r.supplier_doc, r.total_tyiyn, r.reversal_of,
                  p.id as party_id, coalesce(o.number, r.number) as "shown!",
                  coalesce(o.supplier_doc, r.supplier_doc) as "doc!",
                  coalesce((select -l.amount_tyiyn from party_ledger l
                            where l.doc_type = 'receipt' and l.doc_id = r.id and l.amount_tyiyn < 0
                              and l.reversal_of is null limit 1), 0)::bigint as "debt!"
           from receipts r
           join parties p on p.id = r.supplier_id
           left join receipts o on o.id = r.reversal_of
           where p.role = 'supplier' and p.active and not p.no_purchase_messages
             and r.created_at > $1 and r.created_at > now() - interval '2 days'
             and not exists (select 1 from client_message_keys k where k.key = 'invoice:' || r.id::text)
             -- Об отмене пишем, только если о самой накладной уже писали.
             and (r.reversal_of is null
                  or exists (select 1 from client_message_keys k where k.key = 'invoice:' || r.reversal_of::text))
           order by r.created_at
           limit 200"#,
        since
    )
    .fetch_all(&mut *conn)
    .await?;
    let link = ch.public_url.as_ref().map(|u| format!("{u}/cabinet"));
    let mut made = 0;
    for r in rows {
        let cancelled = r.reversal_of.is_some();
        let number = if r.doc.trim().is_empty() {
            r.shown.to_string()
        } else {
            r.doc.trim().to_string()
        };
        let total = r.total_tyiyn.abs();
        let text = invoice_text(
            &number,
            total,
            total.saturating_sub(r.debt),
            cancelled,
            link.as_deref(),
        );
        let chats = supplier_chats(conn, r.party_id).await?;
        let mut tx = sqlx::Connection::begin(conn).await?;
        let key = format!("invoice:{}", r.id);
        let rows: Vec<(String, String, &str, String)> = match (&ch.telegram, chats.is_empty()) {
            (Some(_), false) => chats
                .iter()
                .map(|c| ("telegram".into(), c.to_string(), "pending", String::new()))
                .collect(),
            (None, false) => vec![(
                "none".into(),
                String::new(),
                "not_sent",
                "Телеграм-бот выключен на сервере".into(),
            )],
            (_, true) => vec![(
                "none".into(),
                String::new(),
                "not_sent",
                "поставщик не подключён к боту".into(),
            )],
        };
        let mut first = None;
        for (channel, recipient, status, error) in rows {
            let id = new_id();
            first.get_or_insert(id);
            sqlx::query!(
                r#"insert into client_messages (id, branch_id, party_id, kind, test, channel, recipient,
                                                text, template, language, params, status, error)
                   values ($1, $2, $3, 'invoice', false, $4, $5, $6, '', '', '[]', $7, $8)"#,
                id,
                r.branch_id,
                r.party_id,
                channel,
                recipient,
                text,
                status,
                error
            )
            .execute(&mut *tx)
            .await?;
        }
        let taken = sqlx::query!(
            "insert into client_message_keys (key, message_id) values ($1, $2) on conflict do nothing",
            key,
            first
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if taken == 0 {
            tx.rollback().await?;
            continue;
        }
        tx.commit().await?;
        made += 1;
    }
    Ok(made)
}
