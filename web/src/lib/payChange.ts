/**
 * «Изменить оплату» чека и «из наличных в QR» за смену (SPEC-37, ADR-132): типы ответа сервера и
 * раскладка полей окна в оплаты запроса. Деньги — целые тыйыны (инвариант 1).
 */
import { parseSom } from './format'
import type { Payment, PaymentMethod } from './types'

/** Пересчёт «Кассы», в который вошли наличные чека или смены. */
export interface Counted {
  shift_id: string
  number: number
  /** `close` — при закрытии смены, `open` — при открытии с «Пересчитал». */
  at: 'open' | 'close'
  business_date: string
  cashier_id: string
  cashier_name: string
  open_now: boolean
}

export interface BankAccountBrief {
  id: string
  name: string
  default_card: boolean
  default_transfer: boolean
  default_invoice: boolean
  fee_rate_bp: number
}

export interface PartyBrief {
  id: string
  name: string
  kind: 'person' | 'company'
  inn: string
  balance_tyiyn: number
}

export interface QrBrief {
  id: string
  amount_tyiyn: number
  paid_at: string | null
}

export interface PayChangeInfo {
  sale_id: string
  number: number
  business_date: string
  created_at: string
  cashier_name: string
  total_tyiyn: number
  payments: Payment[]
  /** Баллы и аккумулятор — не меняются. */
  fixed: Payment[]
  to_split_tyiyn: number
  party: PartyBrief | null
  accounts: BankAccountBrief[]
  qr: QrBrief[]
  counted: Counted | null
  diff_tyiyn: number | null
  withheld_tyiyn: number | null
  open_shift_number: number | null
  repaid_tyiyn: number
}

/** «Что изменится»: сервер считает пробным проведением. */
export interface Effect {
  cash_delta_tyiyn: number
  counted: Counted | null
  diff_before_tyiyn: number | null
  diff_after_tyiyn: number | null
  withheld_tyiyn: number | null
  withhold_tyiyn: number
  /** Поправка удержания, если галочка стоит: не ноль — галочку показываем. */
  withhold_possible_tyiyn: number
  open_shift_number: number | null
  fee_tyiyn: number
  profit_tyiyn: number
  month_note: string | null
  lines: string[]
}

export interface PayChangeOut {
  new_sale_id: string | null
  new_number: number | null
  effect: Effect
}

export interface TransferRow {
  id: string
  created_at: string
  user_name: string
  amount_tyiyn: number
  fee_tyiyn: number
  account_id: string
  account_name: string
  comment: string
  reversed_at: string | null
  reverse_comment: string | null
  counted: boolean
}

export interface CashToBankInfo {
  shift_id: string
  number: number
  business_date: string
  open: boolean
  cashier_name: string
  cash_sales_tyiyn: number
  cash_returns_tyiyn: number
  moved_tyiyn: number
  limit_tyiyn: number
  diff_tyiyn: number | null
  withheld_tyiyn: number
  accounts: BankAccountBrief[]
  items: TransferRow[]
}

export interface CashToBankOut {
  id: string | null
  effect: Effect
}

/** Изменение после пересчёта в отчёте смены (блок «После закрытия»). */
export interface AfterItem {
  id: string
  kind: 'sale' | 'shift'
  reversal: boolean
  created_at: string
  user_name: string
  original_id: string | null
  original_number: number | null
  new_id: string | null
  new_number: number | null
  old_text: string
  new_text: string
  cash_delta_tyiyn: number
  diff_before_tyiyn: number | null
  diff_after_tyiyn: number | null
  withhold_tyiyn: number
  comment: string
}

export interface AfterBank {
  account_id: string
  name: string
  amount_tyiyn: number
  fee_tyiyn: number
}

export interface AfterTotals {
  cash_sales_tyiyn: number
  cash_to_bank_tyiyn: number
  bank: AfterBank[]
  diff_tyiyn: number
  withheld_tyiyn: number
  net_profit_tyiyn?: number
}

export interface AfterClose {
  at: 'open' | 'close'
  items: AfterItem[]
  before: AfterTotals
  after: AfterTotals
}

/** Способы, которые меняет окно; баллы и аккумулятор не меняются. */
export const CHANGEABLE: PaymentMethod[] = ['cash', 'card', 'transfer', 'invoice', 'debt']

export type PayForm = Record<'cash' | 'card' | 'transfer' | 'invoice' | 'debt', string> & {
  cardAccount: string
  transferAccount: string
  invoiceAccount: string
  /** Оплаченный QR без чека, привязанный к части «Карта / QR». */
  qr: string
}

const defaultAccount = (accounts: BankAccountBrief[], key: 'default_card' | 'default_transfer' | 'default_invoice'): string =>
  (accounts.find((a) => a[key]) ?? accounts[0])?.id ?? ''

/** Поля окна по оплате сейчас: суммы способов, счета — как в чеке, иначе по умолчанию. */
export function initialForm(info: PayChangeInfo): PayForm {
  const sum = (m: PaymentMethod) => info.payments.filter((p) => p.method === m).reduce((a, p) => a + p.amount_tyiyn, 0)
  const text = (m: PaymentMethod) => {
    const v = sum(m)
    return v === 0 ? '' : String(v / 100).replace('.', ',')
  }
  const acc = (m: PaymentMethod, key: 'default_card' | 'default_transfer' | 'default_invoice') =>
    info.payments.find((p) => p.method === m && p.account_id)?.account_id ?? defaultAccount(info.accounts, key)
  return {
    cash: text('cash'),
    card: text('card'),
    transfer: text('transfer'),
    invoice: text('invoice'),
    debt: text('debt'),
    cardAccount: acc('card', 'default_card'),
    transferAccount: acc('transfer', 'default_transfer'),
    invoiceAccount: acc('invoice', 'default_invoice'),
    qr: '',
  }
}

export interface PayLine {
  method: PaymentMethod
  amount_tyiyn: number
  account_id?: string
  bank_qr_id?: string
}

/** Оплаты запроса из полей окна; неверная сумма — `null`. Пустые и нулевые поля не входят. */
export function formPayments(f: PayForm): PayLine[] | null {
  const out: PayLine[] = []
  for (const m of ['cash', 'card', 'transfer', 'invoice', 'debt'] as const) {
    const raw = f[m].trim()
    if (raw === '') continue
    const v = parseSom(raw)
    if (v == null || v < 0) return null
    if (v === 0) continue
    const line: PayLine = { method: m, amount_tyiyn: v }
    if (m === 'card') {
      if (f.cardAccount) line.account_id = f.cardAccount
      if (f.qr) line.bank_qr_id = f.qr
    }
    if (m === 'transfer' && f.transferAccount) line.account_id = f.transferAccount
    if (m === 'invoice' && f.invoiceAccount) line.account_id = f.invoiceAccount
    out.push(line)
  }
  return out
}

/** Сколько ещё распределить: итог − баллы − аккумулятор − поля. */
export function remaining(toSplit: number, lines: PayLine[] | null): number | null {
  if (lines == null) return null
  return toSplit - lines.reduce((a, l) => a + l.amount_tyiyn, 0)
}

/** Оплаченные QR без чека, подходящие к части «Карта / QR»: та же сумма. */
export function matchingQr(qr: QrBrief[], card: number): QrBrief[] {
  return card > 0 ? qr.filter((q) => q.amount_tyiyn === card) : []
}

/** Расхождение словами: «недостача 1 000 с», «излишек 700 с», «без расхождения». */
export function diffWords(d: number, som: (v: number) => string): string {
  if (d === 0) return 'без расхождения'
  return d < 0 ? `недостача ${som(-d)}` : `излишек ${som(d)}`
}
