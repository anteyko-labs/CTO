// Откуда оплачена накладная «оплачена сразу» (ADR-062): касса филиала или свои деньги владельца.
import type { CashAccount } from './types'

/** Значение «из своих, мимо касс». */
export const OUTSIDE = 'outside'

export interface PaySource {
  value: string
  label: string
  /** Остаток кассы; у «из своих» — нет. */
  balance_tyiyn: number | null
}

/** Что можно выбрать: администратору — только «Касса», владельцу — любая касса и свои деньги. */
export function paySources(accounts: CashAccount[], owner: boolean): PaySource[] {
  const list = accounts
    .filter((a) => owner || (a.kind === 'register' && !a.owner_only))
    .map((a) => ({ value: a.id, label: a.name, balance_tyiyn: a.balance_tyiyn }))
  return owner ? [...list, { value: OUTSIDE, label: 'Из своих, мимо касс', balance_tyiyn: null }] : list
}

/** Источник по умолчанию: администратору выбирать не из чего, владелец выбирает сам. */
export function defaultPaySource(sources: PaySource[], owner: boolean): string {
  if (owner) return ''
  return sources.length === 1 ? sources[0].value : ''
}

/** Хватит ли денег в выбранной кассе на накладную. */
export function notEnough(sources: PaySource[], value: string, total: number | null): boolean {
  const s = sources.find((x) => x.value === value)
  return s !== undefined && s.balance_tyiyn !== null && total !== null && total > s.balance_tyiyn
}

/** Как показать источник оплаты в документе. */
export function paidFromText(paidFrom: string | null | undefined): string {
  if (!paidFrom) return '—'
  return paidFrom === OUTSIDE ? 'из своих, мимо касс' : `из «${paidFrom}»`
}
