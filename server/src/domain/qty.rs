//! Количество для текста сервера: литры, килограммы, канистры. Учёт ведётся в целых мл и
//! граммах (инвариант 1); здесь только показ, без округления — все знаки после запятой целые.

/// Тысячные с запятой без лишних нулей: 14 500 → «14,5», −500 → «−0,5», 1 → «0,001».
pub fn fixed3(v: i64) -> String {
    let sign = if v < 0 { "−" } else { "" };
    let a = v.unsigned_abs();
    let frac = format!("{:03}", a % 1000);
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{}", a / 1000)
    } else {
        format!("{sign}{},{frac}", a / 1000)
    }
}

/// Миллилитры литрами: 14 500 → «14,5 л».
pub fn liters(ml: i64) -> String {
    format!("{} л", fixed3(ml))
}

/// Остаток масла так, чтобы литры и канистры сходились без округления (SPEC-27 §4):
/// 14 500 мл по 4 л → «14,5 л = 3 кан. по 4 л + 2,5 л». Целые канистры и остаток в литрах
/// в сумме дают ровно столько мл, сколько на складе. Одна полная канистра, минус или пустая
/// тара — просто литры.
pub fn oil_stock(ml: i64, container_ml: i64) -> String {
    let total = liters(ml);
    if ml <= 0 || container_ml <= 0 {
        return total;
    }
    let full = ml / container_ml;
    let rest = ml % container_ml;
    let mut parts = Vec::new();
    if full > 0 {
        parts.push(format!("{full} кан. по {}", liters(container_ml)));
    }
    if rest > 0 {
        parts.push(liters(rest));
    }
    if parts.len() > 1 || full > 1 {
        format!("{total} = {}", parts.join(" + "))
    } else {
        total
    }
}

/// Количество по единице учёта: «2,5 л», «12,35 кг», «3 шт».
pub fn qty_text(qty: i64, unit: &str) -> String {
    match unit {
        "ml" => liters(qty),
        "g" => format!("{} кг", fixed3(qty)),
        _ if qty < 0 => format!("−{} шт", qty.unsigned_abs()),
        _ => format!("{qty} шт"),
    }
}

/// Остаток товара: масло в таре — с расшифровкой по канистрам, остальное — [`qty_text`].
pub fn stock_text(qty: i64, unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) => oil_stock(qty, c),
        _ => qty_text(qty, unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liters_keep_every_milliliter_and_sign() {
        assert_eq!(liters(14_500), "14,5 л");
        assert_eq!(liters(4000), "4 л");
        assert_eq!(liters(1), "0,001 л");
        assert_eq!(liters(4001), "4,001 л");
        assert_eq!(liters(100), "0,1 л");
        assert_eq!(liters(-500), "−0,5 л");
        assert_eq!(liters(-2500), "−2,5 л");
        assert_eq!(qty_text(12_350, "g"), "12,35 кг");
        assert_eq!(qty_text(-3, "piece"), "−3 шт");
    }

    #[test]
    fn canisters_add_up_to_the_stock() {
        assert_eq!(oil_stock(14_500, 4000), "14,5 л = 3 кан. по 4 л + 2,5 л");
        assert_eq!(oil_stock(4000, 4000), "4 л");
        assert_eq!(oil_stock(8000, 4000), "8 л = 2 кан. по 4 л");
        assert_eq!(oil_stock(4001, 4000), "4,001 л = 1 кан. по 4 л + 0,001 л");
        assert_eq!(oil_stock(3999, 4000), "3,999 л");
        assert_eq!(oil_stock(9000, 4500), "9 л = 2 кан. по 4,5 л");
        assert_eq!(oil_stock(9100, 4500), "9,1 л = 2 кан. по 4,5 л + 0,1 л");
        assert_eq!(oil_stock(-500, 4000), "−0,5 л");
        assert_eq!(oil_stock(0, 4000), "0 л");
        assert_eq!(stock_text(3, "piece", None), "3 шт");
        assert_eq!(stock_text(9000, "ml", Some(4500)), "9 л = 2 кан. по 4,5 л");
    }
}
