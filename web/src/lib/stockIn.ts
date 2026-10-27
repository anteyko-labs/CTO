// Оприходование без поставщика — вариант прихода (SPEC-29 §8, ADR-086): строки того же экрана
// «Новый приход», без поставщика и оплаты. Масло — литрами (по умолчанию, цена за литр) или
// канистрами (SPEC-36 §13, ADR-123), на вес — кг; сервер принимает цену за штуку, канистру или кг
// и сам считает стоимость: divRound(цена × количество, единица цены).
import { litersInput, somInput } from './format'
import { divRound } from './money'
import { isOil, lineCalc, withSalePrice, type LineCalc, type LineInput, type PriceFields, type QtyMode } from './receiptLines'
import type { Product } from './types'
import { kgInput, perCan, perLiter } from './units'

/** Единиц в цене: канистра масла, килограмм, штука — как цена товара. */
const pricePer = (p: Pick<Product, 'unit' | 'container_ml'>): number =>
  p.unit === 'ml' ? (p.container_ml ?? 1000) : p.unit === 'g' ? 1000 : 1

/** Стоимость: цена × количество / единицу цены, до тыйына — как на сервере. */
export const stockInCost = (p: Pick<Product, 'unit' | 'container_ml'>, qty: number, price: number): number =>
  divRound(price * qty, pricePer(p))

/** Расчёт строки оприходования — тот же, что у прихода (`lineCalc`); `unitPrice` уходит на сервер. */
export const stockInLineCalc = (l: LineInput): LineCalc | string => lineCalc(l, true)

/** Оприходование в виде запроса проведения — так его отдаёт `GET /corrections/draft/stock_in/{id}`. */
export interface StockInDraftDoc {
  comment: string
  lines: { product_id: string; qty: number; price_tyiyn: number; sale_price_tyiyn: number | null }[]
}

export interface StockInFixLine extends PriceFields {
  key: string
  product: Product
  count: string
  mode?: QtyMode
}

/**
 * Поля строки из исходного документа: количество как на складе, цена закупки, режим масла.
 * Масло — литрами с ценой литра; если цена канистры не делится на литры, а канистры целые —
 * канистрами, чтобы сумма не сдвинулась на тыйын.
 */
function fixFields(p: Product, qty: number, price: number): { count: string; price: number; mode?: QtyMode } {
  if (p.unit === 'g') return { count: kgInput(qty), price }
  if (!isOil(p)) return { count: String(qty), price }
  const c = p.container_ml
  const perL = perLiter(price, c)
  if (perCan(perL, c) !== price && qty % c === 0) return { count: String(qty / c), price, mode: 'can' }
  return { count: litersInput(qty), price: perL, mode: 'l' }
}

/**
 * Поля экрана из исходного оприходования: количество как на складе, цена закупки за единицу цены,
 * цена продажи — поставленная этим документом, иначе текущая. Карточки нет — ошибка.
 */
export function stockInFixLines(doc: StockInDraftDoc, products: Map<string, Product>, nextKey: () => string): StockInFixLine[] {
  const out: StockInFixLine[] = []
  for (const l of doc.lines) {
    const product = products.get(l.product_id)
    if (!product) throw new Error('Не удалось загрузить товар строки — обновите страницу и попробуйте ещё раз')
    const sale = l.sale_price_tyiyn ?? product.sale_price_tyiyn
    const f = fixFields(product, l.qty, l.price_tyiyn)
    const tare = f.mode === 'l' ? (product.container_ml ?? undefined) : undefined
    out.push({
      key: nextKey(),
      product,
      count: f.count,
      ...(f.mode ? { mode: f.mode } : {}),
      ...withSalePrice({ price: somInput(f.price), salePrice: '', markup: '' }, somInput(sale), tare),
    })
  }
  return out
}
