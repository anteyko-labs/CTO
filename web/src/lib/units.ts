// Количество и цена за единицу по единице учёта товара: масло — литры, весовой товар — кг,
// остальное — штуки (проверка 20.10.2026). Учёт в целых мл, граммах и тыйынах; здесь только показ.
import { formatKg, formatLiters } from './format'
import { divRound } from './money'

export type Unit = 'piece' | 'ml' | 'g'

/** Количество по единице учёта: 3500 мл → «3,5 л», 12 000 г → «12 кг», 3 → «3 шт». */
export function qtyText(unit: Unit | string | undefined, qty: number): string {
  if (unit === 'ml') return formatLiters(qty)
  if (unit === 'g') return formatKg(qty)
  return `${qty} шт`
}

type Priced = {
  unit: Unit
  container_ml: number | null
  stock_qty: number
  avg_cost_tyiyn?: number | null
  stock_value_tyiyn?: number | null
}

/**
 * Средняя цена для показа и её единица. Весовой товар — за кг (розлив без тары — за л): из
 * стоимости остатка, делённой на граммы (целочисленно, округление один раз), без остатка — средняя
 * за грамм × 1000. Масло в таре — за канистру, остальное — за штуку. Нет средней — null.
 */
export function avgPrice(p: Priced): { tyiyn: number; per: string } | null {
  if (p.avg_cost_tyiyn == null) return null
  if (p.unit === 'ml' && p.container_ml) return { tyiyn: p.avg_cost_tyiyn, per: `за ${formatLiters(p.container_ml)}` }
  if (p.unit === 'g' || p.unit === 'ml') {
    // Цена за грамм или миллилитр — доли сома: показываем за 1000 (кг, л).
    const per1000 = p.stock_qty > 0 && p.stock_value_tyiyn != null ? divRound(p.stock_value_tyiyn * 1000, p.stock_qty) : p.avg_cost_tyiyn * 1000
    return { tyiyn: per1000, per: p.unit === 'g' ? 'за кг' : 'за л' }
  }
  return { tyiyn: p.avg_cost_tyiyn, per: 'за шт' }
}
