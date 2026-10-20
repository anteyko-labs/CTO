// Исправление прихода владельцем (SPEC-29 §24): исходная накладная с сервера → поля экрана
// «Новый приход». Экран тот же; здесь только перевод строк и расчёта в его поля.
import { divRound } from './money'
import { somInput } from './format'
import { isOil, withSalePrice, type PriceFields } from './receiptLines'
import type { PayMode } from './receiptPay'
import type { Product } from './types'

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
}

export interface ReceiptFixState {
  supplierId: string
  supplierDoc: string
  comment: string
  payMode: PayMode
  partText: string
  paidFrom: string
  lines: ReceiptFixLine[]
  /** Строки, сумма которых не делится на количество: цена за штуку округлена — проверьте. */
  rounded: string[]
}

/**
 * Поля прихода из исходной накладной. Масло — канистрами, цена — за штуку или канистру (сумма
 * строки ÷ количество); цена продажи — поставленная этим приходом, иначе текущая цена товара.
 * Карточки нет или масло не целыми канистрами — ошибка: строку нельзя потерять.
 */
export function receiptFixState(doc: ReceiptDraftDoc, products: Map<string, Product>, nextKey: () => string): ReceiptFixState {
  const lines: ReceiptFixLine[] = []
  const rounded: string[] = []
  for (const l of doc.lines) {
    // Карточка не загрузилась — исправлять нельзя: иначе строка молча пропала бы, а исходная
    // накладная сторнировалась бы целиком (проверка 20.10.2026).
    const product = products.get(l.product_id)
    if (!product) throw new Error('Не удалось загрузить товар строки — обновите страницу и попробуйте ещё раз')
    if (isOil(product) && l.qty % product.container_ml !== 0) {
      throw new Error(`«${product.name}»: в накладной не целое число канистр — исправьте сторно и новым приходом`)
    }
    const n = isOil(product) ? l.qty / product.container_ml : l.qty
    const per = divRound(l.cost_tyiyn, n)
    if (per * n !== l.cost_tyiyn) rounded.push(product.name)
    const sale = l.sale_price_tyiyn ?? product.sale_price_tyiyn
    lines.push({ key: nextKey(), product, count: String(n), ...withSalePrice({ price: somInput(per), salePrice: '', markup: '' }, somInput(sale)) })
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
