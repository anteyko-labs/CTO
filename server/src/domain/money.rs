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
    fn pour_example_from_spec() {
        // 1500 мл по 333,33 с/л = 49 999,5 тыйын → 50 000.
        assert_eq!(div_round(1500 * 33_333, 1000), Some(50_000));
    }
}
