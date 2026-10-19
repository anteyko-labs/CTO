import { describe, expect, it } from 'vitest'
import { stockInCost, stockInQty } from './stockIn'

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
