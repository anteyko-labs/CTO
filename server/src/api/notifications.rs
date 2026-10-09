//! Уведомления владельцу: важные события из журнала действий (инвариант 5).
//! Отдельной таблицы нет — журнал уже хранит всё, здесь только выборка и отметка «просмотрено».

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/notifications", routing::get(list))
        .route("/notifications/seen", routing::post(mark_seen))
}

/// Действия, о которых владелец должен знать: деньги, цены, отмены и доступы.
pub(crate) const WATCHED: [&str; 19] = [
    "party.limit_request",
    "gift.rule",
    "product.prices",
    "sale.price_override",
    "sale.credit_limit_exceeded",
    "sale.return",
    "receipt.reverse",
    "debt.adjust",
    "user.create",
    "user.update",
    "shift.handover",
    "cash.reverse",
    "cash.transfer_reverse",
    "sale.return_over_paid",
    "sale.below_cost",
    "receipt.cost_above_price",
    "sale.offline_rejected",
    "sale.stale_price",
    "revision.post",
];

#[derive(Serialize)]
struct NotificationOut {
    id: Uuid,
    action: String,
    title: String,
    details: String,
    user_name: Option<String>,
    at: DateTime<Utc>,
    entity_id: Option<Uuid>,
    new: bool,
}

#[derive(Serialize)]
struct NotificationsOut {
    unseen: i64,
    items: Vec<NotificationOut>,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
}

fn money(v: Option<i64>) -> String {
    v.map_or_else(|| "—".into(), crate::domain::money::format_som)
}

pub(crate) fn describe(action: &str, data: &Value) -> (String, String) {
    let num = |key: &str| data.get(key).and_then(Value::as_i64);
    match action {
        "product.prices" => {
            // Что именно поменяли: цену штуки или канистры, розлив за литр; неизменённое не пишем.
            let side = |which: &str, key: &str| {
                data.get(which)
                    .and_then(|v| v.get(key))
                    .and_then(Value::as_i64)
            };
            let mut parts = Vec::new();
            for (key, label) in [("sale", "цена"), ("pour", "розлив за литр")] {
                let (old, new) = (side("old", key), side("new", key));
                if new.is_some() && old != new {
                    parts.push(format!(
                        "{label}: было {}, стало {}",
                        money(old),
                        money(new)
                    ));
                }
            }
            if parts.is_empty() {
                parts.push(format!(
                    "было {}, стало {}",
                    money(num("old_sale_price_tyiyn")),
                    money(num("sale_price_tyiyn"))
                ));
            }
            (
                format!(
                    "Изменена цена: {}",
                    data.get("name").and_then(Value::as_str).unwrap_or("товар")
                ),
                parts.join("; "),
            )
        }
        "sale.price_override" => (
            "Цена изменена прямо в чеке".into(),
            data.get("lines")
                .and_then(Value::as_array)
                .map(|l| format!("строк: {}", l.len()))
                .unwrap_or_default(),
        ),
        "sale.credit_limit_exceeded" => (
            "Долг клиента вышел за лимит".into(),
            format!("в долг {}", money(num("debt"))),
        ),
        "sale.return" => (
            "Оформлен возврат".into(),
            format!(
                "чек № {}, {}",
                num("number").unwrap_or(0),
                money(num("total"))
            ),
        ),
        "receipt.reverse" => (
            "Сторно прихода".into(),
            format!("накладная № {}", num("number").unwrap_or(0)),
        ),
        "party.limit_request" => (
            "Просят поднять лимит долга".into(),
            format!(
                "{}: не хватает {}{}",
                data.get("name").and_then(Value::as_str).unwrap_or(""),
                money(num("amount_tyiyn")),
                data.get("comment")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                    .map(|c| format!(" — {c}"))
                    .unwrap_or_default()
            ),
        ),
        "gift.rule" => (
            "Изменены правила подарков".into(),
            format!("подарков в списке: {}", num("items").unwrap_or(0)),
        ),
        "debt.adjust" => (
            "Правка долга вручную".into(),
            format!(
                "{} — {}",
                money(num("amount_tyiyn")),
                data.get("comment").and_then(Value::as_str).unwrap_or("")
            ),
        ),
        "user.create" => (
            "Создан пользователь".into(),
            data.get("login")
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
        ),
        "user.update" => (
            "Изменён пользователь".into(),
            [
                data.get("login")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                data.get("active").and_then(Value::as_bool).map(|a| {
                    if a {
                        "включён"
                    } else {
                        "отключён"
                    }
                    .to_string()
                }),
                data.get("password_changed")
                    .and_then(Value::as_bool)
                    .filter(|c| *c)
                    .map(|_| "сменён пароль".to_string()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", "),
        ),
        "shift.handover" => {
            let diff = num("diff").unwrap_or(0);
            let diff_text = match diff {
                0 => String::new(),
                d if d < 0 => format!(", недостача {}", money(Some(-d))),
                d => format!(", излишек {}", money(Some(d))),
            };
            (
                format!("Смена № {} закрыта", num("number").unwrap_or(0)),
                format!(
                    "должно было быть {}, пересчитали {}{}; в сейф {}, в кассе осталось {}",
                    money(num("expected")),
                    money(num("counted")),
                    diff_text,
                    money(num("to_safe")),
                    money(num("left"))
                ),
            )
        }
        "revision.post" => (
            format!("Проведена ревизия № {}", num("number").unwrap_or(0)),
            format!(
                "пересчитано товаров: {}; недостача {}, излишек {}",
                num("lines").unwrap_or(0),
                money(num("shortage")),
                money(num("surplus"))
            ),
        ),
        "sale.stale_price" => (
            "Чек без сети продан по старой цене".into(),
            format!(
                "чек № {}, строк: {} — прайс сменили, пока касса была без связи",
                num("number").unwrap_or(0),
                data.get("lines")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            ),
        ),
        "sale.offline_rejected" => (
            "Чек без сети не принят сервером".into(),
            format!(
                "{} — разберите его на кассе",
                data.get("error").and_then(Value::as_str).unwrap_or("")
            ),
        ),
        "sale.below_cost" => (
            "Чек без сети продан дешевле закупки".into(),
            format!(
                "чек № {}, строк: {}",
                num("number").unwrap_or(0),
                data.get("lines")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            ),
        ),
        "receipt.cost_above_price" => (
            "Закупка дороже цены продажи".into(),
            format!(
                "накладная № {}: {} — поднимите цену, иначе товар не продать",
                num("number").unwrap_or(0),
                data.get("names")
                    .and_then(Value::as_array)
                    .map(|n| n
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
        ),
        "sale.return_over_paid" => (
            "Возврат деньгами больше оплаченного".into(),
            format!(
                "чек № {}: деньгами платили {}, вернули {}",
                num("number").unwrap_or(0),
                money(num("paid")),
                money(num("refund"))
            ),
        ),
        "cash.reverse" => (
            "Сторно внесения или изъятия".into(),
            format!(
                "{} — {}",
                money(num("amount_tyiyn")),
                data.get("comment").and_then(Value::as_str).unwrap_or("")
            ),
        ),
        "cash.transfer_reverse" => (
            "Сторно перемещения денег".into(),
            format!(
                "перемещение № {}, {} — {}",
                num("number").unwrap_or(0),
                money(num("amount_tyiyn")),
                data.get("comment").and_then(Value::as_str).unwrap_or("")
            ),
        ),
        _ => (action.into(), String::new()),
    }
}

async fn seen_at(state: &AppState, user: &CurrentUser) -> AppResult<Option<DateTime<Utc>>> {
    let key = format!("notifications_seen:{}", user.id);
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = $2",
        user.branch_id,
        key
    )
    .fetch_optional(&state.pool)
    .await?;
    Ok(
        v.and_then(|v| v.get("at").and_then(Value::as_str).map(str::to_string))
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc)),
    )
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<NotificationsOut>> {
    // Сводные события — только владельцу (инвариант 13).
    if !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let seen = seen_at(&state, &user).await?;
    let watched: Vec<String> = WATCHED.iter().map(|s| (*s).to_string()).collect();
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let rows = sqlx::query!(
        r#"select a.id, a.action, a.data, a.entity_id, a.created_at, u.full_name as "user_name?"
           from audit_log a
           left join users u on u.id = a.user_id
           where a.branch_id = $1 and a.action = any($2)
           order by a.created_at desc
           limit $3"#,
        user.branch_id,
        &watched,
        limit
    )
    .fetch_all(&state.pool)
    .await?;
    let items: Vec<NotificationOut> = rows
        .into_iter()
        .map(|r| {
            let (title, details) = describe(&r.action, &r.data);
            NotificationOut {
                id: r.id,
                action: r.action,
                title,
                details,
                user_name: r.user_name,
                at: r.created_at,
                entity_id: r.entity_id,
                new: seen.is_none_or(|s| r.created_at > s),
            }
        })
        .collect();
    let unseen = i64::try_from(items.iter().filter(|i| i.new).count()).unwrap_or(i64::MAX);
    Ok(Json(NotificationsOut { unseen, items }))
}

async fn mark_seen(State(state): State<AppState>, ctx: Ctx) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let key = format!("notifications_seen:{}", ctx.user.id);
    let value = json!({ "at": Utc::now().to_rfc3339() });
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, $2, $3)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        key,
        value
    )
    .execute(&state.pool)
    .await?;
    Ok(Json(value))
}
