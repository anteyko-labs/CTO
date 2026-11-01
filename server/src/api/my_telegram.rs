//! «Мой Телеграм» администратора (SPEC-40 §10, ADR-172): администратор, войдя на сайт, сам
//! получает код привязки своего чата (тот же код на 15 минут, ссылка и QR, что даёт владелец в
//! «Пользователях», ADR-135), видит, привязан ли чат и к какому Телеграм-имени, что увидит в боте
//! по своим правам, и может отвязать чат.
//!
//! Контроль владельца: настройка филиала «Сотрудники привязывают Телеграм сами»
//! (`settings`, ключ `telegram_admins`, по умолчанию — да); выключена — код администратору не
//! выдаётся (403), выданные им самим коды удаляются той же транзакцией. Владельцу —
//! уведомление о привязке (`telegram.admin_link`) и о том, что администратор отвязал чат сам
//! (`telegram.admin_unlink`). Владелец по-прежнему отвязывает любого (`/users/{id}/telegram`).
//!
//! Код привязывается только к самому вошедшему администратору (id — из сессии, не из запроса);
//! только роль `admin`, только свой филиал (I12, I16); изменения — с `op_id` (I8) и в журнал (I5).

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::bot_admin::{self, ItemOut};
use crate::auth::{Ctx, CurrentUser, Owner, Role};
use crate::error::{AppError, AppResult};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/me/telegram", routing::get(status).delete(unlink))
        .route("/me/telegram/code", routing::post(new_code))
        .route(
            "/settings/telegram/admins",
            routing::get(get_settings).put(put_settings),
        )
}

/// Отказ администратору, когда владелец выключил самостоятельную привязку.
pub const SELF_OFF: &str = "Привязку Телеграма делает владелец — попросите у него код";
/// Отказ не администратору: свой чат владелец привязывает в «Настройки → Телеграм».
const OWNER_HERE: &str = "Владелец привязывает свой чат в «Настройки → Телеграм»";

/// Настройка филиала: сотрудники (входы роли `admin`) привязывают Телеграм сами (по умолчанию — да).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct AdminsSettings {
    pub self_link: bool,
}

impl Default for AdminsSettings {
    fn default() -> Self {
        Self { self_link: true }
    }
}

/// Настройка филиала; нет записи или она не читается — по умолчанию.
pub async fn load(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<AdminsSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'telegram_admins'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| serde_json::from_value::<AdminsSettings>(v).ok())
            .unwrap_or_default(),
    )
}

/// «Мой Телеграм» — только администратору (I12).
fn admin_only(user: &CurrentUser) -> AppResult<()> {
    if user.role == Role::Admin {
        Ok(())
    } else {
        Err(AppError::ForbiddenWith(OWNER_HERE.into()))
    }
}

#[derive(Serialize)]
struct MeOut {
    /// Ключ бота задан на сервере.
    enabled: bool,
    /// Владелец разрешил привязывать самому.
    self_link: bool,
    linked: bool,
    linked_at: Option<DateTime<Utc>>,
    /// Телеграм-имя привязанного чата: @имя или имя в Телеграме.
    tg_name: Option<String>,
    /// Что видит в боте — пункты меню по его правам (как «Что видит в боте» у владельца).
    items: Vec<ItemOut>,
    /// Какие события ему приходят (включил владелец).
    events: Vec<&'static str>,
}

async fn status(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<MeOut>> {
    admin_only(&user)?;
    let mut conn = state.pool.acquire().await?;
    let settings = load(&mut conn, user.branch_id).await?;
    let link = bot_admin::link_of(&mut conn, user.id).await?;
    let items = bot_admin::items_of(&mut conn, &user).await?;
    let events = bot_admin::events_of(&mut conn, &user).await?;
    Ok(Json(MeOut {
        enabled: state.telegram_bot,
        self_link: settings.self_link,
        linked: link.is_some(),
        linked_at: link.as_ref().map(|l| l.0),
        tg_name: link.and_then(|l| l.1),
        items,
        events,
    }))
}

#[derive(Deserialize)]
struct CodeIn {
    op_id: Uuid,
}

#[derive(Serialize, Deserialize)]
struct CodeOut {
    code: String,
    expires_at: DateTime<Utc>,
}

/// Код привязки своего чата: 15 минут, одноразовый, прежний код этого пользователя пропадает —
/// те же правила, что у кода от владельца (ADR-135). Привязывается к вошедшему (id из сессии).
async fn new_code(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CodeIn>,
) -> AppResult<Json<CodeOut>> {
    admin_only(&ctx.user)?;
    const KIND: &str = "telegram.self_code";
    let mut tx = state.pool.begin().await?;
    // Повтор того же op_id — прежний ответ, даже если владелец уже выключил настройку (I8).
    if let Some(done) = ops::begin_op::<CodeOut>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    if !load(&mut tx, ctx.user.branch_id).await?.self_link {
        return Err(AppError::ForbiddenWith(SELF_OFF.into()));
    }
    // Вход отключён или роль сменилась за время запроса — кода нет (сессия таких уже не пускает).
    let me = sqlx::query!(
        "select active, role, full_name from users where id = $1 and branch_id = $2 for update",
        ctx.user.id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::Forbidden)?;
    if !me.active || me.role != "admin" {
        return Err(AppError::Forbidden);
    }
    let code = format!("{:06}", rand::random_range(0..1_000_000u32));
    sqlx::query!(
        "delete from telegram_codes where user_id = $1 or expires_at < now()",
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    let expires_at = sqlx::query_scalar!(
        r#"insert into telegram_codes (code, user_id, expires_at, by_self)
           values ($1, $2, now() + interval '15 minutes', true)
           on conflict (code) do update set user_id = excluded.user_id, expires_at = excluded.expires_at, by_self = true
           returning expires_at"#,
        code,
        ctx.user.id
    )
    .fetch_one(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "telegram.admin_code",
        "user",
        Some(ctx.user.id),
        json!({ "full_name": me.full_name, "via": "self" }),
    )
    .await?;
    let out = CodeOut { code, expires_at };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct OpQuery {
    op_id: Option<Uuid>,
}

/// «Отвязать» свой чат: можно всегда, и при выключенной настройке (потерял телефон — закрыть
/// бот сразу). Владельцу — уведомление `telegram.admin_unlink`.
async fn unlink(
    State(state): State<AppState>,
    ctx: Ctx,
    Query(q): Query<OpQuery>,
) -> AppResult<Json<Value>> {
    admin_only(&ctx.user)?;
    const KIND: &str = "telegram.self_unlink";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_opt::<Value>(&mut tx, &ctx, q.op_id, KIND).await? {
        return Ok(Json(done));
    }
    sqlx::query!("delete from telegram_codes where user_id = $1", ctx.user.id)
        .execute(&mut *tx)
        .await?;
    let gone = sqlx::query_scalar!(
        "delete from telegram_links where user_id = $1 returning chat_id",
        ctx.user.id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(chat) = gone {
        bot_admin::clear(&mut tx, chat).await?;
        ops::audit(
            &mut tx,
            &ctx,
            "telegram.admin_unlink",
            "user",
            Some(ctx.user.id),
            json!({ "full_name": ctx.user.full_name, "chat": chat }),
        )
        .await?;
    }
    let out = json!({ "ok": true, "unlinked": gone.is_some() });
    ops::finish_opt(&mut tx, &ctx, q.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn get_settings(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<AdminsSettings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, user.branch_id).await?))
}

#[derive(Deserialize)]
struct SettingsIn {
    op_id: Uuid,
    #[serde(flatten)]
    settings: AdminsSettings,
}

/// «Сотрудники привязывают Телеграм сами»: только владелец, журнал (I5), `op_id` (I8).
/// Выключил — коды, которые администраторы филиала выдали себе сами, удаляются; уже
/// привязанные чаты остаются (отвязать — «Пользователи → Телеграм»).
async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<AdminsSettings>> {
    ctx.user.require_owner()?;
    const KIND: &str = "settings.telegram_admins";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<AdminsSettings>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load(&mut tx, ctx.user.branch_id).await?;
    let settings = req.settings;
    let value = serde_json::to_value(&settings).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'telegram_admins', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    let dropped = if settings.self_link {
        0
    } else {
        sqlx::query!(
            r#"delete from telegram_codes c using users u
               where u.id = c.user_id and c.by_self and u.branch_id = $1"#,
            ctx.user.branch_id
        )
        .execute(&mut *tx)
        .await?
        .rows_affected()
    };
    if before != settings {
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "settings",
            None,
            json!({ "before": before, "after": settings, "codes_dropped": dropped }),
        )
        .await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &settings).await?;
    tx.commit().await?;
    Ok(Json(settings))
}
