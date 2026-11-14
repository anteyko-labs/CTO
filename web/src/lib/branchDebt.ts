// Долги клиента раздельно по филиалам (задача 036, ADR-197): крупно — долг этого филиала,
// ниже серым — долги в других филиалах в скобках: «(в Автодом: 1 200 с)».
import { formatSom } from './format'
import type { BranchBalance } from './types'

type DebtParty = { debts_separate?: boolean; branch_balances?: BranchBalance[] }

/** Долги клиента в других филиалах; пусто — долги не раздельно или других нет. */
export function otherBranchDebts(p: DebtParty | null | undefined): BranchBalance[] {
  if (!p?.debts_separate) return []
  return (p.branch_balances ?? []).filter((b) => !b.here && b.balance_tyiyn !== 0)
}

/** Строка под долгом: «(в Автодом: 1 200 с; в Айкос: аванс 300 с)»; пусто — нечего показать. */
export function otherBranchDebtText(p: DebtParty | null | undefined): string {
  const list = otherBranchDebts(p)
  if (!list.length) return ''
  const parts = list.map((b) => `в ${b.name}: ${b.balance_tyiyn > 0 ? '' : 'аванс '}${formatSom(Math.abs(b.balance_tyiyn))}`)
  return `(${parts.join('; ')})`
}
