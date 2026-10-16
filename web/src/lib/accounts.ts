// Свои счета (SPEC-28 §5, ADR-076): кассы и безналичные счета, итоги «Обзора счетов», выбор банка
// для карты и перевода. Права и остатки проверяет сервер; здесь — только показ и выбор.
import type { CashAccount } from './types'

/** Касса смены: к ней привязана смена, в неё идут наличные чеков. Одна на филиал. */
export function isTill(a: Pick<CashAccount, 'kind' | 'is_default'>): boolean {
  return a.kind === 'register' && a.is_default
}

export function isBank(a: Pick<CashAccount, 'kind'>): boolean {
  return a.kind === 'bank'
}

/** Тип счёта словом, как в старой программе. */
export function accountType(a: Pick<CashAccount, 'kind'>): 'Наличный' | 'Безналичный' {
  return isBank(a) ? 'Безналичный' : 'Наличный'
}

/** Пометка рядом с названием: касса смены, основной безналичный, сейф. */
export function accountNote(a: Pick<CashAccount, 'kind' | 'is_default'>): string {
  if (isTill(a)) return 'касса смены'
  if (isBank(a) && a.is_default) return 'основной'
  if (a.kind === 'safe') return 'сейф'
  return ''
}

/** Итог «Обзора счетов» по действующим счетам: наличные, безнал, всего. */
export function accountTotals(accounts: readonly CashAccount[]): { cash: number; bank: number; total: number } {
  let cash = 0
  let bank = 0
  for (const a of accounts) {
    if (a.active === false) continue
    if (isBank(a)) bank += a.balance_tyiyn
    else cash += a.balance_tyiyn
  }
  return { cash, bank, total: cash + bank }
}

/** Безналичные счета для выбора: действующие, основной первым. */
export function bankChoices<T extends Pick<CashAccount, 'id' | 'kind' | 'is_default'> & { active?: boolean }>(accounts: readonly T[]): T[] {
  return accounts.filter((a) => isBank(a) && a.active !== false).sort((x, y) => Number(y.is_default) - Number(x.is_default))
}

/** Основной безналичный счёт (или первый действующий). */
export function defaultBankId(accounts: readonly (Pick<CashAccount, 'id' | 'kind' | 'is_default'> & { active?: boolean })[]): string {
  return bankChoices(accounts)[0]?.id ?? ''
}

/** Хватит ли на счёте денег на расход: с разрешённым минусом — всегда. */
export function enoughOn(a: Pick<CashAccount, 'balance_tyiyn' | 'allow_negative'> | undefined, amount: number): boolean {
  if (!a) return true
  return a.allow_negative || amount <= a.balance_tyiyn
}
