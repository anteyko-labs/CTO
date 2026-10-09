//! Единый тип ошибок и его превращение в HTTP-ответ `{error: {code, message}}`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("требуется вход")]
    Unauthorized,
    #[error("недостаточно прав")]
    Forbidden,
    #[error("не найдено")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("{0}")]
    Internal(String),
}

pub type AppResult<T> = Result<T, AppError>;

pub fn invalid(msg: impl Into<String>) -> AppError {
    AppError::Validation(msg.into())
}

pub fn overflow() -> AppError {
    AppError::Validation("слишком большое значение".into())
}

impl AppError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            Self::Db(e) if bad_text(e) => (StatusCode::UNPROCESSABLE_ENTITY, "validation"),
            Self::Db(_) | Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = match &self {
            Self::Db(e) if bad_text(e) => "в тексте недопустимые символы".to_string(),
            Self::Db(e) => {
                tracing::error!(error = %e, "ошибка базы данных");
                "внутренняя ошибка".to_string()
            }
            Self::Internal(e) => {
                tracing::error!(error = %e, "внутренняя ошибка");
                "внутренняя ошибка".to_string()
            }
            other => other.to_string(),
        };
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

/// База не принимает такой текст: нулевой символ или неверная кодировка (22021, 22P05).
fn bad_text(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if matches!(d.code().as_deref(), Some("22021" | "22P05")))
}

/// Нарушение уникальности в базе (код 23505).
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}
