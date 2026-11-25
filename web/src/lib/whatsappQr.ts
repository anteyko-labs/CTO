// WhatsApp по QR-коду (задача 052, ADR-217): состояние номера на экране «Сообщения клиентам».
// Шлюз на сервере отдаёт state: off | qr | code | connecting | connected | logged_out | unreachable.

export type WaState = 'off' | 'qr' | 'code' | 'connecting' | 'connected' | 'logged_out' | 'unreachable'

export interface WaGateStatus {
  state: WaState
  phone?: string
  since?: string | null
  qr?: string
  code?: string
  error?: string
}

export interface WaStatus {
  configured: boolean
  gate: WaGateStatus
  sent_today: number
  daily_limit: number
  pending: number
}

export interface WaInboxRow {
  created_at: string
  phone: string
  party_id: string | null
  party_name: string | null
  direction: 'in' | 'out' | 'human'
  text: string
}

export const STATE_LABELS: Record<WaState, string> = {
  off: 'не подключён',
  qr: 'ждёт QR-код',
  code: 'ждёт код',
  connecting: 'подключается…',
  connected: 'подключён',
  logged_out: 'отключён с телефона — подключите заново',
  unreachable: 'шлюз не отвечает',
}

export const DIRECTION_LABELS: Record<WaInboxRow['direction'], string> = {
  in: 'Клиент',
  out: 'Бот',
  human: 'С телефона',
}

/** Опрашивать шлюз чаще, пока идёт подключение. */
export function polling(state: WaState): boolean {
  return state === 'qr' || state === 'code' || state === 'connecting'
}

/** «ABCD1234» → «ABCD-1234», как показывает WhatsApp. */
export function codeText(code: string): string {
  const c = code.replace(/[^0-9A-Za-z]/g, '').toUpperCase()
  return c.length === 8 ? `${c.slice(0, 4)}-${c.slice(4)}` : c
}

/** Номер для кода: 9–15 цифр после очистки, иначе подсказка. */
export function pairPhoneProblem(phone: string): string | null {
  const d = phone.replace(/\D/g, '')
  if (d === '') return 'введите номер телефона магазина'
  if (d.length < 9 || d.length > 15) return 'номер — например 0555 12 34 56'
  return null
}
