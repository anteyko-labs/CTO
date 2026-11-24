//! Подтверждение заправки юрлицом (задача 039 C, ADR-202). Чек в долг фирмы-клиента — запрос
//! «Подтвердите обслуживание» в Телеграм фирмы и её кабинет. Режим — настройка филиала `legal`:
//! `wait` — чек ждёт подтверждения, `after` — чек сразу, подтверждение — отметка, `timeout` — ждёт
//! N минут, дальше кассир может провести без подтверждения, `off` — не спрашивать.
//!
//! Режимы `wait` и `timeout`: касса шлёт черновик чека (`POST /sales/approvals`, тот же `op_id`),
//! опрашивает запрос и после подтверждения проводит тот же чек обычным `POST /sales` — повтор с
//! тем же `op_id` второго чека не создаёт (I7, I8). Сервер в чеке сверяет запрос (`on_sale`).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::sales::SaleReq;
use crate::api::telegram::{Button, Keys, Reply};
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sales/approvals", routing::post(create))
        .route("/sales/approvals/{id}", routing::get(get_one))
        .route("/sales/approvals/{id}/force", routing::post(force))
        .route(
            "/settings/legal",
            routing::get(get_settings).put(put_settings),
        )
}

pub const MODES: [&str; 4] = ["wait", "after", "timeout", "off"];

/// «Настройки → Юрлица»: `legal.approval_mode`, `legal.approval_timeout_min`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct LegalSettings {
    pub approval_mode: String,
    pub approval_timeout_min: i64,
}

impl Default for LegalSettings {
    fn default() -> Self {
        Self {
            approval_mode: "timeout".into(),
            approval_timeout_min: 10,
        }
    }
}

pub async fn load_settings(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<LegalSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'legal'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let mut s = v
        .and_then(|v| serde_json::from_value::<LegalSettings>(v).ok())
        .unwrap_or_default();
    if !MODES.contains(&s.approval_mode.as_str()) {
        s.approval_mode = LegalSettings::default().approval_mode;
    }
    if !(1..=240).contains(&s.approval_timeout_min) {
        s.approval_timeout_min = 10;
    }
    Ok(s)
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<LegalSettings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_settings(&mut conn, user.branch_id).await?))
}

#[derive(Deserialize)]
struct SettingsIn {
    op_id: Uuid,
    #[serde(flatten)]
    settings: LegalSettings,
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<LegalSettings>> {
    ctx.user.require_owner()?;
    const KIND: &str = "settings.legal";
    let s = req.settings;
    if !MODES.contains(&s.approval_mode.as_str()) {
        return Err(invalid(
            "режим: ждать, после, с таймаутом или не спрашивать",
        ));
    }
    if !(1..=240).contains(&s.approval_timeout_min) {
        return Err(invalid("таймаут — от 1 до 240 минут"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<LegalSettings>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load_settings(&mut tx, ctx.user.branch_id).await?;
    let value = serde_json::to_value(&s).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'legal', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != s {
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "settings",
            None,
            json!({ "before": before, "after": s }),
        )
        .await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &s).await?;
    tx.commit().await?;
    Ok(Json(s))
}

/// Запрос для кассы и кабинета.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApprovalOut {
    pub id: Option<Uuid>,
    /// `not_needed` — запрос не нужен (режим выключен или «после», не фирма, нет долга);
    /// иначе — статус запроса.
    pub status: String,
    pub mode: String,
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
    pub summary: Value,
    pub debt_tyiyn: i64,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: String,
    pub reason: String,
    pub forced: bool,
    /// Чек проведён без сети, запрос создан после синхронизации.
    pub offline: bool,
    /// Кассир может нажать «Провести без подтверждения».
    pub can_force: bool,
    /// Чек по запросу можно проводить.
    pub can_post: bool,
}

fn not_needed(mode: &str) -> ApprovalOut {
    ApprovalOut {
        id: None,
        status: "not_needed".into(),
        mode: mode.into(),
        sale_id: None,
        sale_number: None,
        summary: json!({}),
        debt_tyiyn: 0,
        created_at: None,
        expires_at: None,
        decided_at: None,
        decided_by: String::new(),
        reason: String::new(),
        forced: false,
        offline: false,
        can_force: false,
        can_post: true,
    }
}

/// Таймаут истёк — запрос «просрочен» (кассир может провести без подтверждения).
async fn expire_due(conn: &mut PgConnection) -> AppResult<()> {
    sqlx::query!(
        "update sale_approvals set status = 'expired'
         where status = 'pending' and mode = 'timeout' and expires_at <= now()"
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn load(conn: &mut PgConnection, id: Uuid) -> AppResult<ApprovalOut> {
    expire_due(conn).await?;
    let r = sqlx::query!(
        r#"select a.id, a.status, a.mode, a.sale_id, s.number as "sale_number?", a.summary,
                  a.debt_tyiyn, a.created_at, a.expires_at, a.decided_at, a.decided_by, a.reason,
                  a.forced_at, a.offline
           from sale_approvals a left join sales s on s.id = a.sale_id where a.id = $1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let forced = r.forced_at.is_some();
    Ok(ApprovalOut {
        id: Some(r.id),
        can_force: r.status == "expired" && !forced && r.sale_id.is_none(),
        can_post: matches!(r.status.as_str(), "approved" | "skipped") || forced,
        status: r.status,
        mode: r.mode,
        sale_id: r.sale_id,
        sale_number: r.sale_number,
        summary: r.summary,
        debt_tyiyn: r.debt_tyiyn,
        created_at: Some(r.created_at),
        expires_at: r.expires_at,
        decided_at: r.decided_at,
        decided_by: r.decided_by,
        reason: r.reason,
        forced,
        offline: r.offline,
    })
}

fn debt_of(req: &SaleReq) -> i64 {
    req.payments
        .iter()
        .filter(|p| p.method == "debt")
        .map(|p| p.amount_tyiyn)
        .sum()
}

/// Фирма-клиент этого филиала: только у неё спрашивают.
async fn is_company(conn: &mut PgConnection, party_id: Uuid, branch_id: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from parties where id = $1 and branch_id = $2
                  and kind = 'company' and role = 'customer') as "e!""#,
        party_id,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// У фирмы есть кому ответить: Телеграм или кабинет (п. 9).
async fn reachable(conn: &mut PgConnection, party_id: Uuid) -> AppResult<bool> {
    if !crate::api::telegram::party_chats(conn, party_id)
        .await?
        .is_empty()
    {
        return Ok(true);
    }
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from party_accounts where party_id = $1) as "e!""#,
        party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Что видит фирма: машина, работник, позиции, сумма, кассир.
async fn summary(conn: &mut PgConnection, req: &SaleReq, debt: i64) -> AppResult<Value> {
    let plate = match req.vehicle_id {
        Some(v) => {
            sqlx::query_scalar!("select plate from party_vehicles where id = $1", v)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let contact = match req.contact_id {
        Some(c) => {
            sqlx::query_scalar!("select full_name from party_contacts where id = $1", c)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    let cashier = sqlx::query_scalar!(
        "select full_name from employees where id = $1",
        req.cashier_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or_default();
    let mut lines = Vec::new();
    for l in req.lines.iter().take(50) {
        let name = if let Some(p) = l.product_id {
            sqlx::query_scalar!("select name from products where id = $1", p)
                .fetch_optional(&mut *conn)
                .await?
        } else if let Some(s) = l.service_id {
            sqlx::query_scalar!("select name from services where id = $1", s)
                .fetch_optional(&mut *conn)
                .await?
        } else {
            None
        };
        lines.push(name.unwrap_or_default());
    }
    let total: i64 = req.payments.iter().map(|p| p.amount_tyiyn).sum();
    Ok(json!({
        "plate": plate, "contact": contact, "cashier": cashier, "lines": lines,
        "total_tyiyn": total, "debt_tyiyn": debt,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn insert(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: &SaleReq,
    party_id: Uuid,
    mode: &str,
    status: &str,
    draft: Option<&Value>,
    sale_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
    offline: bool,
) -> AppResult<Uuid> {
    let id = ops::new_id();
    let debt = debt_of(req);
    let summary = summary(conn, req, debt).await?;
    sqlx::query!(
        r#"insert into sale_approvals (id, branch_id, party_id, op_id, sale_id, mode, status, draft,
                                      summary, debt_tyiyn, user_id, expires_at, vehicle_id, offline)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)"#,
        id,
        ctx.user.branch_id,
        party_id,
        req.op_id,
        sale_id,
        mode,
        status,
        draft,
        summary,
        debt,
        ctx.user.id,
        expires_at,
        req.vehicle_id,
        offline
    )
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

#[derive(Deserialize)]
struct CreateIn {
    draft: Value,
}

/// Касса, режимы «ждать» и «с таймаутом»: черновик чека → запрос фирме. Повтор с тем же `op_id`
/// чека возвращает тот же запрос.
async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateIn>,
) -> AppResult<Json<ApprovalOut>> {
    let mut sale: SaleReq =
        serde_json::from_value(req.draft.clone()).map_err(|e| invalid(format!("чек: {e}")))?;
    let mut tx = state.pool.begin().await?;
    // Личная машина работника — запрос фирме, которая платит (задача 037, ADR-199).
    crate::api::legal_garage::payer_rewrite(&mut tx, &mut sale).await?;
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 1))",
        sale.op_id.to_string()
    )
    .execute(&mut *tx)
    .await?;
    let existing = sqlx::query!(
        "select id, branch_id from sale_approvals where op_id = $1",
        sale.op_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(e) = existing {
        if e.branch_id != ctx.user.branch_id {
            return Err(AppError::NotFound);
        }
        let out = load(&mut tx, e.id).await?;
        tx.commit().await?;
        return Ok(Json(out));
    }
    let s = load_settings(&mut tx, ctx.user.branch_id).await?;
    let mode = s.approval_mode.as_str();
    let debt = debt_of(&sale);
    let Some(party_id) = sale.party_id else {
        return Ok(Json(not_needed(mode)));
    };
    if !matches!(mode, "wait" | "timeout")
        || debt <= 0
        || !is_company(&mut tx, party_id, ctx.user.branch_id).await?
    {
        return Ok(Json(not_needed(mode)));
    }
    let id = if reachable(&mut tx, party_id).await? {
        let expires =
            (mode == "timeout").then(|| Utc::now() + Duration::minutes(s.approval_timeout_min));
        insert(
            &mut tx,
            &ctx,
            &sale,
            party_id,
            mode,
            "pending",
            Some(&req.draft),
            None,
            expires,
            false,
        )
        .await?
    } else {
        insert(
            &mut tx,
            &ctx,
            &sale,
            party_id,
            mode,
            "skipped",
            Some(&req.draft),
            None,
            None,
            false,
        )
        .await?
    };
    let out = load(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn branch_of(conn: &mut PgConnection, id: Uuid) -> AppResult<Uuid> {
    sqlx::query_scalar!("select branch_id from sale_approvals where id = $1", id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(AppError::NotFound)
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ApprovalOut>> {
    let mut conn = state.pool.acquire().await?;
    if branch_of(&mut conn, id).await? != user.branch_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(load(&mut conn, id).await?))
}

/// «Провести без подтверждения»: только после таймаута, чек ещё не проведён.
async fn force(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ApprovalOut>> {
    let mut tx = state.pool.begin().await?;
    expire_due(&mut tx).await?;
    let r = sqlx::query!(
        "select branch_id, party_id, status, forced_at, sale_id, debt_tyiyn
         from sale_approvals where id = $1 for update",
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if r.branch_id != ctx.user.branch_id {
        return Err(AppError::NotFound);
    }
    if r.forced_at.is_none() {
        if r.status != "expired" || r.sale_id.is_some() {
            return Err(AppError::Conflict(
                "провести без подтверждения можно только после таймаута".into(),
            ));
        }
        sqlx::query!(
            "update sale_approvals set forced_at = now(), forced_by = $2 where id = $1",
            id,
            ctx.user.id
        )
        .execute(&mut *tx)
        .await?;
        journal(&mut tx, id, "forced", ctx.user.full_name.as_str()).await?;
    }
    let out = load(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Проверка в проведении чека: долг фирме в режимах «ждать» и «с таймаутом» — только по
/// подтверждённому (или проведённому без подтверждения) запросу; режим «после» — запрос после
/// чека. Чек без сети уже состоялся — не останавливается.
/// `responsibility` — пояснение кассира (задача 046, ADR-213): нет ответа фирмы — чек проводится,
/// запрос остаётся открытым с `forced_*`, возвращается причина для журнала. Отказ фирмы не обходится.
pub async fn on_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    req: &SaleReq,
    debt: i64,
    offline: bool,
    responsibility: Option<&str>,
) -> AppResult<Option<String>> {
    let Some(party_id) = req.party_id else {
        return Ok(None);
    };
    if debt <= 0 {
        return Ok(None);
    }
    expire_due(conn).await?;
    let row = sqlx::query!(
        "select id, party_id, status, forced_at, debt_tyiyn, sale_id from sale_approvals
         where op_id = $1 for update",
        req.op_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(r) = row {
        if r.party_id != party_id || r.debt_tyiyn != debt {
            return Err(AppError::ConflictCode(
                "approval_changed",
                "Чек изменился после запроса фирме — запросите подтверждение заново".into(),
            ));
        }
        let ok = matches!(r.status.as_str(), "approved" | "skipped") || r.forced_at.is_some();
        let mut reason = None;
        if !ok && !offline {
            match (r.status.as_str(), responsibility) {
                ("rejected", _) => {
                    return Err(AppError::ConflictCode(
                        "approval_rejected",
                        "Фирма отклонила обслуживание — чек не проведён".into(),
                    ));
                }
                (_, Some(note)) => {
                    force_by_cashier(conn, ctx, r.id, note).await?;
                    reason = Some(NO_ANSWER.to_string());
                }
                _ => {
                    return Err(AppError::ConflictCode(
                        "approval_required",
                        "Ждём подтверждения фирмы".into(),
                    ));
                }
            }
        }
        sqlx::query!(
            "update sale_approvals set sale_id = $2 where id = $1 and sale_id is null",
            r.id,
            sale_id
        )
        .execute(&mut *conn)
        .await?;
        return Ok(reason);
    }
    if req.correction.is_some() {
        return Ok(None);
    }
    let s = load_settings(conn, ctx.user.branch_id).await?;
    let mode = s.approval_mode.as_str();
    if mode == "off" || !is_company(conn, party_id, ctx.user.branch_id).await? {
        return Ok(None);
    }
    // Чек без сети уже состоялся (I10): после синхронизации — запрос фирме с отметкой
    // «проведено без сети», фирма отвечает как в режиме «после», владельцу — событие.
    if offline {
        if !reachable(conn, party_id).await? {
            return Ok(None);
        }
        let id = insert(
            conn,
            ctx,
            req,
            party_id,
            mode,
            "pending",
            None,
            Some(sale_id),
            None,
            true,
        )
        .await?;
        journal(conn, id, "offline", "").await?;
        return Ok(None);
    }
    if !reachable(conn, party_id).await? {
        // Спросить некого — чек как раньше, отметка «не спрошено».
        insert(
            conn,
            ctx,
            req,
            party_id,
            mode,
            "skipped",
            None,
            Some(sale_id),
            None,
            false,
        )
        .await?;
        return Ok(None);
    }
    if mode == "after" {
        insert(
            conn,
            ctx,
            req,
            party_id,
            mode,
            "pending",
            None,
            Some(sale_id),
            None,
            false,
        )
        .await?;
        return Ok(None);
    }
    if let Some(note) = responsibility {
        // Запроса ещё не было — он появляется уже с чеком, фирма ответит позже (отметка).
        let id = insert(
            conn,
            ctx,
            req,
            party_id,
            mode,
            "pending",
            None,
            Some(sale_id),
            None,
            false,
        )
        .await?;
        force_by_cashier(conn, ctx, id, note).await?;
        return Ok(Some(NO_ANSWER.to_string()));
    }
    Err(AppError::ConflictCode(
        "approval_required",
        "Сначала запросите подтверждение фирмы".into(),
    ))
}

/// Причина в журнале чека под ответственность кассира, когда фирма ещё не ответила.
const NO_ANSWER: &str = "фирма ещё не подтвердила обслуживание";

/// Чек проведён под ответственность кассира до ответа фирмы: запрос остаётся открытым.
async fn force_by_cashier(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    note: &str,
) -> AppResult<()> {
    sqlx::query!(
        "update sale_approvals set forced_at = now(), forced_by = $2, forced_note = $3
         where id = $1",
        id,
        ctx.user.id,
        note
    )
    .execute(&mut *conn)
    .await?;
    // Владельцу — одно событие чека `sale.cashier_responsibility`, а не ещё и ответ фирмы.
    Ok(())
}

/// Отметка запроса на экране чека.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SaleMark {
    pub status: String,
    pub forced: bool,
    pub reason: String,
    pub decided_by: String,
    /// Чек проведён без сети, фирму спросили после синхронизации.
    #[serde(default)]
    pub offline: bool,
    /// Под ответственность кассира (задача 046): кто провёл и его пояснение.
    #[serde(default)]
    pub forced_note: Option<String>,
    #[serde(default)]
    pub forced_by: Option<String>,
}

pub async fn mark_of(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<Option<SaleMark>> {
    let r = sqlx::query!(
        r#"select a.status, a.forced_at, a.reason, a.decided_by, a.offline, a.forced_note,
                  u.full_name as "forced_by?"
           from sale_approvals a left join users u on u.id = a.forced_by
           where a.sale_id = $1"#,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(r.map(|r| SaleMark {
        status: r.status,
        forced: r.forced_at.is_some(),
        reason: r.reason,
        decided_by: r.decided_by,
        offline: r.offline,
        forced_by: r.forced_note.as_ref().and(r.forced_by),
        forced_note: r.forced_note,
    }))
}

/// Событие владельцу `party.sale_approval`: решение фирмы или проведение без подтверждения.
async fn journal(conn: &mut PgConnection, id: Uuid, decision: &str, by: &str) -> AppResult<()> {
    let r = sqlx::query!(
        r#"select a.branch_id, a.party_id, p.name, a.debt_tyiyn, a.reason, a.mode, a.offline,
                  s.number as "number?"
           from sale_approvals a join parties p on p.id = a.party_id
           left join sales s on s.id = a.sale_id where a.id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, 'party.sale_approval', 'party', $3, $4)"#,
        ops::new_id(),
        r.branch_id,
        r.party_id,
        json!({
            "approval_id": id, "name": r.name, "decision": decision, "reason": r.reason,
            "offline": r.offline,
            "debt_tyiyn": r.debt_tyiyn, "number": r.number, "mode": r.mode, "by": by,
        })
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Решение фирмы — из кабинета или бота: любой с доступом к фирме (5а). Решить можно, пока
/// запрос ждёт (или просрочен, но чек ещё не проведён без подтверждения).
pub async fn decide(
    conn: &mut PgConnection,
    party_id: Uuid,
    id: Uuid,
    approve: bool,
    reason: &str,
    by: &str,
) -> AppResult<ApprovalOut> {
    decide_as(conn, party_id, id, approve, reason, by, true).await
}

/// Запрос ещё ждёт ответа: ждёт или просрочен, но чек не проведён без подтверждения.
fn is_open(status: &str, forced: bool, has_sale: bool) -> bool {
    status == "pending" || (status == "expired" && !forced && !has_sale)
}

/// Решение; из бота отказ можно без причины («Пропустить»).
async fn decide_as(
    conn: &mut PgConnection,
    party_id: Uuid,
    id: Uuid,
    approve: bool,
    reason: &str,
    by: &str,
    need_reason: bool,
) -> AppResult<ApprovalOut> {
    let reason = reason.trim();
    if !approve && need_reason && reason.is_empty() {
        return Err(invalid("укажите причину отказа"));
    }
    if reason.chars().count() > 500 {
        return Err(invalid("причина — не длиннее 500 знаков"));
    }
    expire_due(conn).await?;
    let r = sqlx::query!(
        "select party_id, status, forced_at, sale_id, mode from sale_approvals where id = $1 for update",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if r.party_id != party_id {
        return Err(AppError::NotFound);
    }
    if !is_open(&r.status, r.forced_at.is_some(), r.sale_id.is_some()) {
        return Err(AppError::Conflict("на этот запрос уже ответили".into()));
    }
    let status = if approve { "approved" } else { "rejected" };
    sqlx::query!(
        "update sale_approvals set status = $2, decided_at = now(), decided_by = $3, reason = $4
         where id = $1",
        id,
        status,
        by,
        reason
    )
    .execute(&mut *conn)
    .await?;
    journal(conn, id, status, by).await?;
    load(conn, id).await
}

/// Запросы фирмы для кабинета.
pub async fn list_for_party(
    conn: &mut PgConnection,
    party_id: Uuid,
) -> AppResult<Vec<ApprovalOut>> {
    expire_due(conn).await?;
    let ids = sqlx::query_scalar!(
        "select id from sale_approvals where party_id = $1 and status <> 'skipped'
         order by created_at desc limit 50",
        party_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        out.push(load(conn, id).await?);
    }
    Ok(out)
}

/// Запрос с кнопками в чат фирмы.
pub struct Notice {
    pub approval_id: Uuid,
    pub chat_id: i64,
    pub text: String,
}

fn money(t: i64) -> String {
    crate::domain::money::format_som(t)
}

pub fn notice_text(name: &str, a: &ApprovalOut) -> String {
    let s = &a.summary;
    let text = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let lines: Vec<String> = s
        .get("lines")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .filter(|x| !x.is_empty())
                .map(|x| format!("• {x}"))
                .collect()
        })
        .unwrap_or_default();
    let mut out = format!("🚗 Подтвердите обслуживание — {name}\n");
    if !text("plate").is_empty() {
        out.push_str(&format!("Машина: {}\n", text("plate")));
    }
    if !text("contact").is_empty() {
        out.push_str(&format!("Клиент: {}\n", text("contact")));
    }
    if !lines.is_empty() {
        out.push_str(&format!("{}\n", lines.join("\n")));
    }
    out.push_str(&format!("В долг: {}\n", money(a.debt_tyiyn)));
    if !text("cashier").is_empty() {
        out.push_str(&format!("Кассир: {}", text("cashier")));
    }
    out.trim_end().to_string()
}

/// Неотправленные запросы в чаты фирм (за сутки).
pub async fn notices(conn: &mut PgConnection) -> AppResult<Vec<Notice>> {
    expire_due(conn).await?;
    let rows = sqlx::query!(
        r#"select a.id, a.party_id, p.name from sale_approvals a join parties p on p.id = a.party_id
           where a.status in ('pending', 'expired')
             and (a.forced_at is null or (a.forced_note is not null and a.status = 'pending'))
             and a.created_at > now() - interval '1 day'
           order by a.created_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let a = load(conn, r.id).await?;
        let text = notice_text(&r.name, &a);
        for chat_id in crate::api::telegram::party_chats(conn, r.party_id).await? {
            let sent = sqlx::query_scalar!(
                r#"select exists (select 1 from sale_approval_notices
                          where approval_id = $1 and chat_id = $2) as "e!""#,
                r.id,
                chat_id
            )
            .fetch_one(&mut *conn)
            .await?;
            if !sent {
                out.push(Notice {
                    approval_id: r.id,
                    chat_id,
                    text: text.clone(),
                });
            }
        }
    }
    Ok(out)
}

pub async fn mark_notice(conn: &mut PgConnection, n: &Notice) -> AppResult<()> {
    sqlx::query!(
        "insert into sale_approval_notices (approval_id, chat_id) values ($1, $2) on conflict do nothing",
        n.approval_id,
        n.chat_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Решение фирмы по личной машине работника — ему в бот (задача 039, ADR-202): подтверждать
/// сам себе работник не может, запрос уходит только фирме.
pub struct OwnerNotice {
    pub approval_id: Uuid,
    pub chats: Vec<i64>,
    pub text: String,
}

pub async fn owner_notices(conn: &mut PgConnection) -> AppResult<Vec<OwnerNotice>> {
    let rows = sqlx::query!(
        r#"select a.id, a.status, a.reason, v.plate, v.party_id as owner, p.name
           from sale_approvals a join party_vehicles v on v.id = a.vehicle_id
           join parties p on p.id = a.party_id
           where a.vehicle_id is not null and a.owner_notified_at is null
             and a.status in ('approved', 'rejected') and v.party_id <> a.party_id
             and a.decided_at > now() - interval '1 day'
           order by a.decided_at"#
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::new();
    for r in rows {
        let chats = crate::api::telegram::party_chats(conn, r.owner).await?;
        out.push(OwnerNotice {
            approval_id: r.id,
            chats,
            text: owner_text(&r.name, &r.plate, &r.status, &r.reason),
        });
    }
    Ok(out)
}

pub fn owner_text(firm: &str, plate: &str, status: &str, reason: &str) -> String {
    if status == "approved" {
        format!("✅ Фирма {firm} подтвердила заправку вашей машины {plate}.")
    } else if reason.is_empty() {
        format!("✖ Фирма {firm} отклонила заправку вашей машины {plate}.")
    } else {
        format!("✖ Фирма {firm} отклонила заправку вашей машины {plate}: {reason}.")
    }
}

pub async fn mark_owner_notice(conn: &mut PgConnection, approval_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "update sale_approvals set owner_notified_at = now() where id = $1",
        approval_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Кнопки под запросом: `ap:ok:<id>` и `ap:no:<id>`.
pub fn buttons(id: Uuid) -> Vec<(String, String)> {
    vec![
        ("✅ Подтвердить".into(), format!("ap:ok:{id}")),
        ("✖ Отклонить".into(), format!("ap:no:{id}")),
    ]
}

/// Нажатие кнопки в чате фирмы: запрос должен быть этой фирмы. «✖ Отклонить» спрашивает
/// причину: следующее сообщение чата — причина, «Пропустить» — без причины.
pub async fn press_bot(
    conn: &mut PgConnection,
    party_id: Uuid,
    chat_id: i64,
    data: &str,
) -> AppResult<Reply> {
    let text = |t: &str| Ok(Reply::new(t, Keys::Customer));
    let mut parts = data.splitn(3, ':');
    let (_, act, id) = (parts.next(), parts.next(), parts.next());
    let Some(id) = id.and_then(|i| Uuid::parse_str(i).ok()) else {
        return text(STALE);
    };
    match act {
        Some("ok") => answer(conn, party_id, id, true, "").await,
        Some("skip") => {
            crate::api::bot_cars::clear_input(conn, chat_id).await?;
            answer(conn, party_id, id, false, "").await
        }
        Some("no") => {
            expire_due(conn).await?;
            let r = sqlx::query!(
                "select branch_id, status, forced_at, sale_id from sale_approvals
                 where id = $1 and party_id = $2",
                id,
                party_id
            )
            .fetch_optional(&mut *conn)
            .await?;
            let Some(r) = r else {
                return text("Запрос не найден.");
            };
            if !is_open(&r.status, r.forced_at.is_some(), r.sale_id.is_some()) {
                return text("На этот запрос уже ответили.");
            }
            crate::api::bot_cars::wait_for(
                conn,
                r.branch_id,
                chat_id,
                "ap_reason",
                None,
                json!({ "approval_id": id }),
            )
            .await?;
            Ok(Reply::with_buttons(
                ASK_REASON,
                vec![vec![Button {
                    text: "Пропустить".into(),
                    data: format!("ap:skip:{id}"),
                }]],
            ))
        }
        _ => text(STALE),
    }
}

const STALE: &str = "Кнопка устарела.";
pub const ASK_REASON: &str = "Напишите причину отказа одним сообщением — её увидят кассир и владелец. Или нажмите «Пропустить».";

/// Бот ждёт причину отказа (`ap_reason`): сообщение чата — причина. `None` — ничего не ждём.
pub async fn on_reason(
    conn: &mut PgConnection,
    party_id: Uuid,
    chat_id: i64,
    text: Option<&str>,
) -> AppResult<Option<Reply>> {
    let wait = sqlx::query_scalar!(
        "select data from bot_input where chat_id = $1 and kind = 'ap_reason' and expires_at > now()",
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(data) = wait else {
        return Ok(None);
    };
    let id = data
        .get("approval_id")
        .and_then(Value::as_str)
        .and_then(|i| Uuid::parse_str(i).ok());
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(Some(Reply::new(ASK_REASON, Keys::Customer)));
    };
    crate::api::bot_cars::clear_input(conn, chat_id).await?;
    let Some(id) = id else {
        return Ok(Some(Reply::new(STALE, Keys::Customer)));
    };
    let reason: String = text.chars().take(500).collect();
    Ok(Some(answer(conn, party_id, id, false, &reason).await?))
}

async fn answer(
    conn: &mut PgConnection,
    party_id: Uuid,
    id: Uuid,
    approve: bool,
    reason: &str,
) -> AppResult<Reply> {
    let t = match decide_as(conn, party_id, id, approve, reason, "Телеграм", false).await {
        Ok(_) if approve => "✅ Обслуживание подтверждено.".to_string(),
        Ok(_) if reason.is_empty() => "✖ Обслуживание отклонено.".to_string(),
        Ok(_) => format!("✖ Обслуживание отклонено: {reason}."),
        Err(AppError::NotFound) => "Запрос не найден.".into(),
        Err(AppError::Conflict(m)) => format!("{}.", capitalize(&m)),
        Err(e) => return Err(e),
    };
    Ok(Reply::new(t, Keys::Customer))
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
