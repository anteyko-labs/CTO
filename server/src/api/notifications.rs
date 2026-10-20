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
pub(crate) const WATCHED: [&str; 60] = [
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
    "oil.transfer_reverse",
    "sale.debt_without_inn",
    "auth.login_flood",
    "employee.create",
    "employee.update",
    "sale.offline_after_close",
    "vehicle.plate_conflict",
    "receipt.supplier_payment",
    "sale.offline_bypass",
    "sale.cost_unknown",
    "party.limit_change",
    "shift.reopen",
    "debt.reverse",
    "payout.reverse",
    "payroll.accrual_reverse",
    "payroll.rule",
    "payroll.rule_close",
    "settings.loyalty",
    "gift.rule_toggle",
    "backup.failed",
    "backup.size",
    "telegram.code_flood",
    "telegram.customer_ambiguous",
    "receipt.reverse_negative",
    "battery.intake_reverse",
    "settings.payment_fee",
    "telegram.employee_link",
    "defect.mark",
    "defect.unmark",
    "defect.resolve",
    "defect.resolve_reverse",
    "defect.reverse_negative",
    "shift.reclose_surplus",
    "cash.negative",
    "stock_in.post",
    "stock_in.reverse",
    "supplier_return.post",
    "supplier_return.reverse",
    "oil.transfer_admin",
    "employee.pin",
    "correction.post",
];

#[derive(Serialize)]
pub(crate) struct NotificationOut {
    id: Uuid,
    action: String,
    pub(crate) title: String,
    pub(crate) details: String,
    pub(crate) user_name: Option<String>,
    pub(crate) at: DateTime<Utc>,
    entity_id: Option<Uuid>,
    new: bool,
}

#[derive(Serialize)]
struct NotificationsOut {
    /// Новых событий (после «просмотрено») во всём журнале.
    unseen: i64,
    /// Всего событий — для страниц (SPEC-28 §2).
    total: i64,
    items: Vec<NotificationOut>,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

fn money(v: Option<i64>) -> String {
    v.map_or_else(|| "—".into(), crate::domain::money::format_som)
}

/// Процент из сотых долей: 200 → «2,00 %». Только показ, без дробных чисел.
fn percent(bp: i64) -> String {
    format!("{},{:02} %", bp / 100, (bp % 100).abs())
}

/// Ставка оплаты сотрудника словами: вид и сумма или процент.
fn pay_rule(data: &Value) -> String {
    let kind = match data.get("kind").and_then(Value::as_str).unwrap_or("") {
        "monthly_salary" => "оклад",
        "revenue_percent" => "% с чеков",
        "service_fee" => "за замену",
        "shift_fee" => "за смену",
        "per_service" => "за услугу",
        other => other,
    };
    let value = match (
        data.get("amount_tyiyn").and_then(Value::as_i64),
        data.get("rate_bp").and_then(Value::as_i64),
    ) {
        (_, Some(bp)) if bp != 0 => percent(bp),
        (Some(a), _) => crate::domain::money::format_som(a),
        _ => String::new(),
    };
    [kind.to_string(), value]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// « — комментарий» или ничего.
/// Товары оприходования словами: «Хомут × 3 шт, Шайба × 2 шт» (ADR-086); прежняя запись — один
/// товар в `name` и `qty_text`.
fn stock_in_goods(data: &Value) -> String {
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match data.get("lines").and_then(Value::as_array) {
        Some(lines) => lines
            .iter()
            .map(|l| format!("{} × {}", text(l, "name"), text(l, "qty_text")))
            .collect::<Vec<_>>()
            .join(", "),
        None => format!("{} × {}", text(data, "name"), text(data, "qty_text")),
    }
}

fn comment(data: &Value) -> String {
    data.get("comment")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| format!(" — {c}"))
        .unwrap_or_default()
}

pub(crate) fn describe(action: &str, data: &Value) -> (String, String) {
    let num = |key: &str| data.get(key).and_then(Value::as_i64);
    let text = |key: &str| {
        data.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
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
            // Цену поставили в приходе или оприходовании (SPEC-28 §1, ADR-086): откуда и с какой
            // наценкой.
            let source = num("receipt_number")
                .map(|n| format!("приход № {n}"))
                .or_else(|| num("stock_in_number").map(|n| format!("оприходование № {n}")));
            if let Some(source) = source {
                let markup = num("markup_permille")
                    .map(|m| {
                        format!(
                            ", наценка {}{},{} %",
                            if m < 0 { "−" } else { "" },
                            m.unsigned_abs() / 10,
                            m.unsigned_abs() % 10
                        )
                    })
                    .unwrap_or_default();
                parts.push(format!("{source}{markup}"));
            }
            (
                format!(
                    "Изменена цена: {}",
                    data.get("name").and_then(Value::as_str).unwrap_or("товар")
                ),
                parts.join("; "),
            )
        }
        // «Исправить» владельца (SPEC-29 §24): что исправлено и что поменялось.
        "correction.post" => {
            let no = |key: &str| num(key).map(|n| format!(" № {n}")).unwrap_or_default();
            let changes = data
                .get("diff")
                .and_then(Value::as_array)
                .map(|d| {
                    let mut parts: Vec<String> = d
                        .iter()
                        .take(5)
                        .map(|x| {
                            format!(
                                "{}: {} → {}",
                                x.get("what").and_then(Value::as_str).unwrap_or(""),
                                x.get("old").and_then(Value::as_str).unwrap_or(""),
                                x.get("new").and_then(Value::as_str).unwrap_or("")
                            )
                        })
                        .collect();
                    if d.len() > 5 {
                        parts.push(format!("и ещё {}", d.len() - 5));
                    }
                    if parts.is_empty() {
                        "без изменений".into()
                    } else {
                        parts.join("; ")
                    }
                })
                .unwrap_or_default();
            (
                format!(
                    "Исправлен документ: {}{} → {}",
                    text("label"),
                    no("original_number"),
                    if num("new_number").is_some() {
                        format!("№ {}", num("new_number").unwrap_or(0))
                    } else {
                        "новый".into()
                    }
                ),
                format!("{}{}", changes, comment(data)),
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
        "shift.reclose_surplus" => (
            format!(
                "После переоткрытия смены № {} излишек {} — удержание {} можно простить в «Расчёте»",
                num("number").unwrap_or(0),
                money(num("surplus")),
                money(num("withheld"))
            ),
            "удержание недостачи с кассира само не снимается: сторнируйте его, если недостачи не было"
                .into(),
        ),
        "defect.reverse_negative" => (
            format!(
                "Отмена решения по браку № {}: остаток ушёл в минус",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} — полученный взамен товар уже продан, остаток ушёл в минус — проверьте склад",
                data.get("lines")
                    .and_then(Value::as_array)
                    .map(|l| l
                        .iter()
                        .map(|x| format!(
                            "{}: {}",
                            x.get("name").and_then(Value::as_str).unwrap_or("товар"),
                            x.get("after").and_then(Value::as_i64).unwrap_or(0)
                        ))
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
        ),
        "receipt.reverse_negative" => (
            format!(
                "Сторно накладной № {}: остаток ушёл в минус",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} — товар уже продан, остаток ушёл в минус — проведите верный приход",
                data.get("lines")
                    .and_then(Value::as_array)
                    .map(|l| l
                        .iter()
                        .map(|x| format!(
                            "{}: {}",
                            x.get("name").and_then(Value::as_str).unwrap_or("товар"),
                            x.get("after").and_then(Value::as_i64).unwrap_or(0)
                        ))
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ),
        ),
        "battery.intake_reverse" => (
            format!(
                "Отменён приём аккумуляторов № {}",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} г, в кассу вернулось {}{}",
                num("grams").unwrap_or(0),
                money(num("amount")),
                comment(data)
            ),
        ),
        // Счёт ушёл ниже нуля — один раз на переход через ноль (ADR-077).
        "cash.negative" => (
            format!(
                "Счёт «{}» ушёл в минус: {} ({})",
                text("account"),
                money(num("balance")),
                text("doc")
            ),
            format!(
                "операция {}; деньги учтены по факту — пополните счёт или проверьте операцию",
                money(num("amount"))
            ),
        ),
        "settings.payment_fee" => (
            // Ставка — у безналичного счёта (ADR-077); прежние записи — по способу оплаты.
            match data.get("account").and_then(Value::as_str) {
                Some(a) => format!("Комиссия банка: «{a}»"),
                None => format!(
                    "Комиссия банка: {}",
                    if text("method") == "card" {
                        "карта"
                    } else {
                        "перевод"
                    }
                ),
            },
            format!(
                "{}стало {} с {}",
                data.get("before")
                    .and_then(|b| b.get("rate_bp"))
                    .and_then(Value::as_i64)
                    .map(|b| format!("было {}, ", percent(b)))
                    .unwrap_or_default(),
                data.get("after")
                    .and_then(|a| a.get("rate_bp"))
                    .and_then(Value::as_i64)
                    .map(percent)
                    .unwrap_or_default(),
                data.get("after")
                    .and_then(|a| a.get("active_from"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            ),
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
                // Право «Перелив масла» выдано или снято (ADR-082).
                {
                    let has = |k: &str| {
                        data.get(k)
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.iter().any(|p| p == "oil_transfer"))
                    };
                    match (has("permissions_before"), has("permissions")) {
                        (false, true) if data.get("permissions_before").is_some() => {
                            Some("выдано право «Перелив масла»".to_string())
                        }
                        (true, false) => Some("снято право «Перелив масла»".to_string()),
                        _ => None,
                    }
                },
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", "),
        ),
        "auth.login_flood" => (
            format!("Подбирают пароль: {}", text("login")),
            format!(
                "{} неверных паролей с разных адресов за {} мин. Вход не закрыт; если это не вы — смените пароль",
                num("failures").unwrap_or(0),
                num("minutes").unwrap_or(0)
            ),
        ),
        "employee.create" | "employee.update" => {
            let flag = |key: &str, label: &str| {
                data.get(key)
                    .and_then(Value::as_bool)
                    .filter(|v| *v)
                    .map(|_| label.to_string())
            };
            let details = [
                flag("is_cashier", "кассир"),
                flag("is_master", "мастер"),
                data.get("active")
                    .and_then(Value::as_bool)
                    .filter(|a| !a)
                    .map(|_| "отключён".to_string()),
                flag("pay_rules_added", "назначена оплата кассира по умолчанию"),
                flag("phone_changed", "изменён телефон (кабинет в боте)"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
            (
                format!(
                    "{}: {}",
                    if action == "employee.create" {
                        "Добавлен сотрудник"
                    } else {
                        "Изменён сотрудник"
                    },
                    text("full_name")
                ),
                details,
            )
        }
        "sale.offline_after_close" => (
            format!(
                "Чек без сети пришёл после пересчёта смены № {}",
                num("shift_number").unwrap_or(0)
            ),
            format!(
                "чек № {}, наличными {} — эти деньги легли в кассу позже и дадут расхождение при следующем пересчёте",
                num("number").unwrap_or(0),
                money(num("cash"))
            ),
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
                {
                    // Куда сдали и кассир (SPEC-29 §14); у прежних записей — «в сейф».
                    let to = text("to_name");
                    let cashier = text("cashier");
                    let handed = if to.is_empty() {
                        format!("в сейф {}", money(num("to_safe")))
                    } else {
                        format!("сдали кассу: {} → {to}", money(num("to_safe")))
                    };
                    format!(
                        "должно было быть {}, пересчитали {}{}; {handed}, в кассе осталось {}{}",
                        money(num("expected")),
                        money(num("counted")),
                        diff_text,
                        money(num("left")),
                        if cashier.is_empty() {
                            String::new()
                        } else {
                            format!(", кассир {cashier}")
                        }
                    )
                },
            )
        }
        "revision.post" => (
            format!("Проведена ревизия № {}", num("number").unwrap_or(0)),
            format!(
                "пересчитано товаров: {}; недостача {}, излишек {}",
                num("lines").unwrap_or(0),
                money(num("shortage")),
                money(num("surplus"))
            ) + &match num("over_norm") {
                Some(n) if n > 0 => format!("; масло сверх нормы: {n}"),
                _ => String::new(),
            },
        ),
        "sale.debt_without_inn" => (
            format!("Долг без ПИН/ИНН: чек № {}", num("number").unwrap_or(0)),
            format!(
                "проведён без связи на {} — допишите ПИН или ИНН в карточке клиента для документа",
                money(num("debt"))
            ),
        ),
        "oil.transfer_reverse" => (
            format!("Отменён перелив № {}", num("number").unwrap_or(0)),
            format!(
                "{} → {}, {} мл вернулись в источник; причина: {}",
                text("from_name"),
                text("to_name"),
                num("qty_ml").unwrap_or(0),
                text("comment")
            ),
        ),
        "vehicle.plate_conflict" => (
            format!("Номер {} записан на другого клиента", text("plate")),
            format!(
                "{} машину {} клиенту {}, а номер записан на {} — разберитесь, чья машина",
                if text("source") == "staff" {
                    "в карточке добавили"
                } else {
                    "в боте добавили"
                },
                text("plate"),
                text("client"),
                text("other")
            ),
        ),
        "receipt.supplier_payment" => (
            format!(
                "Оплата поставщику: накладная № {}",
                num("number").unwrap_or(0)
            ),
            // «Из своих» — только в записях до ADR-079.
            if data.get("outside").and_then(Value::as_bool) == Some(true) {
                format!("{} — из своих, мимо касс", money(num("amount")))
            } else {
                format!("{} из «{}»", money(num("amount")), text("account"))
            },
        ),
        "sale.offline_bypass" => (
            format!(
                "Чек без сети прошёл в обход проверок: № {}",
                num("number").unwrap_or(0)
            ),
            data.get("reasons")
                .and_then(Value::as_array)
                .map(|r| {
                    r.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .unwrap_or_default(),
        ),
        "sale.cost_unknown" => (
            format!(
                "Продажа без цены закупки: чек № {}",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} — укажите цену закупки на складе, иначе 2 % кассиру с этих строк не начислятся",
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
        "party.limit_change" => {
            let limit = |key: &str| {
                data.get(key).and_then(Value::as_i64).map_or_else(
                    || "без лимита".to_string(),
                    crate::domain::money::format_som,
                )
            };
            let days = |key: &str| {
                data.get(key)
                    .and_then(Value::as_i64)
                    .map_or_else(|| "без срока".to_string(), |d| format!("{d} дн."))
            };
            (
                format!("Изменён лимит долга: {}", text("name")),
                format!(
                    "лимит: было {}, стало {}; срок оплаты: было {}, стало {}",
                    limit("old_limit"),
                    limit("new_limit"),
                    days("old_days"),
                    days("new_days")
                ),
            )
        }
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
        "shift.reopen" => (
            "Смена переоткрыта".into(),
            format!("причина: {}", text("reason")),
        ),
        "debt.reverse" => (
            "Сторно записи долга".into(),
            format!("{}{}", money(num("amount_tyiyn")), comment(data)),
        ),
        "payout.reverse" => (
            "Сторно выплаты сотруднику".into(),
            format!("{}{}", money(num("amount_tyiyn")), comment(data)),
        ),
        "payroll.accrual_reverse" => (
            "Сторно начисления сотруднику".into(),
            format!(
                "{}{}",
                money(num("amount_tyiyn").map(i64::saturating_abs)),
                comment(data)
            ),
        ),
        "payroll.rule" => (
            "Изменена оплата сотрудника".into(),
            format!(
                "{}{}",
                pay_rule(data),
                if data.get("default").and_then(Value::as_bool) == Some(true) {
                    " (по умолчанию)"
                } else {
                    ""
                }
            ),
        ),
        "payroll.rule_close" => (
            "Закрыта ставка сотрудника".into(),
            format!("{}, действует по {}", pay_rule(data), text("active_to")),
        ),
        "settings.loyalty" => (
            "Изменён процент баллов".into(),
            format!(
                "начисление {} от оплаченного деньгами",
                percent(num("rate_bp").unwrap_or(0))
            ),
        ),
        "gift.rule_toggle" => (
            if data.get("active").and_then(Value::as_bool) == Some(true) {
                "Правило подарка включено".into()
            } else {
                "Правило подарка отключено".into()
            },
            String::new(),
        ),
        "backup.failed" => ("⚠️ Копия базы не создана".into(), text("reason")),
        "backup.size" => ("⚠️ Копии базы занимают много места".into(), text("reason")),
        "telegram.code_flood" => (
            "Подбирают код привязки Телеграма".into(),
            format!(
                "{} неверных кодов за {} мин — привязка к боту закрыта на 15 минут",
                num("failures").unwrap_or(0),
                num("minutes").unwrap_or(0)
            ),
        ),
        "telegram.customer_ambiguous" => (
            "Клиент не смог подключиться к боту".into(),
            format!(
                "номер {} записан в нескольких карточках: {} — оставьте номер в одной",
                text("phone"),
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
        "defect.mark" => (
            format!("В брак: {} × {}", text("name"), text("qty_text")),
            format!(
                "{} — {}{}",
                money(num("cost")),
                text("reason"),
                data.get("supplier")
                    .and_then(Value::as_str)
                    .map(|s| format!("; поставщик {s}"))
                    .unwrap_or_default()
            ),
        ),
        "defect.unmark" => (
            format!(
                "Брак № {} снят: товар вернулся в продажу",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} × {}, {}{}",
                text("name"),
                text("qty_text"),
                money(num("cost")),
                comment(data)
            ),
        ),
        "defect.resolve" => (
            format!(
                "Брак № {}: {}",
                num("number").unwrap_or(0),
                crate::api::defects::kind_words(&text("kind"))
            ),
            {
                let mut parts = vec![format!("{} × {}", text("name"), text("qty_text"))];
                if let Some(q) = data.get("replace_qty_text").and_then(Value::as_str) {
                    parts.push(format!("взамен {q}"));
                }
                if let Some(r) = num("refund") {
                    parts.push(format!("возмещение {}", money(Some(r))));
                }
                match num("writeoff") {
                    Some(w) if w != 0 => parts.push(format!("в убыток {}", money(Some(w)))),
                    _ => {}
                }
                match num("diff") {
                    Some(d) if d != 0 => parts.push(format!(
                        "разница {}",
                        crate::domain::money::format_som_signed(d)
                    )),
                    _ => {}
                }
                parts.join(", ") + &comment(data)
            },
        ),
        "defect.resolve_reverse" => (
            format!("Отменено решение по браку № {}", num("number").unwrap_or(0)),
            format!("{}{}", text("name"), comment(data)),
        ),
        // Перелив администратора с правом (ADR-082): владелец узнаёт о каждом.
        "oil.transfer_admin" => (
            format!(
                "Перелив № {} провёл администратор",
                num("number").unwrap_or(0)
            ),
            format!(
                "{} → {}, {} мл, себестоимость {}",
                text("from_name"),
                text("to_name"),
                num("qty_ml").unwrap_or(0),
                money(num("value"))
            ),
        ),
        "employee.pin" => (
            if data.get("set").and_then(Value::as_bool).unwrap_or(false) {
                format!("Задан PIN кассира: {}", text("full_name"))
            } else {
                format!("Снят PIN кассира: {}", text("full_name"))
            },
            String::new(),
        ),
        // Оприходование — несколько строк (ADR-086); у прежних записей журнала товар один, в
        // `name` и `qty_text`.
        "stock_in.post" => {
            let lines = stock_in_goods(data);
            let price = match data.get("lines").and_then(Value::as_array) {
                Some(l) if l.len() == 1 => l[0].get("price").and_then(Value::as_i64),
                Some(_) => None,
                None => num("price"),
            };
            (
                format!(
                    "Оприходовано без поставщика № {}: {lines}",
                    num("number").unwrap_or(0)
                ),
                match price {
                    Some(p) => format!(
                        "по цене закупки {}, на {}{}",
                        money(Some(p)),
                        money(num("cost")),
                        comment(data)
                    ),
                    None => format!("на {}{}", money(num("cost")), comment(data)),
                },
            )
        }
        "stock_in.reverse" => (
            format!(
                "Сторно оприходования № {}: {}",
                num("number").unwrap_or(0),
                stock_in_goods(data)
            ),
            // В минус: у прежних записей — остаток числом, у новых — товары словами.
            match data.get("negative").and_then(|v| {
                v.as_i64()
                    .map(|n| n.to_string())
                    .or_else(|| v.as_str().map(str::to_string))
            }) {
                Some(after) => format!(
                    "{}{} — товар уже продан, остаток ушёл в минус: {after}",
                    money(num("cost")),
                    comment(data)
                ),
                None => format!("{}{}", money(num("cost")), comment(data)),
            },
        ),
        "supplier_return.post" => (
            format!(
                "Возврат поставщику № {}: {}",
                num("number").unwrap_or(0),
                text("supplier")
            ),
            {
                let goods = data
                    .get("lines")
                    .and_then(Value::as_array)
                    .map(|l| {
                        l.iter()
                            .map(|x| {
                                format!(
                                    "{} × {}",
                                    x.get("name").and_then(Value::as_str).unwrap_or("товар"),
                                    x.get("qty_text").and_then(Value::as_str).unwrap_or("")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                let how = if text("settle") == "money" {
                    "деньгами"
                } else {
                    "в счёт долга"
                };
                let diff = match num("diff") {
                    Some(d) if d != 0 => format!(
                        ", разница {}",
                        crate::domain::money::format_som_signed(d)
                    ),
                    _ => String::new(),
                };
                format!(
                    "{goods}; {} {how}{diff}{}",
                    money(num("amount")),
                    comment(data)
                )
            },
        ),
        "supplier_return.reverse" => (
            format!(
                "Сторно возврата поставщику № {}: {}",
                num("number").unwrap_or(0),
                text("supplier")
            ),
            format!("{}{}", money(num("amount")), comment(data)),
        ),
        "telegram.employee_link" => (
            format!("Работник {} подключился к боту", text("name")),
            "видит в боте свой заработок, удержания и премии".into(),
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
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let offset = q.offset.unwrap_or(0).max(0);
    let mut conn = state.pool.acquire().await?;
    let items = page(&mut conn, user.branch_id, limit, offset, seen).await?;
    let watched: Vec<String> = WATCHED.iter().map(|s| (*s).to_string()).collect();
    let c = sqlx::query!(
        r#"select count(*) as "total!",
                  count(*) filter (where $3::timestamptz is null or a.created_at > $3) as "unseen!"
           from audit_log a
           where a.branch_id = $1 and a.action = any($2)"#,
        user.branch_id,
        &watched,
        seen
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Json(NotificationsOut {
        unseen: c.unseen,
        total: c.total,
        items,
    }))
}

/// Последние важные события филиала: для экрана и для вопросов боту (SPEC-21).
pub(crate) async fn latest(
    conn: &mut sqlx::PgConnection,
    branch_id: Uuid,
    limit: i64,
    seen: Option<DateTime<Utc>>,
) -> AppResult<Vec<NotificationOut>> {
    page(conn, branch_id, limit, 0, seen).await
}

/// Страница событий, новые сверху (SPEC-28 §2).
async fn page(
    conn: &mut sqlx::PgConnection,
    branch_id: Uuid,
    limit: i64,
    offset: i64,
    seen: Option<DateTime<Utc>>,
) -> AppResult<Vec<NotificationOut>> {
    let watched: Vec<String> = WATCHED.iter().map(|s| (*s).to_string()).collect();
    let rows = sqlx::query!(
        r#"select a.id, a.action, a.data, a.entity_id, a.created_at, u.full_name as "user_name?"
           from audit_log a
           left join users u on u.id = a.user_id
           where a.branch_id = $1 and a.action = any($2)
           order by a.created_at desc, a.id desc
           limit $3 offset $4"#,
        branch_id,
        &watched,
        limit,
        offset
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let (title, mut details) = describe(&r.action, &r.data);
            // Проведено кнопкой в Телеграм-боте (SPEC-24) — видно и на экране.
            if r.data.get("source").and_then(Value::as_str) == Some("bot") {
                details = if details.is_empty() {
                    "через бот".into()
                } else {
                    format!("{details} · через бот")
                };
            }
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
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_events_are_described_in_words() {
        let (title, text) = describe(
            "receipt.reverse_negative",
            &json!({ "number": 7, "lines": [{ "name": "Фильтр", "after": -3 }] }),
        );
        assert_eq!(title, "Сторно накладной № 7: остаток ушёл в минус");
        assert!(text.contains("Фильтр: -3"), "{text}");
        assert!(text.contains("проведите верный приход"), "{text}");
        let (title, text) = describe(
            "battery.intake_reverse",
            &json!({ "number": 3, "grams": 4000, "amount": 40000, "comment": "ошиблись" }),
        );
        assert_eq!(title, "Отменён приём аккумуляторов № 3");
        assert!(
            text.contains("4000 г") && text.contains("ошиблись"),
            "{text}"
        );
        let (_, text) = describe(
            "settings.payment_fee",
            &json!({ "method": "card", "before": { "rate_bp": 50 }, "after": { "rate_bp": 100, "active_from": "2026-10-20" } }),
        );
        assert_eq!(text, "было 0,50 %, стало 1,00 % с 2026-10-20");
        let (title, _) = describe(
            "settings.payment_fee",
            &json!({ "account": "Оптима", "after": { "rate_bp": 50, "active_from": "2026-10-20" } }),
        );
        assert_eq!(title, "Комиссия банка: «Оптима»");
        let (title, text) = describe(
            "cash.negative",
            &json!({ "account": "Касса", "balance": -120_000, "amount": -500_000, "doc": "расход № 15" }),
        );
        assert_eq!(title, "Счёт «Касса» ушёл в минус: −1 200 с (расход № 15)");
        assert!(text.contains("−5 000 с"), "{text}");
        for action in WATCHED {
            assert_ne!(
                describe(action, &json!({})).0,
                action,
                "{action} без описания"
            );
        }
    }
}
