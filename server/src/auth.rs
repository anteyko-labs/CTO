//! Пароли, сессии и извлечение текущего пользователя (SPEC-01).

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "avtodom_session";
pub const SESSION_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            _ => Err(invalid("неизвестная роль")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentUser {
    pub id: Uuid,
    pub branch_id: Uuid,
    pub login: String,
    pub full_name: String,
    pub role: Role,
}

impl CurrentUser {
    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }

    pub fn require_owner(&self) -> AppResult<()> {
        if self.is_owner() {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// Пользователь и устройство изменяющего запроса.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub user: CurrentUser,
    pub device_id: Uuid,
}

pub async fn hash_password(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

pub fn new_token() -> (String, Vec<u8>) {
    let raw: [u8; 32] = rand::random();
    let token = hex::encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

pub fn session_token(parts: &Parts) -> Option<&str> {
    cookie_value(parts, SESSION_COOKIE)
}

pub fn device_id(parts: &Parts) -> Option<Uuid> {
    parts
        .headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_token(parts).ok_or(AppError::Unauthorized)?;
        let hash = token_hash(token);
        let row = sqlx::query!(
            r#"select u.id, u.branch_id, u.login, u.full_name, u.role
               from sessions s join users u on u.id = s.user_id
               where s.token_hash = $1 and s.expires_at > now() and u.active"#,
            hash
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;
        Ok(Self {
            id: row.id,
            branch_id: row.branch_id,
            login: row.login,
            full_name: row.full_name,
            role: Role::parse(&row.role)?,
        })
    }
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        let device_id = device_id(parts).ok_or_else(|| invalid("нужен заголовок X-Device-Id"))?;
        Ok(Self { user, device_id })
    }
}

/// Владелец, иначе 403.
pub struct Owner(pub CurrentUser);

impl FromRequestParts<AppState> for Owner {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        user.require_owner()?;
        Ok(Self(user))
    }
}
