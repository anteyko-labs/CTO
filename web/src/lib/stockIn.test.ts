import { describe, expect, it } from 'vitest'
import { firstCount, nextCount } from './receiptLines'
import { stockInCost, stockInFixLines, stockInLineCalc } from './stockIn'
import type { Product } from './types'

describe('оприходование', () => {
  it('стоимость: цена за канистру × литры, до тыйына', () => {
    expect(stockInCost({ unit: 'piece', container_ml: null }, 3, 20_000)).toBe(60_000)
    expect(stockInCost({ unit: 'ml', container_ml: 4000 }, 6000, 150_000)).toBe(225_000)
    expect(stockInCost({ unit: 'g', container_ml: null }, 12_500, 5_000)).toBe(62_500)
  })
})

describe('оприходование строками на экране прихода', () => {
  const product = (over: Partial<Product>): Product => ({ id: 'p', name: 'Товар', unit: 'piece', container_ml: null, sale_price_tyiyn: 30_000, ...over }) as Product

  it('строка: штуки; масло литрами по цене литра — на сервер цена канистры (SPEC-36 §13)', () => {
    expect(stockInLineCalc({ product: product({}), count: '3', price: '200' })).toEqual({ n: 3, qty: 3, cost: 60_000, sale: 90_000, salePrice: 30_000, unitPrice: 20_000 })
    const oil = product({ unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 })
    // 8,5 л по 375 с за литр = 3 187,50 с; цена канистры для сервера 1 500 с; продажа — за канистру.
    expect(stockInLineCalc({ product: oil, count: '8,5', price: '375', salePrice: '2000', mode: 'l' })).toEqual({
      n: 2,
      qty: 8500,
      cost: 318_750,
      sale: 425_000,
      salePrice: 200_000,
      unitPrice: 150_000,
    })
    // Без режима — литры (по умолчанию в оприходовании).
    expect(stockInLineCalc({ product: oil, count: '0', price: '375' })).toBe('укажите литры')
    // Канистрами: 2 кан. по 1 500 с.
    expect(stockInLineCalc({ product: oil, count: '2', price: '1500', mode: 'can' })).toMatchObject({ qty: 8000, cost: 300_000, unitPrice: 150_000 })
    expect(stockInLineCalc({ product: product({}), count: '1', price: '' })).toBe('укажите цену')
  })

  it('на вес — кг и цена за кг', () => {
    const scrap = product({ unit: 'g', sale_price_tyiyn: 12_000 })
    expect(stockInLineCalc({ product: scrap, count: '12,5', price: '50' })).toEqual({ n: 1, qty: 12_500, cost: 62_500, sale: 150_000, salePrice: 12_000, unitPrice: 5_000 })
  })

  it('повторный скан: литры — ещё канистра, штуки — ещё одна, вес — ещё кг', () => {
    const oil = product({ unit: 'ml', container_ml: 4000 })
    expect(firstCount(oil, 'l')).toBe('4')
    expect(nextCount(oil, 'l', '4')).toBe('8')
    expect(firstCount(oil, 'can')).toBe('1')
    expect(nextCount(oil, 'can', '1')).toBe('2')
    expect(nextCount(product({}), undefined, '2')).toBe('3')
    expect(nextCount(product({ unit: 'g' }), undefined, '1,5')).toBe('2,5')
  })

  it('исправление: строки из документа, цена продажи — поставленная им или текущая', () => {
    const a = product({ id: 'a', name: 'Хомут' })
    const oil = product({ id: 'o', name: 'Масло', unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 })
    const barrel = product({ id: 'b', name: 'Бочка', unit: 'ml', container_ml: 208_000, sale_price_tyiyn: 9_000_000 })
    let k = 0
    const lines = stockInFixLines(
      {
        comment: 'нашли',
        lines: [
          { product_id: 'a', qty: 3, price_tyiyn: 20_000, sale_price_tyiyn: 32_000 },
          { product_id: 'o', qty: 6000, price_tyiyn: 150_000, sale_price_tyiyn: null },
          // Цена бочки не делится на литры, бочка целая — канистрами, без сдвига суммы.
          { product_id: 'b', qty: 208_000, price_tyiyn: 8_000_001, sale_price_tyiyn: null },
        ],
      },
      new Map([
        ['a', a],
        ['o', oil],
        ['b', barrel],
      ]),
      () => String(++k),
    )
    expect(lines.map((l) => [l.product.name, l.count, l.mode, l.price, l.salePrice])).toEqual([
      ['Хомут', '3', undefined, '200', '320'],
      ['Масло', '6', 'l', '375', '2000'],
      ['Бочка', '1', 'can', '80000,01', '90000'],
    ])
    // Наценка масла — к закупке канистры: 1 500 → 2 000 = 33,3 %.
    expect(lines[1].markup).toBe('33,3')
  })
})
