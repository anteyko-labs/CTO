//! Расчётный период оклада (задача 003 Н11, ADR-{B9.2}): у работника свой день расчёта D (1–28),
//! период — с D-го по (D−1)-е следующего месяца, к выплате — D-го. D = 1 — календарный месяц.
//! То же правило на экране — `periodOf` в `web/src/lib/payrollPeriod.ts`, тесты сверяют примеры.

use chrono::{Datelike, Days, Months, NaiveDate};
use serde::Serialize;

use crate::domain::money::div_round;

/// Последний допустимый день расчёта: такой день есть в каждом месяце.
pub const MAX_DAY: u32 = 28;

/// Расчётный период: первый и последний день, день выплаты — следующий за последним.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub pay: NaiveDate,
}

/// Период дня расчёта `day`, в который входит `date`. День вне 1–28 приводится к границе.
pub fn period_of(day: u32, date: NaiveDate) -> Option<Period> {
    let d = day.clamp(1, MAX_DAY);
    let first = date.with_day(1)?;
    let start_month = if date.day() >= d {
        first
    } else {
        first.checked_sub_months(Months::new(1))?
    };
    let from = start_month.with_day(d)?;
    let pay = from.checked_add_months(Months::new(1))?;
    let to = pay.pred_opt()?;
    Some(Period { from, to, pay })
}

/// Период, который заканчивается в месяце `month` (любой день месяца): в него входит 1-е число.
/// При D = 1 — сам этот месяц.
pub fn period_ending_in(day: u32, month: NaiveDate) -> Option<Period> {
    period_of(day, month.with_day(1)?)
}

/// Предыдущий и следующий период.
pub fn prev(day: u32, p: Period) -> Option<Period> {
    period_of(day, p.from.checked_sub_days(Days::new(1))?)
}

pub fn next(day: u32, p: Period) -> Option<Period> {
    period_of(day, p.pay)
}

/// Дней в периоде `[from, to]` включительно.
pub fn days(from: NaiveDate, to: NaiveDate) -> i64 {
    (to - from).num_days() + 1
}

/// Заработано по окладу на день `date` — ориентир, не начисление: ставка × прошло дней / дней в
/// периоде, одно округление до тыйына (I1, I2). До начала периода — 0, после конца — вся ставка.
pub fn earned_to_date(rate: i64, from: NaiveDate, to: NaiveDate, date: NaiveDate) -> i64 {
    if date < from {
        return 0;
    }
    let total = days(from, to);
    let passed = days(from, date.min(to));
    div_round(i128::from(rate) * i128::from(passed), i128::from(total)).unwrap_or(rate)
}

/// «20.10–19.11».
pub fn label(from: NaiveDate, to: NaiveDate) -> String {
    format!("{}–{}", from.format("%d.%m"), to.format("%d.%m"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn day_one_is_the_calendar_month() {
        let p = period_of(1, d(2026, 10, 15)).unwrap();
        assert_eq!(
            (p.from, p.to, p.pay),
            (d(2026, 10, 1), d(2026, 10, 31), d(2026, 11, 1))
        );
        let p = period_of(1, d(2026, 2, 28)).unwrap();
        assert_eq!((p.from, p.to), (d(2026, 2, 1), d(2026, 2, 28)));
    }

    #[test]
    fn day_twenty_runs_to_the_nineteenth() {
        // Пример заказчика: с 20 окт по 19 ноя, к выплате 20 ноя.
        for date in [d(2026, 10, 20), d(2026, 11, 1), d(2026, 11, 19)] {
            let p = period_of(20, date).unwrap();
            assert_eq!(
                (p.from, p.to, p.pay),
                (d(2026, 10, 20), d(2026, 11, 19), d(2026, 11, 20)),
                "{date}"
            );
        }
        let p = period_of(20, d(2026, 11, 20)).unwrap();
        assert_eq!(p.from, d(2026, 11, 20));
        assert_eq!(label(p.from, p.to), "20.11–19.12");
    }

    #[test]
    fn year_and_february_boundaries() {
        let p = period_of(28, d(2027, 1, 10)).unwrap();
        assert_eq!(
            (p.from, p.to, p.pay),
            (d(2026, 12, 28), d(2027, 1, 27), d(2027, 1, 28))
        );
        let p = period_of(28, d(2027, 2, 28)).unwrap();
        assert_eq!((p.from, p.to), (d(2027, 2, 28), d(2027, 3, 27)));
        let p = period_of(15, d(2028, 2, 29)).unwrap();
        assert_eq!((p.from, p.to), (d(2028, 2, 15), d(2028, 3, 14)));
        // Вне 1–28 — к границе.
        assert_eq!(
            period_of(31, d(2026, 10, 30)),
            period_of(28, d(2026, 10, 30))
        );
        assert_eq!(period_of(0, d(2026, 10, 30)), period_of(1, d(2026, 10, 30)));
    }

    #[test]
    fn neighbours_and_month_end() {
        let p = period_of(20, d(2026, 11, 5)).unwrap();
        assert_eq!(prev(20, p).unwrap().from, d(2026, 9, 20));
        assert_eq!(next(20, p).unwrap().from, d(2026, 11, 20));
        assert_eq!(
            period_ending_in(20, d(2026, 11, 30)).unwrap().to,
            d(2026, 11, 19)
        );
        assert_eq!(
            period_ending_in(1, d(2026, 11, 30)).unwrap().to,
            d(2026, 11, 30)
        );
    }

    #[test]
    fn earned_to_date_rounds_once() {
        // 30 000 с за 31 день, прошло 11: 30 000 × 11 / 31 = 10 645,16… → 10 645,16 с.
        let rate = 3_000_000;
        assert_eq!(
            earned_to_date(rate, d(2026, 10, 1), d(2026, 10, 31), d(2026, 10, 11)),
            1_064_516
        );
        assert_eq!(
            earned_to_date(rate, d(2026, 10, 1), d(2026, 10, 31), d(2026, 9, 30)),
            0
        );
        assert_eq!(
            earned_to_date(rate, d(2026, 10, 1), d(2026, 10, 31), d(2026, 11, 5)),
            rate
        );
    }
}
