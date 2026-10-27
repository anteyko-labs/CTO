import { describe, expect, it } from 'vitest'
import {
  initialPrices,
  lastPer,
  lastText,
  literHints,
  priceJumpText,
  switchMode,
  unitCost,
  lineCalc,
  withCost,
  withMarkup,
  withSalePrice,
} from './receiptLines'
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
    expect(lineCalc({ product: filter, count: '3', price: '350' })).toEqual({ n: 3, qty: 3, cost: 105_000, sale: 150_000, salePrice: 50_000, unitPrice: 35_000 })
    // Масло: количество в канистрах, на склад — мл, цена продажи — за канистру.
    const oil = product({ unit: 'ml', container_ml: 4500, sale_price_tyiyn: 200_000 })
    expect(lineCalc({ product: oil, count: '2', price: '1500,50' })).toEqual({ n: 2, qty: 9000, cost: 300_100, sale: 400_000, salePrice: 200_000, unitPrice: 150_050 })
    expect(lineCalc({ product: oil, count: '0', price: '1' })).toBe('укажите число канистр')
    expect(lineCalc({ product: filter, count: '1', price: 'абв' })).toBe('укажите цену')
  })

  it('новая цена продажи в строке меняет сумму по продаже', () => {
    const filter = product({ sale_price_tyiyn: 50_000 })
    expect(lineCalc({ product: filter, count: '2', price: '300', salePrice: '400' })).toEqual({ n: 2, qty: 2, cost: 60_000, sale: 80_000, salePrice: 40_000, unitPrice: 30_000 })
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

describe('цена по накладной сильно отличается от прошлой', () => {
  it('предупреждает при отличии больше 30 %, без запрета', () => {
    const last = { price_tyiyn: 42_000 }
    expect(priceJumpText('420', last)).toBe('')
    expect(priceJumpText('546', last)).toBe('')
    expect(priceJumpText('294', last)).toBe('')
    expect(priceJumpText('50', last)).toMatch(/^Цена ниже, чем в прошлый раз/)
    expect(priceJumpText('547', last)).toMatch(/^Цена выше/)
    expect(priceJumpText('', last)).toBe('')
    expect(priceJumpText('50', null)).toBe('')
    expect(priceJumpText('50', { price_tyiyn: 0 })).toBe('')
    // Строка в литрах: 450 с за литр × 4 л = 1 800 с — как прошлая канистра, без предупреждения.
    expect(priceJumpText('450', { price_tyiyn: 180_000 }, 4000)).toBe('')
    expect(priceJumpText('200', { price_tyiyn: 180_000 }, 4000)).toMatch(/^Цена ниже/)
  })
})

describe('масло литрами и товар на вес (SPEC-36 §13)', () => {
  const oil = product({ unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000, last_purchase_price_tyiyn: 180_000 })

  it('18 л по 450 с за литр = 8 100 с, на склад 18 л, продажа — за канистру', () => {
    expect(lineCalc({ product: oil, count: '18', price: '450', mode: 'l' })).toEqual({
      n: 4,
      qty: 18_000,
      cost: 810_000,
      sale: 900_000,
      salePrice: 200_000,
      unitPrice: 180_000,
    })
    // Одно округление: 0,333 л по 451,11 с = 150,22 с (150,21963 → 150,22).
    expect(lineCalc({ product: oil, count: '0,333', price: '451,11', mode: 'l' })).toMatchObject({ qty: 333, cost: 15_022 })
    expect(lineCalc({ product: oil, count: '2,5', price: '450', mode: 'can' })).toBe('укажите число канистр')
    expect(lineCalc({ product: oil, count: '', price: '450', mode: 'l' })).toBe('укажите литры')
  })

  it('на вес: кг до 3 знаков, цена за кг, сумма по продаже без ошибки в 1000 раз', () => {
    const scrap = product({ unit: 'g', sale_price_tyiyn: 12_000 })
    // 129,3 кг по 100 с за кг = 12 930 с; по продаже 120 с за кг = 15 516 с.
    expect(lineCalc({ product: scrap, count: '129,3', price: '100' })).toEqual({ n: 1, qty: 129_300, cost: 1_293_000, sale: 1_551_600, salePrice: 12_000, unitPrice: 10_000 })
    expect(lineCalc({ product: scrap, count: '1,2345', price: '100' })).toBe('укажите килограммы')
  })

  it('режим «л»: цена прошлой закупки — за литр, наценка — к закупке канистры', () => {
    const l = initialPrices(oil, 4000)
    expect(l.price).toBe('450')
    expect(l.markup).toBe('11,1')
    expect(withMarkup(l, '25', 4000).salePrice).toBe('2250')
    expect(withSalePrice(l, '2700', 4000).markup).toBe('50,0')
    expect(withCost(l, '500', 4000).markup).toBe('0,0')
  })

  it('переключатель «кан. | л» переводит количество и цену', () => {
    const can = { product: oil, count: '3', mode: 'can' as const, ...initialPrices(oil) }
    const l = switchMode(can, 'l')
    expect([l.mode, l.count, l.price, l.salePrice, l.markup]).toEqual(['l', '12', '450', '2000', '11,1'])
    const back = switchMode({ ...l, count: '8' }, 'can')
    expect([back.mode, back.count, back.price]).toEqual(['can', '2', '1800'])
    // Не делится на канистры — остаётся литрами, строка покажет ошибку.
    expect(switchMode({ ...l, count: '9,5' }, 'can').count).toBe('9,5')
  })

  it('подсказки: «= N кан. + M л» и «за канистру ≈ …»', () => {
    const h = literHints({ product: oil, count: '18', price: '450', mode: 'l' })
    expect(nb(h?.split ?? '')).toBe('= 4 кан. + 2 л')
    expect(nb(h?.perCan ?? '')).toBe('за канистру 4 л ≈ 1 800 с')
    expect(literHints({ product: oil, count: '2', price: '1800', mode: 'can' })).toBeNull()
  })

  it('прошлая закупка — с единицей', () => {
    expect(lastPer(oil)).toBe('за канистру')
    expect(lastPer(product({ unit: 'g' }))).toBe('за кг')
    expect(lastPer(product({}))).toBeUndefined()
    expect(nb(lastText({ receipt_id: 'r', number: 1, price_tyiyn: 180_000, created_at: '2026-10-12T05:00:00Z', supplier_name: null }, 'за канистру'))).toBe(
      'в прошлый раз: 1 800 с за канистру, 12.10',
    )
  })
})
