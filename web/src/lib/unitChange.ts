// Единица и объём канистры товара (задача 002, П4; задача 003, Н10; ADR-139). Пересчёт
// проводит сервер одной операцией; здесь — то же правило для показа «было → станет» до
// подтверждения. Целые мл, граммы, мм и штуки, без float. Кг и м (ADR-174, ADR-178): со штуками — по
// коэффициенту «1 шт = X кг/м» (`pieceQty`, тысячные); без коэффициента и между разными размерностями —
// только без остатка (сервер помечает такую смену `reset`).
import { formatLiters } from './format'
import { isKgM, qtyText, type StockUnit } from './units'

export type { StockUnit }

export interface UnitTarget {
  unit: StockUnit
  /** Объём канистры, мл — для литров. */
  containerMl: number | null
  /** «1 шт = N мл». */
  pieceMl: number | null
  /** «1 шт = X кг/м» — тысячные: граммы или мм (штуки ↔ кг/м с остатком). */
  pieceQty?: number | null
  /** Литры → литры с другой канистрой: канистр столько же или литров столько же. */
  keep: 'count' | 'volume'
}

export interface UnitPlan {
  /** Остаток: новое = старое × mul / div. */
  mul: number
  div: number
  /** Цена: новая = старая × priceMul / priceDiv. */
  priceMul: number
  priceDiv: number
  /** Разные размерности без коэффициента: только без остатка во всех филиалах. */
  reset?: boolean
}

const LIMIT_ML = 1_000_000

const gcd = (a: number, b: number): number => {
  let [x, y] = [Math.abs(a), Math.abs(b)]
  while (y) [x, y] = [y, x % y]
  return Math.max(x, 1)
}

function checkMl(v: number | null, what: string): number | string {
  if (v === null || !Number.isSafeInteger(v) || v <= 0) return `Укажите ${what}`
  if (v > LIMIT_ML) return `${what[0].toUpperCase()}${what.slice(1)} — не больше 1000 л`
  return v
}

/** Как пересчитать — то же правило, что у сервера (`product_units::plan`); ошибка — текстом. */
export function unitPlan(fromUnit: string, fromContainerMl: number | null, t: UnitTarget): UnitPlan | string {
  const to = t.unit
  if (fromUnit === 'g' && to === 'g') return 'Товар уже в килограммах — менять нечего'
  if (fromUnit === 'mm' && to === 'mm') return 'Товар уже в метрах — менять нечего'
  const coef = t.pieceQty ?? null
  // Штуки ↔ кг/м по «1 шт = X кг/м»: шт → кг/м — остаток × X, цена × 1000 / X; обратно — наоборот.
  if (coef !== null && fromUnit === 'piece' && isKgM(to)) {
    const x = checkQty(coef, to)
    if (typeof x === 'string') return x
    return { mul: x, div: 1, priceMul: 1000, priceDiv: x }
  }
  if (coef !== null && (fromUnit === 'g' || fromUnit === 'mm') && to === 'piece') {
    const x = checkQty(coef, fromUnit)
    if (typeof x === 'string') return x
    return { mul: 1, div: x, priceMul: x, priceDiv: 1000 }
  }
  // Разные размерности без коэффициента — только без остатка; цена канистры c мл → за кг/м ×1000 / c,
  // иначе то же число за новую единицу цены.
  if (fromUnit !== to && (fromUnit === 'g' || fromUnit === 'mm' || isKgM(to))) {
    if (to === 'ml') {
      const c = checkMl(t.containerMl, 'объём канистры')
      if (typeof c === 'string') return c
    }
    const fromCan = fromUnit === 'ml' && fromContainerMl ? fromContainerMl : null
    return { mul: 1, div: 1, priceMul: fromCan ? 1000 : 1, priceDiv: fromCan ?? 1, reset: true }
  }
  if (fromUnit === 'ml' && t.unit === 'piece') {
    const n = checkMl(t.pieceMl, 'сколько литров в одной штуке')
    if (typeof n === 'string') return n
    return { mul: 1, div: n, priceMul: n, priceDiv: fromContainerMl ?? n }
  }
  if (fromUnit === 'piece' && t.unit === 'ml') {
    const c = checkMl(t.containerMl, 'объём канистры')
    if (typeof c === 'string') return c
    const n = t.pieceMl === null ? c : checkMl(t.pieceMl, 'сколько литров в одной штуке')
    if (typeof n === 'string') return n
    return { mul: n, div: 1, priceMul: c, priceDiv: n }
  }
  if (fromUnit === 'ml' && t.unit === 'ml') {
    const c = checkMl(t.containerMl, 'объём канистры')
    if (typeof c === 'string') return c
    if (c === fromContainerMl) return 'Объём канистры тот же — менять нечего'
    if (t.keep === 'volume' || !fromContainerMl) return { mul: 1, div: 1, priceMul: 1, priceDiv: 1 }
    const g = gcd(c, fromContainerMl)
    return { mul: c / g, div: fromContainerMl / g, priceMul: 1, priceDiv: 1 }
  }
  return 'Товар уже в штуках — менять нечего'
}

/** Коэффициент «1 шт = X кг/м» в тысячных: больше нуля, не больше 1000 кг (м). */
function checkQty(v: number, unit: string): number | string {
  const word = unit === 'mm' ? 'метров' : 'килограммов'
  if (!Number.isSafeInteger(v) || v <= 0) return `Укажите, сколько ${word} в одной штуке`
  if (v > LIMIT_ML) return `В одной штуке — не больше 1000 ${unit === 'mm' ? 'м' : 'кг'}`
  return v
}

/** Пара единиц со «1 шт = X кг/м»: штуки ↔ кг или м. */
export const byPieceQty = (fromUnit: string, toUnit: string): boolean =>
  (fromUnit === 'piece' && (toUnit === 'g' || toUnit === 'mm')) || (toUnit === 'piece' && (fromUnit === 'g' || fromUnit === 'mm'))

/**
 * Без коэффициента друг в друга не пересчитываются — только без остатка (ADR-174, ADR-178): кг и м
 * со всем остальным. Литры ↔ штуки — по «1 шт = N л»; штуки ↔ кг/м — по коэффициенту (`coef`).
 */
export const zeroOnly = (fromUnit: string, toUnit: string, coef = false): boolean =>
  fromUnit !== toUnit && (fromUnit === 'g' || fromUnit === 'mm' || toUnit === 'g' || toUnit === 'mm') && !(coef && byPieceQty(fromUnit, toUnit))

/** Остаток после пересчёта; не делится нацело — null. */
export function recount(qty: number, p: UnitPlan): number | null {
  const n = qty * p.mul
  if (!Number.isSafeInteger(n) || n % p.div !== 0) return null
  return n / p.div
}

/** Деление целых с округлением половины от нуля — как `div_round` сервера. */
function divRound(n: number, d: number): number {
  const q = Math.trunc(n / d)
  const r = n - q * d
  return 2 * Math.abs(r) >= d ? q + Math.sign(n) : q
}

/** Цена после пересчёта: цена канистры → цена штуки и обратно. */
export function priceAfter(price: number, p: UnitPlan): number {
  return divRound(price * p.priceMul, p.priceDiv)
}

/** Остаток словами в своей единице: «12 л», «29 шт», «2,5 кг», «1,25 м». */
export const stockText = (qty: number, unit: StockUnit): string => qtyText(unit, qty)

/** Единица товара словами: «литры, канистра 4 л», «штуки». */
export function unitText(unit: StockUnit, containerMl: number | null): string {
  if (unit === 'ml') return containerMl ? `литры, канистра ${formatLiters(containerMl)}` : 'литры'
  if (unit === 'g') return 'килограммы'
  if (unit === 'mm') return 'метры'
  return 'штуки'
}
