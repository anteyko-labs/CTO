// Расчётный период оклада и дата выплаты (задача 003 Н11, ADR-154, ADR-155). Период дня D —
// с D-го по (D−1)-е следующего месяца, к выплате D-го; D = 1 — календарный месяц. То же правило —
// `domain::salary::period_of` на сервере; тесты сверяют одни и те же примеры. Даты — строки
// `YYYY-MM-DD`, без часового пояса устройства.
import { formatSom } from './format'
import { lastDayOf, sameMonth } from './payrollMonth'
import type { SalaryNow } from './types'

export interface Period {
  from: string
  to: string
  /** День выплаты — следующий за концом периода. */
  pay: string
}

const pad = (n: number) => String(n).padStart(2, '0')
const iso = (y: number, m: number, d: number) => {
  // m — 1..12 со сдвигом через год.
  const t = y * 12 + (m - 1)
  return `${Math.floor(t / 12)}-${pad((((t % 12) + 12) % 12) + 1)}-${pad(d)}`
}

/** День до или после `date` на `delta` дней (UTC, без пояса устройства). */
export function addDays(date: string, delta: number): string {
  const [y, m, d] = date.split('-').map(Number)
  const t = new Date(Date.UTC(y, m - 1, d + delta))
  return `${t.getUTCFullYear()}-${pad(t.getUTCMonth() + 1)}-${pad(t.getUTCDate())}`
}

/** Тот же день `delta` месяцев спустя; день больше длины месяца — последний день месяца. */
export function addMonths(date: string, delta: number): string {
  const [y, m, d] = date.split('-').map(Number)
  const first = iso(y, m + delta, 1)
  const last = Number(lastDayOf(first).slice(8))
  return `${first.slice(0, 8)}${pad(Math.min(d, last))}`
}

/** Период дня расчёта `day` (1–28, вне — к границе), в который входит `date`. */
export function periodOf(day: number, date: string): Period {
  const d = Math.min(28, Math.max(1, Math.trunc(day) || 1))
  const [y, m, dd] = date.split('-').map(Number)
  const from = dd >= d ? iso(y, m, d) : iso(y, m - 1, d)
  const pay = addMonths(from, 1)
  return { from, to: addDays(pay, -1), pay }
}

/** «20.10–19.11». */
export const periodLabel = (from: string, to: string): string => `${from.slice(8)}.${from.slice(5, 7)}–${to.slice(8)}.${to.slice(5, 7)}`

/** «20.11». */
export const shortDate = (d: string): string => `${d.slice(8)}.${d.slice(5, 7)}`

/** Календарный месяц: с 1-го по последний день. */
export const isCalendar = (from: string, to: string): boolean => from.slice(8) === '01' && to === lastDayOf(from)

/**
 * Дата выплаты по умолчанию в «За месяц»: текущий месяц — сегодня, прошлый — его последний день,
 * чтобы выплата легла в открытый на экране месяц.
 */
export const payoutDate = (month: string, today: string): string => (sameMonth(month, today) ? today : lastDayOf(month))

/** Самая ранняя дата выплаты: сегодня − `days` дней. */
export const earliestPayDate = (today: string, days: number): string => addDays(today, -Math.max(0, Math.trunc(days)))

/** Подпись под датой выплаты: дата в расчёте не сегодня — деньги всё равно уходят сейчас. */
export function payDateHint(payDate: string, today: string, where: string): string {
  if (!payDate || payDate === today) return ''
  return `в расчёте — ${shortDate(payDate)}, деньги уйдут сегодня ${where}`
}

/**
 * Строка оклада в окне «Выплата» и во вкладке «Оклад»: «ставка 30 000 с за 20.10–19.11, не начислен;
 * авансом 5 000 с; к выплате 20.11 — 25 000 с».
 */
export function salaryText(s: SalaryNow): string {
  const parts = [
    s.accrued_tyiyn !== null
      ? `начислено ${formatSom(s.accrued_tyiyn)} за ${periodLabel(s.period_from, s.period_to)}`
      : `ставка ${formatSom(s.rule_tyiyn ?? 0)} за ${periodLabel(s.period_from, s.period_to)}, не начислен`,
  ]
  if (s.advance_tyiyn !== 0) parts.push(`авансом ${formatSom(s.advance_tyiyn)}`)
  if (s.paid_salary_tyiyn !== 0) parts.push(`выдано ${formatSom(s.paid_salary_tyiyn)}`)
  if (s.carry_tyiyn !== 0) parts.push(`с прошлых периодов ${s.carry_tyiyn > 0 ? '+' : '−'}${formatSom(Math.abs(s.carry_tyiyn))}`)
  parts.push(`к выплате ${shortDate(s.pay_date)} — ${formatSom(s.expected_tyiyn)}`)
  return parts.join('; ')
}
