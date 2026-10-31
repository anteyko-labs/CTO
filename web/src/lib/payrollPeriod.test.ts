import { describe, expect, it } from 'vitest'
import { addDays, addMonths, earliestPayDate, isCalendar, payDateHint, payoutDate, periodLabel, periodOf, salaryText } from './payrollPeriod'
import { formatSom as f } from './format'
import type { SalaryNow } from './types'

describe('период оклада (задача 003 Н11) — те же примеры, что domain::salary', () => {
  it('день 1 — календарный месяц', () => {
    expect(periodOf(1, '2026-10-15')).toEqual({ from: '2026-10-01', to: '2026-10-31', pay: '2026-11-01' })
    expect(periodOf(1, '2026-02-28')).toEqual({ from: '2026-02-01', to: '2026-02-28', pay: '2026-03-01' })
    expect(isCalendar('2026-10-01', '2026-10-31')).toBe(true)
    expect(isCalendar('2026-10-20', '2026-11-19')).toBe(false)
  })

  it('день 20: с 20 окт по 19 ноя, к выплате 20 ноя', () => {
    for (const d of ['2026-10-20', '2026-11-01', '2026-11-19']) {
      expect(periodOf(20, d)).toEqual({ from: '2026-10-20', to: '2026-11-19', pay: '2026-11-20' })
    }
    const p = periodOf(20, '2026-11-20')
    expect(periodLabel(p.from, p.to)).toBe('20.11–19.12')
  })

  it('год, февраль, границы дня', () => {
    expect(periodOf(28, '2027-01-10')).toEqual({ from: '2026-12-28', to: '2027-01-27', pay: '2027-01-28' })
    expect(periodOf(28, '2027-02-28')).toEqual({ from: '2027-02-28', to: '2027-03-27', pay: '2027-03-28' })
    expect(periodOf(15, '2028-02-29')).toEqual({ from: '2028-02-15', to: '2028-03-14', pay: '2028-03-15' })
    expect(periodOf(31, '2026-10-30')).toEqual(periodOf(28, '2026-10-30'))
    expect(periodOf(0, '2026-10-30')).toEqual(periodOf(1, '2026-10-30'))
  })

  it('сдвиг дат без пояса устройства', () => {
    expect(addDays('2026-12-31', 1)).toBe('2027-01-01')
    expect(addDays('2026-03-01', -1)).toBe('2026-02-28')
    expect(addMonths('2026-01-31', 1)).toBe('2026-02-28')
    expect(addMonths('2026-01-15', -1)).toBe('2025-12-15')
  })
})

describe('дата выплаты (задача 003 Н11)', () => {
  it('в «За месяц»: текущий — сегодня, прошлый — последний день, через год', () => {
    expect(payoutDate('2026-10-01', '2026-10-31')).toBe('2026-10-31')
    expect(payoutDate('2026-10-01', '2026-11-05')).toBe('2026-10-31')
    expect(payoutDate('2025-12-01', '2026-01-03')).toBe('2025-12-31')
  })

  it('самая ранняя дата и подпись «деньги уйдут сегодня»', () => {
    expect(earliestPayDate('2026-10-31', 35)).toBe('2026-09-26')
    expect(earliestPayDate('2026-10-31', 0)).toBe('2026-10-31')
    expect(payDateHint('2026-10-31', '2026-10-31', 'из «Касса»')).toBe('')
    expect(payDateHint('2026-10-20', '2026-10-22', 'из «Сейф»')).toBe('в расчёте — 20.10, деньги уйдут сегодня из «Сейф»')
  })
})

describe('строка оклада в окне «Выплата»', () => {
  const s: SalaryNow = {
    employee_id: 'e',
    full_name: 'Азамат',
    is_cashier: true,
    is_master: false,
    salary_day: 20,
    period_from: '2026-10-20',
    period_to: '2026-11-19',
    pay_date: '2026-11-20',
    rule_tyiyn: 3_000_000,
    accrued_tyiyn: null,
    accrual_id: null,
    advance_tyiyn: 500_000,
    paid_salary_tyiyn: 0,
    carry_tyiyn: 0,
    expected_tyiyn: 2_500_000,
  }
  it('не начислен — ставка, аванс и к выплате', () => {
    expect(salaryText(s)).toBe(`ставка ${f(3_000_000)} за 20.10–19.11, не начислен; авансом ${f(500_000)}; к выплате 20.11 — ${f(2_500_000)}`)
  })
  it('в день расчёта — закончившийся период «к выплате сегодня», выданное после конца', () => {
    expect(salaryText({ ...s, accrued_tyiyn: 3_000_000, paid_after_tyiyn: 1_000_000, expected_tyiyn: 1_500_000 }, '2026-11-20')).toBe(
      `начислено ${f(3_000_000)} за 20.10–19.11; авансом ${f(500_000)}; выдано с 20.11 ${f(1_000_000)}; к выплате сегодня — ${f(1_500_000)}`,
    )
    expect(salaryText(s, '2026-11-19')).toContain('к выплате 20.11 — ')
  })
  it('начислен, долг прошлых', () => {
    expect(salaryText({ ...s, accrued_tyiyn: 3_000_000, advance_tyiyn: 0, carry_tyiyn: -100_000, expected_tyiyn: 2_900_000 })).toBe(
      `начислено ${f(3_000_000)} за 20.10–19.11; с прошлых периодов −${f(100_000)}; к выплате 20.11 — ${f(2_900_000)}`,
    )
  })
})
