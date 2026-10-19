import { describe, expect, it } from 'vitest'
import { nextField } from './receiptKeys'

describe('приход с клавиатуры', () => {
  it('Enter — следующее поле строки, затем следующая строка, затем поиск', () => {
    expect(nextField(1, 'count', 2)).toBe('line-1-price')
    expect(nextField(1, 'price', 2)).toBe('line-1-markup')
    expect(nextField(1, 'markup', 2)).toBe('line-1-sale')
    expect(nextField(1, 'sale', 2)).toBe('line-2-count')
    expect(nextField(2, 'sale', 2)).toBeNull()
  })
})
