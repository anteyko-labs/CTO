//! График выходных (задача 037 п.5, ADR-203): «Сотрудники → График» — месяц, по строке на
//! работника, клик — выходной. Выходной не ставится, если в этот день работающих в филиале
//! станет меньше минимума `staff.min_on_duty` (0 — без ограничения, по умолчанию): 409
//! «В <день> останется N, нужно не менее M». Владелец — всегда, администратор — право
//! «График выходных» (`staff_schedule`); минимум задаёт владелец. Событие `staff.day_off`.

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser, require_permission};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

/// Право на график выходных (владельцу не нужно).
pub const PERMISSION: &str = "staff_schedule";
/// Наибольший минимум работающих.
const MAX_MIN_ON_DUTY: i32 = 100;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/staff/days-off",
            routing::get(get_days_off).put(put_day_off),
        )
        .route(
            "/settings/staff",
            routing::get(get_settings).put(put_settings),
        )
}

#[derive(Serialize, Deserialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct StaffSettings {
    /// Сколько работающих в день должно остаться в филиале; 0 — без ограничения.
    #[serde(default)]
    pub min_on_duty: i32,
}

/// Настройки сотрудников филиала; нет или не читаются — по умолчанию (минимум 0).
pub async fn settings_of(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<StaffSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'staff'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// Выходные филиала за период `[from, to)`: (сотрудник, день).
pub async fn days_off_of(
    conn: &mut PgConnection,
    branch_id: Uuid,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Vec<(Uuid, NaiveDate)>> {
    Ok(sqlx::query!(
        "select employee_id, day from employee_days_off where branch_id = $1 and day >= $2 and day < $3 order by day",
        branch_id,
        from,
        to
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| (r.employee_id, r.day))
    .collect())
}

#[derive(Deserialize)]
struct MonthQuery {
    /// ГГГГ-ММ; пусто — текущий месяц по Бишкеку.
    month: Option<String>,
}

#[derive(Serialize)]
struct EmployeeOut {
    id: Uuid,
    full_name: String,
    days_off: Vec<NaiveDate>,
}

#[derive(Serialize)]
struct DaysOffOut {
    month: String,
    days_in_month: u32,
    min_on_duty: i32,
    employees: Vec<EmployeeOut>,
}

async fn month_range(
    conn: &mut PgConnection,
    month: Option<&str>,
) -> AppResult<(NaiveDate, NaiveDate)> {
    let from = match month.map(str::trim).filter(|m| !m.is_empty()) {
        Some(m) => NaiveDate::parse_from_str(&format!("{m}-01"), "%Y-%m-%d")
            .map_err(|_| invalid("месяц — в виде ГГГГ-ММ"))?,
        None => {
            let today =
                sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
                    .fetch_one(&mut *conn)
                    .await?;
            today.with_day0(0).unwrap_or(today)
        }
    };
    let to = from
        .checked_add_months(Months::new(1))
        .ok_or_else(|| invalid("месяц вне пределов"))?;
    Ok((from, to))
}

async fn get_days_off(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<MonthQuery>,
) -> AppResult<Json<DaysOffOut>> {
    let mut conn = state.pool.acquire().await?;
    require_permission(&mut conn, &user, PERMISSION).await?;
    let (from, to) = month_range(&mut conn, q.month.as_deref()).await?;
    let offs = days_off_of(&mut conn, user.branch_id, from, to).await?;
    let employees = sqlx::query!(
        "select id, full_name from employees where branch_id = $1 and active order by full_name",
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|e| EmployeeOut {
        days_off: offs.iter().filter(|o| o.0 == e.id).map(|o| o.1).collect(),
        id: e.id,
        full_name: e.full_name,
    })
    .collect();
    Ok(Json(DaysOffOut {
        month: from.format("%Y-%m").to_string(),
        days_in_month: u32::try_from((to - from).num_days()).unwrap_or(31),
        min_on_duty: settings_of(&mut conn, user.branch_id).await?.min_on_duty,
        employees,
    }))
}

#[derive(Deserialize, Serialize)]
struct DayOffIn {
    employee_id: Uuid,
    day: NaiveDate,
    /// `true` — поставить выходной, `false` — снять.
    off: bool,
}

#[derive(Serialize, Deserialize)]
struct DayOffOut {
    employee_id: Uuid,
    day: NaiveDate,
    off: bool,
    /// Сколько работающих в этот день после изменения.
    on_duty: i64,
}

async fn put_day_off(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ops::WithOp<DayOffIn>>,
) -> AppResult<Json<DayOffOut>> {
    const KIND: &str = "staff.day_off";
    let (op_id, req) = (req.op_id, req.body);
    let mut tx = state.pool.begin().await?;
    require_permission(&mut tx, &ctx.user, PERMISSION).await?;
    if let Some(done) = ops::begin_opt(&mut tx, &ctx, op_id, KIND).await? {
        return Ok(Json(done));
    }
    let branch_id = ctx.user.branch_id;
    // Филиал — по очереди: два выходных одновременно не обходят минимум.
    sqlx::query!(
        "select id from branches where id = $1 for update",
        branch_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let emp = sqlx::query!(
        "select full_name, active from employees where id = $1 and branch_id = $2",
        req.employee_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let was_off = sqlx::query_scalar!(
        r#"select exists(select 1 from employee_days_off where employee_id = $1 and day = $2) as "e!""#,
        req.employee_id,
        req.day
    )
    .fetch_one(&mut *tx)
    .await?;
    let staff = sqlx::query_scalar!(
        r#"select count(*) as "n!" from employees where branch_id = $1 and active"#,
        branch_id
    )
    .fetch_one(&mut *tx)
    .await?;
    let mut off_after = sqlx::query_scalar!(
        r#"select count(*) as "n!" from employee_days_off d join employees e on e.id = d.employee_id
           where d.branch_id = $1 and d.day = $2 and e.active"#,
        branch_id,
        req.day
    )
    .fetch_one(&mut *tx)
    .await?;
    if req.off && !was_off {
        if !emp.active {
            return Err(invalid("сотрудник отключён"));
        }
        off_after += 1;
        let min = settings_of(&mut tx, branch_id).await?.min_on_duty;
        let left = staff - off_after;
        if min > 0 && left < i64::from(min) {
            return Err(AppError::Conflict(format!(
                "В {} останется {}, нужно не менее {}",
                req.day.format("%d.%m"),
                left.max(0),
                min
            )));
        }
        sqlx::query!(
            "insert into employee_days_off (employee_id, day, branch_id, created_by) values ($1, $2, $3, $4)",
            req.employee_id,
            req.day,
            branch_id,
            ctx.user.id
        )
        .execute(&mut *tx)
        .await?;
    } else if !req.off && was_off {
        sqlx::query!(
            "delete from employee_days_off where employee_id = $1 and day = $2",
            req.employee_id,
            req.day
        )
        .execute(&mut *tx)
        .await?;
        if emp.active {
            off_after -= 1;
        }
    }
    let out = DayOffOut {
        employee_id: req.employee_id,
        day: req.day,
        off: req.off,
        on_duty: staff - off_after,
    };
    if req.off != was_off {
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "employee",
            Some(req.employee_id),
            json!({ "name": emp.full_name, "day": req.day, "off": req.off, "on_duty": out.on_duty }),
        )
        .await?;
    }
    ops::finish_opt(&mut tx, &ctx, op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<StaffSettings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(settings_of(&mut conn, user.branch_id).await?))
}

/// Минимум работающих — только владелец.
async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ops::WithOp<StaffSettings>>,
) -> AppResult<Json<StaffSettings>> {
    const KIND: &str = "settings.staff";
    let (op_id, req) = (req.op_id, req.body);
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !(0..=MAX_MIN_ON_DUTY).contains(&req.min_on_duty) {
        return Err(invalid("минимум работающих — от 0 до 100"));
    }
    let value = json!(req);
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_opt(&mut tx, &ctx, op_id, KIND).await? {
        return Ok(Json(done));
    }
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'staff', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(&mut tx, &ctx, KIND, "settings", None, value).await?;
    ops::finish_opt(&mut tx, &ctx, op_id, KIND, &req).await?;
    tx.commit().await?;
    Ok(Json(req))
}
