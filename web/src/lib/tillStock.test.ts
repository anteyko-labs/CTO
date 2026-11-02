import { describe, expect, it } from 'vitest'
import { canAddToCart, shortages } from './tillStock'

const oil = { id: 'o', name: 'Масло 5W-30', stock_qty: 8000 } // мл: 2 канистры по 4 л
const filter = { id: 'f', name: 'Фильтр', stock_qty: 3 }
const cable = { id: 'c', name: 'Провод', stock_qty: 1500 } // мм
const nails = { id: 'n', name: 'Гвозди', stock_qty: 0 } // г

describe('остаток на кассе (задача 018)', () => {
  it('товар с нулём не кладётся, без запрета — кладётся', () => {
    expect(canAddToCart(nails, false)).toBe(false)
    expect(canAddToCart({ stock_qty: -2 }, false)).toBe(false)
    expect(canAddToCart(filter, false)).toBe(true)
    expect(canAddToCart(nails, true)).toBe(true)
  })

  it('ровно в остаток — можно, сверх — нехватка (шт, мм, г)', () => {
    expect(shortages([{ product: filter, units: 3 }], false)).toEqual([])
    expect(shortages([{ product: filter, units: 4 }], false)).toEqual([filter])
    expect(shortages([{ product: cable, units: 1500 }], false)).toEqual([])
    expect(shortages([{ product: cable, units: 1501 }], false)).toEqual([cable])
    expect(shortages([{ product: nails, units: 1 }], false)).toEqual([nails])
  })

  it('канистры и розлив одного масла складываются', () => {
    expect(shortages([{ product: oil, units: 4000 }, { product: oil, units: 4000 }], false)).toEqual([])
    expect(shortages([{ product: oil, units: 8000 }, { product: oil, units: 500 }], false)).toEqual([oil])
  })

  it('услуги не проверяются, «Продажа в минус» снимает запрет', () => {
    expect(shortages([{ product: null, units: 0 }], false)).toEqual([])
    expect(shortages([{ product: filter, units: 10 }], true)).toEqual([])
  })
})
