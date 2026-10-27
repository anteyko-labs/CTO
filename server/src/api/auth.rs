//! Вход, выход, текущий пользователь, ограничение перебора паролей и управление пользователями (SPEC-01, ADR-016).

use axum::extract::{Path, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::response::{AppendHeaders, IntoResponse};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{
    self, Ctx, CurrentUser, Owner, Role, SESSION_COOKIE, SESSION_DAYS, hash_password, new_token,
    verify_login_password,
};
use crate::error::{AppError, AppResult, invalid, is_unique_violation};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", routing::post(login))
        .route("/auth/logout", routing::post(logout))
        .route("/auth/me", routing::get(me))
        .route("/users", routing::get(list_users).post(create_user))
        .route("/users/{id}", routing::patch(update_user))
        .route("/users/{id}/permissions", routing::put(put_permissions))
}

#[derive(Deserialize)]
struct LoginReq {
    login: String,
    password: String,
}

fn session_cookie(state: &AppState, value: &str, max_age: i64) -> String {
    let secure = if state.cookie_secure { "; Secure" } else { "" };
    format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    )
}

/// Неудачных попыток до блокировки и её длительность в минутах.
pub(crate) const MAX_FAILURES: i32 = 5;
const LOCK_MINUTES: i32 = 15;
/// Сколько неудач по одному логину со всех адресов за окно — повод сказать владельцу.
const ALERT_FAILURES_ANY_ADDR: i32 = 20;
/// Неудачных входов с одного адреса по любым логинам за окно: дальше адрес ждёт окно — подбор
/// по многим логинам не крутит argon2 и не растит таблицу попыток (ADR-128).
pub(crate) const MAX_FAILURES_ADDR: i32 = 50;
/// Блокировок PIN кассира подряд, после которых PIN закрыт до нового (ADR-128).
pub(crate) const PIN_LOCKS: i32 = 3;
/// Строки попыток старше суток без действующей блокировки удаляются (ADR-128).
const ATTEMPTS_KEEP_HOURS: i32 = 24;

/// Ключ счётчика неудач адреса по любым логинам.
pub(crate) fn addr_key(addr: &str) -> String {
    format!("*|{addr}")
}

/// Удаляет старые строки попыток входа (последняя неудача старше суток, блокировки нет).
pub async fn clean_login_attempts(pool: &sqlx::PgPool) -> AppResult<u64> {
    Ok(sqlx::query!(
        r#"delete from login_attempts
           where last_failed_at < now() - make_interval(hours => $1)
             and (locked_until is null or locked_until < now())"#,
        ATTEMPTS_KEEP_HOURS
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Чистка таблицы попыток раз в час.
pub fn spawn_cleanup(pool: sqlx::PgPool) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Err(e) = clean_login_attempts(&pool).await {
                tracing::warn!(error = %e, "чистка попыток входа не прошла");
            }
        }
    });
}

/// Состояние PIN кассира перед проверкой.
pub(crate) enum PinGate {
    Open,
    /// Заблокирован на `minutes`; `fresh` — блокировка поставлена этой попыткой.
    Locked {
        minutes: i64,
        fresh: bool,
    },
    /// Закрыт до нового PIN от владельца.
    Closed {
        fresh: bool,
    },
}

/// Попытка PIN кассира (ADR-128). PIN — 4 цифры, поэтому блокировка счёт не обнуляет: каждые
/// `MAX_FAILURES` неверных — блокировка на `LOCK_MINUTES`, после `PIN_LOCKS` блокировок PIN
/// закрыт, пока владелец не задаст новый. Счёт снимают верный PIN и новый PIN; сутки без
/// попыток — счёт заново. Попытка учитывается до проверки одной строкой под блокировкой.
pub(crate) async fn reserve_pin(state: &AppState, key: &str) -> AppResult<PinGate> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query!(
        r#"insert into login_attempts (login, failures, last_failed_at) values ($1, 1, now())
           on conflict (login) do update set
             failures = case when login_attempts.locked_until > now() then login_attempts.failures
                             when login_attempts.last_failed_at < now() - interval '1 day' then 1
                             else login_attempts.failures + 1 end,
             last_failed_at = case when login_attempts.locked_until > now()
                                   then login_attempts.last_failed_at else now() end,
             locked_until = case when login_attempts.locked_until > now()
                                 then login_attempts.locked_until end
           returning failures,
                     coalesce(locked_until > now() + interval '1 year', false) as "closed!",
                     ceil(extract(epoch from locked_until - now()) / 60)::bigint as "left_min?""#,
        key
    )
    .fetch_one(&mut *tx)
    .await?;
    let n = row.failures;
    let gate = if row.closed {
        PinGate::Closed { fresh: false }
    } else if let Some(m) = row.left_min.filter(|m| *m > 0) {
        PinGate::Locked {
            minutes: m,
            fresh: false,
        }
    } else if n > MAX_FAILURES && (n - 1) % MAX_FAILURES == 0 {
        let close = (n - 1) / MAX_FAILURES >= PIN_LOCKS;
        sqlx::query!(
            r#"update login_attempts
               set locked_until = case when $2 then now() + interval '100 years'
                                       else now() + make_interval(mins => $3) end
               where login = $1"#,
            key,
            close,
            LOCK_MINUTES
        )
        .execute(&mut *tx)
        .await?;
        tracing::warn!(key = %key, close, "PIN кассира заблокирован после неверных попыток");
        if close {
            PinGate::Closed { fresh: true }
        } else {
            PinGate::Locked {
                minutes: i64::from(LOCK_MINUTES),
                fresh: true,
            }
        }
    } else {
        PinGate::Open
    };
    tx.commit().await?;
    Ok(gate)
}

/// Попытка входа: жёсткий предел — только на «логин + адрес» (ADR-016, ADR-049). Общий счётчик
/// на логин вход не закрывает: иначе любой мог бы неверными паролями запереть владельца
/// (ADR-059). Он лишь сообщает владельцу о подборе, см. `note_failure`.
pub(crate) async fn reserve_login(
    state: &AppState,
    login: &str,
    addr: &str,
) -> AppResult<Option<i64>> {
    if let Some(m) = reserve_with_limit(state, &addr_key(addr), MAX_FAILURES_ADDR).await? {
        return Ok(Some(m));
    }
    reserve_with_limit(state, &format!("{login}|{addr}"), MAX_FAILURES).await
}

/// Успешный вход снимает счётчик своего адреса. Общий счётчик остаётся: он только для
/// уведомления, и вход владельца не должен прятать подбор с других адресов.
pub(crate) async fn clear_login(state: &AppState, login: &str, addr: &str) -> AppResult<()> {
    sqlx::query!(
        "delete from login_attempts where login = any($1)",
        &[format!("{login}|{addr}"), addr_key(addr)][..]
    )
    .execute(&state.pool)
    .await?;
    Ok(())
}

/// Снимает счётчик неудачных попыток по ключу: верный PIN кассира или пароль владельца при
/// открытии смены (ADR-081), новый PIN, заданный владельцем.
pub(crate) async fn clear_attempts(state: &AppState, key: &str) -> AppResult<()> {
    sqlx::query!("delete from login_attempts where login = $1", key)
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Кому принадлежит логин, под который подбирают пароль: филиал и запись для журнала.
pub(crate) struct LoginTarget<'a> {
    pub branch_id: Uuid,
    pub entity: &'a str,
    pub entity_id: Uuid,
}

/// Неверный пароль к существующему логину: считаем неудачи со всех адресов. Набралось
/// `ALERT_FAILURES_ANY_ADDR` за окно — одно уведомление владельцу `auth.login_flood` (ADR-059),
/// следующее — не раньше, чем через окно. Вход при этом не закрывается.
/// Неизвестный логин (`target = None`) считается так же, чтобы время ответа не выдавало его
/// наличие, но сообщать о нём некому.
pub(crate) async fn note_failure(
    state: &AppState,
    login: &str,
    target: Option<LoginTarget<'_>>,
) -> AppResult<()> {
    let key = format!("{login}|*");
    let mut tx = state.pool.begin().await?;
    // `locked_until` в общей строке — «уведомили до»: в окне повторно не пишем.
    let row = sqlx::query!(
        r#"insert into login_attempts (login, failures, last_failed_at) values ($1, 1, now())
           on conflict (login) do update set
             failures = case when login_attempts.last_failed_at < now() - make_interval(mins => $2)
                             then 1 else login_attempts.failures + 1 end,
             last_failed_at = now()
           returning failures, coalesce(locked_until > now(), false) as "notified!""#,
        key,
        LOCK_MINUTES
    )
    .fetch_one(&mut *tx)
    .await?;
    if let Some(target) = target
        && row.failures >= ALERT_FAILURES_ANY_ADDR
        && !row.notified
    {
        sqlx::query!(
            r#"update login_attempts set failures = 0, locked_until = now() + make_interval(mins => $2)
               where login = $1"#,
            key,
            LOCK_MINUTES
        )
        .execute(&mut *tx)
        .await?;
        // Пользователя у события нет: это не действие сотрудника, а попытки входа извне.
        sqlx::query!(
            r#"insert into audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data)
               values ($1, $2, null, null, 'auth.login_flood', $3, $4, $5)"#,
            new_id(),
            target.branch_id,
            target.entity,
            target.entity_id,
            json!({ "login": login, "failures": row.failures, "minutes": LOCK_MINUTES })
        )
        .execute(&mut *tx)
        .await?;
        tracing::warn!(login = %login, "много неверных паролей с разных адресов");
    }
    tx.commit().await?;
    Ok(())
}

/// Учитывает попытку входа до проверки пароля, одной строкой под блокировкой: параллельные
/// запросы не проскочат лимит (ADR-016). Возвращает минуты до снятия блокировки, если вход закрыт.
/// Неизвестные логины учитываются так же, чтобы блокировка не выдавала их наличие.
/// Успешный вход удаляет строку, поэтому счётчик считает только неудачи.
/// Попытки во время блокировки строку не меняют: блокировку нельзя продлевать бесконечно,
/// через `LOCK_MINUTES` она снимается и счёт начинается заново (ADR-059).
pub(crate) async fn reserve_with_limit(
    state: &AppState,
    key: &str,
    max: i32,
) -> AppResult<Option<i64>> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query!(
        r#"insert into login_attempts (login, failures, last_failed_at) values ($1, 1, now())
           on conflict (login) do update set
             failures = case when login_attempts.locked_until > now() then login_attempts.failures
                             when login_attempts.last_failed_at < now() - make_interval(mins => $2)
                               or login_attempts.locked_until <= now()
                             then 1 else login_attempts.failures + 1 end,
             last_failed_at = case when login_attempts.locked_until > now()
                                   then login_attempts.last_failed_at else now() end,
             locked_until = case when login_attempts.locked_until > now()
                                 then login_attempts.locked_until end
           returning failures,
                     ceil(extract(epoch from locked_until - now()) / 60)::bigint as "left_min?""#,
        key,
        LOCK_MINUTES
    )
    .fetch_one(&mut *tx)
    .await?;
    let left = match row.left_min {
        Some(m) if m > 0 => Some(m),
        _ if row.failures > max => {
            sqlx::query!(
                r#"update login_attempts set failures = 0, locked_until = now() + make_interval(mins => $2)
                   where login = $1"#,
                key,
                LOCK_MINUTES
            )
            .execute(&mut *tx)
            .await?;
            tracing::warn!(login = %key, "вход заблокирован после неудачных попыток");
            Some(i64::from(LOCK_MINUTES))
        }
        _ => None,
    };
    tx.commit().await?;
    Ok(left)
}

async fn login(
    State(state): State<AppState>,
    parts: Parts,
    Json(req): Json<LoginReq>,
) -> AppResult<impl IntoResponse> {
    // Логин и пароль разумной длины и без нулевого символа: иначе это не человек у кассы.
    if req.login.len() > 100 || req.password.len() > 200 || req.login.contains('\0') {
        return Err(AppError::Unauthorized);
    }
    // Ключ блокировки — логин и адрес: подбор с чужого адреса закрывает вход только ему,
    // а владелец со своего устройства по-прежнему входит (ADR-016, ADR-049, ADR-059).
    let login = req.login.trim().to_lowercase();
    let addr = auth::client_addr(&parts);
    if let Some(minutes) = reserve_login(&state, &login, &addr).await? {
        return Err(AppError::TooManyRequests(format!(
            "слишком много неудачных попыток, повторите через {minutes} мин"
        )));
    }
    let row = sqlx::query!(
        "select id, branch_id, login, full_name, role, password_hash from users where lower(login) = $1 and active",
        login
    )
    .fetch_optional(&state.pool)
    .await?;
    // Проверка выполняется и для неизвестного логина, чтобы время ответа не выдавало его наличие.
    let hash = row.as_ref().map_or_else(
        || state.dummy_hash.as_ref().clone(),
        |r| r.password_hash.clone(),
    );
    let ok = verify_login_password(req.password, hash).await?;
    let row = match row {
        Some(r) if ok => r,
        r => {
            let target = r.map(|r| LoginTarget {
                branch_id: r.branch_id,
                entity: "user",
                entity_id: r.id,
            });
            note_failure(&state, &login, target).await?;
            return Err(AppError::Unauthorized);
        }
    };
    clear_login(&state, &login, &addr).await?;
    let user = CurrentUser {
        id: row.id,
        branch_id: row.branch_id,
        login: row.login,
        full_name: row.full_name,
        role: Role::parse(&row.role)?,
    };
    let device_id = auth::device_id(&parts);
    let (token, token_hash) = new_token();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into sessions (token_hash, user_id, device_id, expires_at) values ($1, $2, $3, now() + make_interval(days => $4))",
        token_hash,
        user.id,
        device_id,
        i32::try_from(SESSION_DAYS).unwrap_or(30)
    )
    .execute(&mut *tx)
    .await?;
    let ctx = Ctx {
        user: user.clone(),
        device_id: device_id.unwrap_or_else(Uuid::nil),
        bot_chat: None,
    };
    ops::audit(
        &mut tx,
        &ctx,
        "auth.login",
        "user",
        Some(user.id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    let cookie = session_cookie(&state, &token, SESSION_DAYS * 86_400);
    let mut conn = state.pool.acquire().await?;
    Ok((
        AppendHeaders([(SET_COOKIE, cookie)]),
        Json(me_out(&mut conn, user).await?),
    ))
}

async fn logout(State(state): State<AppState>, parts: Parts) -> AppResult<impl IntoResponse> {
    if let Some(token) = auth::session_token(&parts) {
        sqlx::query!(
            "delete from sessions where token_hash = $1",
            auth::token_hash(token)
        )
        .execute(&state.pool)
        .await?;
    }
    Ok((
        AppendHeaders([(SET_COOKIE, session_cookie(&state, "", 0))]),
        Json(json!({})),
    ))
}

/// Текущий пользователь, его права сверх роли (ADR-082) и филиалы (SPEC-33): `branch_id` — филиал
/// запроса, `home_branch_id` — свой филиал пользователя, `branches` — что он видит (владелец —
/// все, администратор — свой).
#[derive(Serialize)]
struct MeOut {
    #[serde(flatten)]
    user: CurrentUser,
    permissions: Vec<String>,
    home_branch_id: Uuid,
    branch_name: String,
    branches: Vec<crate::api::branches::BranchOut>,
    /// Филиалов больше одного: экраны показывают название филиала.
    several_branches: bool,
}

async fn me_out(conn: &mut sqlx::PgConnection, user: CurrentUser) -> AppResult<MeOut> {
    let permissions = auth::permissions(&mut *conn, user.id).await?;
    let home_branch_id = sqlx::query_scalar!("select branch_id from users where id = $1", user.id)
        .fetch_one(&mut *conn)
        .await?;
    let branches = crate::api::branches::visible(&mut *conn, &user).await?;
    let several_branches = crate::api::branches::several(&mut *conn).await?;
    let branch_name = branches
        .iter()
        .find(|b| b.id == user.branch_id)
        .map(|b| b.name.clone())
        .unwrap_or_default();
    Ok(MeOut {
        user,
        permissions,
        home_branch_id,
        branch_name,
        branches,
        several_branches,
    })
}

async fn me(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<MeOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(me_out(&mut conn, user).await?))
}

#[derive(Serialize, Deserialize)]
struct UserOut {
    id: Uuid,
    login: String,
    full_name: String,
    role: String,
    active: bool,
    /// Права администратора, выданные владельцем (ADR-082, ADR-097).
    permissions: Vec<String>,
}

async fn list_users(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<Vec<UserOut>>> {
    let rows = sqlx::query_as!(
        UserOut,
        r#"select u.id, u.login, u.full_name, u.role, u.active,
                  array(select p.permission from user_permissions p
                        where p.user_id = u.id order by p.permission) as "permissions!"
           from users u where u.branch_id = $1 or u.role = 'owner'
           order by u.role = 'owner' desc, u.full_name"#,
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct CreateUserReq {
    login: String,
    password: String,
    full_name: String,
    role: String,
}

fn check_password(p: &str) -> AppResult<()> {
    if p.chars().count() < 8 {
        Err(invalid("пароль не короче 8 символов"))
    } else {
        Ok(())
    }
}

async fn create_user(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateUserReq>,
) -> AppResult<Json<UserOut>> {
    ctx.user.require_owner()?;
    let login = req.login.trim().to_string();
    let full_name = req.full_name.trim().to_string();
    if login.is_empty() || full_name.is_empty() {
        return Err(invalid("логин и имя обязательны"));
    }
    check_password(&req.password)?;
    let role = Role::parse(&req.role)?;
    let hash = hash_password(req.password).await?;
    let id = new_id();
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, $5, $6)",
        id,
        ctx.user.branch_id,
        login,
        hash,
        role.as_str(),
        full_name
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| if is_unique_violation(&e) { AppError::Conflict("логин занят".into()) } else { e.into() })?;
    // Новому администратору — права по умолчанию, дальше их правит владелец (SPEC-30 §5).
    if role == Role::Admin {
        auth::grant_default_permissions(&mut tx, id, ctx.user.branch_id, ctx.user.id).await?;
    }
    let permissions = auth::permissions(&mut tx, id).await?;
    ops::audit(
        &mut tx,
        &ctx,
        "user.create",
        "user",
        Some(id),
        json!({ "login": login, "role": role.as_str(), "permissions": permissions }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(UserOut {
        id,
        login,
        full_name,
        role: role.as_str().into(),
        active: true,
        permissions,
    }))
}

#[derive(Deserialize)]
struct UpdateUserReq {
    full_name: Option<String>,
    role: Option<String>,
    active: Option<bool>,
    password: Option<String>,
    // Права меняются отдельной операцией `PUT /users/{id}/permissions` (ADR-097): поле
    // `permissions` от старого экрана не принимается, чтобы он не снял права, которых не знает.
}

async fn update_user(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateUserReq>,
) -> AppResult<Json<UserOut>> {
    ctx.user.require_owner()?;
    let role = req.role.as_deref().map(Role::parse).transpose()?;
    let hash = match req.password {
        Some(p) => {
            check_password(&p)?;
            Some(hash_password(p).await?)
        }
        None => None,
    };
    let full_name = req.full_name.map(|s| s.trim().to_string());
    if full_name.as_deref() == Some("") {
        return Err(invalid("имя обязательно"));
    }
    let mut tx = state.pool.begin().await?;
    // Блокируем владельцев, чтобы проверка «последнего владельца» была надёжной. Владелец —
    // всех филиалов (SPEC-33): его правят из любого филиала, администратора — из его филиала.
    let owners =
        sqlx::query_scalar!("select id from users where role = 'owner' and active for update")
            .fetch_all(&mut *tx)
            .await?;
    let before_role = sqlx::query_scalar!(
        "select role from users where id = $1 and (branch_id = $2 or role = 'owner') for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let row = sqlx::query!(
        r#"update users set
             full_name = coalesce($3, full_name),
             role = coalesce($4, role),
             active = coalesce($5, active),
             password_hash = coalesce($6, password_hash)
           where id = $1 and (branch_id = $2 or role = 'owner')
           returning id, login, full_name, role, active, branch_id"#,
        id,
        ctx.user.branch_id,
        full_name,
        role.map(Role::as_str),
        req.active,
        hash
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Владелец стал администратором — как новому администратору, права по умолчанию (SPEC-30 §5).
    if before_role == "owner" && row.role == "admin" {
        let before = auth::permissions(&mut tx, id).await?;
        auth::grant_default_permissions(&mut tx, id, row.branch_id, ctx.user.id).await?;
        let after = auth::permissions(&mut tx, id).await?;
        audit_permissions(
            &mut tx,
            &ctx,
            id,
            &row.login,
            &row.full_name,
            &before,
            &after,
        )
        .await?;
    }
    let out = UserOut {
        id: row.id,
        login: row.login,
        full_name: row.full_name,
        role: row.role,
        active: row.active,
        permissions: auth::permissions(&mut tx, id).await?,
    };
    let still_owner = out.active && out.role == "owner";
    if owners.len() == 1 && owners.contains(&id) && !still_owner {
        return Err(AppError::Conflict(
            "нельзя отключить последнего владельца".into(),
        ));
    }
    if hash.is_some() || out.active.eq(&false) {
        sqlx::query!("delete from sessions where user_id = $1", id)
            .execute(&mut *tx)
            .await?;
    }
    // Отключённый или больше не владелец — его чат в Телеграме отвязывается (ADR-063).
    if !still_owner {
        crate::api::telegram::drop_owner_link(&mut tx, &ctx, id).await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        "user.update",
        "user",
        Some(id),
        json!({
            "login": out.login,
            "role": out.role,
            "active": out.active,
            "password_changed": hash.is_some(),
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct PermissionsReq {
    op_id: Uuid,
    /// Права администратора целиком: что не в списке — снимается (ADR-097).
    permissions: Vec<String>,
}

/// Журнал изменения прав `user.permissions` (SPEC-30 §5): кто (пользователь записи), кому, что
/// включено и что выключено. Ничего не поменялось — записи нет.
async fn audit_permissions(
    conn: &mut sqlx::PgConnection,
    ctx: &Ctx,
    user_id: Uuid,
    login: &str,
    full_name: &str,
    before: &[String],
    after: &[String],
) -> AppResult<()> {
    let granted: Vec<&String> = after.iter().filter(|p| !before.contains(p)).collect();
    let revoked: Vec<&String> = before.iter().filter(|p| !after.contains(p)).collect();
    if granted.is_empty() && revoked.is_empty() {
        return Ok(());
    }
    ops::audit(
        conn,
        ctx,
        "user.permissions",
        "user",
        Some(user_id),
        json!({
            "login": login,
            "full_name": full_name,
            "granted": granted,
            "revoked": revoked,
            "permissions": after,
        }),
    )
    .await
}

/// «Что можно» администратору (SPEC-30 §5, ADR-097): владелец выдаёт и снимает права списком
/// целиком. Одна операция с `op_id`: повтор возвращает тот же ответ.
async fn put_permissions(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PermissionsReq>,
) -> AppResult<Json<UserOut>> {
    const KIND: &str = "user.permissions";
    ctx.user.require_owner()?;
    if let Some(bad) = req
        .permissions
        .iter()
        .find(|p| auth::permission_def(p).is_none())
    {
        return Err(invalid(format!("неизвестное право «{bad}»")));
    }
    let mut list = req.permissions;
    list.sort();
    list.dedup();
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let row = sqlx::query!(
        "select login, full_name, role, active from users where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if row.role != Role::Admin.as_str() {
        return Err(invalid("владельцу права не нужны: он может всё"));
    }
    let before = auth::permissions(&mut tx, id).await?;
    sqlx::query!(
        "delete from user_permissions where user_id = $1 and not (permission = any($2))",
        id,
        &list
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into user_permissions (user_id, branch_id, permission, granted_by)
           select $1, $2, p, $4 from unnest($3::text[]) as p
           on conflict do nothing"#,
        id,
        ctx.user.branch_id,
        &list,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    let after = auth::permissions(&mut tx, id).await?;
    audit_permissions(
        &mut tx,
        &ctx,
        id,
        &row.login,
        &row.full_name,
        &before,
        &after,
    )
    .await?;
    let out = UserOut {
        id,
        login: row.login,
        full_name: row.full_name,
        role: row.role,
        active: row.active,
        permissions: after,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
