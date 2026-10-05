//! Себестоимость по средней через пул стоимости остатка (ADR-011).

use super::money::div_round;

/// Состояние остатка товара в филиале.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pool {
    pub qty: i64,
    pub value: i64,
    pub last_qty: i64,
    pub last_cost: i64,
}

/// Себестоимость списания `q > 0` единиц. `None` при переполнении или `q <= 0`.
pub fn cost_of(q: i64, p: &Pool) -> Option<i64> {
    if q <= 0 {
        return None;
    }
    if p.qty >= q {
        return div_round(i128::from(q) * i128::from(p.value), i128::from(p.qty));
    }
    let covered = p.qty.max(0);
    let part1 = if covered > 0 { p.value } else { 0 };
    let rest = q - covered;
    let part2 = if p.last_qty > 0 {
        div_round(
            i128::from(rest) * i128::from(p.last_cost),
            i128::from(p.last_qty),
        )?
    } else {
        0
    };
    part1.checked_add(part2)
}

/// Средняя цена за `per` единиц для показа (например, за канистру).
pub fn average(p: &Pool, per: i64) -> Option<i64> {
    if p.qty <= 0 {
        return None;
    }
    div_round(i128::from(p.value) * i128::from(per), i128::from(p.qty))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(qty: i64, value: i64) -> Pool {
        Pool {
            qty,
            value,
            last_qty: 0,
            last_cost: 0,
        }
    }

    #[test]
    fn weighted_average_from_spec() {
        // 10 шт на 1000 с + 10 шт на 2000 с → 3000 с, средняя 150 с.
        let p = pool(20, 300_000);
        assert_eq!(average(&p, 1), Some(15_000));
        assert_eq!(cost_of(1, &p), Some(15_000));
    }

    #[test]
    fn selling_everything_leaves_zero_value() {
        let mut p = pool(3, 100);
        for _ in 0..3 {
            let c = cost_of(1, &p).unwrap();
            p.qty -= 1;
            p.value -= c;
        }
        assert_eq!((p.qty, p.value), (0, 0));
    }

    #[test]
    fn shortage_uses_last_receipt_price() {
        let p = Pool {
            qty: 0,
            value: 0,
            last_qty: 4000,
            last_cost: 150_000,
        };
        assert_eq!(cost_of(1000, &p), Some(37_500));
        let p = Pool {
            qty: 2,
            value: 200,
            last_qty: 10,
            last_cost: 1500,
        };
        assert_eq!(cost_of(5, &p), Some(200 + 450));
        let p = Pool {
            qty: -2,
            value: -300,
            last_qty: 1,
            last_cost: 150,
        };
        assert_eq!(cost_of(1, &p), Some(150));
    }

    #[test]
    fn no_history_costs_zero() {
        assert_eq!(cost_of(3, &pool(0, 0)), Some(0));
        assert_eq!(cost_of(0, &pool(5, 5)), None);
    }
}
