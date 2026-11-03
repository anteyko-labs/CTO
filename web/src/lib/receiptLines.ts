// Строка нового прихода: количество, сумма по закупке и по цене продажи; подсказка прошлой закупки
// (SPEC-03, SPEC-27). Масло — канистрами или литрами (режим «кан. | л», SPEC-36 §13, ADR-123), на вес —
// килограммами, в длину — метрами (ADR-178). Только целые тыйыны, мл, граммы и мм; деление — одно, с
// округлением (divRound).
import { formatLiters, formatSom, litersInput, parseLiters, parsePermille, parseSom, somInput } from './format'
import { divRound, markupPermille, saleFromMarkup } from './money'
import type { PriceEntry } from './purchasePrice'
import type { LastReceipt, Product } from './types'
import { byThousandths, isKgM, milliInput, oilCountText, parseMilli, perCan, perLiter, priceUnits } from './units'

export const isOil = (p: Pick<Product, 'unit' | 'container_ml'>): p is Product & { container_ml: number } =>
  p.unit === 'ml' && Boolean(p.container_ml)

/** Масло в строке: канистрами (цена за канистру) или литрами (цена за литр). */
export type QtyMode = 'can' | 'l'

/** Режим цены масла по умолчанию — «Настройки → Склад» (задача 003, Н7, ADR-150). */
export interface OilPriceModes {
  receipt: QtyMode
  stockIn: QtyMode
}

/** Как было до настройки: в приходе — канистры, в оприходовании — литры (SPEC-36 §13). */
export const DEFAULT_OIL_PRICE_MODES: OilPriceModes = { receipt: 'can', stockIn: 'l' }

/** Режим новой строки масла: из настройки склада, без неё — канистры в приходе, литры в оприходовании. */
export const defaultMode = (stockIn: boolean, modes: OilPriceModes = DEFAULT_OIL_PRICE_MODES): QtyMode =>
  stockIn ? modes.stockIn : modes.receipt

/** Строка масла в литрах: цена в ней — за литр. */
export const inLiters = (p: Pick<Product, 'unit' | 'container_ml'>, mode: QtyMode | undefined): boolean => isOil(p) && mode === 'l'

/** Тара, если цена строки — за литр: для перевода в цену канистры (наценка, «за канистру ≈»). */
export const literTare = (p: Pick<Product, 'unit' | 'container_ml'>, mode: QtyMode | undefined): number | undefined =>
  inLiters(p, mode) ? (p.container_ml ?? undefined) : undefined

/** Целое положительное число из строки или null. */
export function parseCount(s: string): number | null {
  const t = s.trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  return Number.isSafeInteger(n) && n > 0 ? n : null
}

const positive = (v: number | null): number | null => (v !== null && v > 0 ? v : null)

/** Расчёт строки: количество в единицах учёта, суммы, цена продажи; `unitPrice` — цена для сервера. */
export interface LineCalc {
  /** Сколько этикеток: штуки, целые канистры; на вес — одна. */
  n: number
  qty: number
  cost: number
  sale: number
  salePrice: number
  /** Цена закупки за единицу цены товара (штуку, канистру, кг) — так её принимает оприходование. */
  unitPrice: number
}

export interface LineInput {
  product: Product
  count: string
  price: string
  salePrice?: string
  mode?: QtyMode
  /** Масло канистрами, а вводили цену за литр (задача 003, Н7): сумма — от неё. */
  literPrice?: string
}

/**
 * Расчёт строки прихода (`stockIn = false`) или оприходования (`true`); текст ошибки, если ввод неверен.
 * Масло канистрами: сумма = канистры × цена канистры. Литрами: сумма = divRound(цена за л × мл, 1000);
 * оприходование сервер считает от цены канистры, поэтому и здесь — от неё (одно округление).
 * Канистрами, но вводили цену за литр (`literPrice`, ADR-150): в приходе сумма = divRound(цена за л ×
 * мл, 1000) — одно округление, без округлённой цены канистры; в оприходовании — от цены канистры.
 * На вес и в длину: кг (м) до 3 знаков, цена за кг (м), сумма = divRound(цена × г (мм), 1000). `salePrice` —
 * новая цена продажи строки (SPEC-28 §1), за штуку, канистру, кг или м; пусто — текущая цена товара.
 */
export function lineCalc(l: LineInput, stockIn = false): LineCalc | string {
  const p = l.product
  const liters = inLiters(p, l.mode ?? defaultMode(stockIn))
  const weight = isKgM(p.unit)
  let qty: number | null
  if (weight) qty = positive(parseMilli(l.count))
  else if (liters) qty = positive(parseLiters(l.count))
  else {
    const n = parseCount(l.count)
    qty = n === null ? null : isOil(p) ? n * p.container_ml : n
  }
  if (qty === null) return weight ? (p.unit === 'mm' ? 'укажите метры' : 'укажите килограммы') : liters ? 'укажите литры' : isOil(p) ? 'укажите число канистр' : 'укажите количество'
  const price = parseSom(l.price)
  if (price === null) return 'укажите цену'
  const salePrice = l.salePrice === undefined || l.salePrice.trim() === '' ? p.sale_price_tyiyn : parseSom(l.salePrice)
  if (salePrice === null) return 'укажите цену продажи'
  let cost: number
  let sale: number
  let unitPrice = price
  let n: number
  if (weight) {
    cost = byThousandths(price, qty)
    sale = byThousandths(salePrice, qty)
    n = 1
  } else if (liters && isOil(p)) {
    const c = p.container_ml
    unitPrice = perCan(price, c)
    cost = stockIn ? divRound(unitPrice * qty, c) : byThousandths(price, qty)
    sale = divRound(salePrice * qty, c)
    n = Math.trunc(qty / c)
  } else if (isOil(p) && l.literPrice !== undefined) {
    const perL = parseSom(l.literPrice)
    if (perL === null) return 'укажите цену'
    const c = p.container_ml
    n = qty / c
    unitPrice = perCan(perL, c)
    cost = stockIn ? n * unitPrice : byThousandths(perL, qty)
    sale = n * salePrice
  } else {
    // Штуки и целые канистры: суммы — умножением, без деления.
    n = isOil(p) ? qty / p.container_ml : qty
    cost = n * price
    sale = n * salePrice
  }
  if (![qty, cost, sale, unitPrice].every(Number.isSafeInteger)) return 'слишком большое число'
  return { n, qty, cost, sale, salePrice, unitPrice }
}

/** Поле количества новой строки: штука, канистра (в литрах — её объём), килограмм или метр. */
export function firstCount(p: Pick<Product, 'unit' | 'container_ml'>, mode: QtyMode | undefined): string {
  return inLiters(p, mode) ? litersInput(p.container_ml ?? 0) : '1'
}

/** Повторный скан: ещё штука, канистра, килограмм или метр; неразобранное — заново с одной. */
export function nextCount(p: Pick<Product, 'unit' | 'container_ml'>, mode: QtyMode | undefined, count: string): string {
  if (isKgM(p.unit)) return milliInput((positive(parseMilli(count)) ?? 0) + 1000)
  if (inLiters(p, mode)) {
    const c = p.container_ml ?? 0
    return litersInput((positive(parseLiters(count)) ?? 0) + c)
  }
  const n = parseCount(count)
  return String(n === null ? 1 : n + 1)
}

/** Наценка для поля ввода: 250 → «25,0», −5 → «-0,5». */
export function markupInput(p: number | null): string {
  if (p === null) return ''
  const abs = Math.abs(p)
  return `${p < 0 ? '-' : ''}${Math.trunc(abs / 10)},${abs % 10}`
}

/** Поля строки прихода, связанные между собой: цена по накладной, наценка, цена продажи. */
export interface PriceFields {
  price: string
  markup: string
  salePrice: string
  /** Строка масла канистрами: введённая цена за литр; не задана — вводили цену канистры. */
  literPrice?: string
  /** Масло: введённая цена продажи за литр (задача 027); не задана — вводили цену канистры. */
  saleLiterPrice?: string
}

/** Закупка в единицах цены продажи: в режиме «л» цена литра × тара = цена канистры (ADR-074/075). */
function costPerSaleUnit(price: string, tare: number | undefined): number | null {
  const cost = parseSom(price)
  return cost === null ? null : tare ? perCan(cost, tare) : cost
}

/** Наценка из цен строки; пусто, если цена по накладной или продажи не разобрана. */
function markupOf(price: string, salePrice: string, tare: number | undefined): string {
  const cost = costPerSaleUnit(price, tare)
  const sale = parseSom(salePrice)
  return cost === null || sale === null ? '' : markupInput(markupPermille(cost, sale))
}

/**
 * Начальные поля строки: цена по накладной (прошлая закупка — за штуку, канистру, кг), цена продажи —
 * текущая цена товара. `tare` — строка масла в литрах: цена закупки — за литр.
 */
export function initialPrices(p: Product, tare?: number): PriceFields {
  const last = p.last_purchase_price_tyiyn
  const price = last != null ? somInput(tare ? perLiter(last, tare) : last) : ''
  const salePrice = somInput(p.sale_price_tyiyn)
  return { price, salePrice, markup: markupOf(price, salePrice, tare) }
}

/** Поменяли цену по накладной: цена продажи остаётся, наценка пересчитывается; цена за литр — от неё. */
export const withCost = (l: PriceFields, price: string, tare?: number): PriceFields => ({
  ...l,
  price,
  markup: markupOf(price, l.salePrice, tare),
  literPrice: undefined,
})

/**
 * Строка масла канистрами — ввели цену за литр (задача 003, Н7): цена канистры = perCan, наценка — к ней;
 * введённый литр хранится, сумма строки считается от него (`lineCalc`).
 */
export function withLiterPrice(l: PriceFields, literPrice: string, containerMl: number): PriceFields {
  const v = parseSom(literPrice)
  const price = v === null ? '' : somInput(perCan(v, containerMl))
  return { ...l, price, literPrice, markup: markupOf(price, l.salePrice, undefined) }
}

/**
 * Цена прихода, введённая в карточке нового товара (задача 003, Н7), — в строку: за литр в строке
 * канистрами — связанным полем «за литр», в строке литрами — как есть; иначе пересчёт perCan/perLiter.
 */
export function withEntry(l: PriceFields, e: PriceEntry, p: Pick<Product, 'unit' | 'container_ml'>, mode: QtyMode | undefined): PriceFields {
  const v = parseSom(e.text)
  if (!isOil(p) || v === null) return withCost(l, e.text)
  const c = p.container_ml
  if (mode === 'l') return withCost(l, e.per === 'l' ? e.text : somInput(perLiter(v, c)), c)
  return e.per === 'l' ? withLiterPrice(l, e.text, c) : withCost(l, e.text)
}

/** Поле «за литр» строки масла канистрами: введённое, иначе — из цены канистры (perLiter). */
export function literPriceText(l: Pick<PriceFields, 'price' | 'literPrice'>, containerMl: number): string {
  if (l.literPrice !== undefined) return l.literPrice
  const v = parseSom(l.price)
  return v === null ? '' : somInput(perLiter(v, containerMl))
}

/** Поменяли цену продажи: наценка пересчитывается от цены по накладной. */
export const withSalePrice = (l: PriceFields, salePrice: string, tare?: number): PriceFields => ({
  ...l,
  salePrice,
  markup: markupOf(l.price, salePrice, tare),
  saleLiterPrice: undefined,
})

/**
 * Масло — ввели цену продажи за литр (задача 027): хранится одна цена — канистры (ADR-074), она =
 * литр × объём с одним округлением (`perCan`); наценка — к ней. Введённый литр остаётся в поле.
 */
export function withSaleLiterPrice(l: PriceFields, literPrice: string, containerMl: number, tare?: number): PriceFields {
  const v = parseSom(literPrice)
  const salePrice = v === null ? '' : somInput(perCan(v, containerMl))
  return { ...l, salePrice, saleLiterPrice: literPrice, markup: markupOf(l.price, salePrice, tare) }
}

/** Поле «за литр» цены продажи масла: введённое, иначе — из цены канистры (perLiter). */
export function saleLiterText(l: Pick<PriceFields, 'salePrice' | 'saleLiterPrice'>, containerMl: number): string {
  if (l.saleLiterPrice !== undefined) return l.saleLiterPrice
  const v = parseSom(l.salePrice)
  return v === null ? '' : somInput(perLiter(v, containerMl))
}

/** Поменяли наценку: цена продажи = закупка × (1 + наценка), до тыйына (`saleFromMarkup`). */
export function withMarkup(l: PriceFields, markup: string, tare?: number): PriceFields {
  const cost = costPerSaleUnit(l.price, tare)
  const m = parsePermille(markup)
  const sale = cost === null || m === null ? null : saleFromMarkup(cost, m)
  return { ...l, markup, salePrice: sale === null ? l.salePrice : somInput(sale), saleLiterPrice: sale === null ? l.saleLiterPrice : undefined }
}

/**
 * Переключили «кан. | л» у строки масла: количество и цена закупки переводятся, цена продажи остаётся
 * за канистру. Литры, не делящиеся на канистры, остаются как есть — строка покажет ошибку.
 */
export function switchMode<L extends PriceFields & { product: Product; count: string; mode?: QtyMode }>(l: L, to: QtyMode): L {
  const p = l.product
  if (!isOil(p) || l.mode === to) return { ...l, mode: to }
  const c = p.container_ml
  const price = parseSom(l.price)
  let count = l.count
  let priceText = l.price
  if (to === 'l') {
    const n = parseCount(l.count)
    if (n !== null) count = litersInput(n * c)
    if (price !== null) priceText = somInput(perLiter(price, c))
  } else {
    const ml = positive(parseLiters(l.count))
    if (ml !== null && ml % c === 0) count = String(ml / c)
    if (price !== null) priceText = somInput(perCan(price, c))
  }
  // Цену за литр, введённую в канистрах, литры берут как есть; из литров в канистры она остаётся
  // введённой — сумма строки не сдвигается на тыйын (ADR-150).
  if (to === 'l' && l.literPrice !== undefined && parseSom(l.literPrice) !== null) priceText = l.literPrice
  const base = { price: l.price, salePrice: l.salePrice, markup: l.markup }
  const prices =
    to === 'can' && price !== null ? withLiterPrice(base, l.price, c) : withCost(base, priceText, to === 'l' ? c : undefined)
  return { ...l, ...prices, mode: to, count }
}

/** Подсказки строки масла в литрах: «= 4 кан. + 2 л» и «за канистру ≈ 1 800 с». */
export function literHints(l: LineInput): { split: string; perCan: string } | null {
  const p = l.product
  if (!isOil(p) || !inLiters(p, l.mode)) return null
  const ml = positive(parseLiters(l.count))
  const price = parseSom(l.price)
  const c = p.container_ml
  return {
    split: ml !== null && ml >= c ? `= ${oilCountText(ml, c)}` : '',
    perCan: price === null ? '' : `за канистру ${formatLiters(c)} ≈ ${formatSom(perCan(price, c))}`,
  }
}

/**
 * «в прошлый раз: 350 с, 12.10, Автоснаб»; `per` — «за канистру», «за кг». У масла в таре
 * (`containerMl`) — и цена за литр: «1 800 с за канистру (450 с за л)» (задача 003, Н7).
 */
export function lastText(r: LastReceipt, per?: string, containerMl?: number | null): string {
  // День и месяц по Бишкеку: приход в 01:00 по Бишкеку — ещё вчера по UTC.
  const d = new Date(r.created_at).toLocaleDateString('ru-RU', { timeZone: 'Asia/Bishkek', day: '2-digit', month: '2-digit' })
  const liter = containerMl ? ` (${formatSom(perLiter(r.price_tyiyn, containerMl))} за л)` : ''
  return ['в прошлый раз: ' + formatSom(r.price_tyiyn) + (per ? ` ${per}` : '') + liter, d, r.supplier_name].filter(Boolean).join(', ')
}

/** Единица прошлой закупки рядом с ценой: у масла — канистра, на вес — кг, в длину — м (SPEC-36 §13). */
export const lastPer = (p: Pick<Product, 'unit' | 'container_ml'>): string | undefined =>
  isOil(p) ? 'за канистру' : p.unit === 'g' ? 'за кг' : p.unit === 'mm' ? 'за м' : undefined

/** Насколько цена по накладной может отойти от прошлой закупки без предупреждения, %. */
export const PRICE_JUMP_PERCENT = 30

/**
 * Предупреждение, когда цена по накладной отличается от прошлой закупки больше чем на 30 %:
 * «Цена ниже, чем в прошлый раз (420 с), больше чем на 30 % — проверьте». Не запрет: цена могла
 * измениться. Прошлая закупка — за канистру: цена литра (`tare`) сравнивается в пересчёте на канистру.
 */
export function priceJumpText(price: string, last: Pick<LastReceipt, 'price_tyiyn'> | null | undefined, tare?: number): string {
  if (!last || last.price_tyiyn <= 0) return ''
  const p = costPerSaleUnit(price, tare)
  if (p === null || p <= 0) return ''
  if (Math.abs(p - last.price_tyiyn) * 100 <= last.price_tyiyn * PRICE_JUMP_PERCENT) return ''
  return `Цена ${p > last.price_tyiyn ? 'выше' : 'ниже'}, чем в прошлый раз (${formatSom(last.price_tyiyn)}), больше чем на ${PRICE_JUMP_PERCENT} % — проверьте`
}

/**
 * Цена по накладной за единицу цены товара (штуку, канистру, килограмм, метр) из проведённой строки:
 * сумма строки × единица / количество, до тыйына. Для колонок «наценка» и «цена продажи» накладной.
 */
export function unitCost(l: { unit: Product['unit']; container_ml: number | null; qty: number; cost_tyiyn: number }): number | null {
  const per = priceUnits(l.unit, l.container_ml)
  const n = l.cost_tyiyn * per
  return l.qty > 0 && Number.isSafeInteger(n) ? divRound(n, l.qty) : null
}

/**
 * Цена за литр проведённой строки масла в таре — только показ (задача 003, Н7): сумма × 1000 / мл,
 * до тыйына. Не масло или нет количества — null.
 */
export function unitCostPerLiter(l: { unit: string; container_ml: number | null; qty: number; cost_tyiyn: number }): number | null {
  if (l.unit !== 'ml' || !l.container_ml || l.qty <= 0) return null
  const n = l.cost_tyiyn * 1000
  return Number.isSafeInteger(n) ? divRound(n, l.qty) : null
}
