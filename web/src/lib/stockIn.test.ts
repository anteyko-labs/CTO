import { describe, expect, it } from 'vitest'
import { convertCount, stockInCost, stockInFirstCount, stockInFixLines, stockInLineCalc, stockInNextCount, stockInQty } from './stockIn'
import type { Product } from './types'

describe('оприходование', () => {
  it('количество: штуки целым, масло литрами', () => {
    expect(stockInQty({ unit: 'piece' }, '3')).toBe(3)
    expect(stockInQty({ unit: 'piece' }, '1,5')).toBeNull()
    expect(stockInQty({ unit: 'ml' }, '8,5')).toBe(8500)
    expect(stockInQty({ unit: 'ml' }, '0')).toBeNull()
  })

  it('стоимость: цена за канистру × литры, до тыйына', () => {
    expect(stockInCost({ unit: 'piece', container_ml: null }, 3, 20_000)).toBe(60_000)
    expect(stockInCost({ unit: 'ml', container_ml: 4000 }, 6000, 150_000)).toBe(225_000)
    expect(stockInCost({ unit: 'g', container_ml: null }, 12_500, 5_000)).toBe(62_500)
  })
})

describe('оприходование строками на экране прихода', () => {
  const product = (over: Partial<Product>): Product => ({ id: 'p', name: 'Товар', unit: 'piece', container_ml: null, sale_price_tyiyn: 30_000, ...over }) as Product

  it('строка: штуки, литры масла, цена за канистру; продажа — по той же формуле', () => {
    expect(stockInLineCalc({ product: product({}), count: '3', price: '200' })).toEqual({ n: 3, qty: 3, cost: 60_000, sale: 90_000, salePrice: 30_000 })
    const oil = product({ unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 })
    expect(stockInLineCalc({ product: oil, count: '8,5', price: '1500', salePrice: '2000' })).toEqual({ n: 2, qty: 8500, cost: 318_750, sale: 425_000, salePrice: 200_000 })
    expect(stockInLineCalc({ product: oil, count: '0', price: '1500' })).toBe('укажите литры')
    expect(stockInLineCalc({ product: product({}), count: '1', price: '' })).toBe('укажите цену')
  })

  it('повторный скан и смена вида: канистры ↔ литры', () => {
    const oil = product({ unit: 'ml', container_ml: 4000 })
    expect(stockInFirstCount(oil)).toBe('4')
    expect(stockInNextCount(oil, '4')).toBe('8')
    expect(stockInNextCount(product({}), '2')).toBe('3')
    expect(convertCount(oil, '2', true)).toBe('8')
    expect(convertCount(oil, '8', false)).toBe('2')
    expect(convertCount(oil, '8,5', false)).toBe('8,5')
    expect(convertCount(product({}), '5', true)).toBe('5')
  })

  it('исправление: строки из документа, цена продажи — поставленная им или текущая', () => {
    const a = product({ id: 'a', name: 'Хомут' })
    const oil = product({ id: 'o', name: 'Масло', unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 })
    let k = 0
    const lines = stockInFixLines(
      {
        comment: 'нашли',
        lines: [
          { product_id: 'a', qty: 3, price_tyiyn: 20_000, sale_price_tyiyn: 32_000 },
          { product_id: 'o', qty: 6000, price_tyiyn: 150_000, sale_price_tyiyn: null },
          { product_id: 'нет', qty: 1, price_tyiyn: 1, sale_price_tyiyn: null },
        ],
      },
      new Map([
        ['a', a],
        ['o', oil],
      ]),
      () => String(++k),
    )
    expect(lines.map((l) => [l.product.name, l.count, l.price, l.salePrice])).toEqual([
      ['Хомут', '3', '200', '320'],
      ['Масло', '6', '1500', '2000'],
    ])
  })
})
