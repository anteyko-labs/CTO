// Отчёт смены в Excel (задача 003 Н13): одна смена — один файл, листы как вкладки отчёта. Столбцы —
// как в таблицах экрана без кнопок; сторно — отдельным столбцом. Себестоимость и прибыль — только
// если их отдал сервер (владельцу, I13): у администратора этих строк в ответе нет.
import { exportTitle, tableSheet, xl, type ExportCol } from './exportTable'
import { formatDateTime, formatTime } from './format'
import { PURPOSE_LABELS } from './payouts'
import {
  CANCEL_REASONS,
  accountsText,
  debtKindText,
  methodName,
  paymentsText,
  type CancelledRow,
  type DebtRow,
  type MoneyRow,
  type PayoutRow,
  type ReceiptRow,
  type SaleRow,
  type ShiftReport,
} from './shiftReport'
import type { Cell, Sheet, SheetRow } from './xlsxWrite'

/** Количество строки чека по её виду числом и единица: розлив — л, вес — кг, длина — м, канистры, штуки. */
export function kindQty(kind: string, qty: number): { qty: Cell; unit: string } {
  if (kind === 'pour') return { qty: xl.liters(qty), unit: 'л' }
  if (kind === 'weight') return { qty: { qty, scale: 3 }, unit: 'кг' }
  if (kind === 'length') return { qty: { qty, scale: 3 }, unit: 'м' }
  if (kind === 'container') return { qty: xl.int(qty), unit: 'кан.' }
  return { qty: xl.int(qty), unit: 'шт' }
}

const time = (iso: string) => formatTime(iso)

export const SALE_COLS: ExportCol<SaleRow>[] = [
  { label: '№', value: (s) => xl.int(s.number) },
  { label: 'Время', value: (s) => time(s.created_at) },
  { label: 'Способ', value: (s) => paymentsText(s.payments) },
  { label: 'Счёт', value: (s) => accountsText(s.payments) },
  { label: 'Кассир', value: (s) => s.cashier_name },
  { label: 'Покупатель', value: (s) => s.party_name ?? 'Розничный' },
  { label: 'Сумма', value: (s) => xl.money(s.total_tyiyn) },
]

export const RETURN_COLS: ExportCol<SaleRow>[] = [
  { label: '№', value: (s) => xl.int(s.number) },
  { label: 'Время', value: (s) => time(s.created_at) },
  { label: 'По чеку', value: (s) => (s.reversal_of_number !== null ? `№ ${s.reversal_of_number}` : 'Без чека') },
  { label: 'Способ', value: (s) => paymentsText(s.payments) },
  { label: 'Счёт', value: (s) => accountsText(s.payments) },
  { label: 'Кассир', value: (s) => s.cashier_name },
  { label: 'Покупатель', value: (s) => s.party_name ?? 'Розничный' },
  { label: 'Сумма', value: (s) => xl.money(-s.total_tyiyn) },
]

export const RECEIPT_COLS: ExportCol<ReceiptRow>[] = [
  { label: '№', value: (x) => xl.int(x.number) },
  { label: 'Время', value: (x) => time(x.created_at) },
  { label: 'Поставщик', value: (x) => x.supplier_name ?? '' },
  { label: 'Накладная поставщика', value: (x) => x.supplier_doc },
  { label: 'Сумма', value: (x) => xl.money(x.total_tyiyn) },
  { label: 'Из кассы смены', value: (x) => xl.money(x.paid_till_tyiyn) },
  { label: 'С других счетов', value: (x) => xl.money(x.paid_other_tyiyn) },
  { label: 'Счёт', value: (x) => x.paid_account_name ?? '' },
  { label: 'В долг', value: (x) => xl.money(x.on_debt_tyiyn) },
  { label: 'Кто', value: (x) => x.user_name },
  { label: 'Отметка', value: (x) => (x.reversal_of ? 'сторно' : '') },
]

export const MONEY_COLS: ExportCol<MoneyRow>[] = [
  { label: 'Время', value: (m) => time(m.created_at) },
  { label: 'Вид', value: (m) => (m.kind === 'expenses' && m.counterparty ? `${m.label} (${m.counterparty})` : m.label) },
  { label: 'Сумма', value: (m) => xl.money(m.amount_tyiyn) },
  { label: 'Счёт', value: (m) => m.account_name },
  { label: 'Другой счёт', value: (m) => m.to_account_name ?? '' },
  { label: 'Пользователь', value: (m) => m.user_name },
  { label: 'Кому / от кого', value: (m) => (m.kind === 'expenses' ? '' : (m.counterparty ?? '')) },
  { label: 'Комментарий', value: (m) => m.comment },
  { label: 'Отметка', value: (m) => (m.reversal ? 'сторно' : '') },
]

export const DEBT_COLS: ExportCol<DebtRow>[] = [
  { label: 'Время', value: (d) => time(d.created_at) },
  { label: 'Что', value: (d) => debtKindText(d) },
  { label: 'Клиент', value: (d) => d.party_name },
  { label: 'Чек', value: (d) => (d.sale_number !== null ? xl.int(d.sale_number) : '') },
  { label: 'Сумма', value: (d) => xl.money(d.amount_tyiyn) },
  { label: 'Счёт', value: (d) => d.account_name ?? '' },
  { label: 'Кто', value: (d) => d.user_name },
  { label: 'Комментарий', value: (d) => d.comment },
  { label: 'Отметка', value: (d) => (d.reversal ? 'сторно' : '') },
]

export const PAYOUT_COLS: ExportCol<PayoutRow>[] = [
  { label: 'Время', value: (p) => time(p.created_at) },
  { label: 'Сотрудник', value: (p) => p.employee_name },
  { label: 'Сумма', value: (p) => xl.money(p.amount_tyiyn) },
  { label: 'За что', value: (p) => PURPOSE_LABELS[p.purpose ?? 'general'] ?? p.purpose ?? '' },
  { label: 'Комментарий', value: (p) => p.comment },
  { label: 'Откуда', value: (p) => p.account_name ?? 'не из кассы' },
  { label: 'Кто выдал', value: (p) => p.user_name },
  { label: 'Отметка', value: (p) => (p.reversal ? 'сторно' : '') },
]

export const CANCELLED_COLS: ExportCol<CancelledRow>[] = [
  { label: 'Время', value: (c) => time(c.removed_at) },
  { label: 'Товар или услуга', value: (c) => c.name },
  { label: 'Количество', value: (c) => kindQty(c.kind, c.qty).qty },
  { label: 'Ед.', value: (c) => kindQty(c.kind, c.qty).unit },
  { label: 'Цена', value: (c) => xl.money(c.unit_price_tyiyn) },
  { label: 'Сумма', value: (c) => xl.money(c.amount_tyiyn) },
  { label: 'Как', value: (c) => CANCEL_REASONS[c.reason] },
  { label: 'Чек', value: (c) => (c.sale_number !== null ? `проведён № ${c.sale_number}` : 'не проведён') },
  { label: 'Кассир', value: (c) => c.cashier_name ?? '' },
  { label: 'Пользователь', value: (c) => c.user_name },
]

/** Лист «Обзор»: движение денег кассы и выручка; прибыль — только если её прислал сервер. */
function overviewRows(d: ShiftReport): SheetRow[] {
  const c = d.cash
  const v = d.revenue
  const rows: SheetRow[] = []
  const line = (label: string, value: number | null, level = 0, bold = false) => rows.push({ cells: [label, value === null ? null : xl.money(value)], level, bold })
  line('Движение денег (наличные кассы смены)', null, 0, true)
  line('Остаток на начало смены', c.opening_tyiyn, 1)
  line('Приход', c.income_tyiyn, 1, true)
  for (const l of c.income) line(l.label, l.amount_tyiyn, 2)
  line('Расход', c.outgo_tyiyn, 1, true)
  for (const l of c.outgo) line(l.label, l.amount_tyiyn, 2)
  line('Должно быть в кассе', c.expected_tyiyn, 1, true)
  if (c.counted_tyiyn !== null) line('Остаток на конец смены (пересчитали)', c.counted_tyiyn, 1)
  if (c.diff_tyiyn !== null) line('Разница (излишек +, недостача −)', c.diff_tyiyn, 1)
  if (c.handover_tyiyn !== null) {
    line(`Сдали после закрытия${c.handover_to ? ` в «${c.handover_to}»` : ''}`, c.handover_tyiyn, 1)
    line('Осталось в кассе', c.left_tyiyn ?? 0, 1)
  }
  line('Выручка за смену', null, 0, true)
  line(`Продажи (${v.sales_count})`, v.sales_tyiyn, 1)
  line(`Возвраты (${v.returns_count})`, -v.returns_tyiyn, 1)
  line('Выручка', v.net_tyiyn, 1, true)
  for (const m of v.methods) line(methodName(m.method), m.amount_tyiyn, 2)
  if (v.rest_tyiyn !== 0) line('Исправления и прочее', v.rest_tyiyn, 2)
  // Себестоимость и прибыль — владельцу (I13): у администратора сервер этих полей не присылает.
  if (d.owner_view && v.gross_tyiyn !== undefined) {
    line('Себестоимость', v.cost_tyiyn ?? 0, 1)
    line('Валовая прибыль', v.gross_tyiyn, 1, true)
    if (v.net_profit_tyiyn !== undefined) {
      for (const l of (v.profit ?? []).filter((x) => x.key !== 'gross')) line(l.label, l.amount_tyiyn, 2)
      line('Чистая прибыль (без оклада)', v.net_profit_tyiyn, 1, true)
    }
  }
  return rows
}

/** Все листы отчёта смены одним файлом. */
export function shiftReportSheets(d: ShiftReport, now = new Date()): Sheet[] {
  const s = d.shift
  const name = `Смена № ${s.number}`
  const title = (part: string) =>
    exportTitle(
      `${name} — ${part}`,
      [`кассир ${s.cashier_name}`, `касса «${s.account_name}»`, `с ${formatDateTime(s.opened_at)}${s.closed_at ? ` по ${formatDateTime(s.closed_at)}` : ' (открыта)'}`],
      now,
    )
  const sales = d.sales.filter((x) => x.kind === 'sale')
  const returns = d.sales.filter((x) => x.kind === 'return')
  const sum = <T>(rows: T[], f: (r: T) => number) => rows.reduce((a, r) => a + f(r), 0)
  return [
    { name: 'Обзор', columns: [{ label: 'Показатель' }, { label: 'Сумма' }], rows: overviewRows(d), title: title('обзор'), filter: false },
    tableSheet('Продажи', SALE_COLS, sales, { title: title('продажи'), total: ['Итого', null, null, null, null, null, xl.money(sum(sales, (x) => x.total_tyiyn))] }),
    tableSheet('Возвраты', RETURN_COLS, returns, { title: title('возвраты'), total: ['Итого', null, null, null, null, null, null, xl.money(-sum(returns, (x) => x.total_tyiyn))] }),
    tableSheet('Приходы', RECEIPT_COLS, d.receipts, {
      title: title('приходы'),
      total: ['Итого', null, null, null, xl.money(sum(d.receipts, (x) => x.total_tyiyn)), xl.money(sum(d.receipts, (x) => x.paid_till_tyiyn))],
    }),
    tableSheet('Деньги', MONEY_COLS, d.money, { title: title('денежные операции') }),
    tableSheet('Долги', DEBT_COLS, d.debts, { title: title('долги') }),
    tableSheet('Выплаты', PAYOUT_COLS, d.payouts, { title: title('выплаты'), total: ['Итого', null, xl.money(d.payouts_tyiyn)] }),
    tableSheet('Отменённое', CANCELLED_COLS, d.cancelled, { title: title('отменённые товары'), total: ['Итого', null, null, null, null, xl.money(d.cancelled_tyiyn)] }),
  ]
}
