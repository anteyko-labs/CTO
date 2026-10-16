// Строка нового прихода: количество, сумма по закупке и по цене продажи; подсказка прошлой закупки
// (SPEC-03, SPEC-27). Только целые тыйыны и мл: суммы — умножением, без деления.
import { formatSom, parseSom } from './format'
import type { LastReceipt, Product } from './types'

export const isOil = (p: Product): p is Product & { container_ml: number } => p.unit === 'ml' && Boolean(p.container_ml)

/** Целое положительное число из строки или null. */
export function parseCount(s: string): number | null {
  const t = s.trim()
  if (!/^\d+$/.test(t)) return null
  const n = Number(t)
  return Number.isSafeInteger(n) && n > 0 ? n : null
}

/** Расчёт строки: количество в единицах учёта, сумма по закупке и по цене продажи; текст ошибки, если ввод неверен. */
export function lineCalc(l: { product: Product; count: string; price: string }): { n: number; qty: number; cost: number; sale: number } | string {
  const n = parseCount(l.count)
  if (n === null) return isOil(l.product) ? 'укажите число канистр' : 'укажите количество'
  const price = parseSom(l.price)
  if (price === null) return 'укажите цену'
  const qty = isOil(l.product) ? n * l.product.container_ml : n
  const cost = n * price
  // Цена продажи — за штуку или канистру, как и количество строки: сумма целая, без деления.
  const sale = n * l.product.sale_price_tyiyn
  if (!Number.isSafeInteger(qty) || !Number.isSafeInteger(cost) || !Number.isSafeInteger(sale)) return 'слишком большое число'
  return { n, qty, cost, sale }
}

/** «в прошлый раз: 350 с, 12.10, Автоснаб». */
export function lastText(r: LastReceipt): string {
  // День и месяц по Бишкеку: приход в 01:00 по Бишкеку — ещё вчера по UTC.
  const d = new Date(r.created_at).toLocaleDateString('ru-RU', { timeZone: 'Asia/Bishkek', day: '2-digit', month: '2-digit' })
  return ['в прошлый раз: ' + formatSom(r.price_tyiyn), d, r.supplier_name].filter(Boolean).join(', ')
}

