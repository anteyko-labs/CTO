// График выходных (задача 037 п.5, ADR-203): месяц, по строке на работника, клик — выходной.
// Сервер не даёт поставить выходной, если работающих в день станет меньше минимума (0 — без ограничения).

export interface DaysOffEmployee {
  id: string
  full_name: string
  days_off: string[]
}

export interface DaysOff {
  month: string
  days_in_month: number
  min_on_duty: number
  employees: DaysOffEmployee[]
}

/** Дата дня месяца «ГГГГ-ММ-ДД». */
export function dayOf(month: string, day: number): string {
  return `${month}-${String(day).padStart(2, '0')}`
}

/** Сколько работников в этот день не на выходном. */
export function onDuty(employees: readonly DaysOffEmployee[], date: string): number {
  return employees.filter((e) => !e.days_off.includes(date)).length
}

/** День ниже минимума (минимум 0 — никогда). */
export function belowMinimum(employees: readonly DaysOffEmployee[], date: string, min: number): boolean {
  return min > 0 && onDuty(employees, date) < min
}

/** Можно ли поставить ещё один выходной в этот день, не нарушив минимум. */
export function canTakeOff(employees: readonly DaysOffEmployee[], date: string, min: number): boolean {
  return min <= 0 || onDuty(employees, date) - 1 >= min
}
