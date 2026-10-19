// Деньги и отчёты (SPEC-27 §6): подписи движений и способов оплаты, выбор периода сводки,
// сортировка экрана «Долги». Только показ — все суммы считает сервер.
import { formatDate, shiftDate } from './format'
import type { DebtRow } from './types'

/** Виды движений кассы словами — экраны «Смена», «Счёт», «Куда сколько поступило». */
export const MOVEMENT_LABELS: Record<string, string> = {
  sale: 'Продажи',
  sale_return: 'Возвраты',
  cash_in: 'Внесения',
  cash_out: 'Изъятия',
  transfer_in: 'Переводы сюда',
  transfer_out: 'Переводы отсюда',
  bank_fee: 'Комиссия банка',
  expense: 'Расходы',
  payout: 'Выплаты',
  supplier_payment: 'Оплата поставщикам',
  supplier_refund: 'Возмещения за брак',
  debt_repayment: 'Погашения долгов',
  battery_intake: 'Приём аккумуляторов',
  count_diff: 'Пересчёт',
  reversal: 'Сторно',
}

/** Способы оплаты чека словами. */
export const METHOD_LABELS: Record<string, string> = {
  cash: 'Наличные',
  card: 'Карта / QR банка',
  transfer: 'QR владельцу',
  debt: 'В долг',
  bonus: 'Баллы',
  battery: 'Аккумулятор',
}

/** Самый длинный период сводки — как у отчёта о прибыли. */
export const MAX_PERIOD_DAYS = 367

/** Дней в периоде, оба конца включительно. */
export function periodDays(from: string, to: string): number {
  const a = Date.parse(`${from}T00:00:00Z`)
  const b = Date.parse(`${to}T00:00:00Z`)
  return Math.round((b - a) / 86_400_000) + 1
}

export interface Range {
  from: string
  to: string
}

/** Быстрые кнопки сводки: сегодня, 7 и 30 дней по сегодня. */
export const QUICK = [
  ['today', 'Сегодня', 0],
  ['week', '7 дней', 6],
  ['month', '30 дней', 29],
] as const

export type QuickKey = (typeof QUICK)[number][0]

export function quickRange(key: QuickKey, today: string): Range {
  const back = QUICK.find((q) => q[0] === key)?.[2] ?? 0
  return { from: shiftDate(today, -back), to: today }
}

/** Какой быстрой кнопке соответствует период; свой выбор — null. */
export function quickOf(r: Range, today: string): QuickKey | null {
  return QUICK.find((q) => r.to === today && r.from === shiftDate(today, -q[2]))?.[0] ?? null
}

/**
 * Период из полей «с» и «по»: начало позже конца — меняем местами, длиннее года — обрезаем
 * начало (сервер больше не примет).
 */
export function normalizeRange(from: string, to: string): Range {
  let [a, b] = from <= to ? [from, to] : [to, from]
  if (periodDays(a, b) > MAX_PERIOD_DAYS) a = shiftDate(b, -(MAX_PERIOD_DAYS - 1))
  return { from: a, to: b }
}

/** Заголовок сводки: «сегодня», «за 7 дней», «за 12.10.2026», «с 01.10.2026 по 12.10.2026». */
export function rangeTitle(r: Range, today: string): string {
  const q = quickOf(r, today)
  if (q === 'today') return 'сегодня'
  if (q === 'week') return 'за 7 дней'
  if (q === 'month') return 'за 30 дней'
  if (r.from === r.to) return `за ${formatDate(r.from)}`
  return `с ${formatDate(r.from)} по ${formatDate(r.to)}`
}

export type DebtSortKey = 'name' | 'phone' | 'debt' | 'overdue' | 'last_payment' | 'limit'

/** Сортировка экрана «Долги»; пустые значения (нет телефона, лимита, оплат) — всегда внизу. */
export function sortDebts(rows: DebtRow[], key: DebtSortKey, desc: boolean): DebtRow[] {
  const val = (r: DebtRow): string | number | null => {
    switch (key) {
      case 'name':
        return r.name.toLocaleLowerCase('ru')
      case 'phone':
        return r.phone || null
      case 'debt':
        return Math.abs(r.balance_tyiyn)
      case 'overdue':
        return r.overdue_days
      case 'last_payment':
        return r.last_payment_at
      case 'limit':
        return r.credit_limit_tyiyn
    }
  }
  return [...rows].sort((a, b) => {
    const x = val(a)
    const y = val(b)
    if (x === null && y === null) return a.name.localeCompare(b.name, 'ru')
    if (x === null) return 1
    if (y === null) return -1
    const c = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y), 'ru')
    return (desc ? -c : c) || a.name.localeCompare(b.name, 'ru')
  })
}
