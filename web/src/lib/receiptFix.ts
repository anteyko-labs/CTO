// Исправление прихода владельцем (SPEC-29 §24): исходная накладная с сервера → поля экрана
// «Новый приход». Экран тот же; здесь только перевод строк и расчёта в его поля.
import { divRound } from './money'
import { litersInput, somInput } from './format'
import { isOil, lineCalc, withSalePrice, type PriceFields, type QtyMode } from './receiptLines'
import type { PayMode } from './receiptPay'
import type { Product } from './types'
import { kgInput } from './units'

/** Накладная в виде запроса проведения — так её отдаёт `GET /corrections/draft/receipt/{id}`. */
export interface ReceiptDraftDoc {
  supplier_id: string | null
  payment: 'paid' | 'debt'
  supplier_doc: string
  comment: string
  paid_from: string | null
  paid_now_tyiyn: number | null
  lines: { product_id: string; qty: number; cost_tyiyn: number; sale_price_tyiyn: number | null }[]
}

export interface ReceiptFixLine extends PriceFields {
  key: string
  product: Product
  count: string
  mode?: QtyMode
}

export interface ReceiptFixState {
  supplierId: string
  supplierDoc: string
  comment: string
  payMode: PayMode
  partText: string
  paidFrom: string
  lines: ReceiptFixLine[]
  /** Строки, сумма которых не делится на количество: цена за единицу округлена — проверьте. */
  rounded: string[]
}

/**
 * Количество и цена строки в полях экрана: штуки и канистры — целым числом, цена за штуку или
 * канистру (сумма ÷ количество); масло не целыми канистрами — литрами с ценой литра (SPEC-36 §13);
 * на вес — кг с ценой за кг.
 */
function fixFields(p: Product, qty: number, cost: number): { count: string; per: number; mode?: QtyMode } {
  if (p.unit === 'g') return { count: kgInput(qty), per: divRound(cost * 1000, qty) }
  if (!isOil(p)) return { count: String(qty), per: divRound(cost, qty) }
  if (qty % p.container_ml !== 0) return { count: litersInput(qty), per: divRound(cost * 1000, qty), mode: 'l' }
  const n = qty / p.container_ml
  return { count: String(n), per: divRound(cost, n), mode: 'can' }
}

/**
 * Поля прихода из исходной накладной; цена продажи — поставленная этим приходом, иначе текущая цена
 * товара. Сумма строки с округлённой ценой за единицу не сходится с исходной — строка в `rounded`.
 * Карточки нет — ошибка: строку нельзя потерять.
 */
export function receiptFixState(doc: ReceiptDraftDoc, products: Map<string, Product>, nextKey: () => string): ReceiptFixState {
  const lines: ReceiptFixLine[] = []
  const rounded: string[] = []
  for (const l of doc.lines) {
    // Карточка не загрузилась — исправлять нельзя: иначе строка молча пропала бы, а исходная
    // накладная сторнировалась бы целиком (проверка 20.10.2026).
    const product = products.get(l.product_id)
    if (!product) throw new Error('Не удалось загрузить товар строки — обновите страницу и попробуйте ещё раз')
    const f = fixFields(product, l.qty, l.cost_tyiyn)
    const tare = f.mode === 'l' ? (product.container_ml ?? undefined) : undefined
    const sale = l.sale_price_tyiyn ?? product.sale_price_tyiyn
    const line: ReceiptFixLine = {
      key: nextKey(),
      product,
      count: f.count,
      ...(f.mode ? { mode: f.mode } : {}),
      ...withSalePrice({ price: somInput(f.per), salePrice: '', markup: '' }, somInput(sale), tare),
    }
    const c = lineCalc(line)
    if (typeof c === 'string' || c.cost !== l.cost_tyiyn) rounded.push(product.name)
    lines.push(line)
  }
  return {
    supplierId: doc.supplier_id ?? '',
    supplierDoc: doc.supplier_doc,
    comment: doc.comment,
    payMode: doc.payment === 'paid' ? 'paid' : doc.paid_now_tyiyn ? 'part' : 'debt',
    partText: doc.paid_now_tyiyn ? somInput(doc.paid_now_tyiyn) : '',
    paidFrom: doc.paid_from ?? '',
    lines,
    rounded,
  }
}
