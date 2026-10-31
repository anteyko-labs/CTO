// Неотправленные сканы ревизии на устройстве (задача 003, Н14). Скан, на который сервер не
// ответил и после автоматических повторов (обрыв Wi-Fi на складе), не пропадает молча: он стоит
// в очереди со своим op_id — в localStorage по ревизии и пользователю, переживает перезагрузку, —
// у строки отметка «⏳ +1 шт не отправлено», вверху экрана «Не отправлено: N — отправится при связи».
// При связи очередь уходит с теми же op_id: дошедший без ответа скан сервер второй раз не
// прибавит (ADR-062), недошедший — прибавит один раз.
//
// Одно устройство — несколько вкладок: каждая запись в хранилище — слияние со свежей очередью из
// хранилища по op_id (добавить своё, убрать только свои отправленные), а не «вся очередь вкладки
// поверх»; другие вкладки узнают об изменении по событию `storage`. Один и тот же op_id из двух
// вкладок сервер записывает один раз. Чтение и запись хранилища — под общей для вкладок
// блокировкой `navigator.locks` (где она есть: только защищённая страница): две вкладки не
// прочитают одну и ту же очередь одновременно, чтобы затем одна затёрла скан другой. Без неё —
// пишется сразу, как раньше.
import { ApiError, noServerAnswer } from './api'
import { plural } from './format'

export interface QueuedScan {
  opId: string
  /** Товар строки; `null` — код, которого нет на экране, отсканирован без связи: товар уточнится
   *  по коду при связи. */
  productId: string | null
  /** Код со сканера — у скана, товар которого ещё не найден. */
  barcode: string | null
  /** Прибавка в единицах товара: 1 шт или 1 канистра (мл); у скана по коду — 0 до уточнения. */
  qty: number
  /** Когда отсканирован, мс: очереди старше недели чистятся. */
  at: number
}

/**
 * Чем кончилась отправка: записан; ответа нет или сервер временно не может (5xx, 429) — ждать
 * связи; сессия кончилась (401) — ждать входа; сервер окончательно отказал по существу (ревизию
 * провели, неверные данные) — не повторять.
 */
export type SendOutcome = 'ok' | 'wait' | 'login' | 'refused'

type Store = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>
type ListStore = Store & Pick<Storage, 'key' | 'length'>

const PREFIX = 'avtodom.revision.unsent.'
/** Неделя: старше — очередь не отправляется, а чистится. */
export const QUEUE_MAX_AGE_MS = 7 * 24 * 3600 * 1000

/** Очередь — своя у ревизии и пользователя: на общем планшете сканы одного не уходят под другим. */
export const queueKey = (revisionId: string, userId: string): string => `${PREFIX}${revisionId}.${userId}`

function parseKey(key: string): { revisionId: string; userId: string } | null {
  if (!key.startsWith(PREFIX)) return null
  const [revisionId, userId, ...rest] = key.slice(PREFIX.length).split('.')
  return revisionId && userId && rest.length === 0 ? { revisionId, userId } : null
}

const valid = (x: unknown): x is QueuedScan => {
  if (typeof x !== 'object' || x === null) return false
  const r = x as Record<string, unknown>
  if (typeof r.opId !== 'string' || typeof r.at !== 'number') return false
  if (typeof r.productId === 'string') return typeof r.qty === 'number' && r.qty > 0
  return r.productId === null && typeof r.barcode === 'string' && r.barcode.trim() !== ''
}

interface Stored {
  /** Чьи сканы: имя для «неотправленных сканов пользователя …». */
  user: string
  items: QueuedScan[]
}

/** Очередь из хранилища: нет — пустая; испорченная — пустая, а не ошибка экрана. Хранилище
 *  недоступно — `null`. */
function readStored(store: Store, key: string): Stored | null {
  let raw: string | null
  try {
    raw = store.getItem(key)
  } catch {
    return null
  }
  try {
    const v = raw ? (JSON.parse(raw) as unknown) : null
    if (typeof v !== 'object' || v === null || !Array.isArray((v as Stored).items)) return { user: '', items: [] }
    const s = v as Stored
    return {
      user: typeof s.user === 'string' ? s.user : '',
      items: s.items.filter(valid).map((x) => ({ opId: x.opId, productId: x.productId, barcode: x.productId ? null : x.barcode, qty: x.qty, at: x.at })),
    }
  } catch {
    return { user: '', items: [] }
  }
}

/** Очередь ревизии пользователя с устройства; хранилище недоступно — `null`. */
export function readQueue(store: Store, revisionId: string, userId: string): QueuedScan[] | null {
  return readStored(store, queueKey(revisionId, userId))?.items ?? null
}

/**
 * Изменить очередь: свежая из хранилища (её могла изменить другая вкладка) → `change` → в
 * хранилище; пустая — ключ убирается. Хранилище недоступно — меняется `mem`, очередь живёт в
 * памяти вкладки до перезагрузки. Возвращает новую очередь.
 */
export function updateQueue(
  store: Store,
  revisionId: string,
  user: { id: string; name: string },
  change: (q: QueuedScan[]) => QueuedScan[],
  mem: QueuedScan[],
): QueuedScan[] {
  const key = queueKey(revisionId, user.id)
  const next = change(readStored(store, key)?.items ?? mem)
  try {
    if (next.length === 0) store.removeItem(key)
    else store.setItem(key, JSON.stringify({ user: user.name, items: next } satisfies Stored))
  } catch {
    // Память устройства полна или закрыта: очередь остаётся на экране до перезагрузки.
  }
  return next
}

/** Блокировка записи очереди, общая для вкладок устройства (`navigator.locks`). */
export interface QueueLocks {
  request(name: string, callback: () => unknown): Promise<unknown>
}

/** Одна блокировка на все очереди ревизий: записи редкие, а чистка проходит по всем ключам. */
export const QUEUE_LOCK = 'avtodom.revision.unsent'

/** `navigator.locks` браузера; нет (старый браузер, страница не по https) — `null`. */
export function browserLocks(): QueueLocks | null {
  const locks = typeof navigator === 'undefined' ? undefined : (navigator as { locks?: QueueLocks }).locks
  return locks && typeof locks.request === 'function' ? locks : null
}

/**
 * `fn` — под блокировкой очереди; блокировки нет или она не далась — сразу. `fn` выполняется
 * ровно один раз.
 */
export function withQueueLock<T>(locks: QueueLocks | null, fn: () => T): Promise<T> {
  let done = false
  let out: T
  const run = () => {
    done = true
    out = fn()
    return out
  }
  if (!locks) return new Promise<T>((resolve) => resolve(run()))
  return locks.request(QUEUE_LOCK, run).then(
    () => out,
    (e: unknown) => {
      if (done) throw e
      return run()
    },
  )
}

type Change = (q: QueuedScan[]) => QueuedScan[]

/** Очередь вкладки: запись — под блокировкой, а на экране изменение сразу. */
export interface QueueWriter {
  /** Очередь на экране: записанная и поверх — изменения, которые ждут блокировки. */
  readonly current: QueuedScan[]
  /** Изменить: на экране — сразу, в хранилище — слиянием со свежей очередью под блокировкой. */
  put(change: Change): Promise<void>
  /** Другая вкладка изменила хранилище — перечитать; свои ждущие изменения остаются поверх. */
  reload(): void
}

export function queueWriter(
  store: Store,
  revisionId: string,
  user: { id: string; name: string },
  locks: QueueLocks | null,
  onChange: (q: QueuedScan[]) => void,
): QueueWriter {
  let stored = readQueue(store, revisionId, user.id) ?? []
  const pending: Change[] = []
  let current = stored
  const show = () => {
    current = pending.reduce((q, f) => f(q), stored)
    onChange(current)
  }
  return {
    get current() {
      return current
    },
    put(change) {
      pending.push(change)
      show()
      return withQueueLock(locks, () => {
        stored = updateQueue(store, revisionId, user, change, stored)
        pending.splice(pending.indexOf(change), 1)
        show()
      })
    },
    reload() {
      const q = readQueue(store, revisionId, user.id)
      if (!q) return
      stored = q
      show()
    },
  }
}

/** Новый скан: товара строки с прибавкой или — без связи — кода, товар которого уточнится. */
export const newScan = (opId: string, what: { productId: string; qty: number } | { barcode: string }, at: number = Date.now()): QueuedScan =>
  'barcode' in what ? { opId, productId: null, barcode: what.barcode, qty: 0, at } : { opId, productId: what.productId, barcode: null, qty: what.qty, at }

/** Скан встаёт в очередь, как только отсканирован; тот же второй раз — нет. */
export const queueAdd = (q: QueuedScan[], s: QueuedScan): QueuedScan[] => (q.some((x) => x.opId === s.opId) ? q : [...q, s])

/** Убрать свои отправленные (записанные или окончательно отклонённые) — чужие сканы остаются. */
export const queueWithout = (q: QueuedScan[], opIds: readonly string[]): QueuedScan[] => q.filter((x) => !opIds.includes(x.opId))

/** Скан по коду: товар нашёлся — тот же op_id, теперь с товаром и прибавкой. */
export const queueResolve = (q: QueuedScan[], opId: string, productId: string, qty: number): QueuedScan[] =>
  q.map((x) => (x.opId === opId ? { ...x, productId, barcode: null, qty } : x))

/** Неотправленный скан — в очереди и не отправляется сейчас впервые со своего экрана. */
const unsentIn = (q: QueuedScan[], fresh: ReadonlySet<string>) => q.filter((x) => !fresh.has(x.opId))

/** Что отправить при связи: очередь по порядку, кроме того, что уже идёт. */
export const queueToSend = (q: QueuedScan[], inflight: ReadonlySet<string>): QueuedScan[] => q.filter((x) => !inflight.has(x.opId))

/** Неотправленное по строкам: товар → сумма прибавок (для отметки «⏳ +N не отправлено»). */
export function unsentByLine(q: QueuedScan[], fresh: ReadonlySet<string>): Map<string, number> {
  const out = new Map<string, number>()
  for (const x of unsentIn(q, fresh)) if (x.productId) out.set(x.productId, (out.get(x.productId) ?? 0) + x.qty)
  return out
}

/** Коды, товар которых уточнится при связи. */
export const unsentCodes = (q: QueuedScan[], fresh: ReadonlySet<string>): string[] => [
  ...new Set(unsentIn(q, fresh).flatMap((x) => (x.productId === null && x.barcode ? [x.barcode] : []))),
]

export const unsentCount = (q: QueuedScan[], fresh: ReadonlySet<string>): number => unsentIn(q, fresh).length

/** «1 скан», «3 скана», «5 сканов». */
export const scansText = (n: number): string => `${n} ${plural(n, ['скан', 'скана', 'сканов'])}`

export const unsentText = (n: number): string => `Не отправлено: ${n} — отправится при связи`

/** «Сначала отправится 1 неотправленный скан этой строки» — «= итог» ждёт сканы своей строки. */
export const unsentFirstText = (n: number): string =>
  `Сначала ${plural(n, ['отправится', 'отправятся', 'отправятся'])} ${n} ${plural(n, ['неотправленный скан', 'неотправленных скана', 'неотправленных сканов'])} этой строки — нет связи, итог не записан. Он остался в поле: запишите его, когда сканы уйдут.`

/** Ревизию провели, а сканы с устройства ещё шли: сколько не учтено. */
export const closedText = (n: number, number: number, status: 'posted' | 'cancelled'): string =>
  `${scansText(n)} не ${plural(n, ['учтён', 'учтены', 'учтены'])}: ревизия № ${number} ${status === 'cancelled' ? 'отменена' : 'проведена'}`

/** Коды общих ответов, когда сервер не объяснил отказ сам (не наш сервер, обрыв). */
const GENERIC = new Set(['error', 'network', 'bad_response'])

/**
 * Ошибка отправки скана. Окончательный отказ — только ответ сервера по существу: 4xx с кодом
 * предметной области (`revision_closed`, `revision_overlap`, неверные данные 422). Ответа нет,
 * 5xx (перезапуск базы, тайм-аут пула — запись откатилась), 429, 408 и ответ не нашего сервера —
 * скан остаётся в очереди; 401 — ждёт входа.
 */
export function sendOutcome(e: unknown): SendOutcome {
  if (noServerAnswer(e) || !(e instanceof ApiError)) return 'wait'
  if (e.status === 401) return 'login'
  if (e.status >= 400 && e.status < 500 && ![408, 429].includes(e.status) && !GENERIC.has(e.code)) return 'refused'
  return 'wait'
}

/** Неотправленные сканы этой ревизии на устройстве под другими пользователями. */
export function otherQueues(store: ListStore, revisionId: string, userId: string): { user: string; count: number }[] {
  const out: { user: string; count: number }[] = []
  for (const key of keys(store)) {
    const k = parseKey(key)
    if (!k || k.revisionId !== revisionId || k.userId === userId) continue
    const s = readStored(store, key)
    if (s && s.items.length > 0) out.push({ user: s.user || 'другого пользователя', count: s.items.length })
  }
  return out
}

function keys(store: ListStore): string[] {
  const out: string[] = []
  try {
    for (let i = 0; i < store.length; i++) {
      const k = store.key(i)
      if (k?.startsWith(PREFIX)) out.push(k)
    }
  } catch {
    // Хранилище закрыто — чистить нечего.
  }
  return out
}

/**
 * Чистка очередей устройства: ревизия проведена или отменена — очередь убирается, а сколько в ней
 * было, возвращается (показать «N сканов не учтены»); сканы старше недели — убираются; испорченные
 * ключи — тоже.
 */
export function cleanQueues(
  store: ListStore,
  closed: ReadonlyMap<string, { number: number; status: 'posted' | 'cancelled' }>,
  now: number = Date.now(),
): { revisionId: string; number: number; status: 'posted' | 'cancelled'; count: number }[] {
  const lost = new Map<string, { revisionId: string; number: number; status: 'posted' | 'cancelled'; count: number }>()
  for (const key of keys(store)) {
    const k = parseKey(key)
    const s = readStored(store, key)
    if (!s) continue
    try {
      const c = k ? closed.get(k.revisionId) : undefined
      if (!k || c) {
        if (k && c && s.items.length > 0) {
          const was = lost.get(k.revisionId)
          lost.set(k.revisionId, { revisionId: k.revisionId, ...c, count: (was?.count ?? 0) + s.items.length })
        }
        store.removeItem(key)
        continue
      }
      const items = s.items.filter((x) => now - x.at < QUEUE_MAX_AGE_MS)
      if (items.length === 0) store.removeItem(key)
      else if (items.length !== s.items.length) store.setItem(key, JSON.stringify({ user: s.user, items } satisfies Stored))
    } catch {
      // Не вышло — в следующий раз.
    }
  }
  return [...lost.values()]
}
