import { describe, expect, it } from 'vitest'
import { avgPrice, qtyText } from './units'

describe('единицы учёта', () => {
  it('количество: литры, килограммы, штуки', () => {
    expect(qtyText('ml', 3500)).toBe('3,5 л')
    expect(qtyText('g', 12_000)).toBe('12 кг')
    expect(qtyText('piece', 3)).toBe('3 шт')
    expect(qtyText(undefined, 2)).toBe('2 шт')
  })

  it('средняя цена весового товара — за кг из стоимости остатка', () => {
    // 12 кг лома на 840 с: 70 с за кг, а не «0,07 за шт».
    expect(avgPrice({ unit: 'g', container_ml: null, stock_qty: 12_000, avg_cost_tyiyn: 7, stock_value_tyiyn: 84_000 })).toEqual({ tyiyn: 7000, per: 'за кг' })
    expect(avgPrice({ unit: 'g', container_ml: null, stock_qty: 0, avg_cost_tyiyn: 8, stock_value_tyiyn: 0 })).toEqual({ tyiyn: 8000, per: 'за кг' })
  })

  it('масло в таре — за канистру, штучный — за шт, без средней — null', () => {
    expect(avgPrice({ unit: 'ml', container_ml: 4000, stock_qty: 4000, avg_cost_tyiyn: 200_000, stock_value_tyiyn: 200_000 })).toEqual({ tyiyn: 200_000, per: 'за 4 л' })
    expect(avgPrice({ unit: 'ml', container_ml: null, stock_qty: 2000, avg_cost_tyiyn: 300, stock_value_tyiyn: 600_000 })).toEqual({ tyiyn: 300_000, per: 'за л' })
    expect(avgPrice({ unit: 'piece', container_ml: null, stock_qty: 7, avg_cost_tyiyn: 40_000, stock_value_tyiyn: 280_000 })).toEqual({ tyiyn: 40_000, per: 'за шт' })
    expect(avgPrice({ unit: 'piece', container_ml: null, stock_qty: 7 })).toBeNull()
  })
})
