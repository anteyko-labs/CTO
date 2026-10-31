//! Права кассира без входа (задача 003 Н10, ADR-{B8.1}): карточка кассира → «Что можно на кассе».
//! Владелец снимает у кассира права групп «Касса» и «Деньги»; пока открыта смена этого кассира,
//! у вошедшего администратора действует пересечение прав входа и кассира (`auth::access`).
//! По умолчанию — «как у входа» (ничего не снято).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser, PERMISSIONS, cashier_limitable};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/employees/{id}/permissions",
        routing::get(get_rights).put(put_rights),
    )
}

#[derive(Serialize, Deserialize)]
struct RightsOut {
    employee_id: Uuid,
    full_name: String,
    /// Права, которые можно снять у кассира: ключи групп «Касса» и «Деньги».
    keys: Vec<String>,
    /// Снятые владельцем; пусто — «как у входа».
    denied: Vec<String>,
}

fn limitable_keys() -> Vec<String> {
    PERMISSIONS
        .iter()
        .filter(|p| cashier_limitable(p.key))
        .map(|p| p.key.to_string())
        .collect()
}

/// Снятые права в порядке списка прав, без повторов; чужой ключ — 422.
fn normalize(denied: &[String]) -> AppResult<Vec<String>> {
    if let Some(bad) = denied.iter().find(|k| !cashier_limitable(k)) {
        return Err(invalid(format!(
            "у кассира снимаются только права кассы и денег, а не «{bad}»"
        )));
    }
    Ok(PERMISSIONS
        .iter()
        .filter(|p| denied.iter().any(|k| k == p.key))
        .map(|p| p.key.to_string())
        .collect())
}

/// Права кассира — только владелец (их задаёт он, администратору их знать не нужно).
async fn get_rights(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<RightsOut>> {
    user.require_owner()?;
    let r = sqlx::query!(
        "select full_name, cashier_denied from employees where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(RightsOut {
        employee_id: id,
        full_name: r.full_name,
        keys: limitable_keys(),
        denied: r.cashier_denied,
    }))
}

#[derive(Deserialize)]
struct RightsReq {
    op_id: Uuid,
    /// Что кассиру нельзя на кассе: ключи прав групп «Касса» и «Деньги».
    denied: Vec<String>,
}

/// Владелец задаёт, что можно кассиру на кассе. Одна транзакция с журналом
/// `employee.permissions` (что снято и что возвращено, было → стало, I5); повтор `op_id` —
/// тот же ответ (I8).
async fn put_rights(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<RightsReq>,
) -> AppResult<Json<RightsOut>> {
    const KIND: &str = "employee.permissions";
    ctx.user.require_owner()?;
    let denied = normalize(&req.denied)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let prev = sqlx::query!(
        r#"select full_name, is_cashier, cashier_denied from employees
           where id = $1 and branch_id = $2 for update"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if !prev.is_cashier && !denied.is_empty() {
        return Err(invalid("права на кассе задаются только кассиру"));
    }
    sqlx::query!(
        "update employees set cashier_denied = $3 where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id,
        &denied
    )
    .execute(&mut *tx)
    .await?;
    let off: Vec<&String> = denied
        .iter()
        .filter(|k| !prev.cashier_denied.contains(k))
        .collect();
    let on: Vec<&String> = prev
        .cashier_denied
        .iter()
        .filter(|k| !denied.contains(k))
        .collect();
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "employee",
        Some(id),
        json!({
            "full_name": prev.full_name,
            "off": off,
            "on": on,
            "before": prev.cashier_denied,
            "after": denied,
        }),
    )
    .await?;
    let out = RightsOut {
        employee_id: id,
        full_name: prev.full_name,
        keys: limitable_keys(),
        denied,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_till_and_money_rights_are_limitable() {
        let keys = limitable_keys();
        assert!(keys.contains(&"discount".to_string()));
        assert!(keys.contains(&"expenses".to_string()));
        assert!(!keys.contains(&"receipts".to_string()));
        assert!(normalize(&["receipts".into()]).is_err());
        // Порядок — как в списке прав, повторы убираются.
        assert_eq!(
            normalize(&["expenses".into(), "discount".into(), "discount".into()]).ok(),
            Some(vec!["discount".to_string(), "expenses".to_string()])
        );
    }
}
