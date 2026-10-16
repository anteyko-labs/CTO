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

/// Цена литра на розлив у масла в канистре: цена канистры × 1000 / объём канистры в мл,
/// с округлением до тыйына (ADR-074). У такого масла одна цена — цена канистры; розлив за литр
/// не хранится, а считается здесь. На экране то же правило — `canisterPourPrice` в
/// `web/src/lib/money.ts`. `None` при объёме не больше нуля или переполнении.
pub fn canister_pour_price(canister_price: i64, container_ml: i64) -> Option<i64> {
    div_round(i128::from(canister_price) * 1000, i128::from(container_ml))
}

/// Цена литра на розлив товара: у масла (оно всегда в таре) — от цены канистры
/// ([`canister_pour_price`]); у остального товара розлива нет — `None`.
pub fn pour_price_per_l(unit: &str, container_ml: Option<i64>, sale_price: i64) -> Option<i64> {
    match (unit, container_ml) {
        ("ml", Some(c)) => canister_pour_price(sale_price, c),
        _ => None,
    }
}

/// Наценка цены продажи над ценой по накладной в десятых долях процента: (продажа − закупка) /
/// закупка, до 0,1 % половиной от нуля (SPEC-28 §1, ADR-075). 250 — это 25,0 %. Обе цены — за одну
/// единицу цены товара (штуку, канистру). `None` при закупке не больше нуля (наценки нет) или
/// переполнении. На экране то же правило — `markupPermille` в `web/src/lib/money.ts`.
pub fn markup_permille(cost: i64, sale: i64) -> Option<i64> {
    if cost <= 0 {
        return None;
    }
    div_round(
        (i128::from(sale) - i128::from(cost)) * 1000,
        i128::from(cost),
    )
}

/// Цена продажи из наценки: закупка × (1000 + наценка в десятых процента) / 1000, до тыйына
/// половиной от нуля (SPEC-28 §1, ADR-075). Наценка меньше −100 % или переполнение — `None`.
/// На экране то же правило — `saleFromMarkup` в `web/src/lib/money.ts`.
pub fn sale_from_markup(cost: i64, permille: i64) -> Option<i64> {
    if cost < 0 || permille < -1000 {
        return None;
    }
    div_round(i128::from(cost) * (1000 + i128::from(permille)), 1000)
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
    fn pour_price_comes_from_canister() {
        // Канистра 4 л за 2 000 с — литр 500 с: 4 л розливом = канистра, 2,5 л = 1 250 с.
        assert_eq!(canister_pour_price(200_000, 4000), Some(50_000));
        assert_eq!(div_round(4000 * 50_000, 1000), Some(200_000));
        assert_eq!(div_round(2500 * 50_000, 1000), Some(125_000));
        // 1 999,99 с за 4 л = 49 999,75 тыйын за литр → 50 000; 1 999,98 с → 49 999,5 → 50 000 (половина
        // от нуля); 1 999,96 с → ровно 49 999.
        assert_eq!(canister_pour_price(199_999, 4000), Some(50_000));
        assert_eq!(canister_pour_price(199_998, 4000), Some(50_000));
        assert_eq!(canister_pour_price(199_996, 4000), Some(49_999));
        // Бочка 200 л за 120 000 с — 600 с за литр; 1 л за 333,33 с — та же цена.
        assert_eq!(canister_pour_price(12_000_000, 200_000), Some(60_000));
        assert_eq!(canister_pour_price(33_333, 1000), Some(33_333));
        assert_eq!(canister_pour_price(1, 0), None);
        assert_eq!(pour_price_per_l("ml", Some(4000), 200_000), Some(50_000));
        assert_eq!(pour_price_per_l("piece", None, 50_000), None);
        assert_eq!(pour_price_per_l("g", None, 50_000), None);
    }

    #[test]
    fn markup_and_sale_price_are_linked() {
        // Те же случаи, что web/src/lib/money.test.ts.
        // Закупка 1 600 с, продажа 2 000 с — наценка 25,0 %; обратно 25,0 % → 2 000 с.
        assert_eq!(markup_permille(160_000, 200_000), Some(250));
        assert_eq!(sale_from_markup(160_000, 250), Some(200_000));
        // 300 с → 350 с: 16,666… % → 16,7 %; из 16,7 % — 350,10 с (до тыйына).
        assert_eq!(markup_permille(30_000, 35_000), Some(167));
        assert_eq!(sale_from_markup(30_000, 167), Some(35_010));
        // Продажа ниже закупки — наценка отрицательная.
        assert_eq!(markup_permille(10_000, 9_000), Some(-100));
        assert_eq!(sale_from_markup(10_000, -100), Some(9_000));
        // Половина тыйына — от нуля: 333,33 с × 1,105 = 368,330 65 с → 36 833 тыйына.
        assert_eq!(sale_from_markup(33_333, 105), Some(36_833));
        // 0,05 % — половина десятой: от нуля, 0,1 %.
        assert_eq!(markup_permille(200_000, 200_100), Some(1));
        // Без закупки наценки нет; меньше −100 % не бывает.
        assert_eq!(markup_permille(0, 100), None);
        assert_eq!(sale_from_markup(100, -1001), None);
        assert_eq!(sale_from_markup(100, -1000), Some(0));
    }

    #[test]
    fn pour_example_from_spec() {
        // 1500 мл по 333,33 с/л = 49 999,5 тыйын → 50 000.
        assert_eq!(div_round(1500 * 33_333, 1000), Some(50_000));
    }
}
