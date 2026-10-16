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

/// Вес для текста сервера: «12 кг», «12,35 кг» (граммы без лишних нулей). Только для сообщений.
pub fn format_kg(grams: i64) -> String {
    let sign = if grams < 0 { "−" } else { "" };
    let g = grams.unsigned_abs();
    let frac = format!("{:03}", g % 1000);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{} кг", g / 1000)
    } else {
        format!("{sign}{},{frac} кг", g / 1000)
    }
}

/// Сумма со знаком: «+300 с», «−1 000 с», «0 с» — недостача и излишек, правки начислений.
pub fn format_som_signed(t: i64) -> String {
    if t > 0 {
        format!("+{}", format_som(t))
    } else {
        format_som(t)
    }
}

/// Разбор десятичной строки в целое с `scale` знаками после запятой: «12,5» при `scale = 2` →
/// 1250. Пробелы внутри (разряды) пропускаются, дробь — через запятую или точку. Отрицательные,
/// лишние знаки после запятой и всё, что не цифры, — `None`. Тот же разбор, что `parseFixed`
/// в `web/src/lib/format.ts`: суммы с экрана и из бота разбираются одинаково (инвариант 1).
pub fn parse_fixed(input: &str, scale: u32) -> Option<i64> {
    let s: String = input
        .trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    // Как `replace(',', '.')` на экране: только первая запятая.
    let s = s.replacen(',', ".", 1);
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s.as_str(), ""),
    };
    if whole.is_empty()
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
        || frac.len() > usize::try_from(scale).ok()?
    {
        return None;
    }
    let mult = 10i64.checked_pow(scale)?;
    let w: i64 = whole.parse().ok()?;
    let mut f: i64 = if frac.is_empty() {
        0
    } else {
        frac.parse().ok()?
    };
    for _ in frac.len()..usize::try_from(scale).ok()? {
        f = f.checked_mul(10)?;
    }
    // Как `Number.isSafeInteger` на экране: не больше 2^53 − 1.
    w.checked_mul(mult)?
        .checked_add(f)
        .filter(|v| *v <= 9_007_199_254_740_991)
}

/// «1 234,5» → 123 450 тыйын; `None` при ошибке. Как `parseSom` на экране.
pub fn parse_som(input: &str) -> Option<i64> {
    parse_fixed(input, 2)
}

/// «2,5» → 2500 мл (или граммов из килограммов); `None` при ошибке. Как `parseLiters` на экране.
pub fn parse_liters(input: &str) -> Option<i64> {
    parse_fixed(input, 3)
}

/// Сумма, как её сказал человек: «1 500», «1500,50», «1 500 сом», «300 с». Слово валюты в конце
/// отбрасывается, остальное разбирает [`parse_som`] — та же функция, что у экранов.
pub fn parse_spoken_som(input: &str) -> Option<i64> {
    let t = input.trim().trim_end_matches('.').trim_end();
    let lower = t.to_lowercase();
    let mut cut = t.len();
    for suffix in ["сомов", "сома", "сом", "с"] {
        if let Some(rest) = lower.strip_suffix(suffix) {
            // «с» — только отдельным словом или сразу после цифры: «300 с», «300с».
            if rest.ends_with(|c: char| c.is_ascii_digit() || c.is_whitespace()) {
                cut = rest.len();
            }
            break;
        }
    }
    // Длина `lower` в байтах та же: кириллица в нижнем регистре занимает столько же.
    parse_som(t.get(..cut)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kg_text() {
        assert_eq!(format_kg(12_000), "12 кг");
        assert_eq!(format_kg(12_350), "12,35 кг");
        assert_eq!(format_kg(-500), "−0,5 кг");
    }

    #[test]
    fn parses_like_the_screen() {
        // Те же случаи, что web/src/lib/format.test.ts.
        assert_eq!(parse_som("1 234,5"), Some(123_450));
        assert_eq!(parse_som("0.1"), Some(10));
        assert_eq!(parse_som("333,33"), Some(33_333));
        assert_eq!(parse_som("1,234"), None);
        assert_eq!(parse_som("-5"), None);
        assert_eq!(parse_som("abc"), None);
        assert_eq!(parse_som(""), None);
        assert_eq!(parse_som("12."), Some(1200));
        assert_eq!(parse_som("1,2,3"), None);
        assert_eq!(parse_liters("1,5"), Some(1500));
        assert_eq!(parse_liters("0,1"), Some(100));
        assert_eq!(parse_som("99999999999999999999"), None);
    }

    #[test]
    fn parses_spoken_amounts() {
        assert_eq!(parse_spoken_som("1 500"), Some(150_000));
        assert_eq!(parse_spoken_som("1500,50"), Some(150_050));
        assert_eq!(parse_spoken_som("1 500 сом"), Some(150_000));
        assert_eq!(parse_spoken_som("1\u{a0}500 сомов"), Some(150_000));
        assert_eq!(parse_spoken_som("300 с"), Some(30_000));
        assert_eq!(parse_spoken_som("300с."), Some(30_000));
        assert_eq!(parse_spoken_som("350,50"), Some(35_050));
        assert_eq!(parse_spoken_som("полторы тысячи"), None);
        assert_eq!(parse_spoken_som("-100"), None);
        assert_eq!(parse_spoken_som("сом"), None);
    }

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
        assert_eq!(format_som_signed(30_000), "+300 с");
        assert_eq!(format_som_signed(-100_000), "−1 000 с");
        assert_eq!(format_som_signed(0), "0 с");
    }

    #[test]
    fn pour_example_from_spec() {
        // 1500 мл по 333,33 с/л = 49 999,5 тыйын → 50 000.
        assert_eq!(div_round(1500 * 33_333, 1000), Some(50_000));
    }
}
