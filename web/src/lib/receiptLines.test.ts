import { describe, expect, it } from 'vitest'
import { lastText, lineCalc } from './receiptLines'
import type { Product } from './types'

const nb = (s: string) => s.replace(/\u00a0/g, ' ')

const product = (p: Partial<Product>): Product => ({
  id: 'p',
  category_id: 'c',
  category_kind: 'other',
  name: 'Товар',
  brand: '',
  article: '',
  unit: 'piece',
  container_ml: null,
  attrs: {},
  archived: false,
  barcodes: [],
  sale_price_tyiyn: 0,
  pour_price_per_l_tyiyn: null,
  min_stock: 0,
  stock_qty: 0,
  needs_review: false,
  last_purchase_price_tyiyn: null,
  ...p,
})

describe('строка прихода', () => {
  it('сумма по закупке и по цене продажи', () => {
    const filter = product({ sale_price_tyiyn: 50_000 })
    expect(lineCalc({ product: filter, count: '3', price: '350' })).toEqual({ n: 3, qty: 3, cost: 105_000, sale: 150_000 })
    // Масло: количество в канистрах, на склад — мл, цена продажи — за канистру.
    const oil = product({ unit: 'ml', container_ml: 4500, sale_price_tyiyn: 200_000 })
    expect(lineCalc({ product: oil, count: '2', price: '1500,50' })).toEqual({ n: 2, qty: 9000, cost: 300_100, sale: 400_000 })
    expect(lineCalc({ product: oil, count: '0', price: '1' })).toBe('укажите число канистр')
    expect(lineCalc({ product: filter, count: '1', price: 'абв' })).toBe('укажите цену')
  })

  it('прошлая закупка: цена, день по Бишкеку и поставщик', () => {
    expect(nb(lastText({ receipt_id: 'r', number: 7, price_tyiyn: 35_000, created_at: '2026-10-11T19:30:00Z', supplier_name: 'Автоснаб' }))).toBe(
      'в прошлый раз: 350 с, 12.10, Автоснаб',
    )
    expect(nb(lastText({ receipt_id: 'r', number: 7, price_tyiyn: 35_000, created_at: '2026-10-12T05:00:00Z', supplier_name: null }))).toBe(
      'в прошлый раз: 350 с, 12.10',
    )
  })
})
