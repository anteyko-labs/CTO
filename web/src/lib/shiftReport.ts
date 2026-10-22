// Отчёт смены, как в Умаге (SPEC-30 §7, §8): типы ответа `GET /shifts/{id}/report` и правила
// показа. Все суммы считает сервер; здесь — подписи, отбор и высота столбиков.
import { formatPercentBp } from './format'
import type { Shift } from './types'

export interface ReportLine {
  key: string
  label: string
  amount_tyiyn: number
}

export interface CashFlow {
  opening_expected_tyiyn: number
  opening_tyiyn: number
  income: ReportLine[]
  income_tyiyn: number
  outgo: ReportLine[]
  outgo_tyiyn: number
  expected_tyiyn: number
  counted_tyiyn: number | null
  diff_tyiyn: number | null
  handover_tyiyn: number | null
  handover_to: string | null
  left_tyiyn: number | null
}

export interface MethodSum {
  method: string
  amount_tyiyn: number
}

export interface Revenue {
  sales_count: number
  sales_tyiyn: number
  returns_count: number
  returns_tyiyn: number
  net_tyiyn: number
  methods: MethodSum[]
  rest_tyiyn: number
  bank_fee_tyiyn: number
  /** Только владельцу: администратору сервер их не присылает (инвариант 6). */
  cost_sold_tyiyn?: number
  cost_returned_tyiyn?: number
  cost_tyiyn?: number
  gross_tyiyn?: number
  cost_unknown_lines?: number
}

export interface Compare {
  shift_id: string
  number: number
  net_tyiyn: number
  diff_tyiyn: number
  change_bp: number | null
  /** Смена ещё идёт: прошлая взята за то же время от открытия. */
  same_time?: boolean
}

export interface HourRow {
  at: string
  sales_tyiyn: number
  returns_tyiyn: number
  count: number
}

export interface PayRow {
  method: string
  amount_tyiyn: number
  account_name: string | null
}

export interface SaleRow {
  id: string
  number: number
  kind: 'sale' | 'return'
  sale_type: string
  created_at: string
  cashier_name: string
  user_name: string
  party_name: string | null
  total_tyiyn: number
  reversal_of: string | null
  reversal_of_number: number | null
  payments: PayRow[]
}

export interface ReceiptRow {
  id: string
  number: number
  created_at: string
  supplier_name: string | null
  supplier_doc: string
  total_tyiyn: number
  reversal_of: string | null
  user_name: string
  paid_till_tyiyn: number
  paid_other_tyiyn: number
  paid_account_name: string | null
  on_debt_tyiyn: number
}

export interface MoneyRow {
  id: string
  created_at: string
  kind: string
  label: string
  reversal: boolean
  amount_tyiyn: number
  account_name: string
  to_account_name: string | null
  user_name: string
  counterparty: string | null
  comment: string
}

export interface DebtRow {
  id: string
  created_at: string
  kind: 'sale' | 'return' | 'repayment' | 'adjust'
  reversal: boolean
  party_id: string
  party_name: string
  sale_id: string | null
  sale_number: number | null
  amount_tyiyn: number
  account_name: string | null
  user_name: string
  comment: string
}

export interface PayoutRow {
  id: string
  number: number
  created_at: string
  employee_name: string
  amount_tyiyn: number
  comment: string
  account_name: string | null
  reversal: boolean
  user_name: string
}

export interface CancelledRow {
  id: string
  removed_at: string
  reason: 'remove' | 'clear' | 'held'
  kind: string
  name: string
  qty: number
  unit_price_tyiyn: number
  amount_tyiyn: number
  user_name: string
  cashier_name: string | null
  sale_id: string | null
  sale_number: number | null
}

export interface ShiftReport {
  shift: Shift
  owner_view: boolean
  cash: CashFlow
  revenue: Revenue
  compare: Compare | null
  hours: HourRow[]
  sales: SaleRow[]
  sales_totals: { count: number; sales_tyiyn: number; paid_tyiyn: number; bonus_tyiyn: number; debt_tyiyn: number }
  returns_totals: { count: number; total_tyiyn: number; methods: MethodSum[] }
  receipts: ReceiptRow[]
  money: MoneyRow[]
  money_totals: ReportLine[]
  debts: DebtRow[]
  payouts: PayoutRow[]
  payouts_tyiyn: number
  cancelled: CancelledRow[]
  cancelled_tyiyn: number
  /** Оплачено по QR банка за смену, а чека с этой оплатой нет (SPEC-31). */
  qr_unclaimed?: { id: string; amount_tyiyn: number; paid_at: string | null; user_name: string }[]
}

/** Вкладки отчёта в порядке Умага. */
export const TABS = [
  ['overview', 'Обзор'],
  ['sales', 'Продажи'],
  ['returns', 'Возвраты'],
  ['receipts', 'Закупки'],
  ['money', 'Денежные операции'],
  ['debts', 'Долги'],
  ['payouts', 'Выплаты'],
  ['cancelled', 'Отменённые товары'],
] as const

export type TabKey = (typeof TABS)[number][0]

export function tabOf(v: string | null): TabKey {
  return TABS.find((t) => t[0] === v)?.[0] ?? 'overview'
}

const METHODS: Record<string, string> = {
  cash: 'Наличные',
  card: 'Карта / QR банка',
  transfer: 'QR владельцу',
  invoice: 'По счёту',
  debt: 'В долг',
  bonus: 'Баллами',
  battery: 'Аккумулятором',
}

/** Способ оплаты словами; новый способ без подписи — как пришёл. */
export const methodName = (m: string): string => METHODS[m] ?? m

/** Способы оплаты чека одной строкой: «Наличные + Карта / QR банка». */
export function paymentsText(p: PayRow[]): string {
  return [...new Set(p.map((x) => methodName(x.method)))].join(' + ') || '—'
}

/** Счета чека: «Касса», «Счёт», «В долг»; у баллов и аккумулятора — их название. */
export function accountsText(p: PayRow[]): string {
  return [...new Set(p.map((x) => x.account_name ?? methodName(x.method)))].join(', ') || '—'
}

/**
 * Отбор чеков вкладки: по номеру (цифры из поиска — подстрока номера; у возврата — и номер
 * исходного чека) и по способу оплаты.
 */
export function filterSales(rows: SaleRow[], query: string, method: string): SaleRow[] {
  const digits = query.replace(/\D/g, '')
  return rows.filter(
    (r) =>
      (!digits || String(r.number).includes(digits) || (r.reversal_of_number !== null && String(r.reversal_of_number).includes(digits))) &&
      (!method || r.payments.some((p) => p.method === method)),
  )
}

/** Способы оплаты, которые встречаются в чеках, — для отбора. */
export function methodsIn(rows: SaleRow[]): string[] {
  const order = Object.keys(METHODS)
  const all = [...new Set(rows.flatMap((r) => r.payments.map((p) => p.method)))]
  return all.sort((a, b) => (order.indexOf(a) + 1 || 99) - (order.indexOf(b) + 1 || 99) || a.localeCompare(b))
}

/** Изменение к прошлой смене: «+12,34 %» / «−5,00 %»; прошлой выручки не было — пусто. */
export function changeText(bp: number | null): string {
  if (bp === null) return ''
  return `${bp > 0 ? '+' : ''}${formatPercentBp(bp)}`
}

/** Высота столбика в процентах от самого большого часа; ненулевой — не ниже 2 %, чтобы был виден. */
export function barHeights(values: number[]): number[] {
  const max = Math.max(0, ...values)
  if (max <= 0) return values.map(() => 0)
  return values.map((v) => (v <= 0 ? 0 : Math.max(2, Math.round((v / max) * 100))))
}

/** Почему строку отменили — словами. */
export const CANCEL_REASONS: Record<CancelledRow['reason'], string> = {
  remove: 'Удалена из чека',
  clear: '«Очистить»',
  held: 'Удалён отложенный чек',
}

/** Вид записи долга словами. */
export function debtKindText(d: DebtRow): string {
  const base = d.kind === 'sale' ? 'Продажа в долг' : d.kind === 'return' ? 'Возврат чека в долг' : d.kind === 'repayment' ? 'Оплата долга' : 'Правка долга'
  return d.reversal ? `Сторно: ${base.toLowerCase()}` : base
}

/** Итог вкладки «Долги»: в долг продано (за вычетом возвратов), оплачено, правки. */
export function debtTotals(rows: DebtRow[]): { sold: number; repaid: number; adjusted: number } {
  let sold = 0
  let repaid = 0
  let adjusted = 0
  for (const d of rows) {
    if (d.kind === 'sale' || d.kind === 'return') sold += d.amount_tyiyn
    else if (d.kind === 'repayment') repaid -= d.amount_tyiyn
    else adjusted += d.amount_tyiyn
  }
  return { sold, repaid, adjusted }
}
