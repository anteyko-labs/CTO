//! Отменённые товары (SPEC-30 §8, ADR-099): строки, удалённые из чека до проведения, —
//! крестиком, «Очистить» или вместе с отложенным чеком. Уходят с проведённым чеком
//! (`removed_lines`) или отдельной операцией; без сети — очередью устройства, как чеки.
//! Это след, а не документ: строка с ошибкой пропускается и ничего не отклоняет — чек уже
//! пробит, кассир не должен застрять из-за записи об удалённой строке.

use axum::extract::State;
use axum::{Json, Router, routing};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::Ctx;
use crate::domain::money::{div_round, mul};
use crate::error::{AppResult, invalid};
use crate::ops::{self, MAX_AMOUNT_TYIYN, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/sales/cancelled-lines", routing::post(post_cancelled))
}

/// Строк одной записью — не больше, чем в чеке (ADR-090).
const MAX_LINES: usize = 300;

/// Строка, удалённая из чека, как её прислала касса.
#[derive(Deserialize, Clone, Debug)]
struct LineReq {
    /// `remove` — крестиком, `clear` — «Очистить», `held` — удалили отложенный чек.
    #[serde(default)]
    reason: String,
    kind: String,
    #[serde(default)]
    product_id: Option<Uuid>,
    #[serde(default)]
    service_id: Option<Uuid>,
    /// Название, которое видела касса: если карточки на сервере нет.
    #[serde(default)]
    name: String,
    qty: i64,
    #[serde(default)]
    unit_price_tyiyn: i64,
    /// Когда удалили на устройстве.
    #[serde(default)]
    removed_at: Option<DateTime<Utc>>,
}

/// Отдельная запись: «Очистить» и удаление отложенного чека (SPEC-30 §8).
#[derive(Deserialize)]
pub struct CancelledReq {
    pub op_id: Uuid,
    /// Смена, открытая на устройстве; без неё — смена кассы в момент удаления.
    #[serde(default)]
    pub shift_id: Option<Uuid>,
    /// Время на устройстве, когда строки удалили.
    #[serde(default)]
    pub client_time: Option<DateTime<Utc>>,
    /// Строки — как пришли: неверная пропускается, а не отклоняет запись.
    #[serde(default)]
    pub lines: Vec<Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct CancelledOut {
    /// Сколько строк записано.
    pub count: i64,
    pub amount_tyiyn: i64,
}

/// Чья запись: проведённый чек и смена, в которую его пробили.
pub struct Target {
    pub op_id: Uuid,
    pub sale_id: Option<Uuid>,
    /// Смена и её кассир.
    pub shift: Option<(Uuid, Uuid)>,
}

/// Время устройства, если ему можно верить (то же окно, что у чека без сети, SPEC-08);
/// иначе — время сервера.
fn device_time(at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> DateTime<Utc> {
    match at {
        Some(t) if t > now - Duration::days(2) && t < now + Duration::minutes(5) => t,
        _ => now,
    }
}

/// Сумма строки: розлив и вес — за литр и килограмм, остальное — за штуку (как в чеке).
fn line_amount(kind: &str, qty: i64, price: i64) -> Option<i64> {
    match kind {
        "pour" | "weight" => div_round(i128::from(qty) * i128::from(price), 1000),
        _ => mul(qty, price),
    }
}

/// Строки из тела запроса: массив; что не разобралось — пропускается.
fn parse_lines(raw: &[Value]) -> AppResult<Vec<LineReq>> {
    if raw.len() > MAX_LINES {
        return Err(invalid(format!(
            "отменённых строк за раз — не больше {MAX_LINES}"
        )));
    }
    Ok(raw
        .iter()
        .filter_map(|v| serde_json::from_value::<LineReq>(v.clone()).ok())
        .collect())
}

/// Строки, удалённые из чека до проведения, из поля чека `removed_lines`: что угодно, кроме
/// массива, — нет строк.
pub fn lines_of(raw: &Value) -> &[Value] {
    raw.as_array().map_or(&[], Vec::as_slice)
}

/// Записывает строки. Неверная строка (нет количества, цена меньше нуля, вид неизвестен)
/// пропускается. Название — с карточки на сервере, без неё — как видела касса.
pub async fn record(
    conn: &mut PgConnection,
    ctx: &Ctx,
    target: Target,
    raw: &[Value],
) -> AppResult<CancelledOut> {
    let lines = parse_lines(raw)?;
    let now = sqlx::query_scalar!(r#"select now() as "t!""#)
        .fetch_one(&mut *conn)
        .await?;
    let mut out = CancelledOut::default();
    for l in lines {
        if !matches!(
            l.kind.as_str(),
            "piece" | "container" | "pour" | "weight" | "service"
        ) || l.qty <= 0
            || l.qty > 1_000_000_000
            || l.unit_price_tyiyn < 0
            || l.unit_price_tyiyn > MAX_AMOUNT_TYIYN
        {
            continue;
        }
        let Some(amount) = line_amount(&l.kind, l.qty, l.unit_price_tyiyn)
            .filter(|a| (0..=MAX_AMOUNT_TYIYN).contains(a))
        else {
            continue;
        };
        let reason = match l.reason.as_str() {
            "clear" => "clear",
            "held" => "held",
            _ => "remove",
        };
        // Карточка с сервера: название на момент удаления, как в строке чека.
        let (product_id, service_id, known) = if l.kind == "service" {
            let name = match l.service_id {
                Some(id) => {
                    sqlx::query_scalar!("select name from services where id = $1", id)
                        .fetch_optional(&mut *conn)
                        .await?
                }
                None => None,
            };
            (None, name.as_ref().and(l.service_id), name)
        } else {
            let name = match l.product_id {
                Some(id) => {
                    sqlx::query_scalar!(
                        r#"select name || case attrs ->> 'polarity' when 'прямая' then ' (Л)'
                                                                when 'обратная' then ' (П)' else '' end as "name!"
                           from products where id = $1"#,
                        id
                    )
                    .fetch_optional(&mut *conn)
                    .await?
                }
                None => None,
            };
            (name.as_ref().and(l.product_id), None, name)
        };
        let name = match known {
            Some(n) => n,
            None => l.name.trim().chars().take(200).collect::<String>(),
        };
        if name.is_empty() {
            continue;
        }
        sqlx::query!(
            r#"insert into cancelled_lines (id, branch_id, op_id, sale_id, shift_id, cashier_employee_id,
                                            reason, kind, product_id, service_id, name, qty,
                                            unit_price_tyiyn, amount_tyiyn, user_id, device_id, removed_at)
               values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)"#,
            new_id(),
            ctx.user.branch_id,
            target.op_id,
            target.sale_id,
            target.shift.map(|s| s.0),
            target.shift.map(|s| s.1),
            reason,
            l.kind,
            product_id,
            service_id,
            name,
            l.qty,
            l.unit_price_tyiyn,
            amount,
            ctx.user.id,
            ctx.device_id,
            device_time(l.removed_at, now)
        )
        .execute(&mut *conn)
        .await?;
        out.count += 1;
        out.amount_tyiyn = out
            .amount_tyiyn
            .checked_add(amount)
            .ok_or_else(crate::error::overflow)?;
    }
    if out.count > 0 {
        ops::audit(
            conn,
            ctx,
            "sale.cancelled_lines",
            if target.sale_id.is_some() {
                "sale"
            } else {
                "shift"
            },
            target.sale_id.or(target.shift.map(|s| s.0)),
            json!({ "count": out.count, "amount": out.amount_tyiyn }),
        )
        .await?;
    }
    Ok(out)
}

/// Смена записи: присланная устройством (если она этого филиала), иначе смена кассы в момент
/// удаления — как у чека без сети.
async fn shift_for(
    conn: &mut PgConnection,
    ctx: &Ctx,
    requested: Option<Uuid>,
    at: Option<DateTime<Utc>>,
) -> AppResult<Option<(Uuid, Uuid)>> {
    if let Some(id) = requested
        && let Some(cashier) = sqlx::query_scalar!(
            "select cashier_employee_id from shifts where id = $1 and branch_id = $2",
            id,
            ctx.user.branch_id
        )
        .fetch_optional(&mut *conn)
        .await?
    {
        return Ok(Some((id, cashier)));
    }
    let now = sqlx::query_scalar!(r#"select now() as "t!""#)
        .fetch_one(&mut *conn)
        .await?;
    let register = crate::api::cash::default_account(conn, ctx.user.branch_id).await?;
    Ok(crate::api::cash::shift_open_at(
        conn,
        ctx.user.branch_id,
        register,
        Some(device_time(at, now)),
    )
    .await?
    .map(|s| (s.id, s.cashier_employee_id)))
}

pub async fn post_cancelled_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: CancelledReq,
) -> AppResult<CancelledOut> {
    const KIND: &str = "sale.cancelled_lines";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let shift = shift_for(conn, ctx, req.shift_id, req.client_time).await?;
    let out = record(
        conn,
        ctx,
        Target {
            op_id: req.op_id,
            sale_id: None,
            shift,
        },
        &req.lines,
    )
    .await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_cancelled(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CancelledReq>,
) -> AppResult<Json<CancelledOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_cancelled_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CancelledRow {
    pub id: Uuid,
    pub removed_at: DateTime<Utc>,
    pub reason: String,
    pub kind: String,
    pub name: String,
    pub qty: i64,
    pub unit_price_tyiyn: i64,
    pub amount_tyiyn: i64,
    pub user_name: String,
    pub cashier_name: Option<String>,
    /// Чек, проведённый после удаления строки.
    pub sale_id: Option<Uuid>,
    pub sale_number: Option<i64>,
}

/// Отменённые строки смены — отчёт смены и бот (SPEC-30 §7, §8).
pub async fn of_shift(conn: &mut PgConnection, shift_id: Uuid) -> AppResult<Vec<CancelledRow>> {
    Ok(sqlx::query_as!(
        CancelledRow,
        r#"select c.id, c.removed_at, c.reason, c.kind, c.name, c.qty, c.unit_price_tyiyn,
                  c.amount_tyiyn, u.full_name as user_name, e.full_name as "cashier_name?",
                  c.sale_id, s.number as "sale_number?"
           from cancelled_lines c
           join users u on u.id = c.user_id
           left join employees e on e.id = c.cashier_employee_id
           left join sales s on s.id = c.sale_id
           where c.shift_id = $1
           order by c.removed_at, c.id"#,
        shift_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_follows_line_kind() {
        assert_eq!(line_amount("piece", 3, 1_000), Some(3_000));
        // Розлив: 1,5 л × 600 с за литр.
        assert_eq!(line_amount("pour", 1_500, 60_000), Some(90_000));
        assert_eq!(line_amount("weight", 333, 100), Some(33));
    }

    #[test]
    fn device_time_is_trusted_only_within_window() {
        let now = Utc::now();
        let ok = now - Duration::hours(3);
        assert_eq!(device_time(Some(ok), now), ok);
        assert_eq!(device_time(Some(now - Duration::days(3)), now), now);
        assert_eq!(device_time(Some(now + Duration::hours(1)), now), now);
        assert_eq!(device_time(None, now), now);
    }

    #[test]
    fn bad_lines_are_skipped_not_rejected() {
        let raw = vec![
            json!({ "kind": "piece", "qty": 1, "unit_price_tyiyn": 100 }),
            json!({ "kind": "piece", "qty": "много" }),
            json!("строка"),
        ];
        assert_eq!(parse_lines(&raw).map(|l| l.len()).ok(), Some(1));
        assert_eq!(lines_of(&json!(null)).len(), 0);
        assert_eq!(lines_of(&json!([1, 2])).len(), 2);
        assert!(parse_lines(&vec![json!({}); MAX_LINES + 1]).is_err());
    }
}
