//! Первичная настройка пустой базы (SPEC-01).

use sqlx::PgPool;

use crate::auth::hash_password;
use crate::error::AppResult;
use crate::ops::new_id;

/// Если пользователей нет — создаёт филиал «Основной» и владельца.
pub async fn ensure_owner(pool: &PgPool, owner: Option<(String, String)>) -> AppResult<()> {
    let users = sqlx::query_scalar!(r#"select count(*) as "n!" from users"#)
        .fetch_one(pool)
        .await?;
    if users > 0 {
        return Ok(());
    }
    let Some((login, password)) = owner else {
        tracing::warn!(
            "пользователей нет: задайте BOOTSTRAP_OWNER_LOGIN и BOOTSTRAP_OWNER_PASSWORD"
        );
        return Ok(());
    };
    let hash = hash_password(password).await?;
    let mut tx = pool.begin().await?;
    let branch_id = match sqlx::query_scalar!("select id from branches order by created_at limit 1")
        .fetch_optional(&mut *tx)
        .await?
    {
        Some(id) => id,
        None => {
            let id = new_id();
            sqlx::query!(
                "insert into branches (id, name) values ($1, 'Основной')",
                id
            )
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    sqlx::query!(
        "insert into users (id, branch_id, login, password_hash, role, full_name) values ($1, $2, $3, $4, 'owner', 'Владелец')",
        new_id(),
        branch_id,
        login,
        hash
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    tracing::info!(login = %login, "создан владелец");
    Ok(())
}
