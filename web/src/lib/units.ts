// Количество и цена за единицу по единице учёта товара: масло — литры, на вес — кг, в длину —
// метры, остальное — штуки (проверка 20.10.2026; ADR-174, ADR-178). Учёт в целых мл, граммах,
// миллиметрах и тыйынах; здесь показ и ввод. Ввод масла — «канистры + литры», кг и м — десятичным
// с запятой или точкой, до 3 знаков (SPEC-36 §13, ADR-123): в учёте по-прежнему одно целое число
// мл, граммов или миллиметров (SPEC-03, инвариант 1). Перевод «текст ↔ целое» — только здесь.
import { formatKg, formatLiters, formatMeters, litersInput, parseLiters } from './format'
import { divRound } from './money'

/** Единица учёта товара в API: штуки, мл, граммы, миллиметры. */
export type StockUnit = 'piece' | 'ml' | 'g' | 'mm'
/** Прежнее имя. */
export type Unit = StockUnit

const UNITS: readonly StockUnit[] = ['piece', 'ml', 'g', 'mm']

/** Строка из API → единица; неизвестная — null (никакого молчаливого «шт»). */
export const asUnit = (u: string | null | undefined): StockUnit | null =>
  UNITS.includes(u as StockUnit) ? (u as StockUnit) : null

const never = (u: never): never => {
  throw new Error(`Неизвестная единица учёта: ${String(u)}`)
}

/** Учёт тысячными: мл, граммы, миллиметры — ввод десятичным до 3 знаков. */
export function isMilli(unit: StockUnit): boolean {
  switch (unit) {
    case 'piece':
      return false
    case 'ml':
    case 'g':
    case 'mm':
      return true
    default:
      return never(unit)
  }
}

/** Кг и м — «товар на вес и в длину»: без канистры, цена за 1000 единиц учёта. */
export const isKgM = (unit: StockUnit): boolean => unit === 'g' || unit === 'mm'

/** Коротко для подписей: «шт», «л», «кг», «м». */
export function unitShort(unit: StockUnit): string {
  switch (unit) {
    case 'piece':
      return 'шт'
    case 'ml':
      return 'л'
    case 'g':
      return 'кг'
    case 'mm':
      return 'м'
    default:
      return never(unit)
  }
}

/** За что цена продажи: «за шт», «за кан.» (масло в таре), «за л», «за кг», «за м». */
export function perWord(unit: StockUnit, containerMl: number | null = null): string {
  switch (unit) {
    case 'piece':
      return 'за шт'
    case 'ml':
      return containerMl ? 'за кан.' : 'за л'
    case 'g':
      return 'за кг'
    case 'mm':
      return 'за м'
    default:
      return never(unit)
  }
}

/**
 * Сколько единиц учёта в цене (как `catalog::price_unit` на сервере): масло — канистра (без тары —
 * литр, 1000 мл), кг и м — 1000 г/мм, штука — 1.
 */
export function priceUnits(unit: StockUnit, containerMl: number | null): number {
  switch (unit) {
    case 'piece':
      return 1
    case 'ml':
      return containerMl ?? 1000
    case 'g':
    case 'mm':
      return 1000
    default:
      return never(unit)
  }
}

/** Вид строки чека по единице товара: мл → канистра, г → на вес, мм → в длину, иначе штука. */
export function lineKindOf(unit: StockUnit): 'piece' | 'container' | 'weight' | 'length' {
  switch (unit) {
    case 'piece':
      return 'piece'
    case 'ml':
      return 'container'
    case 'g':
      return 'weight'
    case 'mm':
      return 'length'
    default:
      return never(unit)
  }
}

/** Строка чека в тысячных (розлив — мл, на вес — г, в длину — мм): ввод десятичным, сумма — цена × q / 1000. */
export const isMilliKind = (kind: string): boolean => kind === 'pour' || kind === 'weight' || kind === 'length'

/**
 * Количество строки чека словами по её виду: розлив «2,5 л», канистры «2 кан. × 4 л», на вес «1,5 кг»,
 * в длину «1,25 м», услуги «1 усл.», штуки «3 шт». Как `domain/qty.rs::sale_qty_text` на сервере.
 */
export function saleQtyText(kind: string, qty: number, containerMl?: number | null): string {
  switch (kind) {
    case 'pour':
      return formatLiters(qty)
    case 'container':
      return `${qty} кан.${containerMl ? ` × ${formatLiters(containerMl)}` : ''}`
    case 'weight':
      return formatKg(qty)
    case 'length':
      return formatMeters(qty)
    case 'service':
      return `${qty} усл.`
    default:
      return `${qty} шт`
  }
}

/** Цена строки чека с единицей: у розлива «/л», на вес «/кг», в длину «/м», иначе без суффикса. */
export const salePriceSuffix = (kind: string): string =>
  kind === 'pour' ? '/л' : kind === 'weight' ? '/кг' : kind === 'length' ? '/м' : ''

/** Количество по единице учёта: 3500 мл → «3,5 л», 12 000 г → «12 кг», 1250 мм → «1,25 м», 3 → «3 шт». */
export function qtyText(unit: StockUnit, qty: number): string {
  switch (unit) {
    case 'piece':
      return `${qty} шт`
    case 'ml':
      return formatLiters(qty)
    case 'g':
      return formatKg(qty)
    case 'mm':
      return formatMeters(qty)
    default:
      return never(unit)
  }
}

/**
 * Ввод количества → целое в единицах учёта: штуки — целым, мл/г/мм — десятичным «2,5» или «2.5»
 * до 3 знаков (2 500). Минус, 4 знака, пусто, буквы — null. 0 — допустим (в ревизии «не нашли»).
 */
export function parseQty(unit: StockUnit, text: string): number | null {
  if (isMilli(unit)) return text.trim() ? parseLiters(text) : null
  const t = text.trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  return Number.isSafeInteger(n) ? n : null
}

/** Целое в единицах учёта → поле ввода: 2500 г → «2,5», 3 шт → «3». */
export const qtyInputText = (unit: StockUnit, q: number): string => (isMilli(unit) ? litersInput(q) : String(q))

type Priced = {
  unit: StockUnit
  container_ml: number | null
  stock_qty: number
  avg_cost_tyiyn?: number | null
  stock_value_tyiyn?: number | null
}

/**
 * Средняя цена для показа и её единица. Кг- и м-товар — за кг и за м (розлив без тары — за л): из
 * стоимости остатка, делённой на граммы (мм) (целочисленно, округление один раз), без остатка — средняя
 * за грамм × 1000. Масло в таре — за канистру, остальное — за штуку. Нет средней — null.
 */
export function avgPrice(p: Priced): { tyiyn: number; per: string } | null {
  if (p.avg_cost_tyiyn == null) return null
  if (p.unit === 'ml' && p.container_ml) return { tyiyn: p.avg_cost_tyiyn, per: `за ${formatLiters(p.container_ml)}` }
  if (isMilli(p.unit)) {
    // Цена за грамм, миллиметр или миллилитр — доли сома: показываем за 1000 (кг, м, л).
    const per1000 = p.stock_qty > 0 && p.stock_value_tyiyn != null ? divRound(p.stock_value_tyiyn * 1000, p.stock_qty) : p.avg_cost_tyiyn * 1000
    return { tyiyn: per1000, per: perWord(p.unit) }
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

/** Тысячные (кг, м, л) из ввода: «2,5» → 2 500; до трёх знаков после запятой, иначе null. */
export const parseMilli = (input: string): number | null => parseLiters(input)

/** Тысячные для поля ввода: 129 300 → «129,3». */
export const milliInput = (q: number): string => litersInput(q)

/** «2,5» кг → 2 500 г — то же, что `parseMilli`. */
export const parseKg = parseMilli

/** Граммы для поля ввода — то же, что `milliInput`. */
export const kgInput = milliInput

/** Количество при пересчёте: масло в таре — «8 кан. + 2,5 л», остальное — как `qtyText`. */
export function countText(p: { unit: StockUnit; container_ml: number | null }, q: number): string {
  return p.unit === 'ml' && p.container_ml ? oilCountText(q, p.container_ml) : qtyText(p.unit, q)
}

/** Цена за литр из цены канистры: 1 800 с за 4 л → 450 с, до тыйына. */
export const perLiter = (perCan: number, containerMl: number): number => divRound(perCan * 1000, containerMl)

/** Цена канистры из цены за литр: 450 с × 4 л → 1 800 с, до тыйына. */
export const perCan = (perLiterTyiyn: number, containerMl: number): number => divRound(perLiterTyiyn * containerMl, 1000)

/** Сумма по цене за литр, кг или м: divRound(цена × мл (г), 1000) — одно округление. */
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
