//! Фото накладной: снимки и PDF у прихода (SPEC-29 §3, ADR-080).
//!
//! Файл лежит в базе (`receipt_files.data`), поэтому копия базы уносит его вместе с приходом.
//! Загружают владелец и администратор — и при проведении, и позже; файл не правится и не
//! удаляется, владелец может только скрыть его с причиной (журнал).

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// Предел одного файла: 5 МБ. Снимок с телефона экран сжимает до загрузки.
pub const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
/// Сколько файлов у одной накладной — с запасом на многостраничную.
const MAX_FILES: i64 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/receipts/{id}/files",
            routing::get(list)
                .post(upload)
                .layer(axum::extract::DefaultBodyLimit::max(
                    MAX_FILE_BYTES + 64 * 1024,
                )),
        )
        .route("/receipt-files/{id}", routing::get(download))
        .route("/receipt-files/{id}/hide", routing::post(hide))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileOut {
    pub id: Uuid,
    pub receipt_id: Uuid,
    pub name: String,
    pub content_type: String,
    pub size_bytes: i32,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    /// Скрыт владельцем: когда, кем и почему. Администратору скрытые не показываются.
    pub hidden_at: Option<DateTime<Utc>>,
    pub hidden_by: Option<String>,
    pub hidden_reason: Option<String>,
}

/// Вид файла по первым байтам: присланный заголовок `Content-Type` должен с ним совпасть.
pub fn sniff(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else {
        None
    }
}

async fn files_of(
    conn: &mut PgConnection,
    user: &CurrentUser,
    receipt_id: Uuid,
) -> AppResult<Vec<FileOut>> {
    let rows = sqlx::query_as!(
        FileOut,
        r#"select f.id, f.receipt_id, f.name, f.content_type, f.size_bytes, u.full_name as user_name,
                  f.created_at, f.hidden_at, h.full_name as "hidden_by?", f.hidden_reason
           from receipt_files f
           join users u on u.id = f.user_id
           left join users h on h.id = f.hidden_by
           where f.receipt_id = $1 and f.branch_id = $2 and ($3 or f.hidden_at is null)
           order by f.created_at, f.id"#,
        receipt_id,
        user.branch_id,
        user.is_owner()
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<FileOut>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(files_of(&mut conn, &user, id).await?))
}

#[derive(Deserialize)]
pub struct UploadQuery {
    pub op_id: Uuid,
    #[serde(default)]
    pub name: String,
}

async fn upload(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<FileOut>> {
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
    let out = upload_tx(&mut tx, &ctx, id, q, &declared, &body).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Загрузка файла к приходу — одна операция с `op_id` (повтор не заводит второй файл).
pub async fn upload_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    receipt_id: Uuid,
    q: UploadQuery,
    declared: &str,
    data: &[u8],
) -> AppResult<FileOut> {
    const KIND: &str = "receipt.file_add";
    // Фото накладной — часть прихода: право администратора `receipts` (ADR-097).
    crate::auth::require_permission(&mut *conn, &ctx.user, "receipts").await?;
    if let Some(done) = ops::begin_op(conn, ctx, q.op_id, KIND).await? {
        return Ok(done);
    }
    if data.is_empty() {
        return Err(invalid("файл пустой"));
    }
    if data.len() > MAX_FILE_BYTES {
        return Err(invalid(
            "файл больше 5 МБ — сфотографируйте заново или сожмите",
        ));
    }
    let kind = sniff(data)
        .ok_or_else(|| invalid("фото накладной — снимок JPEG, PNG, WEBP или файл PDF"))?;
    if !declared.is_empty() && declared != kind && declared != "application/octet-stream" {
        return Err(invalid("тип файла не совпадает с содержимым"));
    }
    let branch_id = ctx.user.branch_id;
    let r = sqlx::query!(
        "select number, reversal_of from receipts where id = $1 and branch_id = $2 for update",
        receipt_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if r.reversal_of.is_some() {
        return Err(invalid(
            "фото прикрепляют к самой накладной, не к её сторно",
        ));
    }
    let count = sqlx::query_scalar!(
        r#"select count(*) as "n!" from receipt_files where receipt_id = $1 and hidden_at is null"#,
        receipt_id
    )
    .fetch_one(&mut *conn)
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
    let size = i32::try_from(data.len()).map_err(|_| invalid("файл слишком большой"))?;
    let fid = new_id();
    sqlx::query!(
        r#"insert into receipt_files (id, branch_id, receipt_id, op_id, name, content_type, size_bytes,
                                      data, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        fid,
        branch_id,
        receipt_id,
        q.op_id,
        name,
        kind,
        size,
        data,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    ops::audit(
        conn,
        ctx,
        KIND,
        "receipt",
        Some(receipt_id),
        json!({ "number": r.number, "file": fid, "name": name, "size": size, "type": kind }),
    )
    .await?;
    let out = files_of(conn, &ctx.user, receipt_id)
        .await?
        .into_iter()
        .find(|f| f.id == fid)
        .ok_or_else(|| AppError::Internal("файл не найден после записи".into()))?;
    ops::finish_op(conn, ctx, q.op_id, KIND, &out).await?;
    Ok(out)
}

async fn download(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Response> {
    let f = sqlx::query!(
        "select content_type, data, hidden_at from receipt_files where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    if f.hidden_at.is_some() && !user.is_owner() {
        return Err(AppError::NotFound);
    }
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
    // Скрытый владельцем файл не должен остаться в кэше браузера администратора: каждый показ
    // спрашивает сервер.
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
pub struct HideReq {
    pub reason: String,
}

/// Скрыть файл — только владелец, с причиной; файл остаётся в базе и в журнале (ADR-080).
async fn hide(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<HideReq>,
) -> AppResult<Json<FileOut>> {
    ctx.user.require_owner()?;
    let reason = req.reason.trim();
    if reason.is_empty() {
        return Err(invalid("укажите, почему скрываете файл"));
    }
    let mut tx = state.pool.begin().await?;
    let f = sqlx::query!(
        r#"select f.receipt_id, f.name, f.hidden_at, r.number
           from receipt_files f join receipts r on r.id = f.receipt_id
           where f.id = $1 and f.branch_id = $2 for update of f"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Повтор после потерянного ответа — тот же результат, а не ошибка (инвариант 4).
    if f.hidden_at.is_some() {
        let out = files_of(&mut tx, &ctx.user, f.receipt_id)
            .await?
            .into_iter()
            .find(|x| x.id == id)
            .ok_or(AppError::NotFound)?;
        tx.commit().await?;
        return Ok(Json(out));
    }
    sqlx::query!(
        "update receipt_files set hidden_at = now(), hidden_by = $2, hidden_reason = $3 where id = $1",
        id,
        ctx.user.id,
        reason
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "receipt.file_hide",
        "receipt",
        Some(f.receipt_id),
        json!({ "number": f.number, "file": id, "name": f.name, "reason": reason }),
    )
    .await?;
    let out = files_of(&mut tx, &ctx.user, f.receipt_id)
        .await?
        .into_iter()
        .find(|x| x.id == id)
        .ok_or(AppError::NotFound)?;
    tx.commit().await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::sniff;

    #[test]
    fn file_kind_by_first_bytes() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0]), Some("image/jpeg"));
        assert_eq!(
            sniff(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0]),
            Some("image/png")
        );
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"%PDF-1.7\n"), Some("application/pdf"));
        assert_eq!(sniff(b"<html>"), None);
        assert_eq!(sniff(b""), None);
    }
}
