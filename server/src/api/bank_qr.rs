//! QR на сумму чека (SPEC-31 §2, ADR-106): касса просит у сервера QR банка Optima Business на
//! сумму к оплате картой, сервер сам опрашивает банк каждые 2 с, касса спрашивает у сервера.
//! Чек со ссылкой на оплаченный QR проверяется: оплачен, сумма та же, к другому чеку не привязан.
//! Опрос — параллельно, ждущие первыми; поздние оплаты ловятся 30 минут после срока (ADR-130).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

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
/// Сколько минут после срока ещё ловить позднюю оплату QR.
const LATE_MINUTES: i64 = 30;
/// Сколько запросов статуса в банк идут одновременно (ADR-130).
const POLL_PARALLEL: usize = 8;
/// Не ждущий QR (не оплачен, отменён, ошибка) опрашивается не чаще раза в 10 с (ADR-130).
const LATE_EVERY: std::time::Duration = std::time::Duration::from_secs(10);
/// Касса видит свои ждущие и оплаченные без чека QR за 40 минут (ADR-130).
const OPEN_MINUTES: i32 = 40;
/// Подпись платежа в банке.
pub const NOTE: &str = "Оплата, чек кассы Avtodom";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/bank-qr", routing::post(create))
        .route("/bank-qr/open", routing::get(open_list))
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
    /// Сколько оплачено по ответу банка (у оплаченного).
    pub paid_tyiyn: Option<i64>,
    pub sale_id: Option<Uuid>,
    /// Картинка PNG в base64 — пока QR ждёт оплаты.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qr_base64: Option<String>,
}

struct QrRow {
    id: Uuid,
    amount_tyiyn: i64,
    status: String,
    expires_at: DateTime<Utc>,
    paid_at: Option<DateTime<Utc>>,
    paid_tyiyn: Option<i64>,
    sale_id: Option<Uuid>,
    qr_base64: String,
    left: i64,
}

impl From<QrRow> for BankQrOut {
    fn from(r: QrRow) -> Self {
        let waiting = r.status == "waiting";
        BankQrOut {
            id: r.id,
            amount_tyiyn: r.amount_tyiyn,
            seconds_left: if waiting { r.left } else { 0 },
            qr_base64: waiting.then_some(r.qr_base64),
            status: r.status,
            expires_at: r.expires_at,
            paid_at: r.paid_at,
            paid_tyiyn: r.paid_tyiyn,
            sale_id: r.sale_id,
        }
    }
}

async fn load(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<BankQrOut> {
    let r = sqlx::query_as!(
        QrRow,
        r#"select id, amount_tyiyn, status, expires_at, paid_at, paid_tyiyn, sale_id, qr_base64,
                  greatest(0, extract(epoch from expires_at - now()))::bigint as "left!"
           from bank_qr where id = $1 and branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(r.into())
}

/// QR этой кассы (устройства) за 40 минут: ждущие и оплаченные без чека (ADR-130). Касса
/// спрашивает при открытии и раз в 5 с: оплата не теряется после перезагрузки и «Отменить».
async fn open_list(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<Vec<BankQrOut>>> {
    let rows = sqlx::query_as!(
        QrRow,
        r#"select id, amount_tyiyn, status, expires_at, paid_at, paid_tyiyn, sale_id, qr_base64,
                  greatest(0, extract(epoch from expires_at - now()))::bigint as "left!"
           from bank_qr
           where branch_id = $1 and device_id = $2
             and created_at > now() - make_interval(mins => $3)
             and (status = 'waiting' or (status = 'paid' and sale_id is null))
           order by created_at desc"#,
        ctx.user.branch_id,
        ctx.device_id,
        OPEN_MINUTES
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows.into_iter().map(BankQrOut::from).collect()))
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
    // Новый QR заменяет прежние ждущие и неоплаченные QR этой кассы (ADR-130): их больше не
    // ждут, а поздняя оплата по ним — «оплачен» и сразу владельцу (как после «Отменить»).
    let replaced = sqlx::query!(
        r#"update bank_qr set status = 'cancelled'
           where branch_id = $1 and device_id = $2 and id <> $3 and sale_id is null
             and status in ('waiting', 'unpaid')
           returning id, amount_tyiyn"#,
        branch_id,
        ctx.device_id,
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    for r in replaced {
        ops::audit(
            &mut tx,
            &ctx,
            "bank_qr.cancel",
            "bank_qr",
            Some(r.id),
            json!({ "amount": r.amount_tyiyn, "replaced_by": id }),
        )
        .await?;
    }
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

/// «Отменить»: QR больше не ждём. У банка срока нет (untilDateTime = null), поэтому отменённый
/// QR ещё LATE_MINUTES опрашивается: оплата после отмены становится «оплачен» с тревогой
/// владельцу. Оплаченный не отменяется.
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

/// Когда не ждущий QR последний раз спрашивали у банка (ADR-130: раз в 10 с). В памяти:
/// после перезапуска сервера первый опрос — сразу.
static LATE_POLLED: LazyLock<Mutex<HashMap<Uuid, Instant>>> = LazyLock::new(Default::default);

/// QR, который опрашиваем.
struct Pending {
    id: Uuid,
    branch_id: Uuid,
    transaction_id: String,
    amount_tyiyn: i64,
    user_id: Uuid,
    device_id: Uuid,
    expires_at: DateTime<Utc>,
    status: String,
}

/// Один проход опроса: ждущие QR первыми, запросы в банк — параллельно (не больше 8), у
/// каждого свой срок ответа: медленный ответ по одному QR не задерживает другие (ADR-130).
/// Сбой по одному QR пишется в журнал сервера, проход продолжается.
pub async fn poll_once(pool: &PgPool, bank: &Optima) -> AppResult<()> {
    // Неоплаченный в срок, отменённый и отклонённый QR ещё LATE_MINUTES опрашивается: покупатель
    // может оплатить позже (тест банка 23.10.2026 — оплата через минуту после срока), у банка
    // отменённый на кассе QR живой. Неопрошенная оплата через 12 с становится у банка ошибкой,
    // а деньги — без чека.
    let rows = sqlx::query_as!(
        Pending,
        r#"select id, branch_id, transaction_id, amount_tyiyn, user_id, device_id, expires_at,
                  status as "status!"
           from bank_qr
           where status = 'waiting'
              or (status in ('unpaid', 'cancelled', 'error') and sale_id is null
                  and expires_at > now() - make_interval(mins => $1))
           order by status <> 'waiting', created_at desc
           limit 100"#,
        LATE_MINUTES as i32
    )
    .fetch_all(pool)
    .await?;
    let due: Vec<Pending> = {
        let mut seen = LATE_POLLED.lock().unwrap_or_else(PoisonError::into_inner);
        let keep = std::time::Duration::from_secs(60 * (LATE_MINUTES + QR_MINUTES).unsigned_abs());
        seen.retain(|_, t| t.elapsed() < keep);
        rows.into_iter()
            .filter(|q| {
                if q.status == "waiting" {
                    return true;
                }
                match seen.get(&q.id) {
                    Some(t) if t.elapsed() < LATE_EVERY => false,
                    _ => {
                        seen.insert(q.id, Instant::now());
                        true
                    }
                }
            })
            .collect()
    };
    let mut set = tokio::task::JoinSet::new();
    for q in due {
        while set.len() >= POLL_PARALLEL {
            reap(set.join_next().await);
        }
        let (pool, bank) = (pool.clone(), bank.clone());
        set.spawn(async move {
            let id = q.id;
            (id, poll_one(&pool, &bank, q).await)
        });
    }
    while !set.is_empty() {
        reap(set.join_next().await);
    }
    Ok(())
}

type PollDone = Option<Result<(Uuid, AppResult<()>), tokio::task::JoinError>>;

fn reap(done: PollDone) {
    match done {
        Some(Ok((id, Err(e)))) => tracing::warn!(qr = %id, error = %e, "банк: опрос QR не прошёл"),
        Some(Err(e)) => tracing::warn!(error = %e, "банк: опрос QR оборвался"),
        _ => {}
    }
}

/// Статус одного QR. `PROCESSED` — оплачен (и после срока, и у отменённого, и у отклонённого);
/// `NOT_EXIST` после срока — не оплачен; другой ответ банка после срока ждёт ещё GRACE_SECONDS;
/// `ERROR` — банк отклонил. Банк молчит дольше срока с запасом — не оплачен: кассир отметит
/// оплату вручную.
async fn poll_one(pool: &PgPool, bank: &Optima, q: Pending) -> AppResult<()> {
    let now = Utc::now();
    let late = now > q.expires_at + Duration::seconds(GRACE_SECONDS);
    let expired = now >= q.expires_at;
    let answer = bank.qr_status(&q.transaction_id).await;
    if let Err(e) = &answer
        && *e != BankError::NotFound
    {
        optima::key_problem(pool, q.branch_id, e).await;
    }
    let next = match answer {
        Ok(s) if s.status == "PROCESSED" => Some(("paid", Some(s))),
        // После срока и у отменённого ждём только оплату: остальное уже записано.
        _ if q.status != "waiting" => None,
        Ok(s) if s.status == "ERROR" => Some(("error", None)),
        Ok(s) if s.status == "NOT_EXIST" => expired.then_some(("unpaid", None)),
        Err(BankError::NotFound) => expired.then_some(("unpaid", None)),
        Ok(_) | Err(_) => late.then_some(("unpaid", None)),
    };
    let Some((status, state)) = next else {
        return Ok(());
    };
    let mut tx = pool.begin().await?;
    let paid_sum = state.as_ref().and_then(|s| s.sum);
    // Прежнее состояние — на момент записи, под блокировкой строки: «Отменить» или новый QR
    // могли отменить этот, пока шёл запрос в банк (ADR-130).
    let prev = sqlx::query_scalar!(
        r#"with old as (select id, status from bank_qr where id = $1 for update)
           update bank_qr q set status = $2, paid_tyiyn = $3, operation_id = $4,
                  paid_at = case when $2 = 'paid' then coalesce($5, now()) end
           from old
           where q.id = old.id
             and (old.status = 'waiting'
                  or ($2 = 'paid' and old.status in ('unpaid', 'cancelled', 'error')
                      and q.sale_id is null))
           returning old.status as "prev!""#,
        q.id,
        status,
        paid_sum,
        state.as_ref().and_then(|s| s.operation_id.clone()),
        state.as_ref().and_then(|s| s.processed_at)
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(prev) = prev
        && status == "paid"
    {
        let after_cancel = prev == "cancelled";
        audit_as(
            &mut tx,
            q.branch_id,
            Some(q.user_id),
            Some(q.device_id),
            "bank_qr.paid",
            q.id,
            json!({ "amount": q.amount_tyiyn, "paid": paid_sum,
                    "transaction_id": q.transaction_id, "late": prev != "waiting",
                    "after_cancel": after_cancel }),
        )
        .await?;
        // Кассир этот QR уже не ждёт — владельцу сразу, без 15 минут.
        if after_cancel {
            sqlx::query!("update bank_qr set alerted_at = now() where id = $1", q.id)
                .execute(&mut *tx)
                .await?;
            audit_as(
                &mut tx,
                q.branch_id,
                Some(q.user_id),
                Some(q.device_id),
                "bank_qr.unclaimed",
                q.id,
                json!({ "amount": paid_sum.unwrap_or(q.amount_tyiyn), "after_cancel": true }),
            )
            .await?;
        }
    }
    tx.commit().await?;
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
