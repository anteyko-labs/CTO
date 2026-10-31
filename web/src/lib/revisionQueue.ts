// Неотправленные сканы ревизии на устройстве (задача 003, Н14). Скан, на который сервер не
// ответил и после автоматических повторов (обрыв Wi-Fi на складе), не пропадает молча: он стоит
// в очереди со своим op_id — в памяти и в localStorage по ревизии, переживает перезагрузку, — у
// строки отметка «⏳ +1 шт не отправлено», вверху экрана «Не отправлено: N — отправится при связи».
// При связи очередь уходит с теми же op_id: дошедший без ответа скан сервер второй раз не
// прибавит (ADR-062), недошедший — прибавит один раз.
import { noServerAnswer } from './api'

export interface QueuedScan {
  opId: string
  productId: string
  /** Прибавка в единицах товара: 1 шт или 1 канистра (мл). */
  qty: number
  /** Ответа сервера не было и после повторов: на экране — отметка «не отправлено». Свежий скан,
   *  который ещё отправляется сам, — `false`. */
  unsent: boolean
}

/** Чем кончилась отправка: записан; ответа нет — ждать связи; сервер отказал — не повторять. */
export type SendOutcome = 'ok' | 'no_answer' | 'refused'

type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

export const queueKey = (revisionId: string): string => `avtodom.revision.unsent.${revisionId}`

const valid = (x: unknown): x is Omit<QueuedScan, 'unsent'> => {
  if (typeof x !== 'object' || x === null) return false
  const r = x as Record<string, unknown>
  return typeof r.opId === 'string' && typeof r.productId === 'string' && typeof r.qty === 'number' && r.qty > 0
}

/** Очередь ревизии с устройства. После перезагрузки всё в ней — неотправленное. */
export function readQueue(store: Store, revisionId: string): QueuedScan[] {
  try {
    const raw = store.getItem(queueKey(revisionId))
    const v: unknown = raw ? JSON.parse(raw) : []
    if (!Array.isArray(v)) return []
    return v.filter(valid).map((x) => ({ opId: x.opId, productId: x.productId, qty: x.qty, unsent: true }))
  } catch {
    return []
  }
}

/** Сохранить очередь; пустая — ключ убирается. Хранилище недоступно — очередь живёт в памяти. */
export function writeQueue(store: Store, revisionId: string, q: QueuedScan[]): void {
  try {
    if (q.length === 0) store.removeItem(queueKey(revisionId))
    else store.setItem(queueKey(revisionId), JSON.stringify(q.map(({ opId, productId, qty }) => ({ opId, productId, qty }))))
  } catch {
    // Память устройства полна или закрыта: очередь остаётся на экране до перезагрузки.
  }
}

/** Скан встаёт в очередь, как только отсканирован; тот же второй раз — нет. */
export const queueAdd = (q: QueuedScan[], s: QueuedScan): QueuedScan[] => (q.some((x) => x.opId === s.opId) ? q : [...q, s])

/** После отправки: записан или отклонён — уходит из очереди; ответа нет — остаётся с тем же op_id. */
export function queueAfterSend(q: QueuedScan[], opId: string, outcome: SendOutcome): QueuedScan[] {
  if (outcome === 'no_answer') return q.map((x) => (x.opId === opId && !x.unsent ? { ...x, unsent: true } : x))
  return q.filter((x) => x.opId !== opId)
}

/** Что отправить при связи: неотправленное по порядку, кроме того, что уже идёт. */
export const queueToSend = (q: QueuedScan[], inflight: ReadonlySet<string>): QueuedScan[] =>
  q.filter((x) => x.unsent && !inflight.has(x.opId))

/** Неотправленное по строкам: товар → сумма прибавок (для отметки «⏳ +N не отправлено»). */
export function unsentByLine(q: QueuedScan[]): Map<string, number> {
  const out = new Map<string, number>()
  for (const x of q) if (x.unsent) out.set(x.productId, (out.get(x.productId) ?? 0) + x.qty)
  return out
}

export const unsentCount = (q: QueuedScan[]): number => q.filter((x) => x.unsent).length

export const unsentText = (n: number): string => `Не отправлено: ${n} — отправится при связи`

/** Ошибка отправки: ответа сервера не было — ждать связи; сервер ответил отказом — не повторять. */
export const sendOutcome = (e: unknown): SendOutcome => (noServerAnswer(e) ? 'no_answer' : 'refused')
