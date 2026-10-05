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
    verify_password,
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
const MAX_FAILURES: i32 = 5;
const LOCK_MINUTES: i32 = 15;

/// Сколько минут осталось до снятия блокировки логина, если он заблокирован.
async fn locked_minutes(state: &AppState, key: &str) -> AppResult<Option<i64>> {
    let left = sqlx::query_scalar!(
        r#"select ceil(extract(epoch from locked_until - now()) / 60)::bigint as "m!"
           from login_attempts where login = $1 and locked_until > now()"#,
        key
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(left.map(|m| m.max(1)))
}

/// Учитывает неудачную попытку; счётчик сбрасывается через LOCK_MINUTES без ошибок.
/// Неизвестные логины учитываются так же, чтобы блокировка не выдавала их наличие.
async fn register_failure(state: &AppState, key: &str) -> AppResult<()> {
    sqlx::query!(
        r#"insert into login_attempts (login, failures, last_failed_at) values ($1, 1, now())
           on conflict (login) do update set
             failures = case when login_attempts.last_failed_at < now() - make_interval(mins => $2)
                             then 1 else login_attempts.failures + 1 end,
             last_failed_at = now()"#,
        key,
        LOCK_MINUTES
    )
    .execute(&state.pool)
    .await?;
    let locked = sqlx::query!(
        r#"update login_attempts set failures = 0, locked_until = now() + make_interval(mins => $3)
           where login = $1 and failures >= $2"#,
        key,
        MAX_FAILURES,
        LOCK_MINUTES
    )
    .execute(&state.pool)
    .await?
    .rows_affected();
    if locked > 0 {
        tracing::warn!(login = %key, "вход заблокирован после неудачных попыток");
    }
    Ok(())
}

async fn login(
    State(state): State<AppState>,
    parts: Parts,
    Json(req): Json<LoginReq>,
) -> AppResult<impl IntoResponse> {
    let key = req.login.trim().to_lowercase();
    if let Some(minutes) = locked_minutes(&state, &key).await? {
        return Err(AppError::TooManyRequests(format!(
            "слишком много неудачных попыток, повторите через {minutes} мин"
        )));
    }
    let row = sqlx::query!(
        "select id, branch_id, login, full_name, role, password_hash from users where lower(login) = $1 and active",
        key
    )
    .fetch_optional(&state.pool)
    .await?;
    // Проверка выполняется и для неизвестного логина, чтобы время ответа не выдавало его наличие.
    let hash = row.as_ref().map_or_else(
        || state.dummy_hash.as_ref().clone(),
        |r| r.password_hash.clone(),
    );
    let ok = verify_password(req.password, hash).await;
    let row = match row {
        Some(r) if ok => r,
        _ => {
            register_failure(&state, &key).await?;
            return Err(AppError::Unauthorized);
        }
    };
    sqlx::query!("delete from login_attempts where login = $1", key)
        .execute(&state.pool)
        .await?;
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
    Ok((AppendHeaders([(SET_COOKIE, cookie)]), Json(user)))
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

async fn me(user: CurrentUser) -> Json<CurrentUser> {
    Json(user)
}

#[derive(Serialize)]
struct UserOut {
    id: Uuid,
    login: String,
    full_name: String,
    role: String,
    active: bool,
}

async fn list_users(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<Vec<UserOut>>> {
    let rows = sqlx::query_as!(
        UserOut,
        "select id, login, full_name, role, active from users where branch_id = $1 order by full_name",
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
    ops::audit(
        &mut tx,
        &ctx,
        "user.create",
        "user",
        Some(id),
        json!({ "login": login, "role": role.as_str() }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(UserOut {
        id,
        login,
        full_name,
        role: role.as_str().into(),
        active: true,
    }))
}

#[derive(Deserialize)]
struct UpdateUserReq {
    full_name: Option<String>,
    role: Option<String>,
    active: Option<bool>,
    password: Option<String>,
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
    // Блокируем владельцев филиала, чтобы проверка «последнего владельца» была надёжной.
    let owners = sqlx::query_scalar!(
        "select id from users where branch_id = $1 and role = 'owner' and active for update",
        ctx.user.branch_id
    )
    .fetch_all(&mut *tx)
    .await?;
    let out = sqlx::query_as!(
        UserOut,
        r#"update users set
             full_name = coalesce($3, full_name),
             role = coalesce($4, role),
             active = coalesce($5, active),
             password_hash = coalesce($6, password_hash)
           where id = $1 and branch_id = $2
           returning id, login, full_name, role, active"#,
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
    ops::audit(
        &mut tx,
        &ctx,
        "user.update",
        "user",
        Some(id),
        json!({ "role": out.role, "active": out.active, "password_changed": hash.is_some() }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}
