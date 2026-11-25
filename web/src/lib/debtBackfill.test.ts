import { describe, expect, it } from 'vitest'
import { byEmployee, checksText, confirmText, rateText, type BackfillLine } from './debtBackfill'

const line = (employee: string, name: string, percent: number): BackfillLine => ({
  sale_id: `${employee}-${percent}`,
  sale_number: 1,
  business_date: '2026-11-15',
  party_name: 'ОсОО Долг',
  employee_id: employee,
  employee_name: name,
  rate_bp: 1550,
  base_tyiyn: 100_000,
  percent_tyiyn: percent,
})

describe('процент по ждущим долгам (задача 054)', () => {
  it('ставка с двумя знаками', () => {
    expect(rateText(1550)).toBe('15,5 %')
    expect(rateText(1750)).toBe('17,5 %')
    expect(rateText(200)).toBe('2 %')
    expect(rateText(1525)).toBe('15,25 %')
  })
  it('по кассирам и подтверждение', () => {
    const lines = [line('a', 'Эрбол', 50_000), line('a', 'Эрбол', 25_000), line('b', 'Айбек', 10_000)]
    expect(byEmployee(lines)).toEqual([
      { name: 'Эрбол', count: 2, tyiyn: 75_000 },
      { name: 'Айбек', count: 1, tyiyn: 10_000 },
    ])
    const t = confirmText({ lines, total_tyiyn: 85_000, debt_percent_now: true })
    expect(t).toContain('(3 чека)')
    expect([1, 2, 5, 11, 21, 112].map(checksText)).toEqual(['1 чек', '2 чека', '5 чеков', '11 чеков', '21 чек', '112 чеков'])
    expect(t).toContain('Эрбол')
  })
})
