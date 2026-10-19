import { describe, expect, it } from 'vitest'
import { returnAmount, returnQty } from './supplierReturn'

describe('возврат поставщику', () => {
  it('сумма по умолчанию — цена последнего прихода × количество', () => {
    expect(returnAmount({ unit: 'piece', container_ml: null }, 4, 10_000)).toBe(40_000)
    expect(returnAmount({ unit: 'ml', container_ml: 4000 }, 2000, 150_000)).toBe(75_000)
  })

  it('количество из поля', () => {
    expect(returnQty({ unit: 'piece' }, '4')).toBe(4)
    expect(returnQty({ unit: 'ml' }, '2,5')).toBe(2500)
    expect(returnQty({ unit: 'piece' }, '')).toBeNull()
  })
})
