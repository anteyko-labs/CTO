import { describe, expect, it } from 'vitest'
import { initialPrices, lastText, unitCost, lineCalc, withCost, withMarkup, withSalePrice } from './receiptLines'
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
    expect(lineCalc({ product: filter, count: '3', price: '350' })).toEqual({ n: 3, qty: 3, cost: 105_000, sale: 150_000, salePrice: 50_000 })
    // Масло: количество в канистрах, на склад — мл, цена продажи — за канистру.
    const oil = product({ unit: 'ml', container_ml: 4500, sale_price_tyiyn: 200_000 })
    expect(lineCalc({ product: oil, count: '2', price: '1500,50' })).toEqual({ n: 2, qty: 9000, cost: 300_100, sale: 400_000, salePrice: 200_000 })
    expect(lineCalc({ product: oil, count: '0', price: '1' })).toBe('укажите число канистр')
    expect(lineCalc({ product: filter, count: '1', price: 'абв' })).toBe('укажите цену')
  })

  it('новая цена продажи в строке меняет сумму по продаже', () => {
    const filter = product({ sale_price_tyiyn: 50_000 })
    expect(lineCalc({ product: filter, count: '2', price: '300', salePrice: '400' })).toEqual({ n: 2, qty: 2, cost: 60_000, sale: 80_000, salePrice: 40_000 })
    expect(lineCalc({ product: filter, count: '2', price: '300', salePrice: 'x' })).toBe('укажите цену продажи')
  })

  it('наценка и цена продажи связаны (SPEC-28 §1)', () => {
    const p = product({ sale_price_tyiyn: 200_000, last_purchase_price_tyiyn: 160_000 })
    const l = initialPrices(p)
    expect(l.price).toBe('1600')
    expect(l.markup).toBe('25,0')
    // Наценка 30 % → 2 080 с; цена 1 700 → наценка 6,3 %; цена по накладной 1 000 → 70,0 %.
    const up = withMarkup(l, '30')
    expect(up.salePrice).toBe('2080')
    const set = withSalePrice(l, '1700')
    expect(set.markup).toBe('6,3')
    expect(withCost(set, '1000').markup).toBe('70,0')
    // 300 с и 16,7 % → 350,10 с — до тыйына, как на сервере.
    expect(nb(withMarkup({ price: '300', salePrice: '', markup: '' }, '16,7').salePrice)).toBe('350,10')
    // Ниже закупки — отрицательная наценка.
    expect(withSalePrice({ price: '100', salePrice: '', markup: '' }, '90').markup).toBe('-10,0')
    // Без цены по накладной наценки нет.
    expect(withSalePrice({ price: '', salePrice: '', markup: '' }, '90').markup).toBe('')
  })

  it('цена по накладной за канистру и штуку из проведённой строки', () => {
    expect(unitCost({ unit: 'ml', container_ml: 4000, qty: 8000, cost_tyiyn: 320_000 })).toBe(160_000)
    expect(unitCost({ unit: 'piece', container_ml: null, qty: 3, cost_tyiyn: 100_000 })).toBe(33_333)
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
