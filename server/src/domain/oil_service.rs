//! Бесплатная замена с нашим маслом того же вида (ADR-104, SPEC-30 §15).

/// Строка масла нужной категории в чеке: сколько мл и объём канистры товара.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OilLine {
    pub units_ml: i64,
    pub container_ml: Option<i64>,
}

/// Канистра для правила — наименьший объём канистры среди строк масла этой категории.
pub fn canister_ml(lines: &[OilLine]) -> Option<i64> {
    lines
        .iter()
        .filter_map(|l| l.container_ml)
        .filter(|c| *c > 0)
        .min()
}

/// Масла этой категории в чеке не меньше одной канистры: розлив считается в мл вместе с
/// канистрами. Без строк масла (или без объёма канистры) — нет.
pub fn enough_oil(lines: &[OilLine]) -> bool {
    let Some(can) = canister_ml(lines) else {
        return false;
    };
    let total = lines
        .iter()
        .fold(0i64, |acc, l| acc.saturating_add(l.units_ml));
    total >= can
}

/// Удержание за бесплатную замену при возврате масла (ADR-104): масла было не меньше канистры,
/// после возврата стало меньше, а услуга остаётся (не возвращается). `before_ml` и `after_ml` —
/// масло этой категории в чеке до и после возврата, `canister` — канистра правила,
/// `services_left` — сколько услуг остаётся после возврата, `price` — цена услуги клиенту.
pub fn withhold(
    before_ml: i64,
    after_ml: i64,
    canister: i64,
    services_left: i64,
    price: i64,
) -> i64 {
    if canister <= 0 || services_left <= 0 || price <= 0 {
        return 0;
    }
    if before_ml >= canister && after_ml < canister {
        price.saturating_mul(services_left)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn can(units_ml: i64, c: i64) -> OilLine {
        OilLine {
            units_ml,
            container_ml: Some(c),
        }
    }

    #[test]
    fn canister_or_more_is_free() {
        // Канистра 4 л — бесплатно; 0,5 л на долив — нет.
        assert!(enough_oil(&[can(4000, 4000)]));
        assert!(!enough_oil(&[can(500, 4000)]));
        // Розлив считается в мл: 1 л канистрой + 3 л розливом из канистры 4 л — канистра 1 л есть.
        assert!(enough_oil(&[can(1000, 1000), can(3000, 4000)]));
        // Наименьшая канистра среди строк: 3,5 л розливом при канистрах 4 л — мало.
        assert!(!enough_oil(&[can(3500, 4000)]));
        assert!(!enough_oil(&[]));
    }

    #[test]
    fn withhold_only_when_dropping_below_canister() {
        assert_eq!(withhold(4000, 0, 4000, 1, 20_000), 20_000);
        assert_eq!(withhold(8000, 4000, 4000, 2, 20_000), 0);
        assert_eq!(withhold(8000, 3000, 4000, 2, 20_000), 40_000);
        // Услугу вернули вместе с маслом — удерживать нечего.
        assert_eq!(withhold(4000, 0, 4000, 0, 20_000), 0);
        // Масла и так было меньше канистры — услуга не была бесплатной.
        assert_eq!(withhold(3000, 0, 4000, 1, 20_000), 0);
        assert_eq!(withhold(4000, 0, 4000, 1, 0), 0);
    }
}
