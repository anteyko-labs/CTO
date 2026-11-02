// Единица и объём канистры товара (задача 002, П4; задача 003, Н10; ADR-139). Пересчёт
// проводит сервер одной операцией; здесь — то же правило для показа «было → станет» до
// подтверждения. Целые мл и штуки, без float.
import { formatKg, formatLiters } from './format'

export type StockUnit = 'ml' | 'piece' | 'g'

export interface UnitTarget {
  unit: StockUnit
  /** Объём канистры, мл — для литров. */
  containerMl: number | null
  /** «1 шт = N мл». */
  pieceMl: number | null
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
  // Килограммы (ADR-174) — только без остатка (`zeroOnly`): «1 шт = 1 кг», «1 л = 1 кг».
  if (fromUnit === 'g' && t.unit === 'g') return 'Товар уже в килограммах — менять нечего'
  if (fromUnit === 'piece' && t.unit === 'g') return { mul: 1000, div: 1, priceMul: 1, priceDiv: 1 }
  if (fromUnit === 'ml' && t.unit === 'g') return { mul: 1, div: 1, priceMul: 1000, priceDiv: fromContainerMl ?? 1000 }
  if (fromUnit === 'g' && t.unit === 'piece') return { mul: 1, div: 1000, priceMul: 1, priceDiv: 1 }
  if (fromUnit === 'g' && t.unit === 'ml') {
    const c = checkMl(t.containerMl, 'объём канистры')
    if (typeof c === 'string') return c
    return { mul: 1, div: 1, priceMul: c, priceDiv: 1000 }
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

/** Килограммы и штуки (литры) друг в друга не пересчитываются — только без остатка (ADR-174). */
export const zeroOnly = (fromUnit: string, toUnit: string): boolean => (fromUnit === 'g') !== (toUnit === 'g')

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

/** Остаток словами в своей единице: «12 л», «29 шт». */
export function stockText(qty: number, unit: string): string {
  return unit === 'ml' ? formatLiters(qty) : unit === 'g' ? formatKg(qty) : `${qty} шт`
}

/** Единица товара словами: «литры, канистра 4 л», «штуки». */
export function unitText(unit: string, containerMl: number | null): string {
  if (unit === 'ml') return containerMl ? `литры, канистра ${formatLiters(containerMl)}` : 'литры'
  if (unit === 'g') return 'килограммы'
  return 'штуки'
}
