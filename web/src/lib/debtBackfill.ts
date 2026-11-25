// Процент кассирам по ждущим долговым чекам — разом (задача 054): что покажет кнопка в
// «Настройки → Касса». Сервер считает сам (`GET/POST /payroll/debt-backfill`).
import { formatSom } from './format'

export interface BackfillLine {
  sale_id: string
  sale_number: number
  business_date: string
  party_name: string
  employee_id: string
  employee_name: string
  rate_bp: number
  base_tyiyn: number
  percent_tyiyn: number
}

export interface Backfill {
  lines: BackfillLine[]
  total_tyiyn: number
  debt_percent_now: boolean
}

/** «15,5 %» из 1550 б.п. */
export function rateText(bp: number): string {
  const whole = Math.trunc(bp / 100)
  const frac = bp % 100
  if (frac === 0) return `${whole} %`
  return `${whole},${String(frac).padStart(2, '0').replace(/0$/, '')} %`
}

/** «1 чек», «3 чека», «11 чеков». */
export function checksText(n: number): string {
  const d10 = n % 10
  const d100 = n % 100
  const word = d10 === 1 && d100 !== 11 ? 'чек' : d10 >= 2 && d10 <= 4 && (d100 < 12 || d100 > 14) ? 'чека' : 'чеков'
  return `${n} ${word}`
}

/** По кассирам: имя, чеков, сумма — для подтверждения. */
export function byEmployee(lines: BackfillLine[]): { name: string; count: number; tyiyn: number }[] {
  const out = new Map<string, { name: string; count: number; tyiyn: number }>()
  for (const l of lines) {
    const e = out.get(l.employee_id) ?? { name: l.employee_name, count: 0, tyiyn: 0 }
    e.count += 1
    e.tyiyn += l.percent_tyiyn
    out.set(l.employee_id, e)
  }
  return [...out.values()].sort((a, b) => b.tyiyn - a.tyiyn)
}

/** Текст подтверждения перед «Начислить». */
export function confirmText(b: Backfill): string {
  const who = byEmployee(b.lines)
    .map((e) => `${e.name} — ${formatSom(e.tyiyn)} (${checksText(e.count)})`)
    .join('; ')
  return `Начислить ${formatSom(b.total_tyiyn)} процента кассирам по ждущим долговым чекам (${checksText(b.lines.length)}) датой каждого чека? ${who}. Отменить можно только сторно начислений.`
}
