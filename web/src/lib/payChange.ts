/**
 * «Изменить оплату» чека и «Наличные ↔ QR» за смену (SPEC-37, ADR-132, задача 003 Н1): типы ответа
 * сервера и раскладка полей окна в оплаты запроса. Деньги — целые тыйыны (инвариант 1).
 */
import { parseSom } from './format'
import type { Payment, PaymentMethod, User } from './types'

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
  user_id: string
  user_name: string
  /** Со знаком: + из наличных в QR, − из QR в наличные. */
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
  /** «Из QR в наличные»: предел по каждому счёту. */
  bank_limits: BankLimit[]
  /** Чеки смены для вкладки «По чекам». */
  sales: ShiftSaleFix[]
  /** Администратору: предел владельца за смену (нет — без предела) и сколько уже исправлено. */
  admin_max_tyiyn: number | null
  admin_used_tyiyn: number
}

/** Направление переноса за смену. */
export type Direction = 'to_bank' | 'to_cash'

export interface BankLimit {
  account_id: string
  name: string
  card_sales_tyiyn: number
  card_returns_tyiyn: number
  /** Перенесено на счёт: + из наличных, − обратно. */
  moved_tyiyn: number
  limit_tyiyn: number
}

export interface SalePayBrief {
  method: PaymentMethod
  amount_tyiyn: number
  account_id: string | null
  account_name: string | null
  bank_qr: boolean
}

export interface ShiftSaleFix {
  id: string
  number: number
  created_at: string
  total_tyiyn: number
  payments: SalePayBrief[]
  fixable: boolean
  reason: string | null
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
  /** Перенос: счёт банка и сумма со знаком (+ в QR, − в наличные). */
  account_name?: string | null
  amount_tyiyn?: number
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

/** Предел переноса за смену по направлению: «в QR» — наличные по чекам; «в наличные» — по счёту. */
export function transferLimit(info: Pick<CashToBankInfo, 'limit_tyiyn' | 'bank_limits'>, direction: Direction, account: string): number {
  if (direction === 'to_bank') return info.limit_tyiyn
  return info.bank_limits.find((b) => b.account_id === account)?.limit_tyiyn ?? 0
}

/** Направление переноса словами по знаку суммы: «Касса → Optima» или «Optima → Касса». */
export function transferWords(amount: number, account: string): string {
  return amount < 0 ? `из QR в наличные: «${account}» → Касса` : `из наличных в QR: Касса → «${account}»`
}

/**
 * Кнопка «⇄» строки «По чекам»: вся оплата чека одним способом — наличными или «Картой / QR банка»
 * без привязанного QR (баллы и аккумулятор не меняются). Смешанная, долг, перевод, «По счёту» — `null`:
 * только «Подробно».
 */
export function swapTarget(payments: SalePayBrief[]): 'card' | 'cash' | null {
  const changeable = payments.filter((p) => p.method !== 'bonus' && p.method !== 'battery')
  if (changeable.length === 0) return null
  if (changeable.every((p) => p.method === 'cash')) return 'card'
  if (changeable.every((p) => p.method === 'card' && !p.bank_qr)) return 'cash'
  return null
}

/** Оплаты запроса «Изменить оплату» для кнопки «⇄»: вся сумма способа — на другой способ. */
export function swapPayments(payments: SalePayBrief[], to: 'card' | 'cash', account: string): PayLine[] {
  const sum = payments.filter((p) => p.method !== 'bonus' && p.method !== 'battery').reduce((a, p) => a + p.amount_tyiyn, 0)
  return to === 'card' ? [{ method: 'card', amount_tyiyn: sum, account_id: account }] : [{ method: 'cash', amount_tyiyn: sum }]
}

/**
 * Расшифровка строки движения денег отчёта смены (задача 003 Н1): какие изменения оплаты чеков и
 * переносы в неё вошли. `pay_fix_in` / `pay_fix_out` — чеки, наличных стало больше / меньше;
 * `cash_to_bank` / `bank_to_cash` — переносы этого направления и отмены переносов этого направления.
 */
export function fixItemsFor(key: string, items: AfterItem[]): AfterItem[] {
  const toCash = (i: AfterItem) => (i.amount_tyiyn ?? 0) < 0 !== i.reversal
  switch (key) {
    case 'pay_fix_in':
      return items.filter((i) => i.kind === 'sale' && i.cash_delta_tyiyn > 0)
    case 'pay_fix_out':
      return items.filter((i) => i.kind === 'sale' && i.cash_delta_tyiyn < 0)
    case 'cash_to_bank':
      return items.filter((i) => i.kind === 'shift' && !toCash(i))
    case 'bank_to_cash':
      return items.filter((i) => i.kind === 'shift' && toCash(i))
    default:
      return []
  }
}

/**
 * Можно ли исправлять оплату смены (задача 003 Н1, ADR-149): владелец — при связи с сервером;
 * администратор — ещё и с правом «Наличные ↔ QR в открытой смене» и только в открытой смене.
 * Проверяет сервер; здесь — показывать ли кнопку.
 */
export function canFixPay(user: Pick<User, 'role' | 'permissions'> | null | undefined, online: boolean, shiftOpen: boolean): boolean {
  if (!user || !online) return false
  if (user.role === 'owner') return true
  return shiftOpen && (user.permissions ?? []).includes('till_pay_fix')
}
