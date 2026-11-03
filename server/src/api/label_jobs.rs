//! Станция этикеток (задача 006, ADR-177): телефон ставит задание в очередь филиала, страница
//! станции на ПК склада забирает его по одному и печатает на принтер по умолчанию. Задача 014: у задания и станции —
//! размер (`big`/`small`), каждая станция берёт только свои. Задача 021: у задания — за что цена
//! на этикетке (`piece`/`liter`/`meter`), станция печатает по нему.
//! Задача 027: станция книжки (`book`) — второе окно Chrome на моноблоке кассы; задание несёт
//! машину и чек, станция сама читает книжку и печатает её на свой принтер.
//! Этикетки — без права (SPEC-01): любой вошедший филиала; чужой филиал задание не видит (I16).

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, product_by_id};
use crate::api::refs::{LabelSizeQuery, label_price_per, label_size};
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
        .route("/label-jobs/book", routing::post(create_book_job))
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
    /// `big` — прежний принтер, `small` — 30×20 мм (задача 014); без поля — `big`.
    #[serde(default)]
    size: Option<String>,
    /// Цена на этикетке: `piece` — за шт, `liter` — за литр, `meter` — за метр (задача 021);
    /// без поля — `piece`.
    #[serde(default)]
    price_per: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Created {
    id: Uuid,
    status: String,
}

/// Вид станции: размер этикетки (`big`/`small`) или станция книжки (`book`, задача 027).
fn station_size(size: Option<&str>) -> AppResult<&'static str> {
    match size {
        Some("book") => Ok("book"),
        other => label_size(other),
    }
}

/// Задание «книжка» (задача 027): машина, чек и подписи — как у печати на кассе.
#[derive(Serialize, Deserialize, Clone)]
struct BookJob {
    vehicle_id: Uuid,
    #[serde(default)]
    sale_id: Option<Uuid>,
    #[serde(default)]
    client: Option<String>,
    #[serde(default)]
    master: Option<String>,
    /// После книжки — наклейка с QR бота (задача 024).
    #[serde(default)]
    qr: bool,
}

#[derive(Deserialize)]
struct BookReq {
    op_id: Uuid,
    #[serde(flatten)]
    book: BookJob,
}

#[derive(Serialize, Deserialize)]
struct BookCreated {
    id: Uuid,
    status: String,
    station_online: bool,
}

const BOOK_KIND: &str = "label_job.book";

/// Касса ставит книжку в очередь станции книжки (задача 027). Машина — любая из справочника
/// клиентов (машины общие для филиалов), чужой — 404.
async fn create_book_job(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<BookReq>,
) -> AppResult<Json<BookCreated>> {
    let mut book = req.book;
    let trim = |v: Option<String>| {
        v.map(|s| s.trim().chars().take(200).collect::<String>())
            .filter(|s| !s.is_empty())
    };
    book.client = trim(book.client);
    book.master = trim(book.master);
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<BookCreated>(&mut tx, &ctx, req.op_id, BOOK_KIND).await? {
        return Ok(Json(done));
    }
    let plate = sqlx::query_scalar!(
        "select plate from party_vehicles where id = $1",
        book.vehicle_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if let Some(sale) = book.sale_id {
        sqlx::query_scalar!(
            "select id from sales where id = $1 and branch_id = $2",
            sale,
            ctx.user.branch_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    }
    let id = ops::new_id();
    let data = json!(book);
    sqlx::query!(
        r#"insert into label_jobs (id, branch_id, op_id, items, copies, created_by, size, book)
           values ($1, $2, $3, '[]'::jsonb, 1, $4, 'book', $5)"#,
        id,
        ctx.user.branch_id,
        req.op_id,
        ctx.user.id,
        data
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "label_job",
        Some(id),
        json!({ "size": "book", "plate": plate, "book": data }),
    )
    .await?;
    let station_online = station_online(&mut tx, ctx.user.branch_id, "book").await?;
    let out = BookCreated {
        id,
        status: "queued".into(),
        station_online,
    };
    ops::finish_op(&mut tx, &ctx, req.op_id, BOOK_KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Задание, просроченное в очереди (24 ч; книжка — 2 мин: клиент уже уехал, задача 027), — «устарело»; взятое станцией и не закрытое за
/// 2 мин — снова в очередь (станция перезагрузилась).
async fn housekeep(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        r#"update label_jobs set status = 'failed', error = 'устарело', done_at = now()
           where branch_id = $1 and status = 'queued'
             and (created_at < now() - interval '24 hours'
                  or (size = 'book' and created_at < now() - interval '2 minutes'))"#,
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

/// Станция этого размера отмечалась недавно (задача 014: у каждого принтера своя).
async fn station_online(conn: &mut PgConnection, branch_id: Uuid, size: &str) -> AppResult<bool> {
    let online = sqlx::query_scalar!(
        r#"select exists(select 1 from label_stations
                         where branch_id = $1 and size = $3
                           and last_seen > now() - make_interval(secs => $2)) as "online!""#,
        branch_id,
        STATION_ONLINE_SECS,
        size
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
    let size = label_size(req.size.as_deref())?;
    let price_per = label_price_per(req.price_per.as_deref())?;
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
        r#"insert into label_jobs (id, branch_id, op_id, items, copies, created_by, size, price_per)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        id,
        ctx.user.branch_id,
        req.op_id,
        items,
        copies,
        ctx.user.id,
        size,
        price_per
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "label_job",
        Some(id),
        json!({ "copies": total, "items": items, "size": size, "price_per": price_per }),
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
    size: String,
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
        r#"select j.id, j.status, j.error, j.copies, j.size, j.created_at, j.done_at,
                  u.full_name as "created_by_name!",
                  coalesce('Книжка ' || (select v.plate from party_vehicles v
                                         where v.id = (j.book->>'vehicle_id')::uuid),
                           (select string_agg(p.name, ', ')
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
            size: r.size,
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
    Query(q): Query<LabelSizeQuery>,
) -> AppResult<Json<serde_json::Value>> {
    let size = station_size(q.size.as_deref())?;
    let mut conn = state.pool.acquire().await?;
    housekeep(&mut conn, user.branch_id).await?;
    let jobs = job_rows(&mut conn, user.branch_id, None).await?;
    let online = station_online(&mut conn, user.branch_id, size).await?;
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
    let station_online = station_online(&mut conn, user.branch_id, &job.size).await?;
    Ok(Json(JobStatus {
        job,
        station_online,
    }))
}

#[derive(Deserialize)]
struct StationReq {
    station_id: Uuid,
}

#[derive(Deserialize)]
struct TakeReq {
    station_id: Uuid,
    /// Размер принтера станции; без поля — `big` (станция задачи 006).
    #[serde(default)]
    size: Option<String>,
}

/// Отметка «жив» станции. Станция чужого филиала с тем же номером — не перехватывается.
/// `size` — только при взятии задания: отметки done/failed размер станции не меняют.
async fn seen(
    conn: &mut PgConnection,
    user: &CurrentUser,
    station: Uuid,
    size: Option<&str>,
) -> AppResult<()> {
    let n = sqlx::query!(
        r#"insert into label_stations (id, branch_id, user_id, size) values ($1, $2, $3, coalesce($4, 'big'))
           on conflict (id) do update set last_seen = now(), user_id = excluded.user_id,
                                          size = coalesce($4, label_stations.size)
           where label_stations.branch_id = excluded.branch_id"#,
        station,
        user.branch_id,
        user.id,
        size
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
    /// За что цена на этикетке (задача 021).
    price_per: String,
    items: Vec<TakeItem>,
    /// Задание книжки (задача 027); у этикеток — нет.
    #[serde(skip_serializing_if = "Option::is_none")]
    book: Option<serde_json::Value>,
}

/// Станция берёт одно задание из очереди: две станции одно не возьмут (SKIP LOCKED).
async fn take_job(
    State(state): State<AppState>,
    user: CurrentUser,
    Json(req): Json<TakeReq>,
) -> AppResult<Json<serde_json::Value>> {
    let size = station_size(req.size.as_deref())?;
    let mut conn = state.pool.acquire().await?;
    seen(&mut conn, &user, req.station_id, Some(size)).await?;
    housekeep(&mut conn, user.branch_id).await?;
    let row = sqlx::query!(
        r#"update label_jobs set status = 'printing', taken_at = now(), station_id = $2
           where id = (select id from label_jobs
                       where branch_id = $1 and size = $3 and status = 'queued'
                       order by created_at limit 1
                       for update skip locked)
           returning id, items, copies, created_by, price_per, book"#,
        user.branch_id,
        req.station_id,
        size
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
        price_per: row.price_per,
        items,
        book: row.book,
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
    seen(&mut conn, user, station, None).await?;
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
