import { describe, expect, it } from 'vitest'
import { monthLabel } from './myEarnings'

describe('«Мой заработок» (задача 005 Н1)', () => {
  it('месяц словами', () => {
    expect(monthLabel('2026-11-01')).toBe('ноябрь 2026')
    expect(monthLabel('2027-01-01')).toBe('январь 2027')
  })
})
