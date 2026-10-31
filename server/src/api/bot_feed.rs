//! Что приходит владельцу в бот (задача 002 П11, ADR-138): у каждого владельца свой список
//! выключенных событий и режим чеков. Экран «Уведомления» в программе показывает всё как раньше —
//! настройка влияет только на ленту в Телеграме (`telegram::pending`, `telegram::advance_cursor`).
//! Хранение — `settings` филиала владельца, ключ `bot_feed:<id владельца>`.

use axum::extract::State;
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::auth::{Ctx, Owner};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/settings/bot-feed",
        routing::get(get_settings).put(put_settings),
    )
}

/// Чеки и возвраты — под режимом чеков, а не отдельными выключателями (отбор — в запросах
/// `telegram::pending` и `telegram::advance_cursor`).
#[cfg(test)]
const SALE_ACTIONS: [&str; 3] = ["sale.post", "sale.return", "sale.return_no_receipt"];

/// Закрытие смены с её отчётом приходит всегда: за ним идёт «Итог дня» (ADR-121).
#[cfg(test)]
const ALWAYS: [&str; 1] = ["shift.close"];

/// Режимы чеков: каждый чек; только в долг и возвраты; не присылать (по умолчанию).
const SALE_MODES: [&str; 3] = ["all", "debt_returns", "none"];

/// Группы экрана: ключ и подпись.
const GROUPS: [(&str, &str); 7] = [
    ("sales", "Касса"),
    ("stock", "Склад"),
    ("money", "Деньги"),
    ("debts", "Долги"),
    ("payroll", "Зарплата"),
    ("shift", "Смена"),
    ("other", "Прочее"),
];

/// Событие ленты: действие журнала, группа и подпись для владельца. Все события `WATCHED` и
/// `FEED`, кроме чеков (`SALE_ACTIONS`) — тест сверяет.
const EVENTS: [(&str, &str, &str); 90] = [
    // Касса
    ("sale.price_override", "sales", "Цена изменена прямо в чеке"),
    (
        "sale.return_over_paid",
        "sales",
        "Возврат деньгами больше оплаченного",
    ),
    (
        "sale.below_cost",
        "sales",
        "Чек без сети продан дешевле закупки",
    ),
    (
        "sale.stale_price",
        "sales",
        "Чек без сети продан по старой цене",
    ),
    (
        "sale.offline_rejected",
        "sales",
        "Чек без сети не принят сервером",
    ),
    (
        "sale.offline_bypass",
        "sales",
        "Чек без сети прошёл в обход проверок",
    ),
    (
        "sale.offline_after_close",
        "sales",
        "Чек без сети пришёл после пересчёта смены",
    ),
    ("sale.cost_unknown", "sales", "Продажа без цены закупки"),
    (
        "sale.battery_price",
        "sales",
        "Аккумулятор в оплату не по цене приёма",
    ),
    (
        "correction.post",
        "sales",
        "Исправлен документ или оплата чека",
    ),
    ("gift.rule", "sales", "Изменены правила подарков"),
    (
        "gift.rule_toggle",
        "sales",
        "Правило подарка включено или отключено",
    ),
    (
        "bank_qr.unclaimed",
        "sales",
        "QR оплачен, а чек не проведён",
    ),
    // Склад
    ("receipt.post", "stock", "Приход товара"),
    ("receipt.accept", "stock", "Приёмка товара на планшете"),
    ("acceptance.refuse", "stock", "Отказ от приёмки"),
    ("receipt.reverse", "stock", "Сторно прихода"),
    (
        "receipt.reverse_negative",
        "stock",
        "Сторно накладной: остаток ушёл в минус",
    ),
    (
        "receipt.cost_above_price",
        "stock",
        "Закупка дороже цены продажи",
    ),
    ("product.prices", "stock", "Изменены цены товара"),
    ("product.create", "stock", "Новый товар"),
    ("product.merge", "stock", "Объединены карточки товара"),
    ("revision.post", "stock", "Проведена ревизия"),
    ("revision.cancel", "stock", "Ревизия отменена"),
    ("oil.transfer", "stock", "Перелив масла"),
    (
        "oil.transfer_admin",
        "stock",
        "Перелив масла администратором",
    ),
    ("oil.transfer_reverse", "stock", "Отменён перелив"),
    ("stock_in.post", "stock", "Оприходование"),
    ("stock_in.reverse", "stock", "Сторно оприходования"),
    ("supplier_return.post", "stock", "Возврат поставщику"),
    (
        "supplier_return.reverse",
        "stock",
        "Сторно возврата поставщику",
    ),
    ("defect.mark", "stock", "Товар в брак"),
    (
        "defect.unmark",
        "stock",
        "Брак снят, товар вернулся в продажу",
    ),
    ("defect.resolve", "stock", "Решение по браку"),
    (
        "defect.resolve_reverse",
        "stock",
        "Отменено решение по браку",
    ),
    (
        "defect.reverse_negative",
        "stock",
        "Сторно брака: остаток ушёл в минус",
    ),
    ("battery.intake", "stock", "Приём аккумуляторов"),
    (
        "battery.intake_reverse",
        "stock",
        "Отменён приём аккумуляторов",
    ),
    // Деньги
    ("expense.post", "money", "Расход"),
    ("expense.reverse", "money", "Расход отменён"),
    ("cash.movement", "money", "Внесение или изъятие денег"),
    ("cash.reverse", "money", "Сторно внесения или изъятия"),
    ("cash.transfer", "money", "Перемещение денег между счетами"),
    ("cash.transfer_reverse", "money", "Сторно перемещения денег"),
    ("cash.negative", "money", "Счёт ушёл в минус"),
    ("settings.payment_fee", "money", "Изменена комиссия банка"),
    ("bank_recon.mismatch", "money", "Сверка с банком не сошлась"),
    ("optima.key", "money", "Ключ Optima Business не подходит"),
    (
        "settings.optima",
        "money",
        "Настройка Optima Business изменена",
    ),
    // Долги
    ("debt.repayment", "debts", "Погашение долга"),
    ("debt.adjust", "debts", "Правка долга вручную"),
    ("debt.reverse", "debts", "Сторно записи долга"),
    ("debt.old", "debts", "Старый долг из тетради"),
    ("receipt.supplier_payment", "debts", "Оплата поставщику"),
    (
        "sale.credit_limit_exceeded",
        "debts",
        "Долг клиента вышел за лимит",
    ),
    ("sale.debt_without_inn", "debts", "Долг без ПИН/ИНН"),
    ("party.limit_request", "debts", "Просят поднять лимит долга"),
    ("party.limit_change", "debts", "Изменён лимит долга"),
    // Зарплата
    ("payout.post", "payroll", "Выплата сотруднику"),
    ("payout.reverse", "payroll", "Сторно выплаты сотруднику"),
    ("payroll.salary", "payroll", "Начислен оклад"),
    ("payroll.salary_due", "payroll", "Сегодня расчёт оклада"),
    ("payroll.accrual", "payroll", "Премия или удержание"),
    (
        "payroll.accrual_reverse",
        "payroll",
        "Сторно начисления сотруднику",
    ),
    ("payroll.rule", "payroll", "Изменена оплата сотрудника"),
    ("payroll.rule_close", "payroll", "Закрыта ставка сотрудника"),
    ("employee.create", "payroll", "Новый сотрудник"),
    ("employee.update", "payroll", "Изменён сотрудник"),
    ("employee.pin", "payroll", "Задан или снят PIN кассира"),
    (
        "telegram.employee_link",
        "payroll",
        "Работник подключился к боту",
    ),
    // Смена
    ("shift.open", "shift", "Смена открыта"),
    ("shift.handover", "shift", "Сдача кассы"),
    ("shift.reopen", "shift", "Смена переоткрыта"),
    (
        "shift.reclose_surplus",
        "shift",
        "Излишек после переоткрытия смены",
    ),
    ("shift.pin_locked", "shift", "PIN кассира заблокирован"),
    (
        "shift.cash_to_bank",
        "shift",
        "Наличные смены переведены в QR",
    ),
    (
        "shift.cash_to_bank_reverse",
        "shift",
        "Отменён перевод наличных в QR",
    ),
    // Прочее
    ("user.create", "other", "Создан пользователь"),
    ("user.update", "other", "Изменён пользователь"),
    ("user.permissions", "other", "Права администратора"),
    ("auth.login_flood", "other", "Подбирают пароль"),
    (
        "telegram.code_flood",
        "other",
        "Подбирают код привязки Телеграма",
    ),
    (
        "telegram.customer_ambiguous",
        "other",
        "Клиент не смог подключиться к боту",
    ),
    (
        "telegram.admin_link",
        "other",
        "Телеграм привязан администратору",
    ),
    (
        "vehicle.plate_conflict",
        "other",
        "Номер машины записан на другого клиента",
    ),
    ("settings.loyalty", "other", "Изменён процент баллов"),
    ("old_catalog.disable", "other", "Старая база Умаг выключена"),
    ("old_catalog.enable", "other", "Старая база Умаг включена"),
    ("backup.failed", "other", "Копия базы не создана"),
    ("backup.size", "other", "Копии базы занимают много места"),
];

/// Настройка ленты владельца: выключенные события и режим чеков.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Feed {
    #[serde(default)]
    pub off: Vec<String>,
    #[serde(default = "no_sales")]
    pub sales: String,
}

fn no_sales() -> String {
    "none".into()
}

impl Default for Feed {
    fn default() -> Self {
        Self {
            off: Vec::new(),
            sales: no_sales(),
        }
    }
}

#[derive(Serialize)]
struct EventOut {
    action: &'static str,
    label: &'static str,
    on: bool,
}

#[derive(Serialize)]
struct GroupOut {
    key: &'static str,
    title: &'static str,
    events: Vec<EventOut>,
}

#[derive(Serialize)]
struct FeedOut {
    #[serde(flatten)]
    feed: Feed,
    groups: Vec<GroupOut>,
}

#[derive(Deserialize)]
struct FeedIn {
    op_id: Uuid,
    #[serde(flatten)]
    feed: Feed,
}

fn key(user_id: Uuid) -> String {
    format!("bot_feed:{user_id}")
}

fn out(feed: Feed) -> FeedOut {
    let groups = GROUPS
        .iter()
        .map(|(key, title)| GroupOut {
            key,
            title,
            events: EVENTS
                .iter()
                .filter(|(_, g, _)| g == key)
                .map(|(action, _, label)| EventOut {
                    action,
                    label,
                    on: !feed.off.iter().any(|o| o == action),
                })
                .collect(),
        })
        .collect();
    FeedOut { feed, groups }
}

/// Настройка ленты владельца; нет записи — по умолчанию (всё, кроме чеков).
async fn load(conn: &mut sqlx::PgConnection, user_id: Uuid) -> AppResult<Feed> {
    let v = sqlx::query_scalar!(
        r#"select s.value from settings s join users u on u.id = $1
           where s.branch_id = u.branch_id and s.key = $2"#,
        user_id,
        key(user_id)
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value::<Feed>(v).ok())
        .unwrap_or_default())
}

/// Проверка и порядок: известные события без повторов, режим чеков из списка.
fn normalize(mut feed: Feed) -> AppResult<Feed> {
    if !SALE_MODES.contains(&feed.sales.as_str()) {
        return Err(invalid(
            "режим чеков: каждый чек, только в долг и возвраты или не присылать",
        ));
    }
    if let Some(bad) = feed
        .off
        .iter()
        .find(|a| !EVENTS.iter().any(|(e, _, _)| e == a))
    {
        return Err(invalid(format!("неизвестное событие ленты: {bad}")));
    }
    feed.off = EVENTS
        .iter()
        .map(|(e, _, _)| *e)
        .filter(|e| feed.off.iter().any(|o| o == e))
        .map(str::to_string)
        .collect();
    Ok(feed)
}

async fn get_settings(
    State(state): State<AppState>,
    Owner(user): Owner,
) -> AppResult<Json<FeedOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(out(load(&mut conn, user.id).await?)))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<FeedIn>,
) -> AppResult<Json<Feed>> {
    ctx.user.require_owner()?;
    let mut tx = state.pool.begin().await?;
    let feed = save_tx(&mut tx, &ctx, req.op_id, req.feed).await?;
    tx.commit().await?;
    Ok(Json(feed))
}

/// Сохранить ленту вошедшего владельца одной транзакцией: настройка, журнал (I5), `op_id` (I8).
pub async fn save_tx(
    conn: &mut sqlx::PgConnection,
    ctx: &Ctx,
    op_id: Uuid,
    feed: Feed,
) -> AppResult<Feed> {
    ctx.user.require_owner()?;
    const KIND: &str = "settings.bot_feed";
    let feed = normalize(feed)?;
    if let Some(done) = ops::begin_op::<Feed>(conn, ctx, op_id, KIND).await? {
        return Ok(done);
    }
    let before = load(conn, ctx.user.id).await?;
    let value = serde_json::to_value(&feed).map_err(|e| AppError::Internal(e.to_string()))?;
    // Настройка своя у владельца и общая для всех филиалов — в его домашнем филиале.
    sqlx::query!(
        r#"insert into settings (branch_id, key, value)
           select u.branch_id, $2, $3 from users u where u.id = $1
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.id,
        key(ctx.user.id),
        value
    )
    .execute(&mut *conn)
    .await?;
    if before != feed {
        let turned = |from: &Feed, to: &Feed| -> Vec<String> {
            to.off
                .iter()
                .filter(|a| !from.off.contains(a))
                .cloned()
                .collect()
        };
        ops::audit(
            conn,
            ctx,
            KIND,
            "settings",
            None,
            json!({
                "off": turned(&before, &feed),
                "on": turned(&feed, &before),
                "sales_before": before.sales,
                "sales": feed.sales,
            }),
        )
        .await?;
    }
    ops::finish_op(conn, ctx, op_id, KIND, &feed).await?;
    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::notifications::WATCHED;
    use crate::api::telegram::FEED;

    #[test]
    fn every_bot_event_has_a_switch() {
        // Всё, что уходит владельцу в бот, — на экране настройки, кроме чеков и закрытия смены.
        for action in WATCHED.iter().chain(FEED.iter()) {
            let covered = EVENTS.iter().any(|(a, _, _)| a == action)
                || SALE_ACTIONS.contains(action)
                || ALWAYS.contains(action);
            assert!(covered, "{action} нет на экране «Уведомления в бот»");
        }
        for (action, group, _) in EVENTS {
            assert!(
                WATCHED.contains(&action) || FEED.contains(&action),
                "{action} не уходит в бот"
            );
            assert!(
                GROUPS.iter().any(|(g, _)| *g == group),
                "{action}: группа {group}"
            );
        }
        let mut all: Vec<&str> = EVENTS.iter().map(|(a, _, _)| *a).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), EVENTS.len(), "повтор события");
    }

    #[test]
    fn feed_is_checked_and_ordered() {
        let f = normalize(Feed {
            off: vec![
                "shift.open".into(),
                "receipt.post".into(),
                "shift.open".into(),
            ],
            sales: "debt_returns".into(),
        })
        .unwrap();
        assert_eq!(f.off, vec!["receipt.post", "shift.open"]);
        assert!(
            normalize(Feed {
                off: vec!["shift.close".into()],
                sales: "none".into()
            })
            .is_err()
        );
        assert!(
            normalize(Feed {
                off: vec![],
                sales: "some".into()
            })
            .is_err()
        );
        assert_eq!(Feed::default().sales, "none");
        let parsed: Feed = serde_json::from_value(json!({})).unwrap();
        assert_eq!(parsed, Feed::default());
    }
}
