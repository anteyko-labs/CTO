//! Отчёты владельца для бота (SPEC-24): те же функции, что отдают экраны программы, только
//! готовым русским текстом. Суммы — через `format_som` (округление в одном месте, инвариант 1);
//! длинные списки — первые N строк и «и ещё N». Только чтение, только филиал владельца.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use sqlx::PgConnection;

use crate::api::bot_drafts::{find_party, norm, qty_text, sounds_like};
use crate::api::catalog::{ProductFilter, query_products};
use crate::auth::CurrentUser;
use crate::domain::money::{format_som, format_som_signed};
use crate::error::{AppResult, invalid};

/// Строк в одном списке — не больше; остальное — «и ещё N».
const TOP: usize = 30;

/// Бишкек — UTC+6 круглый год.
fn local(t: DateTime<Utc>) -> String {
    (t + chrono::Duration::hours(6))
        .format("%d.%m.%Y %H:%M")
        .to_string()
}

fn day(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

fn period_label(from: NaiveDate, to: NaiveDate) -> String {
    if from == to {
        format!("за {}", day(from))
    } else {
        format!("с {} по {}", day(from), day(to))
    }
}

/// Первые `TOP` строк и «и ещё N».
fn capped(lines: &mut Vec<String>, items: Vec<String>) {
    let total = items.len();
    lines.extend(items.into_iter().take(TOP));
    if total > TOP {
        lines.push(format!("и ещё {}", total - TOP));
    }
}

/// Вид движения денег словами — как в «💵 Смена».
fn movement_kind(kind: &str) -> &str {
    match kind {
        "sale" => "продажа наличными",
        "sale_return" => "возврат",
        "cash_in" => "внесение",
        "cash_out" => "изъятие",
        "transfer_in" => "перемещение в кассу",
        "transfer_out" => "перемещение из кассы",
        "expense" => "расход",
        "payout" => "выплата сотруднику",
        "supplier_payment" => "оплата поставщику",
        "supplier_refund" => "возмещение поставщика за брак",
        "debt_repayment" => "погашение долга",
        "battery_intake" => "приём аккумуляторов",
        "count_diff" => "пересчёт",
        "reversal" => "сторно",
        "bank_fee" => "комиссия банка",
        "card" => "оплата картой",
        "transfer" => "оплата переводом",
        other => other,
    }
}

// ---------- Продажи ----------

/// Продажи: `dashboard`, `categories`, `hours`, `weekdays`, `checks`, `gifts`, `staff`, `top`,
/// `top_profit`.
pub async fn sales(
    conn: &mut PgConnection,
    user: &CurrentUser,
    report: &str,
    (from, to): (NaiveDate, NaiveDate),
) -> AppResult<String> {
    match report {
        "dashboard" => dashboard(conn, user, from, to).await,
        "categories" => {
            let p = crate::api::reports::profit_data(conn, user, Some(from), Some(to)).await?;
            let mut lines = vec![format!(
                "Продажи по категориям {}: выручка {}, валовая прибыль {}",
                period_label(p.from, p.to),
                format_som(p.totals.goods_tyiyn.saturating_add(p.totals.services_tyiyn)),
                format_som(p.totals.gross_tyiyn)
            )];
            if p.categories.is_empty() {
                lines.push("Продаж товаров нет.".into());
            }
            capped(
                &mut lines,
                p.categories
                    .iter()
                    .map(|c| {
                        format!(
                            "— {}: выручка {}, закупка {}, валовая {}",
                            c.name,
                            format_som(c.revenue_tyiyn),
                            format_som(c.cost_tyiyn),
                            format_som(c.gross_tyiyn)
                        )
                    })
                    .collect(),
            );
            for w in p.warnings {
                lines.push(format!("Внимание: {w}"));
            }
            Ok(lines.join("\n"))
        }
        "hours" => {
            let p = crate::api::reports::profit_data(conn, user, Some(to), Some(to)).await?;
            let mut lines = vec![format!("Продажи по часам {}:", period_label(p.from, p.to))];
            if p.hours.is_empty() {
                lines.push("Продаж нет.".into());
            }
            for h in p.hours {
                lines.push(format!(
                    "{:02}:00–{:02}:00: выручка {}, валовая {}, чеков {}",
                    h.hour,
                    h.hour + 1,
                    format_som(h.revenue_tyiyn),
                    format_som(h.gross_tyiyn),
                    h.sales_count
                ));
            }
            Ok(lines.join("\n"))
        }
        "weekdays" => {
            let mut cells = crate::api::reports::heatmap_data(conn, user, Some(28), None).await?;
            cells.sort_by_key(|c| std::cmp::Reverse(c.revenue_tyiyn));
            const DAYS: [&str; 7] = ["пн", "вт", "ср", "чт", "пт", "сб", "вс"];
            let mut lines = vec![
                "Когда идут продажи за последние 28 дней (сумма чеков с возвратами, самые сильные часы сверху):"
                    .to_string(),
            ];
            if cells.is_empty() {
                lines.push("Продаж нет.".into());
            }
            capped(
                &mut lines,
                cells
                    .iter()
                    .map(|c| {
                        format!(
                            "{} {:02}:00–{:02}:00: {}, чеков {}",
                            DAYS.get(usize::try_from(c.dow - 1).unwrap_or(0))
                                .copied()
                                .unwrap_or(""),
                            c.hour,
                            c.hour + 1,
                            format_som(c.revenue_tyiyn),
                            c.sales_count
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "checks" => {
            let d = crate::api::sales::sales_day(conn, user, Some(to)).await?;
            let t = &d.totals;
            let mut lines = vec![format!(
                "Чеки {}: продаж {}, итог с возвратами {} (наличными {}, картой {}, переводом {}, в долг {}, баллами {}{})",
                period_label(d.date, d.date),
                t.count,
                format_som(t.total_tyiyn),
                format_som(t.cash_tyiyn),
                format_som(t.card_tyiyn),
                format_som(t.transfer_tyiyn),
                format_som(t.debt_tyiyn),
                format_som(t.bonus_tyiyn),
                if t.battery_tyiyn != 0 {
                    format!(", аккумуляторами {}", format_som(t.battery_tyiyn))
                } else {
                    String::new()
                }
            )];
            capped(
                &mut lines,
                d.sales
                    .iter()
                    .map(|s| {
                        let mut parts = vec![format!(
                            "№ {} {} — {}",
                            s.number,
                            (s.created_at + chrono::Duration::hours(6)).format("%H:%M"),
                            format_som(s.total_tyiyn)
                        )];
                        if s.kind == "return" {
                            parts.push("возврат".into());
                        }
                        parts.push(format!("кассир {}", s.cashier_name));
                        if let Some(p) = &s.party_name {
                            parts.push(p.clone());
                        }
                        if s.debt_tyiyn != 0 {
                            parts.push(format!("в долг {}", format_som(s.debt_tyiyn)));
                        }
                        parts.join(", ")
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "gifts" => {
            let rows = crate::api::reports::gifts_data(
                conn,
                user,
                crate::api::reports::GiftsQuery { from, to },
            )
            .await?;
            if rows.is_empty() {
                return Ok(format!("Подарков {} не было.", period_label(from, to)));
            }
            let total = rows
                .iter()
                .fold(0i64, |acc, r| acc.saturating_add(r.cost_tyiyn));
            let mut lines = vec![format!(
                "Подарки {}: обошлись в {}",
                period_label(from, to),
                format_som(total)
            )];
            capped(
                &mut lines,
                rows.iter()
                    .map(|r| {
                        format!(
                            "— {}: {}, чеков {}, себестоимость {}",
                            r.name,
                            qty_text(r.qty, &r.unit),
                            r.checks,
                            format_som(r.cost_tyiyn)
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "staff" => {
            let p = crate::api::reports::profit_data(conn, user, Some(from), Some(to)).await?;
            Ok(format!(
                "Кто сколько заработал {}:\n{}",
                period_label(p.from, p.to),
                crate::api::telegram::staff_lines(&p.staff)
            ))
        }
        // «Чаще берут» — по выручке, «больше приносят» — по валовой прибыли (SPEC-28 §4).
        "top" | "top_profit" => {
            let by_profit = report == "top_profit";
            let t = crate::api::reports::top_products_data(
                conn,
                user,
                crate::api::reports::TopQuery {
                    from: Some(from),
                    to: Some(to),
                    limit: Some(30),
                    sort: by_profit.then(|| "profit".to_string()),
                    ..Default::default()
                },
            )
            .await?;
            if t.items.is_empty() {
                return Ok(format!(
                    "Продаж товаров {} не было.",
                    period_label(t.from, t.to)
                ));
            }
            let mut lines = vec![if by_profit {
                format!(
                    "Больше всех приносят {} (по валовой прибыли):",
                    period_label(t.from, t.to)
                )
            } else {
                format!(
                    "Самые продаваемые товары {} (по выручке):",
                    period_label(t.from, t.to)
                )
            }];
            capped(
                &mut lines,
                t.items
                    .iter()
                    .map(|r| {
                        format!(
                            "— {}: {}, выручка {}, прибыль {}, чеков {}",
                            r.name,
                            crate::domain::qty::stock_text(r.qty, &r.unit, r.container_ml),
                            format_som(r.amount_tyiyn),
                            r.profit_tyiyn.map_or_else(|| "—".to_string(), format_som),
                            r.checks
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        _ => Err(invalid(
            "report: dashboard, categories, hours, weekdays, checks, gifts, staff, top или top_profit",
        )),
    }
}

async fn dashboard(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<String> {
    let d = crate::api::reports::dashboard_data(conn, user, Some(from), Some(to)).await?;
    let t = &d.totals;
    let head = if d.from == d.to && d.to == d.today {
        format!("Сводка {} (на сейчас)", period_label(d.from, d.to))
    } else {
        format!("Сводка {}", period_label(d.from, d.to))
    };
    let mut lines = vec![
        head,
        format!(
            "Чеков: {}, выручка {} (товары {}, работы {}), валовая прибыль {}, чистая прибыль {}",
            t.sales_count,
            format_som(t.goods_tyiyn.saturating_add(t.services_tyiyn)),
            format_som(t.goods_tyiyn),
            format_som(t.services_tyiyn),
            format_som(t.gross_tyiyn),
            format_som(t.net_tyiyn)
        ),
        format!(
            "Средний чек: {}, возвраты: {}",
            format_som(d.average_check_tyiyn),
            format_som(d.returns_tyiyn)
        ),
        format!(
            "Безнал чеков: картой {}, переводом {}{}",
            format_som(d.noncash.total.card_tyiyn),
            format_som(d.noncash.total.transfer_tyiyn),
            // Несколько безналичных счетов — по каждому, куда пришло (ADR-076).
            if d.noncash.by_account.len() > 1 {
                format!(
                    " ({})",
                    d.noncash
                        .by_account
                        .iter()
                        .map(|a| format!("{}: {}", a.name, format_som(a.total_tyiyn)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            } else {
                String::new()
            }
        ),
    ];
    if let Some(c) = &d.compare {
        let when = if c.until_now {
            "К этому часу: сегодня"
        } else {
            "За день: этот день"
        };
        let prev = if c.until_now {
            "вчера"
        } else {
            "накануне"
        };
        lines.push(format!(
            "{when} {} ({} чеков), {prev} {} ({}), неделю назад {} ({})",
            format_som(c.today.revenue_tyiyn),
            c.today.sales_count,
            format_som(c.yesterday.revenue_tyiyn),
            c.yesterday.sales_count,
            format_som(c.week_ago.revenue_tyiyn),
            c.week_ago.sales_count
        ));
    }
    if let Some(p) = &d.previous {
        lines.push(format!(
            "Предыдущий период {}: выручка {}, чистая прибыль {}, чеков {}",
            period_label(p.from, p.to),
            format_som(p.revenue_tyiyn),
            format_som(p.net_tyiyn),
            p.sales_count
        ));
    }
    lines.extend([
        match &d.shift_cashier {
            Some(c) if d.shift_open => format!("Смена открыта, кассир {c}"),
            _ => "Смена не открыта".to_string(),
        },
        format!("Деньги всего: {}", format_som(d.money_total_tyiyn)),
    ]);
    for a in &d.accounts {
        lines.push(format!("— {}: {}", a.name, format_som(a.balance_tyiyn)));
    }
    lines.push(format!(
        "Сотрудникам к выдаче: {}",
        format_som(d.to_pay_tyiyn)
    ));
    lines.push(format!(
        "Должны нам: {}, мы должны: {}",
        format_som(d.debts_in_tyiyn),
        format_som(d.debts_out_tyiyn)
    ));
    lines.push(format!(
        "Склад: кончается {}, на проверку себестоимости {}, залежалых {}",
        d.low_stock, d.needs_review, d.stale_stock
    ));
    Ok(lines.join("\n"))
}

// ---------- Деньги, смены, расходы ----------

/// Деньги: `movements` (касса по названию), `shifts`, `shift` (номер), `expenses_list`.
pub async fn money(
    conn: &mut PgConnection,
    user: &CurrentUser,
    report: &str,
    account: Option<&str>,
    number: Option<i64>,
    (from, to): (NaiveDate, NaiveDate),
) -> AppResult<String> {
    match report {
        "movements" => {
            let all = crate::api::cash::accounts_data(conn, user).await?;
            let q = account.unwrap_or("Касса");
            let acc = match find_account(&all, q) {
                Ok(a) => a,
                Err(text) => return Ok(text),
            };
            let rows = crate::api::cash::movements_data(conn, user, acc.id).await?;
            let mut lines = vec![format!(
                "{}: сейчас {}. Последние движения (новые сверху):",
                acc.name,
                format_som(acc.balance_tyiyn)
            )];
            if rows.is_empty() {
                lines.push("Движений нет.".into());
            }
            capped(
                &mut lines,
                rows.iter()
                    .map(|m| {
                        let note = if m.comment.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", m.comment.chars().take(80).collect::<String>())
                        };
                        format!(
                            "{} {}: {}{note} ({})",
                            local(m.created_at),
                            movement_kind(&m.kind),
                            format_som_signed(m.amount_tyiyn),
                            m.user_name
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "shifts" => {
            let list = crate::api::cash::shifts_data(conn, user, Some(from), Some(to)).await?;
            let mut lines = vec![format!("Смены {}:", period_label(from, to))];
            if list.is_empty() {
                lines.push("Смен нет.".into());
            }
            capped(
                &mut lines,
                list.iter()
                    .map(|s| {
                        let state = match (s.counted_tyiyn, s.diff_tyiyn) {
                            (Some(c), Some(d)) => format!(
                                "закрыта: должно было быть {}, пересчитано {}, расхождение {}{}",
                                format_som(s.expected_tyiyn),
                                format_som(c),
                                format_som_signed(d),
                                s.to_safe_tyiyn
                                    .map(|v| format!(", в сейф {}", format_som(v)))
                                    .unwrap_or_default()
                            ),
                            _ => format!(
                                "открыта, в кассе должно быть {}",
                                format_som(s.expected_tyiyn)
                            ),
                        };
                        format!(
                            "№ {} за {}, кассир {}: чеков {} на {}; {state}",
                            s.number,
                            day(s.business_date),
                            s.cashier_name,
                            s.sales_count,
                            format_som(s.sales_total_tyiyn)
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "shift" => {
            let Some(n) = number else {
                return Err(invalid("нужен номер смены (поле number)"));
            };
            let id = sqlx::query_scalar!(
                "select id from shifts where branch_id = $1 and number = $2",
                user.branch_id,
                n
            )
            .fetch_optional(&mut *conn)
            .await?;
            let Some(id) = id else {
                return Ok(format!("Смены № {n} нет."));
            };
            let s = crate::api::cash::load_shift(conn, user.branch_id, id).await?;
            let mut lines = vec![
                format!(
                    "Смена № {} за {}, кассир {}, открыл {} в {}",
                    s.number,
                    day(s.business_date),
                    s.cashier_name,
                    s.opened_by,
                    local(s.opened_at)
                ),
                format!(
                    "На начало (размен): {}",
                    format_som(s.opening_expected_tyiyn)
                ),
            ];
            if let Some(rows) = s.breakdown.as_array() {
                for r in rows {
                    let kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
                    let sum = r.get("sum_tyiyn").and_then(Value::as_i64).unwrap_or(0);
                    if sum != 0 {
                        lines.push(format!(
                            "— {}: {}",
                            movement_kind(kind),
                            format_som_signed(sum)
                        ));
                    }
                }
            }
            lines.push(format!(
                "Чеков {} на {}, возвраты {}; наличными {}, картой {}, переводом {}, в долг {}, баллами {}, комиссия банка {}",
                s.sales_count,
                format_som(s.sales_total_tyiyn),
                format_som(s.returns_tyiyn),
                format_som(s.cash_sales_tyiyn),
                format_som(s.card_tyiyn),
                format_som(s.transfer_tyiyn),
                format_som(s.debt_tyiyn),
                format_som(s.bonus_tyiyn),
                format_som(s.bank_fee_tyiyn)
            ));
            match (s.closed_at, s.counted_tyiyn, s.diff_tyiyn) {
                (Some(at), Some(c), Some(d)) => {
                    lines.push(format!(
                        "Закрыта {}: должно было быть {}, пересчитано {}, расхождение {}",
                        local(at),
                        format_som(s.expected_tyiyn),
                        format_som(c),
                        format_som_signed(d)
                    ));
                    match (s.to_safe_tyiyn, s.left_tyiyn) {
                        (Some(safe), Some(left)) => lines.push(format!(
                            "В сейф {}, оставлено на размен {}",
                            format_som(safe),
                            format_som(left)
                        )),
                        _ => lines.push("Что сделали с деньгами, ещё не отмечено.".into()),
                    }
                }
                _ => lines.push(format!(
                    "Открыта, в кассе должно быть {}",
                    format_som(s.expected_tyiyn)
                )),
            }
            lines.push(format!(
                "Заработали:\n{}",
                crate::api::telegram::staff_lines(&s.pay)
            ));
            Ok(lines.join("\n"))
        }
        "expenses_list" => {
            let rows = crate::api::expenses::list_data(
                conn,
                user,
                crate::api::expenses::ListQuery {
                    from: Some(from),
                    to: Some(to),
                    article_id: None,
                },
            )
            .await?;
            let total = rows
                .iter()
                .fold(0i64, |acc, r| acc.saturating_add(r.amount_tyiyn));
            let mut lines = vec![format!(
                "Расходы {} по документам: итого {} ({} записей)",
                period_label(from, to),
                format_som(total),
                rows.len()
            )];
            capped(
                &mut lines,
                rows.iter()
                    .map(|e| {
                        let from = match (&e.account_name, e.source.as_str()) {
                            (_, "outside") => "из своих".to_string(),
                            (Some(a), _) => format!("из «{a}»"),
                            _ => String::new(),
                        };
                        let note = if e.comment.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", e.comment.chars().take(80).collect::<String>())
                        };
                        let mark = if e.reversal_of.is_some() {
                            " (сторно)"
                        } else if e.reversed {
                            " (сторнирован)"
                        } else {
                            ""
                        };
                        format!(
                            "№ {} за {}, {}: {} {from}{mark}{note}",
                            e.number,
                            day(e.expense_date),
                            e.article_name,
                            format_som_signed(e.amount_tyiyn)
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "bank" => {
            // Безналичный счёт по названию; без него — по умолчанию для карты (ADR-077).
            let id = match account {
                None => None,
                Some(q) => {
                    let banks: Vec<_> = crate::api::cash::accounts_list(conn, user, true)
                        .await?
                        .into_iter()
                        .filter(|a| a.kind == "bank")
                        .collect();
                    match find_account(&banks, q) {
                        Ok(a) => Some(a.id),
                        Err(text) => return Ok(text),
                    }
                }
            };
            bank_text(conn, user, from, to, id).await
        }
        "accounts" => accounts_text(conn, user).await,
        "income" => income_text(conn, user, from, to).await,
        _ => Err(invalid(
            "report: movements, shifts, shift, expenses_list, bank, accounts или income",
        )),
    }
}

/// Способ оплаты словами — как в отчёте «Куда сколько поступило».
fn method_name(method: &str) -> &str {
    match method {
        "cash" => "наличными",
        "card" => "картой",
        "transfer" => "переводом",
        "debt" => "в долг",
        "bonus" => "баллами",
        "battery" => "аккумуляторами",
        other => other,
    }
}

/// Касса или счёт по названию: точное совпадение или похожее; иначе — текст ответа.
fn find_account<'a>(
    all: &'a [crate::api::cash::AccountOut],
    q: &str,
) -> Result<&'a crate::api::cash::AccountOut, String> {
    let wanted = norm(q);
    let exact = all.iter().any(|a| norm(&a.name) == wanted);
    let found: Vec<_> = all
        .iter()
        .filter(|a| {
            if exact {
                norm(&a.name) == wanted
            } else {
                sounds_like(&a.name, q)
            }
        })
        .collect();
    let names = |list: &[&crate::api::cash::AccountOut]| {
        list.iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!(
            "Касса «{q}» не найдена. Кассы: {}.",
            names(&all.iter().collect::<Vec<_>>())
        )),
        many => Err(format!(
            "По «{q}» подходит несколько касс: {}. Уточните.",
            names(many)
        )),
    }
}

/// «Обзор счетов» (SPEC-28 §5) текстом: все счета с типом, остатком и признаками, итог по
/// наличным и безналу. Владелец видит все, и архивные — отдельно.
async fn accounts_text(conn: &mut PgConnection, user: &CurrentUser) -> AppResult<String> {
    let all = crate::api::cash::accounts_list(conn, user, true).await?;
    let (mut cash, mut bank) = (0i64, 0i64);
    let mut lines = vec!["Обзор счетов:".to_string()];
    for a in all.iter().filter(|a| a.active) {
        if a.kind == "bank" {
            bank = bank.saturating_add(a.balance_tyiyn);
        } else {
            cash = cash.saturating_add(a.balance_tyiyn);
        }
        let mut marks = vec![if a.kind == "bank" {
            "безналичный"
        } else {
            "наличный"
        }];
        if a.is_default && a.kind == "register" {
            marks.push("касса смены");
        }
        // Счета по умолчанию для карты и перевода и ставка банка (ADR-077).
        if a.default_card {
            marks.push("по умолчанию для карты");
        }
        if a.default_transfer {
            marks.push("по умолчанию для перевода");
        }
        let fee = (a.kind == "bank" && a.fee_rate_bp > 0).then(|| {
            format!(
                "комиссия {},{:02} %",
                a.fee_rate_bp / 100,
                a.fee_rate_bp % 100
            )
        });
        if let Some(f) = &fee {
            marks.push(f);
        }
        if !a.allow_negative {
            marks.push("минус не разрешён");
        }
        if a.owner_only {
            marks.push("только владелец");
        }
        lines.push(format!(
            "— {}: {} ({})",
            a.name,
            format_som(a.balance_tyiyn),
            marks.join(", ")
        ));
    }
    lines.push(format!(
        "Итого: наличные {} · безнал {} · всего {}",
        format_som(cash),
        format_som(bank),
        format_som(cash.saturating_add(bank))
    ));
    let archived: Vec<_> = all
        .iter()
        .filter(|a| !a.active)
        .map(|a| a.name.as_str())
        .collect();
    if !archived.is_empty() {
        lines.push(format!("В архиве: {}", archived.join(", ")));
    }
    Ok(lines.join("\n"))
}

/// Экран «Счёт» (SPEC-27 §6) текстом: то же `bank_data`.
async fn bank_text(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
    account: Option<uuid::Uuid>,
) -> AppResult<String> {
    let b = crate::api::income::bank_data(conn, user, Some(from), Some(to), account).await?;
    let mut lines = vec![
        format!(
            "{} {}: на начало {}, на конец {}, сейчас {}",
            b.name,
            period_label(b.from, b.to),
            format_som(b.opening_tyiyn),
            format_som(b.closing_tyiyn),
            format_som(b.balance_tyiyn)
        ),
        format!(
            "Картой: {} (чеки {}, погашения долга {})",
            format_som(b.card.total_tyiyn),
            format_som(b.card.sales_tyiyn),
            format_som(b.card.repayments_tyiyn)
        ),
        format!(
            "Переводом: {} (чеки {}, погашения долга {})",
            format_som(b.transfer.total_tyiyn),
            format_som(b.transfer.sales_tyiyn),
            format_som(b.transfer.repayments_tyiyn)
        ),
        format!("Комиссия банка: {}", format_som(b.fee_tyiyn)),
        format!(
            "Прочие поступления: {}, списания: {}",
            format_som(b.other_in_tyiyn),
            format_som(b.outgoing_tyiyn)
        ),
    ];
    if b.days.len() > 1 {
        lines.push("По дням:".into());
        capped(
            &mut lines,
            b.days
                .iter()
                .map(|d| {
                    format!(
                        "{}: картой {}, переводом {}, комиссия {}, итог {}",
                        day(d.date),
                        format_som(d.card_tyiyn),
                        format_som(d.transfer_tyiyn),
                        format_som(d.fee_tyiyn),
                        format_som_signed(d.net_tyiyn)
                    )
                })
                .collect(),
        );
    }
    Ok(lines.join("\n"))
}

/// «Куда сколько поступило» (SPEC-27 §6) текстом: то же `income_data`.
async fn income_text(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<String> {
    let r = crate::api::income::income_data(conn, user, Some(from), Some(to)).await?;
    let mut lines = vec![format!(
        "Куда сколько поступило {}: выручка {} (товары {}, работы {}), возвраты {}",
        period_label(r.from, r.to),
        format_som(r.revenue_tyiyn),
        format_som(r.goods_tyiyn),
        format_som(r.services_tyiyn),
        format_som(r.returns_tyiyn)
    )];
    lines.push("Оплаты чеков:".into());
    for m in r
        .methods
        .iter()
        .filter(|m| m.sales_tyiyn != 0 || m.returns_tyiyn != 0)
    {
        lines.push(format!(
            "— {}: {} (продажи {}, возвраты {})",
            method_name(&m.method),
            format_som(m.net_tyiyn),
            format_som(m.sales_tyiyn),
            format_som(m.returns_tyiyn)
        ));
    }
    lines.push("По кассам:".into());
    for a in &r.accounts {
        lines.push(format!(
            "— {}: на начало {}, пришло {}, ушло {}, на конец {}",
            a.name,
            format_som(a.opening_tyiyn),
            format_som(a.in_tyiyn),
            format_som(a.out_tyiyn),
            format_som(a.closing_tyiyn)
        ));
    }
    if !r.categories.is_empty() {
        lines.push("По категориям:".into());
        capped(
            &mut lines,
            r.categories
                .iter()
                .map(|c| format!("— {}: {}", c.name, format_som(c.revenue_tyiyn)))
                .collect(),
        );
    }
    Ok(lines.join("\n"))
}

/// Проданное по накладным (SPEC-27 §6): оценка «первым продаётся пришедшее раньше».
async fn receipts_sold_text(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<String> {
    let list = crate::api::income::receipts_sold(conn, user, from, to).await?;
    let mut lines = vec![format!(
        "Накладные {}: сколько продано и заработано (оценка: первым продаётся пришедшее раньше)",
        period_label(from, to)
    )];
    if list.is_empty() {
        lines.push("Накладных нет.".into());
    }
    capped(
        &mut lines,
        list.iter()
            .map(|r| {
                format!(
                    "№ {} от {}{}: закупка {}, продано на {} по закупке, осталось на {}, выручка {}, заработано {}",
                    r.number,
                    local(r.created_at),
                    r.supplier_name
                        .as_ref()
                        .map(|s| format!(", {s}"))
                        .unwrap_or_default(),
                    format_som(r.cost_tyiyn),
                    format_som(r.sold_cost_tyiyn),
                    format_som(r.left_cost_tyiyn),
                    format_som(r.revenue_tyiyn),
                    format_som(r.earned_tyiyn)
                )
            })
            .collect(),
    );
    Ok(lines.join("\n"))
}

// ---------- Склад ----------

/// Склад: `stock` (остатки и стоимость), `stale`, `receipts`, `revisions`, `oil_transfers`.
pub async fn stock(
    conn: &mut PgConnection,
    user: &CurrentUser,
    report: &str,
    query: Option<&str>,
    (from, to): (NaiveDate, NaiveDate),
) -> AppResult<String> {
    match report {
        "stock" => {
            // Тот же запрос, что экран «Склад» (`GET /stock`).
            let list = query_products(
                conn,
                user,
                ProductFilter {
                    q: query.map(str::to_string),
                    limit: 500,
                    ..Default::default()
                },
            )
            .await?;
            let value = list.iter().fold(0i64, |acc, p| {
                acc.saturating_add(p.stock_value_tyiyn.unwrap_or(0))
            });
            let mut have: Vec<_> = list.iter().filter(|p| p.stock_qty != 0).collect();
            have.sort_by_key(|p| std::cmp::Reverse(p.stock_value_tyiyn.unwrap_or(0)));
            let mut lines = vec![format!(
                "Склад на сейчас{}: товаров с остатком {}, стоимость остатков {}{}",
                query.map(|q| format!(" по «{q}»")).unwrap_or_default(),
                have.len(),
                format_som(value),
                if list.len() >= 500 {
                    " (первые 500 карточек)"
                } else {
                    ""
                }
            )];
            capped(
                &mut lines,
                have.iter()
                    .map(|p| {
                        format!(
                            "— {}: {}, стоимость {}, цена продажи {}",
                            p.name,
                            crate::domain::qty::stock_text(p.stock_qty, &p.unit, p.container_ml),
                            format_som(p.stock_value_tyiyn.unwrap_or(0)),
                            format_som(p.sale_price_tyiyn)
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "stale" => {
            let (days, rows) = crate::api::receipts::stale_list(conn, user.branch_id).await?;
            if rows.is_empty() {
                return Ok(format!(
                    "Залежалых товаров (без продаж дольше {days} дней) нет."
                ));
            }
            let mut lines = vec![format!(
                "Залежалые товары — без продаж дольше {days} дней ({}):",
                rows.len()
            )];
            capped(
                &mut lines,
                rows.iter()
                    .map(|r| {
                        format!(
                            "— {}: {}, {} {} дн.",
                            r.name,
                            qty_text(r.stock_qty, &r.unit),
                            if r.sold_ever {
                                "последняя продажа"
                            } else {
                                "не продавался с прихода,"
                            },
                            r.days
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "receipts" => {
            let rows = crate::api::receipts::receipts_data(
                conn,
                user,
                crate::api::receipts::PeriodQuery {
                    from: Some(from),
                    to: Some(to),
                    supplier_id: None,
                },
            )
            .await?;
            let total = rows
                .iter()
                .fold(0i64, |acc, r| acc.saturating_add(r.total_tyiyn));
            let mut lines = vec![format!(
                "Приходы {}: {} накладных на {}",
                period_label(from, to),
                rows.len(),
                format_som(total)
            )];
            capped(
                &mut lines,
                rows.iter()
                    .map(|r| {
                        let mark = if r.reversal_of.is_some() {
                            " (сторно)"
                        } else if r.reversed {
                            " (сторнирована)"
                        } else {
                            ""
                        };
                        let debt = if r.on_debt_tyiyn != 0 {
                            format!(", в долг {}", format_som(r.on_debt_tyiyn))
                        } else {
                            String::new()
                        };
                        format!(
                            "№ {} {}: {} — {}{debt}{mark}",
                            r.number,
                            local(r.created_at),
                            r.supplier_name.as_deref().unwrap_or("без поставщика"),
                            format_som(r.total_tyiyn)
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "revisions" => {
            let rows = crate::api::revisions::list_data(conn, user).await?;
            let mut lines = vec!["Ревизии (новые сверху):".to_string()];
            if rows.is_empty() {
                lines.push("Ревизий нет.".into());
            }
            capped(
                &mut lines,
                rows.iter()
                    .map(|r| {
                        let status = match r.status.as_str() {
                            "posted" => "проведена",
                            "cancelled" => "отменена",
                            _ => "идёт",
                        };
                        format!(
                            "№ {} от {}, {}: {status}, посчитано позиций {}",
                            r.number,
                            local(r.created_at),
                            r.category_name.as_deref().unwrap_or("весь склад"),
                            r.counted
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "oil_transfers" => {
            let rows = crate::api::oil::list_data(conn, user).await?;
            let mut lines = vec!["Переливы масла (новые сверху):".to_string()];
            if rows.is_empty() {
                lines.push("Переливов нет.".into());
            }
            capped(
                &mut lines,
                rows.iter()
                    .map(|t| {
                        format!(
                            "№ {} {}: {} → {}, {}, себестоимость {}{}",
                            t.number,
                            local(t.created_at),
                            t.from_name,
                            t.to_name,
                            qty_text(t.qty_ml, "ml"),
                            format_som(t.value_tyiyn),
                            if t.reversed { " (отменён)" } else { "" }
                        )
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "defects" => {
            // Тот же список, что экран «Остатки → Брак» (SPEC-25): только чтение.
            let rows = crate::api::defects::load_defects(conn, user, None, None).await?;
            let pending: Vec<_> = rows.iter().filter(|d| d.status == "pending").collect();
            let pending_cost = pending
                .iter()
                .fold(0i64, |acc, d| acc.saturating_add(d.cost_tyiyn.unwrap_or(0)));
            let mut lines = vec![format!(
                "Брак: ждут решения {} на {}",
                pending.len(),
                format_som(pending_cost)
            )];
            let line = |d: &crate::api::defects::DefectOut| {
                let status = match (d.status.as_str(), &d.resolution) {
                    ("pending", _) => "ждёт решения".to_string(),
                    ("cancelled", _) => "отметку сняли".to_string(),
                    (_, Some(r)) => {
                        let mut t = crate::api::defects::kind_words(&r.kind).to_string();
                        if let Some(v) = r.refund_tyiyn {
                            t.push_str(&format!(", возмещение {}", format_som(v)));
                        }
                        if let Some(q) = r.replace_qty {
                            t.push_str(&format!(", взамен {}", qty_text(q, &d.unit)));
                        }
                        t
                    }
                    _ => d.status.clone(),
                };
                format!(
                    "№ {} {}: {} × {}, {} — {}; {}{}",
                    d.number,
                    local(d.created_at),
                    d.product_name,
                    qty_text(d.qty, &d.unit),
                    format_som(d.cost_tyiyn.unwrap_or(0)),
                    d.reason,
                    status,
                    d.supplier_name
                        .as_deref()
                        .map(|s| format!(" (поставщик {s})"))
                        .unwrap_or_default()
                )
            };
            capped(&mut lines, pending.iter().map(|d| line(d)).collect());
            let done: Vec<String> = rows
                .iter()
                .filter(|d| {
                    d.status != "pending"
                        && d.resolved_at.is_some_and(|t| {
                            let day = (t + chrono::Duration::hours(6)).date_naive();
                            day >= from && day <= to
                        })
                })
                .map(line)
                .collect();
            lines.push(format!("Решено {}: {}", period_label(from, to), done.len()));
            capped(&mut lines, done);
            Ok(lines.join("\n"))
        }
        "receipts_sold" => receipts_sold_text(conn, user, from, to).await,
        _ => Err(invalid(
            "report: stock, stale, receipts, revisions, oil_transfers, defects или receipts_sold",
        )),
    }
}

// ---------- Клиенты и поставщики ----------

/// Карточка контрагента: `card`, `reconciliation`, `loyalty`, `oil_book`.
pub async fn party(
    conn: &mut PgConnection,
    user: &CurrentUser,
    report: &str,
    query: &str,
    period: Option<(NaiveDate, NaiveDate)>,
) -> AppResult<String> {
    if !matches!(
        report,
        "card" | "reconciliation" | "loyalty" | "oil_book" | "deliveries"
    ) {
        return Err(invalid(
            "report: card, reconciliation, loyalty, oil_book или deliveries",
        ));
    }
    let role = (report == "deliveries").then_some("supplier");
    let p = match find_party(conn, user.branch_id, role, query).await? {
        Ok(p) => p,
        Err(text) => {
            return Ok(text.replace("Черновик не создан. ", "").replace(
                "Черновик не создан — уточните, какой нужен.",
                "Уточните, какой нужен.",
            ));
        }
    };
    // Телефон — только если искали по номеру (SPEC-21: чужие телефоны не передаются).
    let by_phone = query.chars().filter(char::is_ascii_digit).count() >= 5;
    match report {
        "card" => {
            let (from, to) = period.map_or((None, None), |(f, t)| (Some(f), Some(t)));
            let c = crate::api::parties::party_card_data(conn, user, p.id, from, to).await?;
            let mut lines = vec![format!(
                "{} ({})",
                c.party.name,
                if c.party.role == "supplier" {
                    "поставщик"
                } else {
                    "клиент"
                }
            )];
            if by_phone && !c.party.phone.is_empty() {
                lines.push(format!("Телефон: {}", c.party.phone));
            }
            lines.push(format!("Баланс: {}", balance_text(c.party.balance_tyiyn)));
            if c.party.overdue_tyiyn > 0 {
                lines.push(format!("Просрочено: {}", format_som(c.party.overdue_tyiyn)));
            }
            lines.push(format!(
                "Покупок {} на {}; взято в долг {}, погашено {}{}",
                c.purchases,
                format_som(c.purchases_tyiyn),
                format_som(c.debt_taken_tyiyn),
                format_som(c.repaid_tyiyn),
                period
                    .map(|(f, t)| format!(" ({})", period_label(f, t)))
                    .unwrap_or_default()
            ));
            if !c.vehicles.is_empty() {
                lines.push(format!(
                    "Машины: {}",
                    c.vehicles
                        .iter()
                        .filter(|v| v.active)
                        .map(|v| format!("{} {} {}", v.plate, v.brand, v.model)
                            .trim()
                            .to_string())
                        .collect::<Vec<_>>()
                        .join("; ")
                ));
            }
            lines.push("Последние записи:".into());
            let items: Vec<String> = c
                .timeline
                .iter()
                .take(15)
                .map(|t| {
                    let num = t.number.map(|n| format!(" № {n}")).unwrap_or_default();
                    let note = if t.comment.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", t.comment.chars().take(100).collect::<String>())
                    };
                    format!(
                        "{} {}{num}: {}{note}",
                        local(t.at),
                        t.title,
                        format_som(t.amount_tyiyn)
                    )
                })
                .collect();
            if items.is_empty() {
                lines.push("записей нет".into());
            }
            lines.extend(items);
            Ok(lines.join("\n"))
        }
        "reconciliation" => {
            let (from, to) = period.map_or((None, None), |(f, t)| (Some(f), Some(t)));
            let r = crate::api::parties::reconciliation_data(conn, user.branch_id, p.id, from, to)
                .await?;
            let mut lines = vec![
                format!("Акт сверки: {} {}", r.name, period_label(r.from, r.to)),
                format!("Сальдо на начало: {}", balance_text(r.opening_tyiyn)),
            ];
            capped(
                &mut lines,
                r.rows
                    .iter()
                    .map(|x| {
                        let sum = if x.debit_tyiyn != 0 {
                            format!("в пользу точки {}", format_som(x.debit_tyiyn))
                        } else {
                            format!("в пользу контрагента {}", format_som(x.credit_tyiyn))
                        };
                        format!("{} {}: {sum}", day(x.date), x.document)
                    })
                    .collect(),
            );
            lines.push(format!(
                "Обороты: в пользу точки {}, в пользу контрагента {}",
                format_som(r.debit_total_tyiyn),
                format_som(r.credit_total_tyiyn)
            ));
            lines.push(format!(
                "Сальдо на конец: {}",
                balance_text(r.closing_tyiyn)
            ));
            Ok(lines.join("\n"))
        }
        "deliveries" => {
            // Как вкладка «Поставки» в карточке поставщика (SPEC-26).
            let (from, to) = period.map_or((None, None), |(f, t)| (Some(f), Some(t)));
            let d = crate::api::deliveries::deliveries_data(conn, user, p.id, from, to).await?;
            let mut lines = vec![format!(
                "Поставки {} {}: на {}",
                d.supplier_name,
                period
                    .map(|(f, t)| period_label(f, t))
                    .unwrap_or_else(|| "за всё время".into()),
                format_som(d.amount_tyiyn)
            )];
            lines.push(format!("Баланс: {}", balance_text(d.balance_tyiyn)));
            if d.pending_defects > 0 {
                lines.push(format!(
                    "Брак ждёт решения: {} на {}",
                    d.pending_defects,
                    format_som(d.pending_defects_tyiyn.unwrap_or(0))
                ));
            }
            if d.rows.is_empty() {
                lines.push("Поставок нет.".into());
            }
            capped(
                &mut lines,
                d.rows
                    .iter()
                    .map(|r| {
                        let mut t = format!(
                            "— {}: привёз {}, на {}",
                            r.name,
                            qty_text(r.delivered_qty, &r.unit),
                            format_som(r.amount_tyiyn)
                        );
                        if r.returned_qty != 0 {
                            t.push_str(&format!(
                                ", вернули браком {}",
                                qty_text(r.returned_qty, &r.unit)
                            ));
                        }
                        if let (Some(price), Some(at)) = (r.last_price_tyiyn, r.last_at) {
                            t.push_str(&format!(
                                ", последняя цена {} ({})",
                                format_som(price),
                                local(at)
                            ));
                        }
                        t.push_str(&format!(
                            "; на складе {}, из них от него (оценка) {}",
                            qty_text(r.stock_qty, &r.unit),
                            qty_text(r.estimate_qty, &r.unit)
                        ));
                        t
                    })
                    .collect(),
            );
            Ok(lines.join("\n"))
        }
        "loyalty" => {
            let h = crate::api::loyalty::history(conn, p.id, 20).await?;
            let mut lines = vec![format!(
                "Баллы {}: {}, на счету {}",
                p.name,
                if h.member {
                    "участвует"
                } else {
                    "не участвует"
                },
                format_som(h.balance_tyiyn)
            )];
            for r in &h.rows {
                let what = match r.kind.as_str() {
                    "accrual" => "начислено",
                    "redeem" => "списано",
                    "refund" => "возвращено при возврате",
                    "accrual_back" => "снято при возврате",
                    other => other,
                };
                lines.push(format!(
                    "{} {what}: {}{}",
                    local(r.created_at),
                    format_som_signed(r.amount_tyiyn),
                    r.sale_number
                        .map(|n| format!(" (чек № {n})"))
                        .unwrap_or_default()
                ));
            }
            Ok(lines.join("\n"))
        }
        _ => {
            let books = crate::api::oil_book::books(conn, user.branch_id, Some(p.id), None).await?;
            if books.is_empty() {
                return Ok(format!("У {} машин в масляной книжке нет.", p.name));
            }
            let mut lines = vec![format!("Масляная книжка: {}", p.name)];
            for b in books.iter().take(10) {
                lines.push(format!(
                    "{} {} {}: следующая замена масла {}{}",
                    b.plate,
                    b.brand,
                    b.model,
                    b.next_date.map(day).unwrap_or_else(|| "—".into()),
                    b.next_km
                        .map(|k| format!(" или на {k} км"))
                        .unwrap_or_default()
                ));
                // Остальные строки книжки со своим сроком (SPEC-27).
                for n in b.slots.iter().filter(|n| n.slot != "oil") {
                    let next = crate::api::oil_book::next_text(n);
                    if !next.is_empty() {
                        lines.push(format!("  {}: {next}", n.label));
                    }
                }
                for r in b.records.iter().take(3) {
                    let what = r
                        .lines
                        .iter()
                        .map(|l| l.text.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    lines.push(format!(
                        "  {}{}: {}",
                        day(r.change_date),
                        r.mileage_km
                            .map(|k| format!(", {k} км"))
                            .unwrap_or_default(),
                        or_dash(&what)
                    ));
                }
                if b.records.is_empty() {
                    lines.push("  замен ещё не было".into());
                }
            }
            if books.len() > 10 {
                lines.push(format!("и ещё {} машин", books.len() - 10));
            }
            Ok(lines.join("\n"))
        }
    }
}

fn or_dash(s: &str) -> &str {
    if s.trim().is_empty() { "—" } else { s }
}

fn balance_text(b: i64) -> String {
    match b {
        0 => "0 с".into(),
        b if b > 0 => format!("{} (должны точке)", format_som(b)),
        b => format!("{} (точка должна)", format_som(-b)),
    }
}
