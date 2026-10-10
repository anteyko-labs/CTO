//! Общие механизмы проведения: идемпотентность, журнал, номера документов, движения склада.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::Ctx;
use crate::domain::costing::Pool;
use crate::error::{AppError, AppResult, overflow};

pub fn new_id() -> Uuid {
    Uuid::now_v7()
}

/// Потолок суммы одной операции: 100 млн сом. Больше на точке не бывает, а итоги за период
/// с такими суммами переполнили бы счёт и сломали бы смену и отчёты.
pub const MAX_AMOUNT_TYIYN: i64 = 10_000_000_000;

/// Потолок остатка одной кассы: 10 млрд сом. Отдельные операции ограничены суммой выше,
/// а пересчёт смены сверяется с остатком, поэтому ему нужен свой предел.
pub const MAX_BALANCE_TYIYN: i64 = 1_000_000_000_000;

pub fn check_balance(v: i64) -> AppResult<()> {
    if v.checked_abs().is_none_or(|a| a > MAX_BALANCE_TYIYN) {
        return Err(AppError::Validation(
            "сумма больше 10 000 000 000 с — проверьте ввод".into(),
        ));
    }
    Ok(())
}

pub fn check_amount(v: i64) -> AppResult<()> {
    if v.checked_abs().is_none_or(|a| a > MAX_AMOUNT_TYIYN) {
        return Err(AppError::Validation(
            "сумма больше 100 000 000 с — проверьте ввод".into(),
        ));
    }
    Ok(())
}

/// Начинает операцию с `op_id` (ADR-013). Блокирует повторы того же `op_id`
/// до конца транзакции. Если операция уже проведена — возвращает её ответ.
pub async fn begin_op<T: DeserializeOwned>(
    conn: &mut PgConnection,
    ctx: &Ctx,
    op_id: Uuid,
    kind: &str,
) -> AppResult<Option<T>> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        op_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    let Some(row) = sqlx::query!(
        "select user_id, kind, response from operations where op_id = $1",
        op_id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    if row.user_id != ctx.user.id || row.kind != kind {
        return Err(AppError::Conflict(
            "op_id уже использован другой операцией".into(),
        ));
    }
    serde_json::from_value(row.response)
        .map(Some)
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Сохраняет ответ операции в той же транзакции.
pub async fn finish_op<T: Serialize>(
    conn: &mut PgConnection,
    ctx: &Ctx,
    op_id: Uuid,
    kind: &str,
    response: &T,
) -> AppResult<()> {
    let value = serde_json::to_value(response).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        "insert into operations (op_id, user_id, kind, response) values ($1, $2, $3, $4)",
        op_id,
        ctx.user.id,
        kind,
        value
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Запись в журнал действий (инвариант 5).
pub async fn audit(
    conn: &mut PgConnection,
    ctx: &Ctx,
    action: &str,
    entity: &str,
    entity_id: Option<Uuid>,
    data: Value,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        new_id(),
        ctx.user.branch_id,
        ctx.user.id,
        ctx.device_id,
        action,
        entity,
        entity_id,
        data
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Следующее значение счётчика филиала, начиная с 1.
pub async fn next_counter(conn: &mut PgConnection, branch_id: Uuid, name: &str) -> AppResult<i64> {
    let v = sqlx::query_scalar!(
        r#"insert into counters (branch_id, name, value) values ($1, $2, 1)
           on conflict (branch_id, name) do update set value = counters.value + 1
           returning value"#,
        branch_id,
        name
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(v)
}

/// Карточка, в которую влит товар (цепочка объединений), или сам товар.
pub async fn live_product(conn: &mut PgConnection, id: Uuid) -> AppResult<Uuid> {
    let mut cur = id;
    for _ in 0..8 {
        let next = sqlx::query_scalar!("select merged_into from products where id = $1", cur)
            .fetch_optional(&mut *conn)
            .await?
            .flatten();
        match next {
            Some(n) => cur = n,
            None => return Ok(cur),
        }
    }
    Ok(cur)
}

/// После блокировки остатков: ни одну карточку не объединили, пока ждали. Иначе движение
/// легло бы на дубль, остаток которого уже перенесли, — пусть документ проведут ещё раз.
pub async fn ensure_not_merged(conn: &mut PgConnection, ids: &[Uuid]) -> AppResult<()> {
    let merged = sqlx::query_scalar!(
        r#"select count(*) as "n!" from products where id = any($1) and merged_into is not null"#,
        ids
    )
    .fetch_one(&mut *conn)
    .await?;
    if merged > 0 {
        return Err(AppError::Conflict(
            "товар только что объединили с другой карточкой — повторите операцию".into(),
        ));
    }
    Ok(())
}

/// Блокирует строку остатка товара (создаёт при отсутствии) и возвращает пул.
pub async fn lock_pool(
    conn: &mut PgConnection,
    branch_id: Uuid,
    product_id: Uuid,
) -> AppResult<Pool> {
    sqlx::query!(
        "insert into branch_products (branch_id, product_id) values ($1, $2) on conflict do nothing",
        branch_id,
        product_id
    )
    .execute(&mut *conn)
    .await?;
    let r = sqlx::query!(
        r#"select stock_qty, stock_value_tyiyn, last_cost_qty, last_cost_tyiyn
           from branch_products where branch_id = $1 and product_id = $2 for update"#,
        branch_id,
        product_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Pool {
        qty: r.stock_qty,
        value: r.stock_value_tyiyn,
        last_qty: r.last_cost_qty,
        last_cost: r.last_cost_tyiyn,
    })
}

pub struct Movement<'a> {
    pub branch_id: Uuid,
    pub product_id: Uuid,
    pub qty_delta: i64,
    pub value_delta: i64,
    pub doc_type: &'a str,
    pub doc_id: Uuid,
}

/// Движение склада и обновление кэша остатка в одной транзакции (инвариант 6).
/// Строка остатка должна быть заблокирована через [`lock_pool`].
pub async fn apply_movement(conn: &mut PgConnection, m: Movement<'_>) -> AppResult<()> {
    sqlx::query!(
        r#"insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        new_id(),
        m.branch_id,
        m.product_id,
        m.qty_delta,
        m.value_delta,
        m.doc_type,
        m.doc_id
    )
    .execute(&mut *conn)
    .await?;
    // Остаток есть, а стоимость не больше нуля — себестоимость продаж будет неверной: владельцу на проверку.
    let after = sqlx::query!(
        r#"update branch_products
           set stock_qty = stock_qty + $3,
               stock_value_tyiyn = stock_value_tyiyn + $4,
               needs_review = needs_review or (stock_qty + $3 < 0)
                              or (stock_qty + $3 > 0 and stock_value_tyiyn + $4 <= 0)
           where branch_id = $1 and product_id = $2
           returning stock_qty, stock_value_tyiyn, last_cost_qty, last_cost_tyiyn"#,
        m.branch_id,
        m.product_id,
        m.qty_delta,
        m.value_delta
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.code().as_deref() == Some("22003") => overflow(),
        _ => AppError::from(e),
    })?
    .ok_or_else(|| AppError::Internal("строка остатка не найдена".into()))?;
    // Стоимость остатка должна соответствовать тому, что лежит на складе (ADR-044):
    // - товара 0 — стоимость 0;
    // - приход (закупка) закрыл продажи в минус — оставшееся стоит по цене этого прихода;
    // - товар есть, а стоимость ушла в минус — по последней закупочной цене.
    // Разница списывается переоценкой и входит в себестоимость дня.
    if m.doc_type == "revaluation" {
        return Ok(());
    }
    let (qty, value) = (after.stock_qty, after.stock_value_tyiyn);
    let before_qty = qty.saturating_sub(m.qty_delta);
    let per_unit = |cost: i64, units: i64| {
        crate::domain::money::div_round(i128::from(qty) * i128::from(cost), i128::from(units))
    };
    let target = if qty == 0 {
        Some(0)
    } else if m.qty_delta > 0
        && before_qty < 0
        && matches!(m.doc_type, "receipt" | "battery_intake")
    {
        // Только закупка задаёт цену: возврат по чеку с нулевой себестоимостью не обнуляет минус.
        per_unit(m.value_delta, m.qty_delta)
    } else if qty > 0 && value < 0 {
        Some(if after.last_cost_qty > 0 {
            per_unit(after.last_cost_tyiyn, after.last_cost_qty).unwrap_or(0)
        } else {
            0
        })
    } else {
        None
    };
    if let Some(target) = target
        && target != value
    {
        let delta = target.checked_sub(value).ok_or_else(overflow)?;
        sqlx::query!(
            r#"insert into stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id)
               values ($1, $2, $3, 0, $4, 'revaluation', $5)"#,
            new_id(),
            m.branch_id,
            m.product_id,
            delta,
            m.doc_id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update branch_products set stock_value_tyiyn = $3 where branch_id = $1 and product_id = $2",
            m.branch_id,
            m.product_id,
            target
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}
