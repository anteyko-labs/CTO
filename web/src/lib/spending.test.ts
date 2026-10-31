import { describe, expect, it } from 'vitest'
import { SPENDING_EXPORT_COLS, spendingKinds, spendingMark, spendingPath, spendingWhat, summaryLines, summarySheetRows } from './spending'
import type { SpendingRow, SpendingSummary } from './types'

const row = (patch: Partial<SpendingRow>): SpendingRow => ({
  kind: 'expense',
  id: 'x',
  number: 7,
  date: '2026-10-30',
  amount_tyiyn: 12_300,
  article: 'Хозтовары',
  purpose: null,
  payee: 'Хозмаг',
  employee_id: null,
  comment: 'ведро',
  source: 'account',
  account_name: 'Касса',
  user_name: 'Владелец',
  created_at: '2026-10-30T05:00:00Z',
  reversal_of: null,
  reversed: false,
  ...patch,
})

const summary = (patch: Partial<SpendingSummary> = {}): SpendingSummary => ({
  articles: [{ article: 'Хозтовары', amount_tyiyn: 10_000 }],
  expenses_tyiyn: 10_000,
  payouts_tyiyn: 3_000,
  purposes: [
    { purpose: 'bonus', amount_tyiyn: 2_000 },
    { purpose: 'general', amount_tyiyn: 1_000 },
  ],
  bonus_paid_tyiyn: 2_000,
  bonus_accrued_tyiyn: 8_000,
  bonus_unpaid_tyiyn: 6_000,
  total_tyiyn: 13_000,
  expenses: true,
  payouts: true,
  ...patch,
})

describe('«Расходы» — все траты', () => {
  it('«Что»: статья, вид выплаты, начислен бонус', () => {
    expect(spendingWhat(row({}))).toBe('Хозтовары')
    expect(spendingWhat(row({ kind: 'payout', purpose: 'salary', article: 'Зарплата работникам' }))).toBe('Выплата: Оклад')
    expect(spendingWhat(row({ kind: 'payout', purpose: 'bonus' }))).toBe('Выплата: Бонус')
    expect(spendingWhat(row({ kind: 'payout', purpose: null }))).toBe('Выплата: Без вида')
    expect(spendingWhat(row({ kind: 'bonus_accrual', purpose: 'bonus', number: null }))).toBe('начислен бонус')
  })

  it('отметка сторно', () => {
    expect(spendingMark(row({}))).toBe('')
    expect(spendingMark(row({ reversal_of: 'y' }))).toBe('сторно')
    expect(spendingMark(row({ reversed: true }))).toBe('сторнирован')
  })

  it('отборы по правам: без «Расчёта» — только расходы, без «Расходов» — только выплаты', () => {
    expect(spendingKinds({ expenses: true, payroll: true }).map(([k]) => k)).toEqual(['all', 'expenses', 'payouts'])
    expect(spendingKinds({ expenses: true, payroll: false }).map(([k]) => k)).toEqual(['expenses'])
    expect(spendingKinds({ expenses: false, payroll: true }).map(([k]) => k)).toEqual(['payouts'])
  })

  it('адрес: «Всё» без параметра отбора, сторно — по переключателю', () => {
    expect(spendingPath({ from: '2026-10-01', to: '2026-10-31', kind: 'all', reversals: false })).toBe('/expenses/all?from=2026-10-01&to=2026-10-31')
    expect(spendingPath({ from: '2026-10-01', to: '2026-10-31', kind: 'payouts', reversals: true }, true)).toBe(
      '/expenses/all/summary?from=2026-10-01&to=2026-10-31&kind=payouts&reversals=1',
    )
  })

  it('итог: статьи, «Зарплата работникам» с «в т. ч. бонусы», невыданное — отдельно, итог не задваивает', () => {
    const lines = summaryLines(summary())
    expect(lines.map((l) => [l.label, l.amount, l.tone])).toEqual([
      ['Хозтовары', 10_000, 'article'],
      ['Зарплата работникам', 3_000, 'group'],
      ['в т. ч. бонусы', 2_000, 'sub'],
      ['Бонусы: начислено, не выдано (в итог не входит)', 6_000, 'info'],
      ['Итого', 13_000, 'total'],
    ])
    // Итого = статьи + выданное; начисленное не выданное в него не входит.
    const counted = lines.filter((l) => l.tone === 'article' || l.tone === 'group').reduce((a, l) => a + l.amount, 0)
    expect(counted).toBe(lines.at(-1)?.amount)
  })

  it('итог без выплат (нет права «Расчёт») и без начисленных бонусов (настройка выключена)', () => {
    expect(summaryLines(summary({ payouts: false, payouts_tyiyn: 0, total_tyiyn: 10_000 })).map((l) => l.key)).toEqual(['article.Хозтовары', 'total'])
    expect(summaryLines(summary({ bonus_unpaid_tyiyn: null, bonus_accrued_tyiyn: null })).map((l) => l.key)).not.toContain('bonus.unpaid')
    expect(summarySheetRows(summary()).find((r) => r.label.trim() === 'в т. ч. бонусы')?.bold).toBe(false)
  })

  it('выгрузка: деньги целыми тыйынами, у начисления — без номера и с отметкой', () => {
    const cols = SPENDING_EXPORT_COLS.map((c) => c.label)
    expect(cols).toEqual(['№', 'Дата', 'Что', 'Кому', 'За что', 'Касса', 'Сумма', 'Кто провёл', 'Когда', 'Отметка'])
    const bonus = row({ kind: 'bonus_accrual', number: null, purpose: 'bonus', payee: 'Мастер', account_name: null, source: null })
    const cells = SPENDING_EXPORT_COLS.map((c) => c.value(bonus))
    expect(cells[0]).toBe('')
    expect(cells[6]).toEqual({ money: 12_300 })
    expect(cells[9]).toBe('в итог не входит')
    expect(SPENDING_EXPORT_COLS[9].value(row({ reversed: true }))).toBe('сторнирован')
  })
})
