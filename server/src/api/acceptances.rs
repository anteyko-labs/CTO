//! Приёмка товара на планшете (SPEC-39, ADR-134).
//!
//! Накладную готовит человек с правом «Приход товара»: «На приёмку» в «Новом приходе» или кнопка
//! «📦 На приёмку» в боте, когда фото распознано точно (ADR-135). Приёмка ждёт и ничего не
//! проводит. Кассир на планшете (пользователь с правом «Приёмка товара») сверяет привезённое со
//! строками без цен, правит их (принято, другой товар той же единицы, причина, комментарий),
//! выбирает себя, вводит PIN — та же проверка и те же пределы, что при открытии смены, — и
//! расписывается. Одной транзакцией проводится обычный приход по принятому «в долг» поставщику от
//! имени отправившего: сумма строки — доля суммы по накладной с одним округлением; фото накладной
//! переходит к приходу, приёмка закрывается с подписью. Владельцу — `receipt.accept` с
//! расхождениями и фото подписи в бот, кассиру открытой смены — «Примите товар» без цен.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::api::receipts::{ReceiptLineReq, ReceiptReq};
use crate::auth::{Ctx, CurrentUser, Role};
use crate::domain::money::div_round;
use crate::domain::qty::{fixed3, liters, plural};
use crate::error::{AppError, AppResult, invalid, is_unique_violation, overflow};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// Строк в накладной — как в приходе.
const MAX_LINES: usize = 500;
/// «Не по накладной» — не больше 50 строк.
const MAX_EXTRAS: usize = 50;
/// Комментарий, причина отказа — до 500 знаков (ADR-090).
const MAX_TEXT: usize = 500;
/// Подпись — PNG не больше 200 КБ (SPEC-39 §4).
pub const MAX_SIGNATURE: usize = 200 * 1024;
/// Количество строки — с запасом, как в приходе.
const MAX_QTY: i64 = 1_000_000_000;
/// Начало любого PNG.
const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/acceptances", routing::get(list).post(send))
        .route("/acceptances/{id}", routing::get(get_one))
        .route("/acceptances/{id}/accept", routing::post(accept))
        .route("/acceptances/{id}/refuse", routing::post(refuse))
        .route("/acceptances/{id}/cancel", routing::post(cancel))
        .route("/acceptances/{id}/signature", routing::get(signature))
        .route("/acceptances/{id}/photo", routing::get(photo))
        .route("/receipts/{id}/acceptance", routing::get(of_receipt))
}

// ---------- Права ----------

/// Кто видит приёмки: владелец и право «Приход товара» — с ценами и суммами (`true`), право
/// «Приёмка товара» — без них (`false`, SPEC-39 §8). Ни того ни другого — 403.
async fn view_rights(conn: &mut PgConnection, user: &CurrentUser) -> AppResult<bool> {
    if crate::auth::has_permission(conn, user, "receipts").await? {
        return Ok(true);
    }
    if crate::auth::has_permission(conn, user, "acceptance").await? {
        return Ok(false);
    }
    Err(crate::auth::no_permission("acceptance"))
}

// ---------- Вид ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LineOut {
    pub line_no: i32,
    pub product_id: Uuid,
    pub name: String,
    pub article: String,
    /// Коды товара: сканер на планшете прибавляет к «Принято» его строки.
    pub barcodes: Vec<String>,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// По накладной — в единицах учёта: штуки, мл, граммы.
    pub invoice_qty: i64,
    /// Сумма строки и новая цена продажи — только с правом «Приход товара».
    pub line_cost_tyiyn: Option<i64>,
    pub sale_price_tyiyn: Option<i64>,
    /// Итог приёмки строки; у ждущей приёмки — пусто.
    pub accepted_qty: Option<i64>,
    pub accepted_product_id: Option<Uuid>,
    pub accepted_name: Option<String>,
    pub accepted_container_ml: Option<i64>,
    pub reason: Option<String>,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExtraOut {
    pub name: String,
    pub barcode: String,
    pub qty: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AcceptanceOut {
    pub id: Uuid,
    /// `waiting`, `accepted`, `refused`, `cancelled`.
    pub status: String,
    pub supplier_id: Uuid,
    pub supplier_name: String,
    pub supplier_doc: String,
    pub comment: String,
    pub sent_by_name: String,
    pub sent_at: DateTime<Utc>,
    /// Отправили кнопкой в боте.
    pub from_bot: bool,
    pub draft_id: Option<Uuid>,
    /// Вид фото накладной, если оно есть: `image/jpeg`, `application/pdf`…
    pub photo_type: Option<String>,
    pub lines_count: i64,
    /// Сумма по накладной — только с правом «Приход товара» (у планшета цен нет).
    pub total_tyiyn: Option<i64>,
    pub lines: Vec<LineOut>,
    pub extras: Vec<ExtraOut>,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by_name: Option<String>,
    /// Кассир, принявший товар или отказавшийся от приёмки.
    pub employee_name: Option<String>,
    pub reason: Option<String>,
    pub accept_comment: String,
    pub receipt_id: Option<Uuid>,
    pub receipt_number: Option<i64>,
    /// Что стало с приходом: `reversed` — сторнирован, `corrected` — исправлен (`corrected_number`).
    pub receipt_state: Option<String>,
    pub corrected_id: Option<Uuid>,
    pub corrected_number: Option<i64>,
    pub has_signature: bool,
}

/// Приёмка филиала целиком; `prices` — показывать ли суммы и цены.
async fn load(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    prices: bool,
) -> AppResult<AcceptanceOut> {
    let a = sqlx::query!(
        r#"select a.id, a.status, a.supplier_id, s.name as supplier_name, a.supplier_doc, a.comment,
                  su.full_name as sent_by_name, a.sent_at, a.sent_chat_id, a.draft_id,
                  d.content_type as "photo_type?", f.hidden_at as "photo_hidden?",
                  a.closed_at, cu.full_name as "closed_by_name?", e.full_name as "employee_name?",
                  a.reason, a.accept_comment, a.receipt_id, r.number as "receipt_number?",
                  a.signature_png is not null as "has_signature!",
                  (select x.id from receipts x where x.reversal_of = a.receipt_id) as "reversed_by?",
                  c.new_id as "corrected_id?", c.new_number as "corrected_number?"
           from receipt_acceptances a
           join suppliers s on s.id = a.supplier_id
           join users su on su.id = a.sent_by
           left join receipt_drafts d on d.id = a.draft_id
           left join receipt_files f on f.id = d.file_id
           left join users cu on cu.id = a.closed_by
           left join employees e on e.id = a.employee_id
           left join receipts r on r.id = a.receipt_id
           left join doc_corrections c on c.kind = 'receipt' and c.original_id = a.receipt_id
           where a.id = $1 and a.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let rows = sqlx::query!(
        r#"select l.line_no, l.product_id, p.name, p.article, p.unit, p.container_ml, l.invoice_qty,
                  l.line_cost_tyiyn, l.sale_price_tyiyn, l.accepted_qty, l.accepted_product_id,
                  ap.name as "accepted_name?", ap.container_ml as "accepted_container_ml?",
                  l.reason, l.comment,
                  array(select b.code from product_barcodes b where b.product_id = l.product_id
                        order by b.internal, b.code) as "barcodes!"
           from receipt_acceptance_lines l
           join products p on p.id = l.product_id
           left join products ap on ap.id = l.accepted_product_id
           where l.acceptance_id = $1
           order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut total: i64 = 0;
    let mut lines = Vec::with_capacity(rows.len());
    for l in rows {
        total = total.checked_add(l.line_cost_tyiyn).ok_or_else(overflow)?;
        lines.push(LineOut {
            line_no: l.line_no,
            product_id: l.product_id,
            name: l.name,
            article: l.article,
            barcodes: l.barcodes,
            unit: l.unit,
            container_ml: l.container_ml,
            invoice_qty: l.invoice_qty,
            line_cost_tyiyn: prices.then_some(l.line_cost_tyiyn),
            sale_price_tyiyn: if prices { l.sale_price_tyiyn } else { None },
            accepted_qty: l.accepted_qty,
            accepted_product_id: l.accepted_product_id,
            accepted_name: l.accepted_name,
            accepted_container_ml: l.accepted_container_ml,
            reason: l.reason,
            comment: l.comment,
        });
    }
    let extras = sqlx::query_as!(
        ExtraOut,
        "select name, barcode, qty from receipt_acceptance_extras where acceptance_id = $1 order by no",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let receipt_state = match (a.corrected_id, a.reversed_by) {
        (Some(_), _) => Some("corrected".to_string()),
        (None, Some(_)) => Some("reversed".to_string()),
        _ => None,
    };
    Ok(AcceptanceOut {
        id: a.id,
        status: a.status,
        supplier_id: a.supplier_id,
        supplier_name: a.supplier_name,
        supplier_doc: a.supplier_doc,
        comment: a.comment,
        sent_by_name: a.sent_by_name,
        sent_at: a.sent_at,
        from_bot: a.sent_chat_id.is_some(),
        draft_id: a.draft_id,
        // Фото, скрытое владельцем в накладной, администратору не показывается (ADR-080).
        photo_type: a.photo_type.filter(|_| a.photo_hidden.is_none() || prices),
        lines_count: i64::try_from(lines.len()).unwrap_or(i64::MAX),
        total_tyiyn: prices.then_some(total),
        lines,
        extras,
        closed_at: a.closed_at,
        closed_by_name: a.closed_by_name,
        employee_name: a.employee_name,
        reason: a.reason,
        accept_comment: a.accept_comment,
        receipt_id: a.receipt_id,
        receipt_number: a.receipt_number,
        receipt_state,
        corrected_id: a.corrected_id,
        corrected_number: a.corrected_number,
        has_signature: a.has_signature,
    })
}

#[derive(Serialize)]
pub struct ItemOut {
    pub id: Uuid,
    pub supplier_name: String,
    pub supplier_doc: String,
    pub lines_count: i64,
    pub total_tyiyn: Option<i64>,
    pub sent_by_name: String,
    pub sent_at: DateTime<Utc>,
    pub from_bot: bool,
    pub photo_type: Option<String>,
}

#[derive(Serialize)]
pub struct ListOut {
    /// Видны ли суммы (право «Приход товара»).
    pub prices: bool,
    pub items: Vec<ItemOut>,
}

#[derive(Deserialize)]
struct ListQuery {
    #[serde(default)]
    status: Option<String>,
}

/// «Ждут приёмки»: свежие сверху (SPEC-39 §2, §3).
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<ListOut>> {
    if q.status.as_deref().is_some_and(|s| s != "waiting") {
        return Err(invalid("список — только ждущие приёмки: status=waiting"));
    }
    let mut conn = state.pool.acquire().await?;
    let prices = view_rights(&mut conn, &user).await?;
    let rows = sqlx::query!(
        r#"select a.id, s.name as supplier_name, a.supplier_doc, u.full_name as sent_by_name,
                  a.sent_at, a.sent_chat_id, d.content_type as "photo_type?",
                  (select count(*) from receipt_acceptance_lines l where l.acceptance_id = a.id) as "lines!",
                  (select coalesce(sum(l.line_cost_tyiyn), 0)::bigint from receipt_acceptance_lines l
                   where l.acceptance_id = a.id) as "total!"
           from receipt_acceptances a
           join suppliers s on s.id = a.supplier_id
           join users u on u.id = a.sent_by
           left join receipt_drafts d on d.id = a.draft_id
           where a.branch_id = $1 and a.status = 'waiting'
           order by a.sent_at desc, a.id desc
           limit 200"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let items = rows
        .into_iter()
        .map(|r| ItemOut {
            id: r.id,
            supplier_name: r.supplier_name,
            supplier_doc: r.supplier_doc,
            lines_count: r.lines,
            total_tyiyn: prices.then_some(r.total),
            sent_by_name: r.sent_by_name,
            sent_at: r.sent_at,
            from_bot: r.sent_chat_id.is_some(),
            photo_type: r.photo_type,
        })
        .collect();
    Ok(Json(ListOut { prices, items }))
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<AcceptanceOut>> {
    let mut conn = state.pool.acquire().await?;
    let prices = view_rights(&mut conn, &user).await?;
    Ok(Json(load(&mut conn, user.branch_id, id, prices).await?))
}

/// Приёмка прихода — в карточке прихода: кто принял, расхождения, подпись. Владелец и право
/// «Приход товара»; приход не по приёмке — `null`.
async fn of_receipt(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(receipt_id): Path<Uuid>,
) -> AppResult<Json<Option<AcceptanceOut>>> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "receipts").await?;
    let found = sqlx::query!(
        r#"select r.id as "receipt!", a.id as "acceptance?"
           from receipts r left join receipt_acceptances a on a.receipt_id = r.id
           where r.id = $1 and r.branch_id = $2"#,
        receipt_id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    match found.acceptance {
        Some(id) => Ok(Json(Some(load(&mut conn, user.branch_id, id, true).await?))),
        None => Ok(Json(None)),
    }
}

/// Файл из базы — с запретом кэша и угадывания вида (ADR-090).
fn file_response(data: Vec<u8>, content_type: &str) -> Response {
    let mut res = data.into_response();
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("inline"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// Подпись кассира — владельцу и праву «Приход товара».
async fn signature(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let mut conn = state.pool.acquire().await?;
    crate::auth::require_permission(&mut conn, &user, "receipts").await?;
    let png = sqlx::query_scalar!(
        "select signature_png from receipt_acceptances where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .flatten()
    .ok_or(AppError::NotFound)?;
    Ok(file_response(png, "image/png"))
}

/// Фото накладной для экрана приёмки (касание — во весь экран): право «Приёмка товара» или
/// «Приход товара». Скрытое владельцем фото администратору не отдаётся (ADR-080).
async fn photo(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let mut conn = state.pool.acquire().await?;
    view_rights(&mut conn, &user).await?;
    let f = sqlx::query!(
        r#"select coalesce(d.data, f.data) as "data?", d.content_type, f.hidden_at as "hidden_at?"
           from receipt_acceptances a
           join receipt_drafts d on d.id = a.draft_id
           left join receipt_files f on f.id = d.file_id
           where a.id = $1 and a.branch_id = $2"#,
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if f.hidden_at.is_some() && !user.is_owner() {
        return Err(AppError::NotFound);
    }
    let data = f.data.ok_or(AppError::NotFound)?;
    Ok(file_response(data, &f.content_type))
}

// ---------- «На приёмку» ----------

#[derive(Deserialize, Debug, Clone)]
pub struct SendReq {
    pub op_id: Uuid,
    /// Накладная по фото, из которой заполнили приход: её фото перейдёт к приходу.
    #[serde(default)]
    pub draft_id: Option<Uuid>,
    pub supplier_id: Option<Uuid>,
    #[serde(default)]
    pub supplier_doc: String,
    #[serde(default)]
    pub comment: String,
    /// Строки — как у прихода: количество в единицах учёта, сумма строки, новая цена продажи.
    pub lines: Vec<ReceiptLineReq>,
}

/// Текст не длиннее предела (ADR-090), без пробелов по краям.
fn limited(s: &str, max: usize, what: &str) -> AppResult<String> {
    let t = s.trim();
    if t.chars().count() > max {
        return Err(invalid(format!("{what} — не длиннее {max} знаков")));
    }
    Ok(t.to_string())
}

/// Проверки строк — как у проведения прихода (SPEC-03), но склад, долг и цены не меняются.
async fn check_lines(conn: &mut PgConnection, lines: &[ReceiptLineReq]) -> AppResult<i64> {
    if lines.is_empty() {
        return Err(invalid("в накладной нет строк"));
    }
    if lines.len() > MAX_LINES {
        return Err(invalid(
            "в приходе — не больше 500 строк: разбейте накладную",
        ));
    }
    let mut total: i64 = 0;
    let mut prices: BTreeMap<Uuid, i64> = BTreeMap::new();
    for l in lines {
        if l.qty <= 0 || l.cost_tyiyn < 0 {
            return Err(invalid("количество больше нуля, сумма не отрицательна"));
        }
        if l.qty > MAX_QTY {
            return Err(invalid("слишком большое количество — проверьте ввод"));
        }
        ops::check_amount(l.cost_tyiyn)?;
        if let Some(p) = l.sale_price_tyiyn {
            if p < 0 {
                return Err(invalid("цена продажи не отрицательна"));
            }
            ops::check_amount(p)?;
            if *prices.entry(l.product_id).or_insert(p) != p {
                return Err(invalid(
                    "у одного товара в приходе две разные цены продажи — оставьте одну",
                ));
            }
        }
        total = total.checked_add(l.cost_tyiyn).ok_or_else(overflow)?;
    }
    ops::check_amount(total)?;
    let ids: Vec<Uuid> = lines
        .iter()
        .map(|l| l.product_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let known = sqlx::query_scalar!(
        r#"select count(*) as "n!" from products where id = any($1) and not archived"#,
        &ids
    )
    .fetch_one(&mut *conn)
    .await?;
    if usize::try_from(known).ok() != Some(ids.len()) {
        return Err(invalid(
            "товар не найден или в архиве: если карточку объединили, выберите основную",
        ));
    }
    Ok(total)
}

/// «На приёмку» (SPEC-39 §2): проверки — как у прихода, ничего не проводится. Черновик фото —
/// «на приёмке». Из бота — с чатом отправителя (`ctx.bot_chat`).
pub async fn send_tx(conn: &mut PgConnection, ctx: &Ctx, req: SendReq) -> AppResult<AcceptanceOut> {
    const KIND: &str = "acceptance.send";
    crate::auth::require_permission(&mut *conn, &ctx.user, "receipts").await?;
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let supplier_id = req.supplier_id.ok_or_else(|| {
        invalid("укажите поставщика: приёмка проводит накладную в долг поставщику")
    })?;
    let supplier_doc = limited(&req.supplier_doc, 200, "номер накладной")?;
    let comment = limited(&req.comment, MAX_TEXT, "комментарий")?;
    let total = check_lines(conn, &req.lines).await?;
    let supplier = sqlx::query_scalar!("select name from suppliers where id = $1", supplier_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| invalid("поставщик не найден"))?;
    // Та же накладная уже ждёт: изменить приёмку — убрать её и отправить заново.
    if !supplier_doc.is_empty() {
        let twin = sqlx::query_scalar!(
            r#"select exists (select 1 from receipt_acceptances
                              where branch_id = $1 and supplier_id = $2 and status = 'waiting'
                                and lower(supplier_doc) = lower($3)) as "e!""#,
            branch_id,
            supplier_id,
            supplier_doc
        )
        .fetch_one(&mut *conn)
        .await?;
        if twin {
            return Err(AppError::Conflict(format!(
                "накладная № {supplier_doc} от «{supplier}» уже ждёт приёмки — изменить: «Убрать с приёмки» и отправить заново"
            )));
        }
    }
    if let Some(draft) = req.draft_id {
        crate::api::receipt_drafts::to_acceptance(conn, ctx, draft).await?;
    }
    let id = new_id();
    sqlx::query!(
        r#"insert into receipt_acceptances (id, branch_id, draft_id, supplier_id, supplier_doc, comment,
                                            sent_by, sent_device_id, sent_chat_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        id,
        branch_id,
        req.draft_id,
        supplier_id,
        supplier_doc,
        comment,
        ctx.user.id,
        ctx.device_id,
        ctx.bot_chat
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict("эта накладная уже ждёт приёмки".into())
        } else {
            e.into()
        }
    })?;
    for (i, l) in req.lines.iter().enumerate() {
        let line_no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            r#"insert into receipt_acceptance_lines (acceptance_id, line_no, product_id, invoice_qty,
                                                     line_cost_tyiyn, sale_price_tyiyn)
               values ($1, $2, $3, $4, $5, $6)"#,
            id,
            line_no,
            l.product_id,
            l.qty,
            l.cost_tyiyn,
            l.sale_price_tyiyn
        )
        .execute(&mut *conn)
        .await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt_acceptance",
        Some(id),
        json!({
            "supplier": supplier,
            "doc": supplier_doc,
            "lines": req.lines.len(),
            "total": total,
            "draft": req.draft_id,
        }),
    )
    .await?;
    let out = load(conn, branch_id, id, true).await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn send(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SendReq>,
) -> AppResult<Json<AcceptanceOut>> {
    let mut tx = state.pool.begin().await?;
    let out = send_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Закрытие: снять, отказ ----------

/// Приёмка не ждёт — что ответить.
fn closed_conflict(status: &str, number: Option<i64>) -> AppError {
    AppError::Conflict(match status {
        "accepted" => format!(
            "товар уже принят: приход № {}",
            number.map_or_else(String::new, |n| n.to_string())
        ),
        "refused" => "от приёмки уже отказались".into(),
        _ => "накладную уже убрали с приёмки".into(),
    })
}

struct Locked {
    status: String,
    draft_id: Option<Uuid>,
    supplier_id: Uuid,
    supplier: String,
    supplier_doc: String,
    sent_by: Uuid,
    sent_by_name: String,
    number: Option<i64>,
}

/// Приёмка филиала под блокировкой: вторая приёмка той же ждёт первую и получает 409.
async fn lock(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<Locked> {
    let a = sqlx::query!(
        r#"select a.status, a.draft_id, a.supplier_id, s.name as supplier, a.supplier_doc, a.sent_by,
                  u.full_name as sent_by_name, r.number as "number?"
           from receipt_acceptances a
           join suppliers s on s.id = a.supplier_id
           join users u on u.id = a.sent_by
           left join receipts r on r.id = a.receipt_id
           where a.id = $1 and a.branch_id = $2
           for update of a"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Locked {
        status: a.status,
        draft_id: a.draft_id,
        supplier_id: a.supplier_id,
        supplier: a.supplier,
        supplier_doc: a.supplier_doc,
        sent_by: a.sent_by,
        sent_by_name: a.sent_by_name,
        number: a.number,
    })
}

#[derive(Serialize, Deserialize)]
pub struct ClosedOut {
    pub id: Uuid,
    pub status: String,
}

#[derive(Deserialize)]
pub struct CancelReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub reason: String,
}

/// «Убрать с приёмки» (SPEC-39 §2): с причиной, черновик фото снова «новый».
pub async fn cancel_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: CancelReq,
) -> AppResult<ClosedOut> {
    const KIND: &str = "acceptance.cancel";
    crate::auth::require_permission(&mut *conn, &ctx.user, "receipts").await?;
    let reason = limited(&req.reason, MAX_TEXT, "причина")?;
    if reason.is_empty() {
        return Err(invalid("укажите, почему убираете с приёмки"));
    }
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let a = lock(conn, ctx.user.branch_id, id).await?;
    if a.status != "waiting" {
        return Err(closed_conflict(&a.status, a.number));
    }
    sqlx::query!(
        r#"update receipt_acceptances
           set status = 'cancelled', closed_at = now(), closed_by = $2, closed_device_id = $3, reason = $4
           where id = $1"#,
        id,
        ctx.user.id,
        ctx.device_id,
        reason
    )
    .execute(&mut *conn)
    .await?;
    if let Some(d) = a.draft_id {
        crate::api::receipt_drafts::from_acceptance(conn, d).await?;
    }
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt_acceptance",
        Some(id),
        json!({ "supplier": a.supplier, "doc": a.supplier_doc, "reason": reason }),
    )
    .await?;
    let out = ClosedOut {
        id,
        status: "cancelled".into(),
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn cancel(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CancelReq>,
) -> AppResult<Json<ClosedOut>> {
    let mut tx = state.pool.begin().await?;
    let out = cancel_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct RefuseReq {
    pub op_id: Uuid,
    pub employee_id: Uuid,
    #[serde(default)]
    pub pin: String,
    #[serde(default)]
    pub reason: String,
}

/// Кассир приёмки: активный сотрудник филиала с отметкой «кассир» (SPEC-39 §3). Проверка — до
/// PIN: не кассиру попытка PIN не тратится.
async fn check_cashier(state: &AppState, ctx: &Ctx, employee_id: Uuid) -> AppResult<()> {
    let e = sqlx::query!(
        "select is_cashier, active from employees where id = $1 and branch_id = $2",
        employee_id,
        ctx.user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| invalid("кассир не найден"))?;
    if !e.active || !e.is_cashier {
        return Err(invalid(
            "принимает товар кассир: сотрудник не кассир или отключён",
        ));
    }
    Ok(())
}

/// PIN кассира — та же проверка и те же пределы, что при открытии смены (ADR-081, ADR-128).
async fn cashier_pin(
    state: &AppState,
    ctx: &Ctx,
    employee_id: Uuid,
    pin: &str,
) -> AppResult<String> {
    check_cashier(state, ctx, employee_id).await?;
    crate::api::auth::check_cashier_pin(
        state,
        ctx,
        crate::api::auth::CashierPin {
            employee_id,
            pin,
            no_pin_hint: "",
            acceptance: true,
        },
    )
    .await
}

/// «Отказ от приёмки» (SPEC-39 §3): причина, кассир, PIN. Прихода нет, черновик снова «новый».
async fn refuse(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<RefuseReq>,
) -> AppResult<Json<ClosedOut>> {
    const KIND: &str = "acceptance.refuse";
    {
        let mut conn = state.pool.acquire().await?;
        crate::auth::require_permission(&mut conn, &ctx.user, "acceptance").await?;
    }
    let reason = limited(&req.reason, MAX_TEXT, "причина")?;
    if reason.is_empty() {
        return Err(invalid("укажите причину отказа от приёмки"));
    }
    let cashier = cashier_pin(&state, &ctx, req.employee_id, &req.pin).await?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let a = lock(&mut tx, ctx.user.branch_id, id).await?;
    if a.status != "waiting" {
        return Err(closed_conflict(&a.status, a.number));
    }
    sqlx::query!(
        r#"update receipt_acceptances
           set status = 'refused', closed_at = now(), closed_by = $2, closed_device_id = $3,
               employee_id = $4, reason = $5
           where id = $1"#,
        id,
        ctx.user.id,
        ctx.device_id,
        req.employee_id,
        reason
    )
    .execute(&mut *tx)
    .await?;
    if let Some(d) = a.draft_id {
        crate::api::receipt_drafts::from_acceptance(&mut tx, d).await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "receipt_acceptance",
        Some(id),
        json!({
            "supplier": a.supplier,
            "doc": a.supplier_doc,
            "cashier": cashier,
            "employee_id": req.employee_id,
            "reason": reason,
            "sent_by": a.sent_by_name,
        }),
    )
    .await?;
    let out = ClosedOut {
        id,
        status: "refused".into(),
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Приёмка ----------

#[derive(Deserialize, Debug, Clone)]
pub struct AcceptLine {
    pub line_no: i32,
    /// Принято — в единицах учёта (штуки, мл, граммы).
    pub accepted_qty: i64,
    /// Привезли другой товар той же единицы учёта; пусто — товар накладной.
    #[serde(default)]
    pub product_id: Option<Uuid>,
    /// Почему принято меньше: `missing` — не привезли, `damaged` — брак или повреждено,
    /// `other` — другое.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ExtraReq {
    pub name: String,
    #[serde(default)]
    pub barcode: String,
    pub qty: i64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct AcceptReq {
    pub op_id: Uuid,
    pub lines: Vec<AcceptLine>,
    #[serde(default)]
    pub extras: Vec<ExtraReq>,
    pub employee_id: Uuid,
    #[serde(default)]
    pub pin: String,
    /// Подпись: PNG в base64, можно с началом `data:image/png;base64,`.
    #[serde(default)]
    pub signature_png: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AcceptOut {
    pub acceptance_id: Uuid,
    pub receipt_id: Uuid,
    pub receipt_number: i64,
}

/// Base64 без новой библиотеки (стандартный алфавит, `=` в конце; пробелы и переводы строк
/// пропускаются). Не base64 — `None`.
pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let clean: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if !clean.len().is_multiple_of(4) {
        return None;
    }
    let body = clean
        .strip_suffix(b"==")
        .or_else(|| clean.strip_suffix(b"="))
        .unwrap_or(&clean);
    let mut out = Vec::with_capacity(clean.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for &c in body {
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// Подпись из запроса: PNG по первым байтам, не больше 200 КБ (SPEC-39 §4).
pub fn signature_png(text: &str) -> AppResult<Vec<u8>> {
    let t = text.trim();
    let t = t.strip_prefix("data:image/png;base64,").unwrap_or(t);
    if t.is_empty() {
        return Err(invalid("нужна подпись кассира"));
    }
    // Длинее 200 КБ после раскодирования — не раскодируем вовсе.
    if t.len() > MAX_SIGNATURE / 3 * 4 + 8 {
        return Err(invalid("подпись больше 200 КБ — распишитесь ещё раз"));
    }
    let png = decode_base64(t).ok_or_else(|| invalid("подпись — картинка PNG"))?;
    if png.len() > MAX_SIGNATURE {
        return Err(invalid("подпись больше 200 КБ — распишитесь ещё раз"));
    }
    if !png.starts_with(&PNG_MAGIC) {
        return Err(invalid("подпись — картинка PNG"));
    }
    Ok(png)
}

/// Причина расхождения из запроса.
fn reason_of(r: Option<&str>) -> AppResult<Option<&'static str>> {
    match r.map(str::trim) {
        None | Some("") => Ok(None),
        Some("missing") => Ok(Some("missing")),
        Some("damaged") => Ok(Some("damaged")),
        Some("other") => Ok(Some("other")),
        Some(_) => Err(invalid(
            "причина: не привезли (missing), брак или повреждено (damaged), другое (other)",
        )),
    }
}

/// Поля запроса приёмки, которые проверяются без базы: до PIN — ошибка ввода не тратит попытку.
fn check_accept_fields(req: &AcceptReq) -> AppResult<()> {
    if req.lines.is_empty() || req.lines.len() > MAX_LINES {
        return Err(invalid("отметьте строки накладной"));
    }
    for l in &req.lines {
        if l.accepted_qty < 0 || l.accepted_qty > MAX_QTY {
            return Err(invalid(format!(
                "строка {}: принято — целое не меньше нуля",
                l.line_no
            )));
        }
        reason_of(l.reason.as_deref())?;
        limited(&l.comment, MAX_TEXT, "комментарий к строке")?;
    }
    limited(&req.comment, MAX_TEXT, "комментарий")?;
    if req.extras.len() > MAX_EXTRAS {
        return Err(invalid("«не по накладной» — не больше 50 строк"));
    }
    for x in &req.extras {
        let name = limited(&x.name, 120, "название «не по накладной»")?;
        if name.is_empty() && x.barcode.trim().is_empty() {
            return Err(invalid("«не по накладной»: название или код"));
        }
        limited(&x.barcode, 64, "код «не по накладной»")?;
        if x.qty <= 0 || x.qty > 1_000_000 {
            return Err(invalid("«не по накладной»: количество больше нуля"));
        }
    }
    Ok(())
}

/// Количество словами для владельца: «2 шт», «3 кан.», «5,5 л», «12,5 кг».
fn qty_words(q: i64, unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) if c > 0 && q % c == 0 => format!("{} кан.", q / c),
        ("ml", _) => liters(q),
        ("g", _) => format!("{} кг", fixed3(q)),
        _ => format!("{q} шт"),
    }
}

/// «18 из 20», «0 из 2 кан.», «5,5 из 8 л», «12,5 из 15 кг».
fn pair_words(accepted: i64, invoice: i64, unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) if c > 0 && accepted % c == 0 && invoice % c == 0 => {
            format!("{} из {} кан.", accepted / c, invoice / c)
        }
        ("ml", _) => format!("{} из {} л", fixed3(accepted), fixed3(invoice)),
        ("g", _) => format!("{} из {} кг", fixed3(accepted), fixed3(invoice)),
        _ => format!("{accepted} из {invoice}"),
    }
}

/// Строка накладной в приёмке.
struct InvoiceLine {
    line_no: i32,
    product_id: Uuid,
    name: String,
    unit: String,
    container_ml: Option<i64>,
    invoice_qty: i64,
    line_cost: i64,
    sale_price: Option<i64>,
}

/// Отправивший приёмку: приход проводится от его имени, поэтому он ещё активен и может
/// проводить приход (SPEC-39 §4); иначе — 409 «отправьте на приёмку заново».
async fn sender(conn: &mut PgConnection, user_id: Uuid, branch_id: Uuid) -> AppResult<CurrentUser> {
    const AGAIN: &str = "отправивший накладную на приёмку больше не может проводить приход — пусть её отправят на приёмку заново";
    let u = sqlx::query!(
        "select login, full_name, role, active from users where id = $1",
        user_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let role = Role::parse(&u.role)?;
    let user = CurrentUser {
        id: user_id,
        branch_id,
        login: u.login,
        full_name: u.full_name,
        role,
    };
    if !u.active || !crate::auth::has_permission(conn, &user, "receipts").await? {
        return Err(AppError::Conflict(AGAIN.into()));
    }
    Ok(user)
}

/// Приёмка проведена: приход по принятому, приёмка закрыта (SPEC-39 §4). PIN и подпись уже
/// проверены. Одна транзакция; повтор `op_id` — тот же приход.
async fn accept_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: AcceptReq,
    png: Vec<u8>,
    cashier: &str,
) -> AppResult<AcceptOut> {
    const KIND: &str = "receipt.accept";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    let a = lock(conn, branch_id, id).await?;
    if a.status != "waiting" {
        return Err(closed_conflict(&a.status, a.number));
    }
    let sent_by = sender(conn, a.sent_by, branch_id).await?;
    let invoice: Vec<InvoiceLine> = sqlx::query!(
        r#"select l.line_no, l.product_id, p.name, p.unit, p.container_ml, l.invoice_qty,
                  l.line_cost_tyiyn, l.sale_price_tyiyn
           from receipt_acceptance_lines l join products p on p.id = l.product_id
           where l.acceptance_id = $1 order by l.line_no"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| InvoiceLine {
        line_no: r.line_no,
        product_id: r.product_id,
        name: r.name,
        unit: r.unit,
        container_ml: r.container_ml,
        invoice_qty: r.invoice_qty,
        line_cost: r.line_cost_tyiyn,
        sale_price: r.sale_price_tyiyn,
    })
    .collect();
    // Отмечены все строки, каждая — один раз.
    let mut marks: HashMap<i32, &AcceptLine> = HashMap::with_capacity(req.lines.len());
    for m in &req.lines {
        if marks.insert(m.line_no, m).is_some() {
            return Err(invalid(format!("строка {} отмечена дважды", m.line_no)));
        }
    }
    if marks.len() != invoice.len() || invoice.iter().any(|l| !marks.contains_key(&l.line_no)) {
        return Err(invalid("отметьте все строки накладной"));
    }
    // Другой товар — той же единицы учёта и не в архиве (SPEC-39 §3).
    let swaps: Vec<Uuid> = req
        .lines
        .iter()
        .filter_map(|m| m.product_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let swap_products: HashMap<Uuid, (String, String, Option<i64>, bool)> = sqlx::query!(
        "select id, name, unit, container_ml, archived from products where id = any($1)",
        &swaps
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|p| (p.id, (p.name, p.unit, p.container_ml, p.archived)))
    .collect();
    struct Done {
        line_no: i32,
        accepted: i64,
        product: Option<Uuid>,
        reason: Option<&'static str>,
        comment: String,
    }
    let mut done = Vec::with_capacity(invoice.len());
    let mut receipt_lines = Vec::new();
    let mut diffs = Vec::new();
    let mut accepted_lines = 0usize;
    for l in &invoice {
        let m = marks
            .get(&l.line_no)
            .ok_or_else(|| invalid("отметьте все строки накладной"))?;
        let accepted = m.accepted_qty;
        let swap = m.product_id.filter(|p| *p != l.product_id);
        let swap_info = match swap {
            Some(p) => {
                let (name, unit, container, archived) = swap_products
                    .get(&p)
                    .ok_or_else(|| invalid(format!("строка {}: товар не найден", l.line_no)))?;
                if *archived {
                    return Err(invalid(format!(
                        "строка {}: товар {name} в архиве — выберите другой",
                        l.line_no
                    )));
                }
                if *unit != l.unit {
                    return Err(invalid(format!(
                        "строка {}: выберите товар той же единицы учёта (штуки, литры, кг)",
                        l.line_no
                    )));
                }
                Some((name.clone(), *container))
            }
            None => None,
        };
        let reason = if accepted < l.invoice_qty {
            Some(reason_of(m.reason.as_deref())?.ok_or_else(|| {
                invalid(format!(
                    "строка {} ({}): принято меньше накладной — укажите причину: не привезли, брак или повреждено, другое",
                    l.line_no, l.name
                ))
            })?)
        } else {
            None
        };
        let comment = limited(&m.comment, MAX_TEXT, "комментарий к строке")?;
        if accepted > 0 {
            accepted_lines += 1;
            // Сумма строки = сумма по накладной × принято / по накладной, одно округление
            // (инвариант 1); больше накладной — по той же цене за единицу (SPEC-39 §5).
            let cost = div_round(
                i128::from(l.line_cost) * i128::from(accepted),
                i128::from(l.invoice_qty),
            )
            .ok_or_else(overflow)?;
            ops::check_amount(cost)?;
            receipt_lines.push(ReceiptLineReq {
                product_id: swap.unwrap_or(l.product_id),
                qty: accepted,
                cost_tyiyn: cost,
                // Новая цена продажи — товару накладной; другому товару её не ставим.
                sale_price_tyiyn: if swap.is_none() { l.sale_price } else { None },
            });
        }
        if accepted != l.invoice_qty || swap.is_some() || !comment.is_empty() {
            diffs.push(json!({
                "line": l.line_no,
                "name": l.name,
                "unit": l.unit,
                "container_ml": l.container_ml,
                "invoice": l.invoice_qty,
                "accepted": accepted,
                "reason": reason,
                "replaced_by": swap_info.as_ref().map(|s| s.0.clone()),
                "replaced_container_ml": swap_info.as_ref().and_then(|s| s.1),
                "comment": comment,
            }));
        }
        done.push(Done {
            line_no: l.line_no,
            accepted,
            product: swap,
            reason,
            comment,
        });
    }
    if receipt_lines.is_empty() {
        return Err(invalid(
            "ничего не принято — если товар не привезли, нажмите «Отказ от приёмки»",
        ));
    }
    // «Не по накладной»: код со сканера — товар учёта, если такой есть.
    let mut extras = Vec::with_capacity(req.extras.len());
    for x in &req.extras {
        let barcode = limited(&x.barcode, 64, "код «не по накладной»")?;
        let mut name = limited(&x.name, 120, "название «не по накладной»")?;
        let product = if barcode.is_empty() {
            None
        } else {
            sqlx::query!(
                r#"select p.id, p.name from product_barcodes b join products p on p.id = b.product_id
                   where b.code = $1 and not p.archived limit 1"#,
                barcode
            )
            .fetch_optional(&mut *conn)
            .await?
        };
        if name.is_empty() {
            name = product
                .as_ref()
                .map_or_else(|| format!("код {barcode}"), |p| p.name.clone());
        }
        extras.push((name, barcode, product.map(|p| p.id), x.qty));
    }
    let accept_comment = limited(&req.comment, MAX_TEXT, "комментарий")?;
    // Приход — обычное проведение от имени отправившего; устройство — планшет (SPEC-39 §4).
    let sender_ctx = Ctx {
        user: sent_by,
        device_id: ctx.device_id,
        bot_chat: None,
    };
    let receipt = crate::api::receipts::post_receipt_tx(
        conn,
        &sender_ctx,
        ReceiptReq {
            op_id: new_id(),
            supplier_id: Some(a.supplier_id),
            payment: Some("debt".into()),
            supplier_doc: a.supplier_doc.clone(),
            comment: format!("Принято на планшете: {cashier}"),
            lines: receipt_lines,
            paid_from: None,
            paid_now_tyiyn: None,
        },
    )
    .await?;
    if let Some(d) = a.draft_id {
        crate::api::receipt_drafts::attach_accepted(conn, &sender_ctx, d, receipt.id).await?;
    }
    for d in &done {
        sqlx::query!(
            r#"update receipt_acceptance_lines
               set accepted_qty = $3, accepted_product_id = $4, reason = $5, comment = $6
               where acceptance_id = $1 and line_no = $2"#,
            id,
            d.line_no,
            d.accepted,
            d.product,
            d.reason,
            d.comment
        )
        .execute(&mut *conn)
        .await?;
    }
    for (i, (name, barcode, product, qty)) in extras.iter().enumerate() {
        let no = i32::try_from(i + 1).map_err(|_| invalid("слишком много строк"))?;
        sqlx::query!(
            r#"insert into receipt_acceptance_extras (acceptance_id, no, name, barcode, product_id, qty)
               values ($1, $2, $3, $4, $5, $6)"#,
            id,
            no,
            name,
            barcode,
            *product,
            *qty
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        r#"update receipt_acceptances
           set status = 'accepted', closed_at = now(), closed_by = $2, closed_device_id = $3,
               employee_id = $4, accept_comment = $5, signature_png = $6, receipt_id = $7
           where id = $1"#,
        id,
        ctx.user.id,
        ctx.device_id,
        req.employee_id,
        accept_comment,
        png,
        receipt.id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(receipt.id),
        json!({
            "number": receipt.number,
            "acceptance": id,
            "supplier": a.supplier,
            "doc": a.supplier_doc,
            "lines": invoice.len(),
            "accepted_lines": accepted_lines,
            "total": receipt.total_tyiyn,
            "cashier": cashier,
            "employee_id": req.employee_id,
            "sent_by": a.sent_by_name,
            "sender": a.sent_by,
            "diffs": diffs,
            "extras": extras.iter().map(|(name, _, _, qty)| json!({ "name": name, "qty": qty })).collect::<Vec<_>>(),
            "comment": accept_comment,
        }),
    )
    .await?;
    let out = AcceptOut {
        acceptance_id: id,
        receipt_id: receipt.id,
        receipt_number: receipt.number,
    };
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// «Принять товар» на планшете (SPEC-39 §3–4): право «Приёмка товара», кассир, PIN, подпись.
async fn accept(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<AcceptReq>,
) -> AppResult<Json<AcceptOut>> {
    {
        let mut conn = state.pool.acquire().await?;
        crate::auth::require_permission(&mut conn, &ctx.user, "acceptance").await?;
    }
    check_accept_fields(&req)?;
    let png = signature_png(&req.signature_png)?;
    let cashier = cashier_pin(&state, &ctx, req.employee_id, &req.pin).await?;
    let mut tx = state.pool.begin().await?;
    let out = accept_tx(&mut tx, &ctx, id, req, png, &cashier).await?;
    tx.commit().await?;
    Ok(Json(out))
}

// ---------- Уведомления ----------

/// Текст владельцу о приёмке (`receipt.accept`, SPEC-39 §9): заголовок и подробности.
pub(crate) fn accept_text(data: &Value) -> (String, String) {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let num = |v: &Value, key: &str| v.get(key).and_then(Value::as_i64).unwrap_or(0);
    let cashier = Some(text(data, "cashier"))
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "кассир".into());
    let title = format!("Приход № {} принял {cashier}", num(data, "number"));
    let lines = num(data, "lines");
    let doc = text(data, "doc");
    let mut head = text(data, "supplier");
    if !doc.is_empty() {
        head.push_str(&format!(", накладная № {doc}"));
    }
    head.push_str(&format!(
        " — {lines} {} на {}",
        plural(lines, "строка", "строки", "строк"),
        crate::domain::money::format_som(num(data, "total"))
    ));
    let mut parts = Vec::new();
    let mut damaged = Vec::new();
    for d in data
        .get("diffs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = text(d, "name");
        let unit = text(d, "unit");
        let container = d.get("container_ml").and_then(Value::as_i64);
        let (invoice, accepted) = (num(d, "invoice"), num(d, "accepted"));
        let comment = Some(text(d, "comment"))
            .filter(|c| !c.is_empty())
            .map(|c| format!(" («{c}»)"))
            .unwrap_or_default();
        let replaced = text(d, "replaced_by");
        if !replaced.is_empty() {
            let theirs = d.get("replaced_container_ml").and_then(Value::as_i64);
            let mut s = format!(
                "вместо {name} принят {replaced} — {}",
                qty_words(accepted, &unit, theirs)
            );
            if accepted != invoice {
                s.push_str(&format!(
                    " (по накладной {})",
                    qty_words(invoice, &unit, container)
                ));
            }
            s.push_str(&comment);
            parts.push(s);
        } else if accepted > invoice {
            parts.push(format!(
                "{name} — {}, +{} сверх накладной{comment}",
                pair_words(accepted, invoice, &unit, container),
                qty_words(accepted - invoice, &unit, container)
            ));
        } else if accepted < invoice && text(d, "reason") == "damaged" {
            damaged.push(format!(
                "{name} — {}{comment}",
                qty_words(invoice - accepted, &unit, container)
            ));
            continue;
        } else if accepted < invoice {
            let why = match text(d, "reason").as_str() {
                "missing" if accepted == 0 => " — не привезли".to_string(),
                "other" => " — другое".to_string(),
                _ => String::new(),
            };
            parts.push(format!(
                "{name} — {}{why}{comment}",
                pair_words(accepted, invoice, &unit, container)
            ));
        } else {
            parts.push(format!("{name}{comment}"));
        }
    }
    let extras: Vec<String> = data
        .get("extras")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|x| format!("{} {} шт", text(x, "name"), num(x, "qty")))
        .collect();
    let mut details = format!("{head}.");
    if parts.is_empty() && damaged.is_empty() && extras.is_empty() {
        details.push_str(" По накладной, без расхождений.");
    }
    if !parts.is_empty() {
        details.push_str(&format!(" Расхождения: {}.", parts.join("; ")));
    }
    if !damaged.is_empty() {
        details.push_str(&format!(" Брак при приёмке: {}.", damaged.join("; ")));
    }
    if !extras.is_empty() {
        details.push_str(&format!(
            " Не по накладной: {} — провести приходом или оприходованием.",
            extras.join(", ")
        ));
    }
    let comment = text(data, "comment");
    if !comment.is_empty() {
        details.push_str(&format!(" Комментарий: «{comment}»."));
    }
    (title, details)
}

/// Фото подписи кассира следом за «Приход № N принял …» в чат владельца (SPEC-39 §9).
/// Не ушло — только запись в журнал сервера: текст уже в чате, подпись видна в программе.
pub async fn send_signature(
    conn: &mut PgConnection,
    api: &crate::api::telegram::Api,
    chat_id: i64,
    receipt_id: Uuid,
) -> AppResult<()> {
    let row = sqlx::query!(
        r#"select a.signature_png, e.full_name as "cashier?"
           from receipt_acceptances a left join employees e on e.id = a.employee_id
           where a.receipt_id = $1"#,
        receipt_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(r) = row
        && let Some(png) = r.signature_png
    {
        let caption = format!("Подпись: {}", r.cashier.unwrap_or_else(|| "кассир".into()));
        let sent = api.send_photo(chat_id, &png, &caption).await;
        if sent != crate::api::telegram::Delivery::Sent {
            tracing::warn!(chat_id, "телеграм: фото подписи приёмки не отправлено");
        }
    }
    Ok(())
}

/// Сообщение не владельцу по приёмке (SPEC-39 §1): кассиру открытой смены — «Примите товар»
/// без цен; администратору, отправившему накладную, — «Товар принят: приход № N».
pub struct Notice {
    pub audit_id: Uuid,
    pub chat_id: i64,
    pub text: String,
}

/// Ещё не отправленные сообщения по приёмкам за сутки; что ушло — отмечено в `telegram_sent`.
pub async fn notices(conn: &mut PgConnection) -> AppResult<Vec<Notice>> {
    let mut out = Vec::new();
    // Кассир открытой смены филиала, подключённый к боту как работник (SPEC-23), — пока
    // приёмка ждёт. Подключение — то же, что у кабинета работника: номер в карточке тот же.
    let waiting = sqlx::query!(
        r#"select a.id as audit_id, s.name as supplier, x.supplier_doc, te.chat_id, te.phone as link_phone,
                  e.phone, (select count(*) from receipt_acceptance_lines l where l.acceptance_id = x.id) as "lines!"
           from audit_log a
           join receipt_acceptances x on x.id = a.entity_id and x.status = 'waiting'
           join suppliers s on s.id = x.supplier_id
           join shifts sh on sh.branch_id = x.branch_id
                         and not exists (select 1 from shift_closes c
                                         where c.shift_id = sh.id
                                           and not exists (select 1 from shift_reopens r where r.close_id = c.id))
           join employees e on e.id = sh.cashier_employee_id and e.active
           join telegram_employees te on te.employee_id = e.id
           where a.action = 'acceptance.send' and a.created_at > now() - interval '1 day'
             and not exists (select 1 from telegram_sent t where t.chat_id = te.chat_id and t.audit_id = a.id)
           order by a.created_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in waiting {
        if crate::domain::phone::key(&r.phone) != crate::domain::phone::key(&r.link_phone) {
            continue;
        }
        let doc = if r.supplier_doc.is_empty() {
            String::new()
        } else {
            format!(", накладная № {}", r.supplier_doc)
        };
        out.push(Notice {
            audit_id: r.audit_id,
            chat_id: r.chat_id,
            text: format!(
                "📦 Примите товар: {}{doc} — {} {}, на планшете",
                r.supplier,
                r.lines,
                plural(r.lines, "строка", "строки", "строк")
            ),
        });
    }
    // Администратор, приславший накладную, — итог по ней (SPEC-39 §7). Владелец узнаёт из ленты.
    let accepted = sqlx::query!(
        r#"select a.id as audit_id, x.supplier_doc, s.name as supplier, r.number, e.full_name as cashier, t.chat_id
           from audit_log a
           join receipt_acceptances x on x.receipt_id = a.entity_id
           join suppliers s on s.id = x.supplier_id
           join receipts r on r.id = x.receipt_id
           join employees e on e.id = x.employee_id
           join users u on u.id = x.sent_by and u.active and u.role = 'admin'
           join telegram_links t on t.user_id = u.id
           where a.action = 'receipt.accept' and a.created_at > now() - interval '1 day'
             and a.created_at >= t.linked_at
             and not exists (select 1 from telegram_sent z where z.chat_id = t.chat_id and z.audit_id = a.id)
           order by a.created_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    for r in accepted {
        let what = if r.supplier_doc.is_empty() {
            format!("от «{}»", r.supplier)
        } else {
            format!("по накладной № {}", r.supplier_doc)
        };
        out.push(Notice {
            audit_id: r.audit_id,
            chat_id: r.chat_id,
            text: format!(
                "✅ Товар {what} принят: приход № {} ({})",
                r.number, r.cashier
            ),
        });
    }
    Ok(out)
}

/// Сообщение ушло (или Телеграм его никогда не примет).
pub async fn mark_notice(conn: &mut PgConnection, n: &Notice) -> AppResult<()> {
    sqlx::query!(
        "insert into telegram_sent (chat_id, audit_id) values ($1, $2) on conflict do nothing",
        n.chat_id,
        n.audit_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ---------- Бот: «📦 На приёмку» ----------

/// Данные кнопки «📦 На приёмку» под ответом на фото накладной.
pub fn button_data(draft_id: Uuid) -> String {
    format!("a:{draft_id}")
}

/// Нажатие «📦 На приёмку» (SPEC-39 §2): нажимает только тот, кто привязал чат (ADR-070); сервер
/// заново проверяет право и черновик — распознано ли всё ещё точно и не отправлено ли уже.
pub async fn press_bot(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    data: &str,
) -> AppResult<String> {
    const STALE: &str =
        "Кнопка устарела — проверьте накладную в программе: «Приход» → «Накладные по фото».";
    let Some(draft_id) = data
        .strip_prefix("a:")
        .and_then(|s| Uuid::parse_str(s).ok())
    else {
        return Ok(STALE.into());
    };
    let Some((user, linked_by)) = crate::api::receipt_drafts::chat_sender(conn, chat_id).await?
    else {
        return Ok(STALE.into());
    };
    if linked_by.is_some_and(|t| t != from_id) {
        return Ok("Кнопку нажимает тот, кто привязал этот чат.".into());
    }
    if !crate::auth::has_permission(conn, &user, "receipts").await? {
        return Ok("Нет права «Приход товара» — на приёмку не отправить.".into());
    }
    let exact = match crate::api::receipt_drafts::exact_invoice(conn, &user, draft_id).await {
        Ok(e) => e,
        Err(AppError::NotFound) => return Ok(STALE.into()),
        Err(e) => return Err(e),
    };
    let Some(inv) = exact else {
        return Ok("Распознано не всё точно — проверьте накладную в программе: «Приход» → «Накладные по фото».".into());
    };
    let lines = inv.lines.len();
    let ctx = Ctx::bot(user, chat_id);
    let mut tx = conn.begin().await?;
    let sent = send_tx(
        &mut tx,
        &ctx,
        SendReq {
            op_id: new_id(),
            draft_id: Some(draft_id),
            supplier_id: Some(inv.supplier_id),
            supplier_doc: inv.supplier_doc.clone(),
            comment: String::new(),
            lines: inv.lines,
        },
    )
    .await;
    match sent {
        Ok(_) => tx.commit().await?,
        Err(AppError::Conflict(m) | AppError::Validation(m) | AppError::ForbiddenWith(m)) => {
            return Ok(format!("Не отправлено: {m}."));
        }
        Err(e) => return Err(e),
    }
    let n = i64::try_from(lines).unwrap_or(i64::MAX);
    let doc = if inv.supplier_doc.is_empty() {
        String::new()
    } else {
        format!(", накладная № {}", inv.supplier_doc)
    };
    Ok(format!(
        "📦 Отправлено на приёмку: {}{doc} — {n} {}. Кассир примет товар на планшете, итог придёт сюда.",
        inv.supplier_name,
        plural(n, "строка", "строки", "строк")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_like_the_standard() {
        assert_eq!(decode_base64(""), Some(vec![]));
        assert_eq!(decode_base64("Zg=="), Some(b"f".to_vec()));
        assert_eq!(decode_base64("Zm8="), Some(b"fo".to_vec()));
        assert_eq!(decode_base64("Zm9v"), Some(b"foo".to_vec()));
        assert_eq!(decode_base64("Zm9v\nYmFy"), Some(b"foobar".to_vec()));
        assert_eq!(
            decode_base64("/9j/4A=="),
            Some(vec![0xFF, 0xD8, 0xFF, 0xE0])
        );
        assert_eq!(decode_base64("Zm9"), None);
        assert_eq!(decode_base64("Zm9*"), None);
    }

    #[test]
    fn signature_must_be_png_and_small() {
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        assert!(signature_png(png).is_ok_and(|b| b.starts_with(&PNG_MAGIC)));
        assert!(signature_png("").is_err());
        // JPEG — не подпись.
        assert!(signature_png("/9j/4AAQSkZJRgABAQ==").is_err());
        let big = "A".repeat(MAX_SIGNATURE / 3 * 4 + 12);
        assert!(signature_png(&big).is_err());
    }

    #[test]
    fn owner_text_names_every_difference() {
        let data = json!({
            "number": 41, "supplier": "Автоснаб", "doc": "15", "lines": 12, "total": 4_830_000,
            "cashier": "Айбек",
            "diffs": [
                { "name": "Фильтр W712/75", "unit": "piece", "invoice": 20, "accepted": 18, "reason": "missing" },
                { "name": "Масло Shell 4 л", "unit": "ml", "container_ml": 4000, "invoice": 8000, "accepted": 0, "reason": "missing" },
                { "name": "Свеча", "unit": "piece", "invoice": 20, "accepted": 22 },
                { "name": "Колодки", "unit": "piece", "invoice": 4, "accepted": 2, "reason": "damaged" },
                { "name": "Shell 4 л", "unit": "ml", "container_ml": 4000, "invoice": 8000, "accepted": 8000,
                  "replaced_by": "Mobil 4 л", "replaced_container_ml": 4000 }
            ],
            "extras": [{ "name": "щётки", "qty": 2 }]
        });
        let (title, text) = accept_text(&data);
        assert_eq!(title, "Приход № 41 принял Айбек");
        assert!(
            text.starts_with("Автоснаб, накладная № 15 — 12 строк на 48 300 с."),
            "{text}"
        );
        assert!(text.contains("Фильтр W712/75 — 18 из 20"), "{text}");
        assert!(
            text.contains("Масло Shell 4 л — 0 из 2 кан. — не привезли"),
            "{text}"
        );
        assert!(
            text.contains("Свеча — 22 из 20, +2 шт сверх накладной"),
            "{text}"
        );
        assert!(text.contains("Брак при приёмке: Колодки — 2 шт"), "{text}");
        assert!(
            text.contains("вместо Shell 4 л принят Mobil 4 л — 2 кан."),
            "{text}"
        );
        assert!(text.contains("Не по накладной: щётки 2 шт"), "{text}");
        let (_, clean) = accept_text(
            &json!({ "number": 2, "supplier": "Автоснаб", "lines": 1, "total": 100, "cashier": "Айбек", "diffs": [] }),
        );
        assert!(clean.ends_with("По накладной, без расхождений."), "{clean}");
        assert_eq!(pair_words(5500, 8000, "ml", Some(4000)), "5,5 из 8 л");
        assert_eq!(pair_words(12_500, 15_000, "g", None), "12,5 из 15 кг");
    }
}
