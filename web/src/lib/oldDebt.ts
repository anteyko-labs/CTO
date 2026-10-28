// Старый долг из тетради (SPEC-38, ADR-133): проверки даты и подсказки окна «Занести старый долг».
// Окончательно решает сервер; здесь — только что показать до отправки.
import { formatDate, formatSom, shiftDate } from './format'

/** Дата долга — не раньше чем столько лет назад (SPEC-38 §2). */
export const OLD_DEBT_YEARS = 10
/** Начало ответа 409 на повтор — по нему окно спрашивает «Занести ещё раз?». */
export const OLD_DEBT_DUPLICATE = 'Такой долг уже занесён'

/** Документ «Исправить» старого долга (ADR-085): клиент, сумма, дата долга, комментарий. */
export interface OldDebtDoc {
  type?: 'old_debt'
  party_id: string
  amount_tyiyn: number
  date: string
  comment: string
}

/** Та же дата `years` лет назад; 29 февраля — 28-е, как на сервере (`checked_sub_months`). */
function yearsBefore(date: string, years: number): string {
  const [y, m, d] = date.split('-').map(Number)
  const year = y - years
  const last = new Date(Date.UTC(year, m, 0)).getUTCDate()
  return `${String(year).padStart(4, '0')}-${String(m).padStart(2, '0')}-${String(Math.min(d, last)).padStart(2, '0')}`
}

/** Самая ранняя допустимая дата долга. */
export function earliestOldDebtDate(today: string): string {
  return yearsBefore(today, OLD_DEBT_YEARS)
}

/** Ошибка даты долга или `null`: не позже сегодня и не раньше чем 10 лет назад. */
export function oldDebtDateError(date: string, today: string): string | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return 'Укажите дату долга'
  if (date > today) return 'Дата долга — не позже сегодняшнего дня'
  if (date < earliestOldDebtDate(today)) return `Дата долга — не раньше чем ${OLD_DEBT_YEARS} лет назад`
  return null
}

/** Долгу больше года — вероятно, ошиблись годом. */
export function olderThanYear(date: string, today: string): boolean {
  return date < yearsBefore(today, 1)
}

/** Дней между датами YYYY-MM-DD. */
function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / 86_400_000)
}

/** Срок оплаты этого долга: последний день — дата долга + срок − 1, как `domain::debt::due_date`. */
export function oldDebtDue(date: string, dueDays: number | null, today: string): { due: string; overdueDays: number } | null {
  if (dueDays === null || !/^\d{4}-\d{2}-\d{2}$/.test(date)) return null
  const due = shiftDate(date, dueDays - 1)
  return { due, overdueDays: Math.max(0, daysBetween(due, today)) }
}

/** Баланс словами для подсказки: долг суммой, аванс — с пометкой. */
const owed = (balance: number) => (balance >= 0 ? formatSom(balance) : `аванс ${formatSom(-balance)}`)

/** Подсказки под полями: «Долг клиента сейчас … → станет …» и срок оплаты этого долга. */
export function oldDebtPreview(balance: number, amount: number | null, date: string, dueDays: number | null, today: string): string[] {
  const lines: string[] = []
  if (amount !== null && amount > 0) lines.push(`Долг клиента сейчас ${owed(balance)} → станет ${owed(balance + amount)}`)
  const due = oldDebtDue(date, dueDays, today)
  if (due) {
    lines.push(
      due.overdueDays > 0
        ? `Срок оплаты по этому долгу — ${formatDate(due.due)}, просрочен на ${due.overdueDays} дн.`
        : `Срок оплаты по этому долгу — ${formatDate(due.due)}`,
    )
  }
  return lines
}
