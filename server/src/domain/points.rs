//! Баллы по строкам чека (задача 002, П1; ADR-{B1.2}): правило строки — товар → категория →
//! общая ставка; начисляется только с оплаченного деньгами (ADR-053). Всё в тыйынах, без float.

use crate::domain::money::div_round;

/// Правило баллов строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Не начислять.
    None,
    /// Процент от оплаченного деньгами за строку, сотые доли процента (50 = 0,5 %).
    Percent(i64),
    /// Баллы в тыйынах за единицу товара: штуку, литр или килограмм.
    Fixed(i64),
}

/// Откуда взято правило строки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Product,
    Category,
    Common,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Product => "product",
            Self::Category => "category",
            Self::Common => "common",
        }
    }
}

/// Правило из карточки: `inherit` (и неизвестное) — «как выше», `None`.
pub fn rule_of(mode: &str, value: i64) -> Option<Rule> {
    match mode {
        "none" => Some(Rule::None),
        "percent" => Some(Rule::Percent(value)),
        "fixed" => Some(Rule::Fixed(value)),
        _ => None,
    }
}

/// Порядок: товар → категория → общая ставка.
pub fn pick(product: Option<Rule>, category: Option<Rule>, common_bp: i64) -> (Rule, Source) {
    match (product, category) {
        (Some(r), _) => (r, Source::Product),
        (None, Some(r)) => (r, Source::Category),
        (None, None) => (Rule::Percent(common_bp), Source::Common),
    }
}

/// Делит `total` пропорционально весам: целые доли, остаток — наибольшим остаткам (при
/// равенстве — первым строкам); сумма долей — ровно `total`. Нулевые веса — ноль. `None` —
/// переполнение или отрицательные числа.
pub fn split(total: i64, weights: &[i64]) -> Option<Vec<i64>> {
    if total < 0 || weights.iter().any(|w| *w < 0) {
        return None;
    }
    let sum: i128 = weights.iter().map(|w| i128::from(*w)).sum();
    if sum == 0 {
        return Some(vec![0; weights.len()]);
    }
    let mut out = Vec::with_capacity(weights.len());
    let mut rems = Vec::with_capacity(weights.len());
    let mut given: i128 = 0;
    for (i, w) in weights.iter().enumerate() {
        let n = i128::from(total) * i128::from(*w);
        let q = n / sum;
        given += q;
        out.push(i64::try_from(q).ok()?);
        rems.push((n % sum, i));
    }
    let mut left = i128::from(total) - given;
    rems.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for (rem, i) in rems {
        if left <= 0 {
            break;
        }
        if rem == 0 {
            continue;
        }
        out[i] = out[i].checked_add(1)?;
        left -= 1;
    }
    Some(out)
}

/// Строка чека для расчёта баллов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineIn {
    /// Сумма строки после скидок; подарок — 0.
    pub amount: i64,
    /// Единицы учёта: штуки, мл, граммы.
    pub units: i64,
    /// Сколько единиц учёта в единице правила «за единицу»: штука — 1, литр — 1000 мл, кг — 1000 г.
    pub per_unit: i64,
    pub rule: Rule,
}

/// Баллы каждой строки. Деньги чека (`money` — наличные, карта, QR, по счёту) делятся по суммам
/// строк. Строки с одинаковым процентом считаются вместе, одним округлением, и делятся по
/// строкам: чек целиком по общей ставке даёт ровно `money × ставка`, как до правил по строкам.
/// «За единицу» — ставка × единицы в доле оплаченного деньгами, не больше оплаченного за строку.
pub fn accrue(money: i64, lines: &[LineIn]) -> Option<Vec<i64>> {
    let amounts: Vec<i64> = lines.iter().map(|l| l.amount.max(0)).collect();
    let paid = split(money.max(0), &amounts)?;
    let mut out = vec![0i64; lines.len()];
    let mut rates: Vec<i64> = lines
        .iter()
        .filter_map(|l| match l.rule {
            Rule::Percent(bp) if bp > 0 => Some(bp),
            _ => None,
        })
        .collect();
    rates.sort_unstable();
    rates.dedup();
    for bp in rates {
        let idx: Vec<usize> = (0..lines.len())
            .filter(|i| lines[*i].rule == Rule::Percent(bp))
            .collect();
        let group: i64 = idx
            .iter()
            .try_fold(0i64, |acc, i| acc.checked_add(paid[*i]))?;
        let total = div_round(i128::from(group) * i128::from(bp), 10_000)?;
        let weights: Vec<i64> = idx.iter().map(|i| paid[*i]).collect();
        for (k, v) in split(total, &weights)?.into_iter().enumerate() {
            out[idx[k]] = v;
        }
    }
    for (i, l) in lines.iter().enumerate() {
        if let Rule::Fixed(per) = l.rule {
            if per <= 0 || l.amount <= 0 || l.per_unit <= 0 {
                continue;
            }
            let full = div_round(
                i128::from(per) * i128::from(l.units.max(0)),
                i128::from(l.per_unit),
            )?;
            let part = div_round(i128::from(full) * i128::from(paid[i]), i128::from(l.amount))?;
            out[i] = part.min(paid[i]);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pct(amount: i64, bp: i64) -> LineIn {
        LineIn {
            amount,
            units: 1,
            per_unit: 1,
            rule: Rule::Percent(bp),
        }
    }

    #[test]
    fn split_keeps_total() {
        assert_eq!(split(100, &[1, 1, 1]), Some(vec![34, 33, 33]));
        assert_eq!(split(0, &[5, 0]), Some(vec![0, 0]));
        assert_eq!(split(7, &[0, 0]), Some(vec![0, 0]));
        assert_eq!(split(1_996, &[200_000, 0]), Some(vec![1_996, 0]));
        assert_eq!(split(5, &[-1]), None);
    }

    #[test]
    fn common_rate_matches_whole_check() {
        // Фильтр 500 с, масло 2000 с, услуга 0: 0,5 % от 2500 с — 12,50, как прежде одним числом.
        let lines = [pct(50_000, 50), pct(200_000, 50), pct(0, 50)];
        let v = accrue(250_000, &lines).unwrap();
        assert_eq!(v, vec![250, 1_000, 0]);
        // Часть баллами: начисление с денег — 1996 с → 9,98.
        let v = accrue(199_600, &[pct(200_000, 50), pct(0, 50)]).unwrap();
        assert_eq!(v.iter().sum::<i64>(), 998);
        // Три строки по 1 тыйыну денег: сумма округляется один раз.
        let v = accrue(301, &[pct(100, 5_000), pct(100, 5_000), pct(101, 5_000)]).unwrap();
        assert_eq!(v.iter().sum::<i64>(), 151);
    }

    #[test]
    fn product_category_common_order() {
        assert_eq!(
            pick(Some(Rule::None), Some(Rule::Percent(100)), 50),
            (Rule::None, Source::Product)
        );
        assert_eq!(
            pick(None, Some(Rule::Fixed(1_000)), 50),
            (Rule::Fixed(1_000), Source::Category)
        );
        assert_eq!(pick(None, None, 50), (Rule::Percent(50), Source::Common));
        assert_eq!(rule_of("inherit", 5), None);
        assert_eq!(rule_of("percent", 5), Some(Rule::Percent(5)));
    }

    #[test]
    fn fixed_per_unit_in_money_share() {
        // 10 с за литр, 4 л розливом за 2000 с наличными — 40 баллов.
        let oil = LineIn {
            amount: 200_000,
            units: 4_000,
            per_unit: 1_000,
            rule: Rule::Fixed(1_000),
        };
        assert_eq!(accrue(200_000, &[oil]).unwrap(), vec![4_000]);
        // Половина в долг — половина баллов.
        assert_eq!(accrue(100_000, &[oil]).unwrap(), vec![2_000]);
        // Не больше оплаченного за строку; подарок — ничего.
        let cheap = LineIn {
            amount: 1_000,
            units: 2,
            per_unit: 1,
            rule: Rule::Fixed(5_000),
        };
        assert_eq!(accrue(1_000, &[cheap]).unwrap(), vec![1_000]);
        let gift = LineIn { amount: 0, ..cheap };
        assert_eq!(accrue(0, &[gift]).unwrap(), vec![0]);
    }

    #[test]
    fn none_and_mixed_rates() {
        let lines = [
            LineIn {
                amount: 50_000,
                units: 1,
                per_unit: 1,
                rule: Rule::None,
            },
            pct(100_000, 200),
            pct(100_000, 50),
        ];
        assert_eq!(accrue(250_000, &lines).unwrap(), vec![0, 2_000, 500]);
    }
}
