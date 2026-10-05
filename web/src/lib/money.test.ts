import { describe, expect, it } from 'vitest'
import { divRound, pourAmount } from './money'

describe('округление как на сервере', () => {
  it('те же примеры, что в server/src/domain/money.rs', () => {
    expect(divRound(5, 2)).toBe(3)
    expect(divRound(-5, 2)).toBe(-3)
    expect(divRound(7, 3)).toBe(2)
    expect(divRound(1, 3)).toBe(0)
    expect(divRound(2, 3)).toBe(1)
    expect(pourAmount(1500, 33_333)).toBe(50_000)
  })
})
