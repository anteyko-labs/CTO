// Оприходование без поставщика — вариант прихода (SPEC-29 §8, ADR-086): строки того же экрана
// «Новый приход», но количество — как на складе (штуки целым, масло литрами, на вес кг), цена
// закупки — за штуку, канистру или килограмм; стоимость считается так же, как на сервере.
import { litersInput, parseLiters, parseSom, somInput } from './format'
import { divRound } from './money'
import { withSalePrice, type PriceFields } from './receiptLines'
import type { Product } from './types'

/** Единиц в цене: канистра масла, килограмм, штука — как цена товара. */
const pricePer = (p: Pick<Product, 'unit' | 'container_ml'>): number =>
  p.unit === 'ml' ? (p.container_ml ?? 1000) : p.unit === 'g' ? 1000 : 1

/** Количество из поля: штуки целым, масло литрами, на вес килограммами. */
export function stockInQty(p: Pick<Product, 'unit'>, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (p.unit === 'piece') return /^\d+$/.test(t) && Number(t) > 0 ? Number(t) : null
  const v = parseLiters(t)
  return v !== null && v > 0 ? v : null
}

/** Стоимость: цена × количество / единицу цены, до тыйына — как на сервере. */
export const stockInCost = (p: Pick<Product, 'unit' | 'container_ml'>, qty: number, price: number): number =>
  divRound(price * qty, pricePer(p))

/** Количество для поля: штуки — число, масло — литры, на вес — килограммы. */
export const stockInQtyInput = (p: Pick<Product, 'unit'>, qty: number): string => (p.unit === 'piece' ? String(qty) : litersInput(qty))

/** Сколько добавляет повторный скан: штуку, канистру масла или килограмм. */
const step = (p: Pick<Product, 'unit' | 'container_ml'>): number => (p.unit === 'piece' ? 1 : pricePer(p))

/** Поле количества новой строки: одна штука, одна канистра, один килограмм. */
export const stockInFirstCount = (p: Pick<Product, 'unit' | 'container_ml'>): string => stockInQtyInput(p, step(p))

/** Повторный скан: ещё штука, канистра или килограмм; неразобранное — заново с одной. */
export function stockInNextCount(p: Pick<Product, 'unit' | 'container_ml'>, count: string): string {
  const q = stockInQty(p, count)
  return stockInQtyInput(p, q === null ? step(p) : q + step(p))
}

/**
 * Поле количества при смене вида документа: приход считает масло канистрами, оприходование —
 * литрами. Неразобранное и нецелое число канистр остаётся как есть — строка покажет ошибку.
 */
export function convertCount(p: Pick<Product, 'unit' | 'container_ml'>, count: string, toStockIn: boolean): string {
  if (p.unit !== 'ml' || !p.container_ml) return count
  if (toStockIn) {
    const t = count.trim()
    return /^\d+$/.test(t) ? litersInput(Number(t) * p.container_ml) : count
  }
  const q = stockInQty(p, count)
  return q !== null && q % p.container_ml === 0 ? String(q / p.container_ml) : count
}

/**
 * Расчёт строки оприходования — в том же виде, что у прихода (`lineCalc`): количество в единицах
 * учёта, стоимость по закупке и по цене продажи, цена продажи, число этикеток.
 */
export function stockInLineCalc(l: { product: Product; count: string; price: string; salePrice?: string }):
  | { n: number; qty: number; cost: number; sale: number; salePrice: number }
  | string {
  const p = l.product
  const qty = stockInQty(p, l.count)
  if (qty === null) return p.unit === 'ml' ? 'укажите литры' : p.unit === 'g' ? 'укажите килограммы' : 'укажите количество'
  const price = parseSom(l.price)
  if (price === null) return 'укажите цену'
  const salePrice = l.salePrice === undefined || l.salePrice.trim() === '' ? p.sale_price_tyiyn : parseSom(l.salePrice)
  if (salePrice === null) return 'укажите цену продажи'
  const cost = stockInCost(p, qty, price)
  const sale = stockInCost(p, qty, salePrice)
  if (!Number.isSafeInteger(qty) || !Number.isSafeInteger(cost) || !Number.isSafeInteger(sale)) return 'слишком большое число'
  // Этикетки — на штуки и целые канистры; на вес — одна.
  const n = p.unit === 'piece' ? qty : p.unit === 'ml' && p.container_ml ? Math.trunc(qty / p.container_ml) : 1
  return { n, qty, cost, sale, salePrice }
}

/** Оприходование в виде запроса проведения — так его отдаёт `GET /corrections/draft/stock_in/{id}`. */
export interface StockInDraftDoc {
  comment: string
  lines: { product_id: string; qty: number; price_tyiyn: number; sale_price_tyiyn: number | null }[]
}

export interface StockInFixLine extends PriceFields {
  key: string
  product: Product
  count: string
}

/**
 * Поля экрана из исходного оприходования: количество как на складе, цена закупки за единицу цены,
 * цена продажи — поставленная этим документом, иначе текущая. Карточки нет — строка пропускается.
 */
export function stockInFixLines(doc: StockInDraftDoc, products: Map<string, Product>, nextKey: () => string): StockInFixLine[] {
  const out: StockInFixLine[] = []
  for (const l of doc.lines) {
    const product = products.get(l.product_id)
    if (!product) throw new Error('Не удалось загрузить товар строки — обновите страницу и попробуйте ещё раз')
    const sale = l.sale_price_tyiyn ?? product.sale_price_tyiyn
    out.push({
      key: nextKey(),
      product,
      count: stockInQtyInput(product, l.qty),
      ...withSalePrice({ price: somInput(l.price_tyiyn), salePrice: '', markup: '' }, somInput(sale)),
    })
  }
  return out
}
