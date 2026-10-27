// Выплата сотруднику по видам и аванс (SPEC-36 §11, ADR-122): строки окна «Выплата».
import type { PayoutPurpose, PayoutRemains } from './types'

/** Виды в окне «Выплата» по порядку; аванс — последним. */
export const PAYOUT_PURPOSES: PayoutPurpose[] = ['percent', 'service_fee', 'bonus', 'salary', 'advance']

/** Как вид называется на экранах; `general` — прежние выплаты без вида. */
export const PURPOSE_LABELS: Record<string, string> = {
  percent: 'Проценты',
  service_fee: 'Мастеру за услуги',
  bonus: 'Бонус',
  salary: 'Оклад',
  advance: 'Аванс',
  general: 'Без вида',
}

/** Остаток вида из ответа `/payroll/remains`; у аванса остатка нет. */
export function remainOf(r: PayoutRemains, purpose: PayoutPurpose): number {
  switch (purpose) {
    case 'percent':
      return r.remains.percent_tyiyn
    case 'service_fee':
      return r.remains.service_fee_tyiyn
    case 'bonus':
      return r.remains.bonus_tyiyn
    case 'salary':
      return r.remains.salary_tyiyn
    default:
      return 0
  }
}

export interface PayoutLineDraft {
  purpose: PayoutPurpose
  /** Сколько к выдаче по виду. */
  remain: number
  checked: boolean
  /** Сумма строкой, как ввёл человек. */
  sum: string
}

/**
 * Строки окна: виды с остатком — отмечены, сумма = остаток. Администратор выдаёт мастеру только
 * за услуги и, с правом, аванс (ADR-122); аванс всегда снят, сумма пустая.
 */
export function initialLines(r: PayoutRemains, opts: { owner: boolean; advance: boolean }, som: (tyiyn: number) => string): PayoutLineDraft[] {
  const out: PayoutLineDraft[] = []
  for (const purpose of PAYOUT_PURPOSES) {
    if (purpose === 'advance') {
      if (opts.advance) out.push({ purpose, remain: 0, checked: false, sum: '' })
      continue
    }
    if (!opts.owner && purpose !== 'service_fee') continue
    const remain = remainOf(r, purpose)
    if (remain > 0) out.push({ purpose, remain, checked: true, sum: som(remain) })
  }
  return out
}

/** Что уйдёт на сервер: отмеченные строки с суммой; ошибка — первая неверная строка. */
export function payoutLines(
  lines: PayoutLineDraft[],
  parse: (s: string) => number | null,
): { lines: { purpose: PayoutPurpose; amount_tyiyn: number }[]; total: number; error: string | null } {
  const out: { purpose: PayoutPurpose; amount_tyiyn: number }[] = []
  let total = 0
  let error: string | null = null
  for (const l of lines) {
    if (!l.checked) continue
    const amount = parse(l.sum)
    if (amount === null || amount <= 0) {
      error ??= `${PURPOSE_LABELS[l.purpose]}: неверная сумма`
      continue
    }
    // Аванс владельца — сверх заработанного; остальное — не больше остатка вида.
    if (l.purpose !== 'advance' && amount > l.remain) error ??= `${PURPOSE_LABELS[l.purpose]}: больше, чем к выдаче`
    out.push({ purpose: l.purpose, amount_tyiyn: amount })
    total += amount
  }
  if (!error && out.length === 0) error = 'Отметьте, за что выплатить'
  return { lines: out, total, error }
}
