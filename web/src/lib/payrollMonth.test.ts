import { describe, expect, it } from 'vitest'
import { accrualDate, lastDayOf, lastPayout, sameMonth } from './payrollMonth'

describe('«Расчёт за месяц»: действия у строки (задача 002, П13)', () => {
  it('последний день месяца — с високосным февралём и декабрём', () => {
    expect(lastDayOf('2026-10-01')).toBe('2026-10-31')
    expect(lastDayOf('2026-09-01')).toBe('2026-09-30')
    expect(lastDayOf('2028-02-01')).toBe('2028-02-29')
    expect(lastDayOf('2026-02-01')).toBe('2026-02-28')
    expect(lastDayOf('2026-12-01')).toBe('2026-12-31')
  })

  it('бонус в текущем месяце — сегодня, в прошлом — последним днём того месяца', () => {
    expect(sameMonth('2026-10-01', '2026-10-30')).toBe(true)
    expect(sameMonth('2026-09-01', '2026-10-30')).toBe(false)
    expect(accrualDate('2026-10-01', '2026-10-30')).toBe('2026-10-30')
    expect(accrualDate('2026-09-01', '2026-10-30')).toBe('2026-09-30')
  })

  it('последняя выплата — без отменённых и сторно', () => {
    const p = (number: number, extra: Partial<{ reversed: boolean; reversal_of: string | null }> = {}) => ({
      id: `p${number}`,
      number,
      created_at: `2026-10-${String(number).padStart(2, '0')}T10:00:00Z`,
      reversal_of: null,
      reversed: false,
      ...extra,
    })
    expect(lastPayout([])).toBeNull()
    expect(lastPayout([p(1), p(3), p(2)])?.id).toBe('p3')
    expect(lastPayout([p(1), p(3, { reversed: true }), p(4, { reversal_of: 'p3' })])?.id).toBe('p1')
    expect(lastPayout([p(5, { reversed: true })])).toBeNull()
  })
})
