import { describe, expect, it } from 'vitest'
import { returnAmount, returnQty } from './supplierReturn'

describe('возврат поставщику', () => {
  it('сумма по умолчанию — цена последнего прихода × количество', () => {
    expect(returnAmount({ unit: 'piece', container_ml: null }, 4, 10_000)).toBe(40_000)
    expect(returnAmount({ unit: 'ml', container_ml: 4000 }, 2000, 150_000)).toBe(75_000)
    // 1,5 м по 200 с за м = 300 с; 2,5 кг по 100 с за кг = 250 с.
    expect(returnAmount({ unit: 'mm', container_ml: null }, 1500, 20_000)).toBe(30_000)
    expect(returnAmount({ unit: 'g', container_ml: null }, 2500, 10_000)).toBe(25_000)
  })

  it('количество из поля', () => {
    expect(returnQty({ unit: 'piece' }, '4')).toBe(4)
    expect(returnQty({ unit: 'ml' }, '2,5')).toBe(2500)
    expect(returnQty({ unit: 'piece' }, '')).toBeNull()
    expect(returnQty({ unit: 'mm' }, '1,25')).toBe(1250)
    expect(returnQty({ unit: 'mm' }, '0')).toBeNull()
  })
})
