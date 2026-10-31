//! Аналоги фильтров по кросс-номерам (SPEC-17).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/products/{id}/cross",
            routing::get(get_cross).put(put_cross),
        )
        .route("/products/{id}/analogs", routing::get(analogs))
}

/// Номер для сравнения: только буквы и цифры, верхний регистр (как `norm_code` в базе).
pub fn norm(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, 'А'..='я' | 'Ё' | 'ё'))
        .flat_map(char::to_uppercase)
        .collect()
}

#[derive(Serialize)]
pub struct CrossOut {
    pub codes: Vec<String>,
    /// Номера, подобранные сервисом разбора (задача 002 П10): «подобрано автоматически».
    pub auto: Vec<String>,
}

/// Кросс-номера товара по порядку и подобранные сервисом.
pub async fn load_cross(conn: &mut sqlx::PgConnection, id: Uuid) -> AppResult<CrossOut> {
    let rows = sqlx::query!(
        "select code, auto from product_cross_numbers where product_id = $1 order by code",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(CrossOut {
        auto: rows
            .iter()
            .filter(|r| r.auto)
            .map(|r| r.code.clone())
            .collect(),
        codes: rows.into_iter().map(|r| r.code).collect(),
    })
}

async fn get_cross(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CrossOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load_cross(&mut conn, id).await?))
}

#[derive(Deserialize)]
struct CrossReq {
    codes: Vec<String>,
    /// Номера из подбора сервисом, которые человек сохраняет (режим «предлагать»): пометка
    /// «подобрано автоматически» в карточке и журнале. Уже подобранные сервисом номера пометку
    /// сохраняют.
    #[serde(default)]
    auto: Vec<String>,
}

/// Список кросс-номеров заменяется целиком: убранный номер больше не связывает товары.
async fn put_cross(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CrossReq>,
) -> AppResult<Json<CrossOut>> {
    if req.codes.len() > 100 {
        return Err(invalid("не больше 100 кросс-номеров"));
    }
    let mut seen = std::collections::BTreeMap::new();
    for c in &req.codes {
        let code = c.trim();
        if code.is_empty() {
            continue;
        }
        if code.chars().count() > 40 {
            return Err(invalid("кросс-номер не длиннее 40 знаков"));
        }
        let n = norm(code);
        if n.is_empty() {
            return Err(invalid(format!("в номере «{code}» нет ни букв, ни цифр")));
        }
        seen.entry(n).or_insert_with(|| code.to_string());
    }
    let mut tx = state.pool.begin().await?;
    // Кросс-номера — часть карточки товара: право администратора `products` (ADR-097).
    crate::auth::require_permission(&mut tx, &ctx.user, "products").await?;
    sqlx::query_scalar!("select id from products where id = $1 for update", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let was_auto: Vec<String> = sqlx::query_scalar!(
        "select code_norm from product_cross_numbers where product_id = $1 and auto",
        id
    )
    .fetch_all(&mut *tx)
    .await?;
    let picked: Vec<String> = req.auto.iter().map(|c| norm(c)).collect();
    sqlx::query!(
        "delete from product_cross_numbers where product_id = $1",
        id
    )
    .execute(&mut *tx)
    .await?;
    let mut added_auto: Vec<String> = Vec::new();
    for (n, code) in &seen {
        let auto = was_auto.contains(n) || picked.contains(n);
        if picked.contains(n) && !was_auto.contains(n) {
            added_auto.push(code.clone());
        }
        sqlx::query!(
            "insert into product_cross_numbers (product_id, code, code_norm, auto) values ($1, $2, $3, $4)",
            id,
            code,
            n,
            auto
        )
        .execute(&mut *tx)
        .await?;
    }
    let codes: Vec<String> = seen.into_values().collect();
    let mut data = json!({ "codes": codes });
    if !added_auto.is_empty() {
        // Сохранено из подбора сервисом (режим «предлагать»): пометка в журнале.
        data["auto"] = json!(added_auto);
        data["note"] = json!("подобрано автоматически");
    }
    ops::audit(&mut tx, &ctx, "product.cross", "product", Some(id), data).await?;
    let out = load_cross(&mut tx, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct AnalogOut {
    id: Uuid,
    name: String,
    brand: String,
    article: String,
    stock_qty: i64,
    sale_price_tyiyn: i64,
}

/// Аналоги: совпадает хотя бы один номер — кросс-номер с кросс-номером или артикулом.
/// Только цена продажи и остаток: себестоимость здесь не нужна никому (инвариант 13).
async fn analogs(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Vec<AnalogOut>>> {
    let rows = sqlx::query_as!(
        AnalogOut,
        r#"with mine as (
             select code_norm as n from product_cross_numbers where product_id = $1
             union select norm_code(article) from products where id = $1 and norm_code(article) <> ''
           )
           select p.id, p.name, p.brand, p.article,
                  coalesce(bp.stock_qty, 0) as "stock_qty!",
                  coalesce(bp.sale_price_tyiyn, 0) as "sale_price_tyiyn!"
           from products p
           left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
           where p.id <> $1 and not p.archived
             and (norm_code(p.article) in (select n from mine)
                  or exists (select 1 from product_cross_numbers x
                             where x.product_id = p.id and x.code_norm in (select n from mine)))
           order by (coalesce(bp.stock_qty, 0) > 0) desc, p.name
           limit 30"#,
        id,
        user.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use super::norm;

    #[test]
    fn same_number_in_any_spelling() {
        assert_eq!(norm("90915-YZZE1"), "90915YZZE1");
        assert_eq!(norm(" 90915 yzze1 "), "90915YZZE1");
        assert_eq!(norm("W 712/75"), "W71275");
    }
}
