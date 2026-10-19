//! Справочник марок и моделей машин (SPEC-29 §21, ADR-083): общий для всех филиалов, как
//! категории товаров. Пополняется сам: марку или модель, которой нет, вписывает владелец или
//! администратор при добавлении машины в программе. Бот клиента справочник не пополняет.

use axum::extract::State;
use axum::{Json, Router, routing};
use serde::Serialize;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::CurrentUser;
use crate::error::{AppResult, invalid};
use crate::ops::new_id;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/car-makes", routing::get(list))
}

#[derive(Serialize)]
pub struct CarModel {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub struct CarMake {
    pub id: Uuid,
    pub name: String,
    pub models: Vec<CarModel>,
}

/// Марки с моделями по алфавиту.
async fn list(State(state): State<AppState>, _user: CurrentUser) -> AppResult<Json<Vec<CarMake>>> {
    let mut conn = state.pool.acquire().await?;
    let makes = sqlx::query!("select id, name from car_makes order by lower(name)")
        .fetch_all(&mut *conn)
        .await?;
    let models = sqlx::query!("select id, make_id, name from car_models order by lower(name)")
        .fetch_all(&mut *conn)
        .await?;
    Ok(Json(
        makes
            .into_iter()
            .map(|m| CarMake {
                models: models
                    .iter()
                    .filter(|x| x.make_id == m.id)
                    .map(|x| CarModel {
                        id: x.id,
                        name: x.name.clone(),
                    })
                    .collect(),
                id: m.id,
                name: m.name,
            })
            .collect(),
    ))
}

/// Название марки или модели из справочника, как оно там записано («toyota» → «Toyota»);
/// новой нет — добавляет. Пустое — пусто. Вызывается в транзакции добавления машины.
pub async fn remember(
    conn: &mut PgConnection,
    user: &CurrentUser,
    brand: &str,
    model: &str,
) -> AppResult<(String, String)> {
    let brand = brand.split_whitespace().collect::<Vec<_>>().join(" ");
    let model = model.split_whitespace().collect::<Vec<_>>().join(" ");
    if brand.chars().count() > 60 || model.chars().count() > 60 {
        return Err(invalid("марка и модель — не длиннее 60 знаков"));
    }
    if brand.is_empty() {
        return Ok((String::new(), model));
    }
    sqlx::query!(
        "insert into car_makes (id, name, created_by) values ($1, $2, $3) on conflict do nothing",
        new_id(),
        brand,
        user.id
    )
    .execute(&mut *conn)
    .await?;
    let make = sqlx::query!(
        "select id, name from car_makes where lower(name) = lower($1)",
        brand
    )
    .fetch_one(&mut *conn)
    .await?;
    if model.is_empty() {
        return Ok((make.name, model));
    }
    sqlx::query!(
        "insert into car_models (id, make_id, name, created_by) values ($1, $2, $3, $4) on conflict do nothing",
        new_id(),
        make.id,
        model,
        user.id
    )
    .execute(&mut *conn)
    .await?;
    let model = sqlx::query_scalar!(
        "select name from car_models where make_id = $1 and lower(name) = lower($2)",
        make.id,
        model
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok((make.name, model))
}
