//! Срок долга клиента (SPEC-10, SPEC-27 §6–7). Старый долг гасится первым, поэтому срок
//! считается от самого старого непогашенного долга — так же, как просрочка `overdue_tyiyn`.

use chrono::NaiveDate;

/// День самого старого непогашенного долга. `entries` — долги (взял в долг, правка в плюс) от
/// новых к старым: (учётный день, сумма больше нуля). Долга нет — `None`.
pub fn oldest_unpaid(balance: i64, entries: &[(NaiveDate, i64)]) -> Option<NaiveDate> {
    if balance <= 0 {
        return None;
    }
    let mut covered: i64 = 0;
    let mut last = None;
    for &(day, amount) in entries {
        if amount <= 0 {
            continue;
        }
        last = Some(day);
        covered = covered.saturating_add(amount);
        if covered >= balance {
            return Some(day);
        }
    }
    // Долгов меньше баланса (старый перенос без записей) — самый старый, что есть.
    last
}

/// Сколько не оплачено по каждому долгу (SPEC-28 §3): `amounts` — долги от новых к старым, баланс
/// гасит сначала старые, поэтому неоплаченная часть — самые новые долги на сумму баланса. Тот же
/// обход, что [`oldest_unpaid`]. Результат — по одному числу на долг, в том же порядке.
pub fn unpaid_parts(balance: i64, amounts: &[i64]) -> Vec<i64> {
    let mut rest = balance.max(0);
    amounts
        .iter()
        .map(|&a| {
            let left = a.max(0).min(rest);
            rest -= left;
            left
        })
        .collect()
}

/// Последний день оплаты: долг, взятый в день `oldest`, платят в течение `due_days` дней
/// (день долга — первый). На следующий день долг просрочен — ровно когда `overdue_tyiyn > 0`.
pub fn due_date(oldest: NaiveDate, due_days: i32) -> NaiveDate {
    oldest + chrono::Duration::days(i64::from(due_days) - 1)
}

/// Дней просрочки на `today`: 0 — срок не прошёл.
pub fn overdue_days(due: NaiveDate, today: NaiveDate) -> i64 {
    (today - due).num_days().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap_or_default()
    }

    #[test]
    fn unpaid_parts_take_newest_debts() {
        // Долги 300 (новый), 500, 200; баланс 600: новый целиком, 300 из 500, старый оплачен.
        assert_eq!(unpaid_parts(600, &[300, 500, 200]), vec![300, 300, 0]);
        assert_eq!(unpaid_parts(0, &[300]), vec![0]);
        assert_eq!(unpaid_parts(5000, &[300, 200]), vec![300, 200]);
    }

    #[test]
    fn oldest_unpaid_walks_from_newest() {
        // Долги 10-го 300, 5-го 500, 1-го 200; баланс 600: 10-е покрыто, 5-е частично — оно.
        let e = [(d(10), 300), (d(5), 500), (d(1), 200)];
        assert_eq!(oldest_unpaid(600, &e), Some(d(5)));
        assert_eq!(oldest_unpaid(300, &e), Some(d(10)));
        assert_eq!(oldest_unpaid(1000, &e), Some(d(1)));
        assert_eq!(oldest_unpaid(5000, &e), Some(d(1)));
        assert_eq!(oldest_unpaid(0, &e), None);
        assert_eq!(oldest_unpaid(-10, &e), None);
        assert_eq!(oldest_unpaid(100, &[]), None);
    }

    #[test]
    fn due_and_overdue_match_overdue_amount() {
        // 14 дней: долг 1-го — последний день 14-е, 15-го — 1 день просрочки.
        let due = due_date(d(1), 14);
        assert_eq!(due, d(14));
        assert_eq!(overdue_days(due, d(14)), 0);
        assert_eq!(overdue_days(due, d(15)), 1);
        assert_eq!(overdue_days(due, d(3)), 0);
    }
}
