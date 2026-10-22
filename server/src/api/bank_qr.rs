//! QR на сумму чека (SPEC-31 §2, ADR-106): касса просит у сервера QR банка Optima Business на
//! сумму к оплате картой, сервер сам опрашивает банк каждые 2 с, касса спрашивает у сервера.
//! Чек со ссылкой на оплаченный QR проверяется: оплачен, сумма та же, к другому чеку не привязан.

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::api::optima::{self, BankError, Optima, QrRequest};
use crate::api::sales::PaymentReq;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::format_som;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// QR действует 10 минут.
pub const QR_MINUTES: i64 = 10;
/// Оплачен, а чека нет 15 минут — владельцу.
pub const UNCLAIMED_MINUTES: i64 = 15;
/// После срока ещё немного спрашиваем банк: оплата могла пройти в последнюю секунду.
const GRACE_SECONDS: i64 = 60;
/// Подпись платежа в банке.
pub const NOTE: &str = "Оплата, чек кассы Avtodom";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bank-qr", routing::post(create))
        .route("/bank-qr/{id}", routing::get(get_one))
        .route("/bank-qr/{id}/cancel", routing::post(cancel))
}

#[derive(Deserialize)]
struct CreateReq {
    op_id: Uuid,
    amount_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BankQrOut {
    pub id: Uuid,
    pub amount_tyiyn: i64,
    /// `waiting` — ждём, `paid` — оплачен, `unpaid` — не оплачен за срок, `error` — банк
    /// отклонил, `cancelled` — кассир отменил.
    pub status: String,
    pub expires_at: DateTime<Utc>,
    /// Сколько секунд QR ещё действует (по часам сервера).
    pub seconds_left: i64,
    pub paid_at: Option<DateTime<Utc>>,
    pub sale_id: Option<Uuid>,
    /// Картинка PNG в base64 — пока QR ждёт оплаты.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_base64: Option<String>,
}

async fn load(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<BankQrOut> {
    let r = sqlx::query!(
        r#"select id, amount_tyiyn, status, expires_at, paid_at, sale_id, qr_base64,
                  greatest(0, extract(epoch from expires_at - now()))::bigint as "left!"
           from bank_qr where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(BankQrOut {
        id: r.id,
        amount_tyiyn: r.amount_tyiyn,
        seconds_left: if r.status == "waiting" { r.left } else { 0 },
        qr_base64: (r.status == "waiting").then_some(r.qr_base64),
        status: r.status,
        expires_at: r.expires_at,
        paid_at: r.paid_at,
        sale_id: r.sale_id,
    })
}

/// «QR на сумму»: новый одноразовый QR в банке. Повтор с тем же `op_id` — тот же QR.
async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<BankQrOut>> {
    let bank = state
        .optima
        .as_ref()
        .ok_or_else(|| invalid("QR банка не настроен на сервере"))?;
    let branch_id = ctx.user.branch_id;
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма QR больше нуля"));
    }
    ops::check_amount(req.amount_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    // Повтор того же запроса ждёт первый и получает его QR.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        req.op_id.to_string()
    )
    .execute(&mut *tx)
    .await?;
    let done = sqlx::query!(
        "select id, branch_id, user_id from bank_qr where op_id = $1",
        req.op_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(d) = done {
        if d.branch_id != branch_id || d.user_id != ctx.user.id {
            return Err(AppError::Conflict(
                "op_id уже использован другой операцией".into(),
            ));
        }
        let out = load(&mut tx, branch_id, d.id).await?;
        tx.commit().await?;
        return Ok(Json(out));
    }
    let s = optima::load_settings(&mut tx, branch_id).await?;
    let (Some(legal), Some(point), Some(cash)) = (s.legal_party_id, s.sale_point_code, s.cash_code)
    else {
        return Err(invalid(
            "QR банка не настроен: владельцу — «Настройки» → «Optima Business»",
        ));
    };
    let expires_at = Utc::now() + Duration::minutes(QR_MINUTES);
    let qr = match bank
        .generate_qr(&QrRequest {
            legal_party_id: legal,
            sale_point_code: point,
            cash_code: cash,
            amount: req.amount_tyiyn,
            until: expires_at,
            note: NOTE,
        })
        .await
    {
        Ok(q) => q,
        Err(e) => {
            drop(tx);
            optima::key_problem(&state.pool, branch_id, &e).await;
            return Err(e.into());
        }
    };
    let id = new_id();
    sqlx::query!(
        r#"insert into bank_qr (id, branch_id, op_id, user_id, device_id, amount_tyiyn,
                                transaction_id, qr_base64, qr_url, expires_at)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        req.op_id,
        ctx.user.id,
        ctx.device_id,
        req.amount_tyiyn,
        qr.transaction_id,
        qr.qr_base64,
        qr.qr_url,
        expires_at
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "bank_qr.create",
        "bank_qr",
        Some(id),
        json!({ "amount": req.amount_tyiyn, "transaction_id": qr.transaction_id }),
    )
    .await?;
    let out = load(&mut tx, branch_id, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Состояние QR — касса спрашивает каждые 2 с; банк опрашивает сервер.
async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<BankQrOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, user.branch_id, id).await?))
}

/// «Отменить»: QR больше не ждём (у банка он истечёт сам). Оплаченный не отменяется.
async fn cancel(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<BankQrOut>> {
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query!(
        "select status, amount_tyiyn from bank_qr where id = $1 and branch_id = $2 for update",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    match row.status.as_str() {
        "waiting" => {
            sqlx::query!("update bank_qr set status = 'cancelled' where id = $1", id)
                .execute(&mut *tx)
                .await?;
            ops::audit(
                &mut tx,
                &ctx,
                "bank_qr.cancel",
                "bank_qr",
                Some(id),
                json!({ "amount": row.amount_tyiyn }),
            )
            .await?;
        }
        "paid" => {
            return Err(AppError::Conflict(
                "QR уже оплачен — проведите чек с этой оплатой".into(),
            ));
        }
        _ => {}
    }
    let out = load(&mut tx, branch_id, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Запись журнала от имени создавшего QR (опрос идёт без запроса пользователя).
async fn audit_as(
    conn: &mut PgConnection,
    branch_id: Uuid,
    user_id: Option<Uuid>,
    device_id: Option<Uuid>,
    action: &str,
    id: Uuid,
    data: serde_json::Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data)
           values ($1, $2, $3, $4, $5, 'bank_qr', $6, $7)"#,
        new_id(),
        branch_id,
        user_id,
        device_id,
        action,
        id,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Один проход опроса: каждый ждущий QR — запрос статуса в банк. `PROCESSED` — оплачен,
/// `NOT_EXIST` после срока — не оплачен, `ERROR` — банк отклонил. Банк молчит дольше срока
/// с запасом — не оплачен: кассир отметит оплату вручную.
pub async fn poll_once(pool: &PgPool, bank: &Optima) -> AppResult<()> {
    let waiting = sqlx::query!(
        r#"select id, branch_id, transaction_id, amount_tyiyn, user_id, device_id, expires_at
           from bank_qr where status = 'waiting' order by created_at limit 50"#
    )
    .fetch_all(pool)
    .await?;
    for q in waiting {
        let now = Utc::now();
        let late = now > q.expires_at + Duration::seconds(GRACE_SECONDS);
        let next = match bank.qr_status(&q.transaction_id).await {
            Ok(s) if s.status == "PROCESSED" => Some(("paid", Some(s))),
            Ok(s) if s.status == "ERROR" => Some(("error", None)),
            Ok(_) | Err(BankError::NotFound) if now >= q.expires_at => Some(("unpaid", None)),
            Ok(_) | Err(BankError::NotFound) => None,
            Err(e) => {
                optima::key_problem(pool, q.branch_id, &e).await;
                late.then_some(("unpaid", None))
            }
        };
        let Some((status, state)) = next else {
            continue;
        };
        let mut tx = pool.begin().await?;
        let paid_sum = state.as_ref().and_then(|s| s.sum);
        let changed = sqlx::query!(
            r#"update bank_qr set status = $2, paid_tyiyn = $3, operation_id = $4,
                      paid_at = case when $2 = 'paid' then coalesce($5, now()) end
               where id = $1 and status = 'waiting'"#,
            q.id,
            status,
            paid_sum,
            state.as_ref().and_then(|s| s.operation_id.clone()),
            state.as_ref().and_then(|s| s.processed_at)
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if changed > 0 && status == "paid" {
            audit_as(
                &mut tx,
                q.branch_id,
                Some(q.user_id),
                Some(q.device_id),
                "bank_qr.paid",
                q.id,
                json!({ "amount": q.amount_tyiyn, "paid": paid_sum,
                        "transaction_id": q.transaction_id }),
            )
            .await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

/// Оплачен, а чек не проведён 15 минут — владельцу один раз (лента и бот).
pub async fn alert_unclaimed(pool: &PgPool) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query!(
        r#"update bank_qr set alerted_at = now()
           where status = 'paid' and sale_id is null and alerted_at is null
             and paid_at < now() - make_interval(mins => $1::int)
           returning id, branch_id, user_id, device_id, amount_tyiyn, paid_tyiyn, paid_at"#,
        i32::try_from(UNCLAIMED_MINUTES).unwrap_or(15)
    )
    .fetch_all(&mut *tx)
    .await?;
    for r in rows {
        audit_as(
            &mut tx,
            r.branch_id,
            Some(r.user_id),
            Some(r.device_id),
            "bank_qr.unclaimed",
            r.id,
            json!({ "amount": r.paid_tyiyn.unwrap_or(r.amount_tyiyn), "paid_at": r.paid_at }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Оплата картой чека со ссылкой на QR: QR оплачен, сумма та же, к другому чеку не привязан —
/// иначе 409. Деньги ложатся на счёт Optima из настройки. Чек без сети уже состоялся: ссылка
/// с ошибкой снимается, причина уходит владельцу. Возвращает QR, которые привяжет чек.
pub async fn check_for_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    payments: &mut [PaymentReq],
    offline: bool,
    bypass: &mut Vec<String>,
) -> AppResult<Vec<Uuid>> {
    let branch_id = ctx.user.branch_id;
    let mut linked: Vec<Uuid> = Vec::new();
    for p in payments.iter_mut() {
        let Some(qr_id) = p.bank_qr_id else {
            continue;
        };
        let problem = if p.method != "card" {
            Some("QR банка — только у оплаты «Карта / QR банка»".to_string())
        } else if linked.contains(&qr_id) {
            Some("один QR — одна оплата".to_string())
        } else {
            let row = sqlx::query!(
                r#"select status, amount_tyiyn, paid_tyiyn, sale_id from bank_qr
                   where id = $1 and branch_id = $2 for update"#,
                qr_id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            match row {
                None => Some("QR банка не найден".to_string()),
                Some(r) if r.sale_id.is_some() => {
                    Some("этот QR уже оплатил другой чек".to_string())
                }
                Some(r) if r.status != "paid" => Some(format!(
                    "QR банка {}",
                    match r.status.as_str() {
                        "waiting" => "ещё не оплачен",
                        "unpaid" => "не оплачен",
                        "cancelled" => "отменён",
                        _ => "отклонён банком",
                    }
                )),
                Some(r)
                    if r.amount_tyiyn != p.amount_tyiyn
                        || r.paid_tyiyn.is_some_and(|x| x != p.amount_tyiyn) =>
                {
                    Some(format!(
                        "по QR оплачено {}, а картой в чеке {}",
                        format_som(r.paid_tyiyn.unwrap_or(r.amount_tyiyn)),
                        format_som(p.amount_tyiyn)
                    ))
                }
                Some(_) => None,
            }
        };
        match problem {
            None => {
                linked.push(qr_id);
                let s = optima::load_settings(conn, branch_id).await?;
                if let Some(acc) = optima::money_account(conn, branch_id, &s).await? {
                    p.account_id = Some(acc);
                }
            }
            Some(msg) if offline => {
                p.bank_qr_id = None;
                bypass.push(format!("{msg} — оплата картой записана без QR"));
            }
            Some(msg) => return Err(AppError::Conflict(msg)),
        }
    }
    Ok(linked)
}

/// Чек провёл оплаты по QR — QR привязан к чеку.
pub async fn link_sale(conn: &mut PgConnection, sale_id: Uuid, qr_ids: &[Uuid]) -> AppResult<()> {
    if qr_ids.is_empty() {
        return Ok(());
    }
    sqlx::query!(
        "update bank_qr set sale_id = $1 where id = any($2) and sale_id is null",
        sale_id,
        qr_ids
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Оплаченные QR без чека за время смены — строка в отчёте смены.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UnclaimedQr {
    pub id: Uuid,
    pub amount_tyiyn: i64,
    pub paid_at: Option<DateTime<Utc>>,
    pub user_name: String,
}

pub async fn unclaimed_between(
    conn: &mut PgConnection,
    branch_id: Uuid,
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
) -> AppResult<Vec<UnclaimedQr>> {
    Ok(sqlx::query_as!(
        UnclaimedQr,
        r#"select q.id, coalesce(q.paid_tyiyn, q.amount_tyiyn) as "amount_tyiyn!", q.paid_at,
                  u.full_name as user_name
           from bank_qr q join users u on u.id = q.user_id
           where q.branch_id = $1 and q.status = 'paid' and q.sale_id is null
             and q.paid_at >= $2 and q.paid_at < coalesce($3, 'infinity'::timestamptz)
           order by q.paid_at"#,
        branch_id,
        start,
        end
    )
    .fetch_all(&mut *conn)
    .await?)
}
