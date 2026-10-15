import { describe, expect, it } from 'vitest'
import { balanceText, parseQty, qtyInput, qtyInputLabel, qtyText, refundEffect } from './defects'

const nb = (s: string) => s.replace(/ /g, ' ')

describe('брак', () => {
  it('количество в единицах товара', () => {
    expect(parseQty('2', 'piece')).toBe(2)
    expect(parseQty('1,5', 'piece')).toBeNull()
    expect(parseQty('0', 'piece')).toBeNull()
    expect(parseQty('-1', 'piece')).toBeNull()
    expect(parseQty('4', 'ml')).toBe(4000)
    expect(parseQty('1,5', 'ml')).toBe(1500)
    expect(parseQty('0,25', 'g')).toBe(250)
    expect(parseQty('0', 'ml')).toBeNull()
    expect(parseQty('abc', 'ml')).toBeNull()
    expect(qtyInputLabel('ml')).toBe('л')
    expect(qtyInputLabel('g')).toBe('кг')
    expect(qtyInputLabel('piece')).toBe('шт')
  })

  it('показ количества', () => {
    expect(nb(qtyText(2, 'piece'))).toBe('2 шт')
    expect(nb(qtyText(4000, 'ml'))).toBe('4 л')
    expect(nb(qtyText(1250, 'g'))).toBe('1,25 кг')
    expect(qtyInput(1500, 'ml')).toBe('1,5')
    expect(qtyInput(3, 'piece')).toBe('3')
  })

  it('разница возмещения и баланс поставщика', () => {
    expect(nb(refundEffect(10000, 12000))).toBe('доход 20 с')
    expect(nb(refundEffect(10000, 5000))).toBe('убыток 50 с')
    expect(refundEffect(10000, 10000)).toBe('без разницы со стоимостью')
    expect(nb(balanceText(-150000))).toBe('мы должны 1 500 с')
    expect(nb(balanceText(24000))).toBe('он должен нам 240 с')
    expect(balanceText(0)).toBe('расчёты закрыты')
  })
})
