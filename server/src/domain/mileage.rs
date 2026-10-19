//! Пробег в км или милях (SPEC-29 §22, ADR-084): перевод одной функцией, только целые.
//! 1 миля = 1,609344 км — в миллионных долях; округление — половиной от нуля (`div_round`),
//! как во всём учёте. На экране то же правило — `web/src/lib/mileage.ts`.

use crate::domain::money::div_round;

/// Миллионных долей километра в миле.
const MI_MICRO_KM: i128 = 1_609_344;
const MICRO: i128 = 1_000_000;

/// Единица пробега машины: `km` или `mi`.
pub fn is_unit(unit: &str) -> bool {
    matches!(unit, "km" | "mi")
}

/// Мили в километры до целого.
pub fn mi_to_km(mi: i32) -> Option<i32> {
    div_round(i128::from(mi) * MI_MICRO_KM, MICRO).and_then(|v| i32::try_from(v).ok())
}

/// Километры в мили до целого.
pub fn km_to_mi(km: i32) -> Option<i32> {
    div_round(i128::from(km) * MICRO, MI_MICRO_KM).and_then(|v| i32::try_from(v).ok())
}

/// Пробег в единице машины → км.
pub fn to_km(value: i32, unit: &str) -> Option<i32> {
    if unit == "mi" {
        mi_to_km(value)
    } else {
        Some(value)
    }
}

/// Км → пробег в единице машины.
pub fn from_km(km: i32, unit: &str) -> Option<i32> {
    if unit == "mi" { km_to_mi(km) } else { Some(km) }
}

/// «93 000 км», «57 790 миль» — пробег с единицей для бота и печати.
pub fn text(value: i32, unit: &str) -> String {
    let s = value.unsigned_abs().to_string();
    let mut out = String::new();
    if value < 0 {
        out.push('-');
    }
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(ch);
    }
    format!("{out} {}", if unit == "mi" { "миль" } else { "км" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miles_and_km_convert_with_one_rounding() {
        assert_eq!(mi_to_km(1), Some(2));
        assert_eq!(mi_to_km(100), Some(161));
        assert_eq!(mi_to_km(50_000), Some(80_467));
        assert_eq!(km_to_mi(8000), Some(4971));
        assert_eq!(km_to_mi(80_467), Some(50_000));
        assert_eq!(km_to_mi(1), Some(1));
        assert_eq!(mi_to_km(0), Some(0));
        assert_eq!(to_km(1000, "km"), Some(1000));
        assert_eq!(to_km(1000, "mi"), Some(1609));
        assert_eq!(from_km(1609, "mi"), Some(1000));
        assert_eq!(mi_to_km(i32::MAX), None);
        assert_eq!(text(57_790, "mi"), "57 790 миль");
        assert_eq!(text(93_000, "km"), "93 000 км");
        assert!(is_unit("mi") && !is_unit("ml"));
    }
}
