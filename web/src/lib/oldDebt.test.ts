import { describe, expect, it } from 'vitest'
import { earliestOldDebtDate, oldDebtDateError, oldDebtDue, oldDebtPreview, olderThanYear } from './oldDebt'

const nb = (s: string) => s.replace(/ /g, ' ')

describe('старый долг из тетради', () => {
  it('дата — не позже сегодня и не раньше чем 10 лет назад', () => {
    expect(oldDebtDateError('2026-03-15', '2026-10-28')).toBeNull()
    expect(oldDebtDateError('2026-10-28', '2026-10-28')).toBeNull()
    expect(oldDebtDateError('2026-10-29', '2026-10-28')).toMatch(/не позже/)
    expect(oldDebtDateError('2016-10-28', '2026-10-28')).toBeNull()
    expect(oldDebtDateError('2016-10-27', '2026-10-28')).toMatch(/10 лет/)
    expect(oldDebtDateError('', '2026-10-28')).toMatch(/Укажите/)
    expect(earliestOldDebtDate('2028-02-29')).toBe('2018-02-28')
  })

  it('старше года — предупреждение', () => {
    expect(olderThanYear('2025-10-27', '2026-10-28')).toBe(true)
    expect(olderThanYear('2025-10-28', '2026-10-28')).toBe(false)
  })

  it('фирма со сроком 14 дней: срок 28.03, просрочен на 214 дн.', () => {
    expect(oldDebtDue('2026-03-15', 14, '2026-10-28')).toEqual({ due: '2026-03-28', overdueDays: 214 })
    expect(oldDebtDue('2026-10-20', 14, '2026-10-28')).toEqual({ due: '2026-11-02', overdueDays: 0 })
    expect(oldDebtDue('2026-03-15', null, '2026-10-28')).toBeNull()
  })

  it('подсказки под полями', () => {
    expect(oldDebtPreview(200_000, 500_000, '2026-03-15', 14, '2026-10-28').map(nb)).toEqual([
      'Долг клиента сейчас 2 000 с → станет 7 000 с',
      'Срок оплаты по этому долгу — 28.03.2026, просрочен на 214 дн.',
    ])
    expect(oldDebtPreview(0, null, '2026-03-15', null, '2026-10-28')).toEqual([])
    expect(nb(oldDebtPreview(-10_000, 50_000, '2026-03-15', null, '2026-10-28')[0])).toBe('Долг клиента сейчас аванс 100 с → станет 400 с')
  })
})
