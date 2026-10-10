//! Единственное место округления долей тыйына (инвариант 2).

/// Делит `n` на `d` с округлением половины от нуля. `None` при `d <= 0`
/// или если результат не помещается в `i64`.
pub fn div_round(n: i128, d: i128) -> Option<i64> {
    if d <= 0 {
        return None;
    }
    let mut q = n / d;
    let r = n % d;
    if 2 * r.abs() >= d {
        q += n.signum();
    }
    i64::try_from(q).ok()
}

/// Произведение двух целых без переполнения `i64`.
pub fn mul(a: i64, b: i64) -> Option<i64> {
    a.checked_mul(b)
}

/// Сумма для текста сервера: «1 234,50 с». Только для сообщений, не для расчётов.
pub fn format_som(t: i64) -> String {
    let whole = (t / 100).unsigned_abs().to_string();
    // Разряды через пробел, как на экранах кассы.
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (i, ch) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i).is_multiple_of(3) {
            grouped.push(' ');
        }
        grouped.push(ch);
    }
    // Тыйыны только когда они есть: «32 850 с», «1 234,50 с» — как на экранах.
    let frac = (t % 100).unsigned_abs();
    format!(
        "{}{}{} с",
        if t < 0 { "−" } else { "" },
        grouped,
        if frac == 0 {
            String::new()
        } else {
            format!(",{frac:02}")
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounds_half_away_from_zero() {
        assert_eq!(div_round(5, 2), Some(3));
        assert_eq!(div_round(-5, 2), Some(-3));
        assert_eq!(div_round(7, 3), Some(2));
        assert_eq!(div_round(1, 3), Some(0));
        assert_eq!(div_round(2, 3), Some(1));
        assert_eq!(div_round(-1, 3), Some(0));
        assert_eq!(div_round(0, 7), Some(0));
    }

    #[test]
    fn rejects_bad_divisor_and_overflow() {
        assert_eq!(div_round(1, 0), None);
        assert_eq!(div_round(1, -1), None);
        assert_eq!(div_round(i128::from(i64::MAX) * 4, 2), None);
    }

    #[test]
    fn som_text_has_thousands() {
        assert_eq!(format_som(500_000), "5 000 с");
        assert_eq!(format_som(-123_456_789), "−1 234 567,89 с");
        assert_eq!(format_som(50), "0,50 с");
    }

    #[test]
    fn pour_example_from_spec() {
        // 1500 мл по 333,33 с/л = 49 999,5 тыйын → 50 000.
        assert_eq!(div_round(1500 * 33_333, 1000), Some(50_000));
    }
}
