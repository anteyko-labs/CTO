import { describe, expect, it } from 'vitest'
import { batteryAmount, batteryShare, paymentLabel, polarityMark, productTitle } from './battery'

describe('аккумуляторы', () => {
  it('полярность в названии: Л — прямая, П — обратная', () => {
    expect(polarityMark({ polarity: 'прямая' })).toBe('Л')
    expect(polarityMark({ polarity: 'обратная' })).toBe('П')
    expect(polarityMark({})).toBeNull()
    expect(productTitle({ name: 'Varta 60', attrs: { polarity: 'обратная' } })).toBe('Varta 60 (П)')
    expect(productTitle({ name: 'Фильтр', attrs: {} })).toBe('Фильтр')
  })

  it('12 кг × 50 с = 600 с, как на сервере', () => {
    expect(batteryAmount(12_000, 5_000)).toBe(60_000)
    expect(batteryAmount(12_345, 5_000)).toBe(61_725)
    expect(paymentLabel({ method: 'battery', grams: 12_000 })).toBe('Аккумулятор 12\u00a0кг')
    expect(paymentLabel({ method: 'battery', grams: -12_000 })).toBe('Аккумулятор 12\u00a0кг')
    expect(paymentLabel({ method: 'cash' })).toBe('Наличные')
  })

  it('возврат: деньгами — не больше, чем платили деньгами', () => {
    // Чек 10 000 = 9 400 наличными + 600 аккумулятором.
    expect(batteryShare(500_000, 60_000, 940_000)).toBe(0)
    expect(batteryShare(500_000, 60_000, 440_000)).toBe(60_000)
    expect(batteryShare(1_000_000, 60_000, 940_000)).toBe(60_000)
    expect(batteryShare(1_000_000, 0, 940_000)).toBe(0)
  })
})
