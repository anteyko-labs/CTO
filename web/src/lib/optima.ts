// Optima Business (SPEC-31, ADR-106): QR на сумму чека на кассе, настройка и сверка с банком.
// Банк опрашивает сервер; касса спрашивает у сервера состояние QR каждые 2 с.

/** Есть ли на сервере ключ банка и выбраны ли точка и касса — тогда на кассе кнопка «QR на сумму». */
export interface OptimaStatus {
  enabled: boolean
  qr: boolean
}

export interface OptimaSettings {
  /** Ключ OPTIMA_API_KEY задан на сервере. */
  enabled: boolean
  legal_party_id: number | null
  account_number: string
  sale_point_code: number | null
  sale_point_name: string
  cash_code: number | null
  cash_name: string
  /** Безналичный счёт учёта; пусто — счёт для карты по умолчанию. */
  cash_account_id: string | null
  /** Куда пойдут деньги сейчас. */
  account_id: string | null
}

export interface SalePoint {
  account: string
  name: string
  address: string
  code: number
  cashes: { name: string; code: number }[]
}

export type BankQrStatus = 'waiting' | 'paid' | 'unpaid' | 'error' | 'cancelled'

export interface BankQr {
  id: string
  amount_tyiyn: number
  status: BankQrStatus
  expires_at: string
  /** Сколько секунд QR ещё действует — по часам сервера. */
  seconds_left: number
  paid_at: string | null
  sale_id: string | null
  /** PNG в base64 — пока QR ждёт оплаты. */
  qr_base64?: string
}

export interface ReconDay {
  date: string
  loaded: boolean
  ours_count: number
  ours_tyiyn: number
  bank_count: number
  bank_tyiyn: number
  diff_tyiyn: number
  unmatched: number
  bank_fee_tyiyn: number
  our_fee_tyiyn: number
  adjusted_tyiyn: number
}

export interface ReconOurItem {
  kind: 'card' | 'qr' | 'repayment'
  at: string
  amount_tyiyn: number
  fee_tyiyn: number
  sale_id: string | null
  sale_number: number | null
}

export interface ReconBankItem {
  id: string
  kind: 'card' | 'qr'
  at: string
  amount_tyiyn: number
  fee_tyiyn: number
  auth_code: string
  rrn: string
  requisite: string
}

export interface ReconOut {
  enabled: boolean
  configured: boolean
  account_id: string | null
  account_name: string | null
  from: string
  to: string
  days: ReconDay[]
  ours_unmatched: ReconOurItem[]
  bank_unmatched: ReconBankItem[]
  run?: { operations: number; new_operations: number; adjustments: number; adjusted_tyiyn: number }
}

/** QR на кассе: что прислал сервер и когда — по этому идёт обратный отсчёт. */
export interface QrView extends BankQr {
  /** Время устройства, когда пришёл ответ (мс). */
  received: number
}

/** Секунд до конца срока QR по часам устройства от ответа сервера. */
export function secondsLeft(qr: Pick<QrView, 'seconds_left' | 'received'>, now: number): number {
  return Math.max(0, qr.seconds_left - Math.floor((now - qr.received) / 1000))
}

/** «9:05» — обратный отсчёт. */
export function countdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/** Состояние QR словами для кассы. */
export const QR_STATUS_TEXT: Record<BankQrStatus, string> = {
  waiting: 'Ждём оплату',
  paid: 'Оплачено',
  unpaid: 'QR не оплачен',
  error: 'Банк отклонил, повторите',
  cancelled: 'QR отменён',
}

/** Ссылку на QR получает оплата картой, только если QR оплачен ровно на эту сумму. */
export function qrForCard(qr: Pick<BankQr, 'id' | 'status' | 'amount_tyiyn'> | null, cardAmount: number): string | undefined {
  return qr && qr.status === 'paid' && qr.amount_tyiyn === cardAmount && cardAmount > 0 ? qr.id : undefined
}

/** Расчётный счёт банка — 16 цифр (пробелы не считаются). */
export function accountNumberProblem(text: string): string | null {
  const v = text.replace(/\s/g, '')
  if (!v) return null
  return /^\d{16}$/.test(v) ? null : 'расчётный счёт — 16 цифр'
}

/** Сколько дней в периоде сверки (включительно); выписка банка — не больше 14. */
export function periodDays(from: string, to: string): number {
  const ms = Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)
  return Number.isFinite(ms) ? Math.round(ms / 86_400_000) + 1 : 0
}

export const RECON_KIND: Record<ReconOurItem['kind'] | ReconBankItem['kind'], string> = {
  card: 'карта',
  qr: 'QR',
  repayment: 'оплата долга картой',
}
