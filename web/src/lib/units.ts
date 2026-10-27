// Количество и цена за единицу по единице учёта товара: масло — литры, весовой товар — кг,
// остальное — штуки (проверка 20.10.2026). Учёт в целых мл, граммах и тыйынах; здесь показ и ввод.
// Ввод масла — «канистры + литры», весового товара — килограммы (SPEC-36 §13, ADR-123): в учёте
// по-прежнему одно целое число мл или граммов (SPEC-03, инвариант 1).
import { formatKg, formatLiters, litersInput, parseLiters } from './format'
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

/** Тара от 30 л — бочка: подпись «тара по 208 л», а не «кан.». */
const BARREL_ML = 30_000

/** «кан. по 4 л» у канистры, «тара по 208 л» у бочки. */
export function containerLabel(containerMl: number): string {
  return `${containerMl >= BARREL_ML ? 'тара' : 'кан.'} по ${formatLiters(containerMl)}`
}

/** Целые канистры и остаток в мл — делением без остатка, как в `formatOilStock`. */
export function splitOil(ml: number, containerMl: number): { cans: number; restMl: number } {
  if (containerMl <= 0 || ml <= 0) return { cans: 0, restMl: Math.max(ml, 0) }
  return { cans: Math.trunc(ml / containerMl), restMl: ml % containerMl }
}

/** Поля ввода из мл: 34 500 мл по 4 л → «8» и «2,5»; без остатка — пустые литры. */
export function oilFields(ml: number, containerMl: number): { cans: string; liters: string } {
  const { cans, restMl } = splitOil(ml, containerMl)
  return { cans: String(cans), liters: restMl ? litersInput(restMl) : '' }
}

/**
 * «Канистры + литры» → целые мл: канистры × тара + литры × 1000, литры до 3 знаков.
 * Пустое поле — 0; оба пустые — `'empty'` (не считали); неверный ввод — null.
 */
export function parseOil(cans: string, liters: string, containerMl: number): number | 'empty' | null {
  const c = cans.trim()
  const l = liters.trim()
  if (!c && !l) return 'empty'
  if (c && !/^\d+$/.test(c)) return null
  const n = c ? Number(c) : 0
  const rest = l ? parseLiters(l) : 0
  if (rest === null) return null
  const ml = n * containerMl + rest
  return Number.isSafeInteger(ml) ? ml : null
}

/** Масло в виде «8 кан. + 2,5 л»; без канистр — «2,5 л», без остатка — «8 кан.»; минус — «−…». */
export function oilCountText(ml: number, containerMl: number): string {
  if (ml < 0) return `−${oilCountText(-ml, containerMl)}`
  if (containerMl <= 0) return formatLiters(ml)
  const { cans, restMl } = splitOil(ml, containerMl)
  if (cans === 0) return formatLiters(restMl)
  return restMl ? `${cans} кан. + ${formatLiters(restMl)}` : `${cans} кан.`
}

/** «2,5» кг → 2 500 г; до трёх знаков после запятой, иначе null. */
export const parseKg = (input: string): number | null => parseLiters(input)

/** Граммы для поля ввода: 129 300 → «129,3». */
export const kgInput = (g: number): string => litersInput(g)

/** Количество при пересчёте: масло в таре — «8 кан. + 2,5 л», остальное — как `qtyText`. */
export function countText(p: { unit: string; container_ml: number | null }, q: number): string {
  return p.unit === 'ml' && p.container_ml ? oilCountText(q, p.container_ml) : qtyText(p.unit, q)
}

/** Цена за литр из цены канистры: 1 800 с за 4 л → 450 с, до тыйына. */
export const perLiter = (perCan: number, containerMl: number): number => divRound(perCan * 1000, containerMl)

/** Цена канистры из цены за литр: 450 с × 4 л → 1 800 с, до тыйына. */
export const perCan = (perLiterTyiyn: number, containerMl: number): number => divRound(perLiterTyiyn * containerMl, 1000)

/** Сумма по цене за литр или за кг: divRound(цена × мл (г), 1000) — одно округление. */
export const byThousandths = (price: number, qty: number): number => divRound(price * qty, 1000)

/**
 * Вязкость из названия: «QUAKER STATE 5W30 4L» → «5W-30», «Lukoil Luxe 5/40» → «5W-40»,
 * «Castrol Transmax 75W-90» → «75W-90». Не нашлась — null. Так же разбирает сервер (`name_marks`).
 */
export function viscosityFromName(name: string): string | null {
  const m = /(?:^|[^\d.,])(\d{1,2})\s*[wW/]\s*-?\s*(\d{2})(?!\d)/.exec(name)
  return m ? `${Number(m[1])}W-${m[2]}` : null
}

/** Ключ для сравнения вязкостей: «5w30», «5W-30» и «5W 30» — одно. */
export const viscosityKey = (v: string): string => v.toUpperCase().replace(/[^0-9W]/g, '')
