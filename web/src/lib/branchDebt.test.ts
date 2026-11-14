import { describe, expect, it } from 'vitest'
import { otherBranchDebtText, otherBranchDebts } from './branchDebt'
import { formatSom } from './format'

const balances = [
  { branch_id: 'a', name: 'Автодом', balance_tyiyn: 120_000, here: false },
  { branch_id: 'b', name: 'Айкос', balance_tyiyn: 50_000, here: true },
]

describe('долг в других филиалах (задача 036)', () => {
  it('раздельно — другие филиалы в скобках, свой не повторяется', () => {
    expect(otherBranchDebtText({ debts_separate: true, branch_balances: balances })).toBe(`(в Автодом: ${formatSom(120_000)})`)
    expect(otherBranchDebts({ debts_separate: true, branch_balances: balances }).map((b) => b.branch_id)).toEqual(['a'])
  })

  it('аванс в другом филиале — словом «аванс»', () => {
    const p = { debts_separate: true, branch_balances: [{ branch_id: 'a', name: 'Автодом', balance_tyiyn: -30_000, here: false }] }
    expect(otherBranchDebtText(p)).toBe(`(в Автодом: аванс ${formatSom(30_000)})`)
  })

  it('не раздельно или других долгов нет — пусто', () => {
    expect(otherBranchDebtText({ debts_separate: false, branch_balances: balances })).toBe('')
    expect(otherBranchDebtText({ debts_separate: true, branch_balances: [balances[1]] })).toBe('')
    expect(otherBranchDebtText(null)).toBe('')
  })
})
