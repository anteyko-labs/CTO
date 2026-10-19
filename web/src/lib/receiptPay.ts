// Оплата накладной при приходе (ADR-062, SPEC-29 §4): касса или счёт филиала; «из своих, мимо
// касс» больше нет (ADR-079) — остаётся только в подписи прежних накладных.
import { isTill } from './accounts'
import type { CashAccount } from './types'

/** «Из своих, мимо касс» — только в прежних накладных (до ADR-079). */
export const OUTSIDE = 'outside'

export interface PaySource {
  value: string
  label: string
  balance_tyiyn: number
  /** Счёт может уйти ниже нуля (ADR-076). */
  allow_negative?: boolean
}

/** Как рассчитались при приходе: сразу целиком, часть сейчас и остаток в долг, всё в долг. */
export type PayMode = 'paid' | 'part' | 'debt'

/** Что можно выбрать: администратору — только «Касса», владельцу — любая касса или счёт. */
export function paySources(accounts: CashAccount[], owner: boolean): PaySource[] {
  return accounts
    .filter((a) => owner || (isTill(a) && !a.owner_only))
    .map((a) => ({ value: a.id, label: a.name, balance_tyiyn: a.balance_tyiyn, allow_negative: a.allow_negative }))
}

/** Источник по умолчанию: администратору выбирать не из чего, владелец выбирает сам. */
export function defaultPaySource(sources: PaySource[], owner: boolean): string {
  if (owner) return ''
  return sources.length === 1 ? sources[0].value : ''
}

/** Хватит ли денег на выбранном счёте на оплату. */
export function notEnough(sources: PaySource[], value: string, amount: number | null): boolean {
  const s = sources.find((x) => x.value === value)
  return s !== undefined && !s.allow_negative && amount !== null && amount > s.balance_tyiyn
}

/** Сколько уходит с выбранного счёта сейчас: вся накладная, её часть или ничего. */
export function paidNow(mode: PayMode, total: number | null, part: number | null): number | null {
  if (mode === 'debt') return 0
  if (mode === 'paid') return total
  return part
}

/** Часть сейчас: больше нуля и меньше суммы накладной; иначе — текст, что не так. */
export function partProblem(part: number | null, total: number | null): string | null {
  if (part === null) return 'сколько оплачено сейчас — числом'
  if (part <= 0) return 'оплачено сейчас — больше нуля'
  if (total !== null && part >= total) return 'оплачено сейчас меньше суммы накладной — иначе выберите «оплачена сразу»'
  return null
}

/** Как показать источник оплаты в документе. */
export function paidFromText(paidFrom: string | null | undefined): string {
  if (!paidFrom) return '—'
  return paidFrom === OUTSIDE ? 'из своих, мимо касс' : `из «${paidFrom}»`
}
