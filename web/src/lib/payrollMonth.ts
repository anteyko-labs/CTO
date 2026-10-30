// Действия в «Расчёте за месяц» (задача 002, П13): дата начисления и последняя выплата месяца.

/** Выплата из `/payroll/entries` — только то, что нужно для выбора последней. */
export interface MonthPayout {
  id: string
  number: number
  created_at: string
  reversal_of: string | null
  reversed: boolean
}

/** Месяц `YYYY-MM-01` — тот же, что у дня `YYYY-MM-DD`. */
export const sameMonth = (month: string, day: string): boolean => month.slice(0, 7) === day.slice(0, 7)

/** Последний день месяца `YYYY-MM-01`, без часового пояса устройства. */
export function lastDayOf(month: string): string {
  const [y, m] = month.split('-').map(Number)
  // День 0 следующего месяца — последний день этого; считаем в UTC, чтобы пояс не сдвинул дату.
  const d = new Date(Date.UTC(y, m, 0)).getUTCDate()
  return `${month.slice(0, 7)}-${String(d).padStart(2, '0')}`
}

/**
 * Дата бонуса или удержания из месяца: текущий месяц — сегодня, прошлый — его последний день,
 * чтобы начисление попало в открытый на экране месяц и строка сразу пересчиталась.
 */
export const accrualDate = (month: string, today: string): string => (sameMonth(month, today) ? today : lastDayOf(month))

/**
 * Последняя действующая выплата: не отменённая и не сама сторно; из них — с большим номером
 * (номера растут), при равных — позже по времени. Нет — null.
 */
export function lastPayout<T extends MonthPayout>(payouts: T[]): T | null {
  let best: T | null = null
  for (const p of payouts) {
    if (p.reversed || p.reversal_of) continue
    if (!best || p.number > best.number || (p.number === best.number && p.created_at > best.created_at)) best = p
  }
  return best
}
