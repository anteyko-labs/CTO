// Строка нового прихода: количество, сумма по закупке и по цене продажи; подсказка прошлой закупки
// (SPEC-03, SPEC-27). Только целые тыйыны и мл: суммы — умножением, без деления.
import { formatSom, parsePermille, parseSom, somInput } from './format'
import { divRound, markupPermille, saleFromMarkup } from './money'
import type { LastReceipt, Product } from './types'

export const isOil = (p: Product): p is Product & { container_ml: number } => p.unit === 'ml' && Boolean(p.container_ml)

/** Целое положительное число из строки или null. */
export function parseCount(s: string): number | null {
  const t = s.trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  return Number.isSafeInteger(n) && n > 0 ? n : null
}

/**
 * Расчёт строки: количество в единицах учёта, сумма по закупке и по цене продажи, цена продажи за
 * штуку/канистру; текст ошибки, если ввод неверен. `salePrice` — новая цена продажи строки (SPEC-28
 * §1); пусто или не задано — текущая цена товара.
 */
export function lineCalc(l: { product: Product; count: string; price: string; salePrice?: string }):
  | { n: number; qty: number; cost: number; sale: number; salePrice: number }
  | string {
  const n = parseCount(l.count)
  if (n === null) return isOil(l.product) ? 'укажите число канистр' : 'укажите количество'
  const price = parseSom(l.price)
  if (price === null) return 'укажите цену'
  const salePrice = l.salePrice === undefined || l.salePrice.trim() === '' ? l.product.sale_price_tyiyn : parseSom(l.salePrice)
  if (salePrice === null) return 'укажите цену продажи'
  const qty = isOil(l.product) ? n * l.product.container_ml : n
  const cost = n * price
  // Цена продажи — за штуку или канистру, как и количество строки: сумма целая, без деления.
  const sale = n * salePrice
  if (!Number.isSafeInteger(qty) || !Number.isSafeInteger(cost) || !Number.isSafeInteger(sale)) return 'слишком большое число'
  return { n, qty, cost, sale, salePrice }
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
}

/** Наценка из цен строки; пусто, если цена по накладной или продажи не разобрана. */
function markupOf(price: string, salePrice: string): string {
  const cost = parseSom(price)
  const sale = parseSom(salePrice)
  return cost === null || sale === null ? '' : markupInput(markupPermille(cost, sale))
}

/** Начальные поля строки: цена по накладной (прошлая закупка), цена продажи — текущая цена товара. */
export function initialPrices(p: Product): PriceFields {
  const price = p.last_purchase_price_tyiyn != null ? somInput(p.last_purchase_price_tyiyn) : ''
  const salePrice = somInput(p.sale_price_tyiyn)
  return { price, salePrice, markup: markupOf(price, salePrice) }
}

/** Поменяли цену по накладной: цена продажи остаётся, наценка пересчитывается. */
export const withCost = (l: PriceFields, price: string): PriceFields => ({ ...l, price, markup: markupOf(price, l.salePrice) })

/** Поменяли цену продажи: наценка пересчитывается от цены по накладной. */
export const withSalePrice = (l: PriceFields, salePrice: string): PriceFields => ({ ...l, salePrice, markup: markupOf(l.price, salePrice) })

/** Поменяли наценку: цена продажи = закупка × (1 + наценка), до тыйына (`saleFromMarkup`). */
export function withMarkup(l: PriceFields, markup: string): PriceFields {
  const cost = parseSom(l.price)
  const m = parsePermille(markup)
  const sale = cost === null || m === null ? null : saleFromMarkup(cost, m)
  return { ...l, markup, salePrice: sale === null ? l.salePrice : somInput(sale) }
}

/** «в прошлый раз: 350 с, 12.10, Автоснаб». */
export function lastText(r: LastReceipt): string {
  // День и месяц по Бишкеку: приход в 01:00 по Бишкеку — ещё вчера по UTC.
  const d = new Date(r.created_at).toLocaleDateString('ru-RU', { timeZone: 'Asia/Bishkek', day: '2-digit', month: '2-digit' })
  return ['в прошлый раз: ' + formatSom(r.price_tyiyn), d, r.supplier_name].filter(Boolean).join(', ')
}


/**
 * Цена по накладной за единицу цены товара (штуку, канистру, килограмм) из проведённой строки:
 * сумма строки × единица / количество, до тыйына. Для колонок «наценка» и «цена продажи» накладной.
 */
export function unitCost(l: { unit: string; container_ml: number | null; qty: number; cost_tyiyn: number }): number | null {
  const per = l.unit === 'ml' && l.container_ml ? l.container_ml : l.unit === 'ml' || l.unit === 'g' ? 1000 : 1
  const n = l.cost_tyiyn * per
  return l.qty > 0 && Number.isSafeInteger(n) ? divRound(n, l.qty) : null
}
