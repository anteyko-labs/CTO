//! Станция этикеток (задача 006, ADR-177): телефон ставит задание в очередь филиала, страница
//! станции на ПК склада забирает его по одному и печатает на принтер по умолчанию.
//! Этикетки — без права (SPEC-01): любой вошедший филиала; чужой филиал задание не видит (I16).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, product_by_id};
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

const KIND: &str = "label_job.create";
/// Копий в одном задании не больше.
pub const MAX_COPIES: i64 = 500;
/// Станция не отмечалась дольше — «склад не в сети».
const STATION_ONLINE_SECS: f64 = 30.0;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/label-jobs", routing::get(list_jobs).post(create_job))
        .route("/label-jobs/take", routing::post(take_job))
        .route("/label-jobs/{id}", routing::get(get_job))
        .route("/label-jobs/{id}/done", routing::post(done_job))
        .route("/label-jobs/{id}/failed", routing::post(fail_job))
}

#[derive(Serialize, Deserialize, Clone)]
struct JobItem {
    product_id: Uuid,
    copies: i64,
}

#[derive(Deserialize)]
struct CreateReq {
    op_id: Uuid,
    items: Vec<JobItem>,
}

#[derive(Serialize, Deserialize)]
struct Created {
    id: Uuid,
    status: String,
}

/// Задание, просроченное в очереди (24 ч), — «устарело»; взятое станцией и не закрытое за
/// 2 мин — снова в очередь (станция перезагрузилась).
async fn housekeep(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        r#"update label_jobs set status = 'failed', error = 'устарело', done_at = now()
           where branch_id = $1 and status = 'queued' and created_at < now() - interval '24 hours'"#,
        branch_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        r#"update label_jobs set status = 'queued', taken_at = null, station_id = null
           where branch_id = $1 and status = 'printing' and taken_at < now() - interval '2 minutes'"#,
        branch_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn station_online(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<bool> {
    let online = sqlx::query_scalar!(
        r#"select exists(select 1 from label_stations
                         where branch_id = $1 and last_seen > now() - make_interval(secs => $2)) as "online!""#,
        branch_id,
        STATION_ONLINE_SECS
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(online)
}

async fn create_job(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<Created>> {
    if req.items.is_empty() {
        return Err(invalid("нет этикеток для печати"));
    }
    if req.items.len() > 200 {
        return Err(invalid("слишком много товаров в одном задании"));
    }
    let mut total: i64 = 0;
    for i in &req.items {
        if i.copies < 1 {
            return Err(invalid("копий — не меньше одной"));
        }
        total = total.saturating_add(i.copies);
    }
    if total > MAX_COPIES {
        return Err(invalid(format!(
            "в одном задании не больше {MAX_COPIES} этикеток"
        )));
    }
    let copies = i32::try_from(total).map_err(|_| crate::error::overflow())?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<Created>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    for i in &req.items {
        product_by_id(&mut tx, &ctx.user, i.product_id).await?;
    }
    let id = ops::new_id();
    let items = json!(req.items);
    sqlx::query!(
        r#"insert into label_jobs (id, branch_id, op_id, items, copies, created_by)
           values ($1, $2, $3, $4, $5, $6)"#,
        id,
        ctx.user.branch_id,
        req.op_id,
        items,
        copies,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "label_job",
        Some(id),
        json!({ "copies": total, "items": items }),
    )
    .await?;
    let out = Created {
        id,
        status: "queued".into(),
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize)]
struct JobOut {
    id: Uuid,
    status: String,
    error: Option<String>,
    copies: i32,
    created_by_name: String,
    /// Названия товаров через запятую — «что» в списке станции.
    what: String,
    created_at: DateTime<Utc>,
    done_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct JobStatus {
    #[serde(flatten)]
    job: JobOut,
    station_online: bool,
}

async fn job_rows(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Option<Uuid>,
) -> AppResult<Vec<JobOut>> {
    let rows = sqlx::query!(
        r#"select j.id, j.status, j.error, j.copies, j.created_at, j.done_at,
                  u.full_name as "created_by_name!",
                  coalesce((select string_agg(p.name, ', ')
                            from jsonb_to_recordset(j.items) as x(product_id uuid, copies bigint)
                            join products p on p.id = x.product_id), '') as "what!"
           from label_jobs j join users u on u.id = j.created_by
           where j.branch_id = $1 and ($2::uuid is null or j.id = $2)
           order by j.created_at desc
           limit 20"#,
        branch_id,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| JobOut {
            id: r.id,
            status: r.status,
            error: r.error,
            copies: r.copies,
            created_by_name: r.created_by_name,
            what: r.what,
            created_at: r.created_at,
            done_at: r.done_at,
        })
        .collect())
}

/// Последние 20 заданий филиала и признак «станция в сети».
async fn list_jobs(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<serde_json::Value>> {
    let mut conn = state.pool.acquire().await?;
    housekeep(&mut conn, user.branch_id).await?;
    let jobs = job_rows(&mut conn, user.branch_id, None).await?;
    let online = station_online(&mut conn, user.branch_id).await?;
    Ok(Json(json!({ "jobs": jobs, "station_online": online })))
}

/// Статус задания для плашки на телефоне.
async fn get_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<JobStatus>> {
    let mut conn = state.pool.acquire().await?;
    housekeep(&mut conn, user.branch_id).await?;
    let job = job_rows(&mut conn, user.branch_id, Some(id))
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    let station_online = station_online(&mut conn, user.branch_id).await?;
    Ok(Json(JobStatus {
        job,
        station_online,
    }))
}

#[derive(Deserialize)]
struct StationReq {
    station_id: Uuid,
}

/// Отметка «жив» станции. Станция чужого филиала с тем же номером — не перехватывается.
async fn seen(conn: &mut PgConnection, user: &CurrentUser, station: Uuid) -> AppResult<()> {
    let n = sqlx::query!(
        r#"insert into label_stations (id, branch_id, user_id) values ($1, $2, $3)
           on conflict (id) do update set last_seen = now(), user_id = excluded.user_id
           where label_stations.branch_id = excluded.branch_id"#,
        station,
        user.branch_id,
        user.id
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::Conflict("станция другого филиала".into()));
    }
    Ok(())
}

#[derive(Serialize)]
struct TakeItem {
    product: ProductOut,
    copies: i64,
}

#[derive(Serialize)]
struct Taken {
    id: Uuid,
    copies: i32,
    created_by_name: String,
    items: Vec<TakeItem>,
}

/// Станция берёт одно задание из очереди: две станции одно не возьмут (SKIP LOCKED).
async fn take_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<StationReq>,
) -> AppResult<Json<serde_json::Value>> {
    let mut conn = state.pool.acquire().await?;
    seen(&mut conn, &user, req.station_id).await?;
    housekeep(&mut conn, user.branch_id).await?;
    let row = sqlx::query!(
        r#"update label_jobs set status = 'printing', taken_at = now(), station_id = $2
           where id = (select id from label_jobs
                       where branch_id = $1 and status = 'queued'
                       order by created_at limit 1
                       for update skip locked)
           returning id, items, copies, created_by"#,
        user.branch_id,
        req.station_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else {
        return Ok(Json(json!({ "job": null })));
    };
    let created_by_name =
        sqlx::query_scalar!("select full_name from users where id = $1", row.created_by)
            .fetch_one(&mut *conn)
            .await?;
    let list: Vec<JobItem> =
        serde_json::from_value(row.items).map_err(|e| AppError::Internal(e.to_string()))?;
    let mut items = Vec::new();
    for i in list {
        // Строки этикетки — с сервера в момент печати; удалённый товар пропускается.
        let id = ops::live_product(&mut conn, i.product_id).await?;
        if let Ok(product) = product_by_id(&mut conn, &user, id).await {
            items.push(TakeItem {
                product,
                copies: i.copies,
            });
        }
    }
    Ok(Json(json!({ "job": Taken {
        id: row.id,
        copies: row.copies,
        created_by_name,
        items,
    } })))
}

#[derive(Deserialize)]
struct FailReq {
    station_id: Uuid,
    #[serde(default)]
    error: String,
}

async fn finish(
    state: &AppState,
    user: &CurrentUser,
    id: Uuid,
    station: Uuid,
    status: &str,
    error: Option<String>,
) -> AppResult<Json<serde_json::Value>> {
    let mut conn = state.pool.acquire().await?;
    seen(&mut conn, user, station).await?;
    let cur = sqlx::query_scalar!(
        "select status from label_jobs where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    // Повтор отметки — тот же итог.
    if cur == status {
        return Ok(Json(json!({ "id": id, "status": status })));
    }
    let n = sqlx::query!(
        r#"update label_jobs set status = $3, error = $4, done_at = now()
           where id = $1 and branch_id = $2 and status = 'printing'"#,
        id,
        user.branch_id,
        status,
        error
    )
    .execute(&mut *conn)
    .await?
    .rows_affected();
    if n == 0 {
        return Err(AppError::Conflict("задание не печатается".into()));
    }
    Ok(Json(json!({ "id": id, "status": status })))
}

async fn done_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<StationReq>,
) -> AppResult<Json<serde_json::Value>> {
    finish(&state, &user, id, req.station_id, "done", None).await
}

async fn fail_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Json(req): Json<FailReq>,
) -> AppResult<Json<serde_json::Value>> {
    let error = req.error.trim();
    let error = if error.is_empty() {
        "ошибка печати".to_string()
    } else {
        error.chars().take(300).collect()
    };
    finish(&state, &user, id, req.station_id, "failed", Some(error)).await
}
