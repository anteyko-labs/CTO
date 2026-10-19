// Свои счета (SPEC-28 §5, ADR-076, ADR-077): кассы и безналичные счета, итоги «Обзора счетов», выбор
// банка для карты и перевода (свой счёт по умолчанию у каждого способа). Права и остатки проверяет
// сервер; здесь — только показ и выбор.
import { formatSom } from './format'
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

/** Способ безналичной оплаты, у которого свой счёт по умолчанию (ADR-077). */
export type NoncashMethod = 'card' | 'transfer'

type DefaultMarks = { default_card?: boolean; default_transfer?: boolean }

/** Счёт по умолчанию для способа оплаты. */
export function isDefaultFor(a: DefaultMarks, method: NoncashMethod): boolean {
  return Boolean(method === 'card' ? a.default_card : a.default_transfer)
}

/** Пометка рядом с названием: касса смены, «по умолчанию для карты / перевода», сейф. */
export function accountNote(a: Pick<CashAccount, 'kind' | 'is_default'> & DefaultMarks): string {
  if (isTill(a)) return 'касса смены'
  if (isBank(a)) {
    if (a.default_card && a.default_transfer) return 'по умолчанию для карты и перевода'
    if (a.default_card) return 'по умолчанию для карты'
    if (a.default_transfer) return 'по умолчанию для перевода'
    return ''
  }
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

/** Безналичные счета для выбора: действующие; счёт по умолчанию для способа — первым. */
export function bankChoices<T extends Pick<CashAccount, 'id' | 'kind'> & DefaultMarks & { active?: boolean }>(
  accounts: readonly T[],
  method: NoncashMethod = 'card',
): T[] {
  return accounts
    .filter((a) => isBank(a) && a.active !== false)
    .sort((x, y) => Number(isDefaultFor(y, method)) - Number(isDefaultFor(x, method)))
}

/** Счёт по умолчанию для способа оплаты (или первый действующий); нет банков — ''. */
export function defaultBankId(
  accounts: readonly (Pick<CashAccount, 'id' | 'kind'> & DefaultMarks & { active?: boolean })[],
  method: NoncashMethod = 'card',
): string {
  return bankChoices(accounts, method)[0]?.id ?? ''
}

/** Хватит ли на счёте денег на расход: с разрешённым минусом — всегда. */
export function enoughOn(a: Pick<CashAccount, 'balance_tyiyn' | 'allow_negative'> | undefined, amount: number): boolean {
  if (!a) return true
  return a.allow_negative || amount <= a.balance_tyiyn
}

/** Счёт с остатком для выбора: «Касса — 12 300 с» (SPEC-29 §12). Остаток — с сервера, как есть. */
export function accountLabel(a: Pick<CashAccount, 'name' | 'balance_tyiyn'>): string {
  return `${a.name} — ${formatSom(a.balance_tyiyn)}`
}

/** Откуда платить сотруднику или поставщику: владелец — любой действующий счёт, администратор —
 *  только касса смены (ADR-076, SPEC-29 §13). Касса смены — первой. Права проверяет сервер. */
export function payAccounts<T extends Pick<CashAccount, 'kind' | 'is_default' | 'owner_only'> & { active?: boolean }>(
  accounts: readonly T[],
  owner: boolean,
): T[] {
  return accounts
    .filter((a) => a.active !== false && (owner || (isTill(a) && !a.owner_only)))
    .sort((x, y) => Number(isTill(y)) - Number(isTill(x)))
}

/** Куда сдать кассу: любой действующий счёт, кроме самой кассы смены; первым — предложенный
 *  сервером (куда сдавали в прошлый раз или сейф магазина), иначе первый сейф (SPEC-29 §14). */
export function handoverChoices<T extends Pick<CashAccount, 'id' | 'kind' | 'is_default'> & { active?: boolean }>(
  accounts: readonly T[],
  suggested: string | null | undefined,
): { list: T[]; pick: string } {
  const list = accounts.filter((a) => a.active !== false && !isTill(a))
  const pick = list.some((a) => a.id === suggested) ? (suggested as string) : (list.find((a) => a.kind === 'safe')?.id ?? list[0]?.id ?? '')
  return { list, pick }
}

/** Ставка комиссии коротко: 50 → «0,5 %», 100 → «1 %». */
export function feeShort(bp: number): string {
  const abs = Math.abs(bp)
  const frac = String(abs % 100).padStart(2, '0').replace(/0+$/, '')
  return `${bp < 0 ? '−' : ''}${Math.trunc(abs / 100)}${frac ? `,${frac}` : ''} %`
}

/** Подпись под кнопками оплаты на кассе: куда идут деньги и комиссия банка (SPEC-29 §25).
 *  Наличные — в кассу смены; карта и QR банка, QR владельцу — на выбранный счёт или счёт по
 *  умолчанию для способа. Ставка — с сервера или из снимка кассы без сети. */
export function payCaption(
  method: 'cash' | NoncashMethod,
  bank?: Pick<CashAccount, 'name'> & { fee_rate_bp?: number },
): string {
  if (method === 'cash') return '→ Касса · наличные'
  if (!bank) return '→ счёт по умолчанию'
  const fee = bank.fee_rate_bp ?? 0
  return `→ ${bank.name} · ${fee > 0 ? `комиссия ${feeShort(fee)}` : 'без комиссии'}`
}
