// Экран покупателя на втором мониторе кассы (задача 008, ADR-179): QR банка на сумму, «Оплачено»,
// подпись клиента за долг. Касса и экран — два окна одного браузера: связь через BroadcastChannel,
// без сервера и без сети.

/** Имя канала: одно на браузер кассы. */
export const SCREEN_CHANNEL = 'avtodom-customer-screen'
/** Экран отзывается раз в столько мс; молчит дольше `SCREEN_GONE_MS` — его нет. */
export const SCREEN_PING_MS = 1500
export const SCREEN_GONE_MS = 5000
/** «Оплачено ✓» держится столько мс, потом — заставка. */
export const PAID_SHOW_MS = 5000

/** Строка чека на экране покупателя. */
export interface ScreenLine {
  name: string
  qty: string
  amount_tyiyn: number
}

/** Что касса велит показать. */
export type ToScreen =
  | { type: 'idle' }
  | { type: 'cart'; lines: ScreenLine[]; total_tyiyn: number }
  | { type: 'qr'; amount_tyiyn: number; qr_base64: string }
  | { type: 'paid'; amount_tyiyn: number }
  | { type: 'sign'; id: string; title: string; lines: string[] }
  | { type: 'sign-cancel'; id: string }

/** Что экран отвечает кассе. */
export type FromScreen = { type: 'here' } | { type: 'signed'; id: string; png: string }

export type ScreenState =
  | { kind: 'idle' }
  | { kind: 'cart'; lines: ScreenLine[]; total_tyiyn: number }
  | { kind: 'qr'; amount_tyiyn: number; qr_base64: string }
  | { kind: 'paid'; amount_tyiyn: number; until: number }
  | { kind: 'sign'; id: string; title: string; lines: string[] }

/** Состояние экрана после сообщения кассы. Подпись не перебивается QR и заставкой: её снимает
 *  только «sign-cancel» с тем же номером или новая подпись. */
export function screenStep(state: ScreenState, msg: ToScreen, now: number): ScreenState {
  switch (msg.type) {
    case 'sign':
      return { kind: 'sign', id: msg.id, title: msg.title, lines: msg.lines }
    case 'sign-cancel':
      return state.kind === 'sign' && state.id === msg.id ? { kind: 'idle' } : state
    default:
      break
  }
  if (state.kind === 'sign') return state
  switch (msg.type) {
    case 'qr':
      return { kind: 'qr', amount_tyiyn: msg.amount_tyiyn, qr_base64: msg.qr_base64 }
    case 'paid':
      return { kind: 'paid', amount_tyiyn: msg.amount_tyiyn, until: now + PAID_SHOW_MS }
    case 'idle':
      // «Оплачено» досматривается до конца, даже если касса уже очистила чек.
      return state.kind === 'paid' && state.until > now ? state : { kind: 'idle' }
    case 'cart':
      if (state.kind === 'paid' && state.until > now) return state
      return msg.lines.length > 0 ? { kind: 'cart', lines: msg.lines, total_tyiyn: msg.total_tyiyn } : { kind: 'idle' }
  }
}

/** Время вышло — «Оплачено» уходит в заставку. */
export function screenTick(state: ScreenState, now: number): ScreenState {
  return state.kind === 'paid' && state.until <= now ? { kind: 'idle' } : state
}

/** Что показать: QR кассы важнее чека; нет QR — чек, как его набирает касса (дубль кассы). */
export function toScreen(qr: Parameters<typeof qrToScreen>[0], lines: ScreenLine[], total_tyiyn: number): ToScreen {
  const q = qrToScreen(qr)
  if (q.type !== 'idle') return q
  return lines.length > 0 ? { type: 'cart', lines, total_tyiyn } : q
}

/** Что показать по QR кассы: ждущий с картинкой — QR, оплаченный — «Оплачено», иначе ничего. */
export function qrToScreen(qr: { id: string; status: string; amount_tyiyn: number; paid_tyiyn?: number | null; qr_base64?: string | null } | null): ToScreen {
  if (qr?.status === 'waiting' && qr.qr_base64) return { type: 'qr', amount_tyiyn: qr.amount_tyiyn, qr_base64: qr.qr_base64 }
  if (qr?.status === 'paid') return { type: 'paid', amount_tyiyn: qr.paid_tyiyn ?? qr.amount_tyiyn }
  return { type: 'idle' }
}

/** Канал, если браузер его умеет (старый браузер — экрана покупателя просто нет). */
export function openChannel(): BroadcastChannel | null {
  return typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel(SCREEN_CHANNEL)
}
