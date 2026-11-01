// «Мой заработок» (задача 005 Н1, ADR-170): ответ `GET /me/earnings` и подписи экрана.

/** Заработок сотрудника самого входа за месяц — только своё. */
export interface MyEarnings {
  employee_id: string
  full_name: string
  is_cashier: boolean
  is_master: boolean
  today: string
  today_tyiyn: number
  today_lines: string[]
  month_from: string
  month_to: string
  months: string[]
  opening_tyiyn: number
  accrued_tyiyn: number
  paid_tyiyn: number
  advance_tyiyn: number
  closing_tyiyn: number
  lines: string[]
  salary_line: string | null
  days: { date: string; amount_tyiyn: number }[]
  payouts: { date: string; amount_tyiyn: number; purpose: string; reversal: boolean }[]
  owed_now_tyiyn: number
}

const MONTHS = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь']

/** «2026-11-01» → «ноябрь 2026». */
export function monthLabel(iso: string): string {
  const [y, m] = iso.split('-')
  return `${MONTHS[Number(m) - 1] ?? m} ${y}`
}
