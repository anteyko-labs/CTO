// Касса без сети: снимок каталога и очередь чеков на устройстве (SPEC-09, ADR-013).
// Каждый чек сначала ложится в очередь и только потом уходит на сервер — один путь
// кода и для онлайна, и для офлайна, поэтому обрыв связи ничего не теряет.

import { openDB, type IDBPDatabase } from 'idb'
import { ApiError, api, deviceId, noServerAnswer, serverReachable, setServerReachable } from './api'
import type { CategoryKind, Party, PartyContact, PartyVehicle, Product, Sale, Service, Shift } from './types'

export interface SnapshotProduct extends Product {
  barcodes: string[]
}

/** Клиент из снимка: касса без сети выбирает его для долга и машину для замены (ADR-093). */
export interface SnapshotClient extends Party {
  contacts: Pick<PartyContact, 'id' | 'full_name' | 'position' | 'active'>[]
  vehicles: PartyVehicle[]
}

export interface Snapshot {
  version: string
  server_time: string
  products: SnapshotProduct[]
  employees: { id: string; full_name: string; is_cashier: boolean; is_master: boolean }[]
  /** Услуги, категории и клиенты для работы без сети (ADR-093); у старого снимка — нет. */
  services?: Service[]
  categories?: { id: string; name: string; kind: CategoryKind }[]
  clients?: SnapshotClient[]
  /** Безналичные счета для карты и перевода без сети (ADR-076); у старого снимка — нет. */
  bank_accounts?: { id: string; name: string; default_card?: boolean; default_transfer?: boolean; fee_rate_bp?: number }[]
  oil_change_master_fee_tyiyn: number
  saved_at: string
}

export type OutboxStatus = 'pending' | 'sending' | 'done' | 'rejected'

export interface OutboxItem {
  op_id: string
  temp_no: string
  created_at: string
  status: OutboxStatus
  attempts: number
  /** Сколько раз сервер ответил «конфликт» (409) — отдельно от обрывов связи. */
  conflicts?: number
  last_error: string
  body: unknown
  server_number?: number
  /** Кто пробил чек: очередь отправляет только чеки вошедшего пользователя (SPEC-09). */
  user_id?: string
  user_name?: string
  /** Владелец взял чужой чек на себя: уйдёт от владельца с `claimed_user_id`. */
  claimed_by?: string
  /** Раньше этого времени чек не повторяем: пауза 30 с → 1 мин → 5 мин после сбоя. */
  next_try_at?: string
  /** Чек для печати: строки с названиями, итог, оплаты — повторная печать из очереди. */
  receipt?: Sale
  /** Не чек, а другая запись кассы — куда её отправить (отменённые строки, SPEC-30 §8). На экранах
   *  очереди такие записи не видны и чеками не считаются; принятая сервером — удаляется. */
  path?: string
}

/** Запись очереди — чек (а не, например, отменённые строки). */
export const isSaleItem = (i: OutboxItem): boolean => !i.path

/** Кто сейчас работает на кассе: чьи чеки отправлять из очереди. */
export interface QueueUser {
  id: string
  full_name: string
  role: string
}

const DB_NAME = 'avtodom'
const SNAP = 'snapshot'
const OUTBOX = 'outbox'

let dbPromise: Promise<IDBPDatabase> | null = null

function db(): Promise<IDBPDatabase> {
  dbPromise ??= openDB(DB_NAME, 1, {
    upgrade(d) {
      if (!d.objectStoreNames.contains(SNAP)) d.createObjectStore(SNAP)
      if (!d.objectStoreNames.contains(OUTBOX)) d.createObjectStore(OUTBOX, { keyPath: 'op_id' })
    },
  })
  return dbPromise
}

// ---------- Снимок каталога ----------

export async function readSnapshot(): Promise<Snapshot | null> {
  try {
    return (await (await db()).get(SNAP, 'current')) ?? null
  } catch {
    return null
  }
}

/** Тянет снимок с сервера; 304 — прежний снимок, но проверенный сейчас. Без сети молча пропускает. */
export async function refreshSnapshot(): Promise<Snapshot | null> {
  const current = await readSnapshot()
  try {
    const res = await fetch('/api/v1/offline/snapshot', {
      headers: current?.version ? { 'if-none-match': `"${current.version}"` } : {},
      credentials: 'same-origin',
    })
    setServerReachable(![502, 503, 504, 530].includes(res.status))
    if (res.status === 304 && current) {
      // Сервер подтвердил, что снимок не менялся: он свежий, предупреждение «устарел» уходит.
      const fresh = { ...current, saved_at: new Date().toISOString() }
      await (await db()).put(SNAP, fresh, 'current')
      return fresh
    }
    if (!res.ok) return current
    const snap = (await res.json()) as Snapshot
    snap.saved_at = new Date().toISOString()
    await (await db()).put(SNAP, snap, 'current')
    return snap
  } catch {
    setServerReachable(false)
    return current
  }
}

/**
 * После своего чека без сети остаток в снимке уменьшается сразу: следующий покупатель видит,
 * что товара стало меньше (SPEC-09). `units` — в единицах учёта (мл, шт, г).
 */
export async function takeFromSnapshot(lines: { product_id: string; units: number }[]): Promise<void> {
  try {
    const snap = await readSnapshot()
    if (!snap || lines.length === 0) return
    await (await db()).put(SNAP, subtractStock(snap, lines), 'current')
  } catch {
    // Без хранилища остаток обновится со следующим снимком.
  }
}

export function subtractStock(snap: Snapshot, lines: { product_id: string; units: number }[]): Snapshot {
  const by = new Map<string, number>()
  for (const l of lines) by.set(l.product_id, (by.get(l.product_id) ?? 0) + l.units)
  return { ...snap, products: snap.products.map((p) => (by.has(p.id) ? { ...p, stock_qty: p.stock_qty - (by.get(p.id) ?? 0) } : p)) }
}

const plateKey = (v: string) => v.replace(/[\s-]/g, '').toUpperCase()

/** Поиск клиента в снимке: имя, телефон (по цифрам), ПИН/ИНН и госномер машины. */
export function searchSnapshotClients(snap: Snapshot | null, query: string, limit = 8): SnapshotClient[] {
  const q = query.trim().toLowerCase()
  if (!snap?.clients || q.length < 2) return []
  const digits = q.replace(/\D/g, '')
  const plate = plateKey(q)
  return snap.clients
    .filter(
      (c) =>
        c.name.toLowerCase().includes(q) ||
        (digits.length >= 3 && (c.phone.replace(/\D/g, '').includes(digits) || c.inn.includes(digits))) ||
        (plate.length >= 3 && c.vehicles.some((v) => plateKey(v.plate).includes(plate))),
    )
    .slice(0, limit)
}

// ---------- Последняя известная смена ----------

const SHIFT_KEY = 'avtodom.shift'

/**
 * Последняя известная смена (номер, кассир): без связи касса не сбрасывает её и не начинает
 * спрашивать кассира посреди смены, в том числе после перезагрузки (SPEC-09).
 */
export function saveKnownShift(shift: Shift | null): void {
  try {
    localStorage.setItem(SHIFT_KEY, JSON.stringify(shift))
  } catch {
    // Без хранилища смена помнится до перезагрузки.
  }
}

export function knownShift(): Shift | null | undefined {
  try {
    const raw = localStorage.getItem(SHIFT_KEY)
    return raw === null ? undefined : (JSON.parse(raw) as Shift | null)
  } catch {
    return undefined
  }
}

/** Поиск по снимку: название, бренд, артикул и штрихкод — как на сервере. */
export function searchSnapshot(snap: Snapshot, query: string, limit = 15): SnapshotProduct[] {
  const q = query.trim().toLowerCase()
  if (q.length < 2) return []
  const exact = snap.products.filter((p) => p.barcodes.some((c) => c === q))
  if (exact.length > 0) return exact
  return snap.products
    .filter(
      (p) =>
        p.name.toLowerCase().includes(q) ||
        p.brand.toLowerCase().includes(q) ||
        p.article.toLowerCase().includes(q) ||
        p.barcodes.some((c) => c.startsWith(q)),
    )
    .slice(0, limit)
}

export function findByBarcode(snap: Snapshot, code: string): SnapshotProduct | undefined {
  return snap.products.find((p) => p.barcodes.includes(code.trim()))
}

/** Снимок старше двух часов: цены и остатки могли измениться (SPEC-09). */
export function snapshotIsStale(snap: Snapshot | null): boolean {
  if (!snap) return true
  return Date.now() - new Date(snap.saved_at).getTime() > 2 * 60 * 60 * 1000
}

// ---------- Хранилище устройства ----------

let persistPromise: Promise<boolean | null> | null = null

/**
 * Просит браузер не вычищать хранилище: в нём очередь чеков, ещё не ушедших на сервер (SPEC-09).
 * `false` — браузер отказал, и неотправленные чеки могут пропасть при нехватке места;
 * `null` — браузер такого не умеет. Спрашивает один раз за запуск.
 */
export function requestPersistentStorage(): Promise<boolean | null> {
  persistPromise ??= (async () => {
    try {
      const storage = navigator.storage
      if (!storage?.persist) return null
      if (storage.persisted && (await storage.persisted())) return true
      return await storage.persist()
    } catch {
      return null
    }
  })()
  return persistPromise
}

// ---------- Очередь чеков ----------

const listeners = new Set<(items: OutboxItem[]) => void>()

async function allItems(): Promise<OutboxItem[]> {
  try {
    const items: OutboxItem[] = await (await db()).getAll(OUTBOX)
    return items.sort((a, b) => a.created_at.localeCompare(b.created_at))
  } catch {
    return []
  }
}

/** Экраны очереди видят только чеки: отменённые строки уходят сами и чеками не считаются. */
async function emit(): Promise<void> {
  const items = (await allItems()).filter(isSaleItem)
  for (const l of listeners) l(items)
}

export function watchOutbox(fn: (items: OutboxItem[]) => void): () => void {
  listeners.add(fn)
  void emit()
  return () => {
    listeners.delete(fn)
  }
}

export const pendingCount = (items: OutboxItem[]): number =>
  items.filter((i) => i.status === 'pending' || i.status === 'sending').length

export const rejectedItems = (items: OutboxItem[]): OutboxItem[] => items.filter((i) => i.status === 'rejected')

/** Чек этого пользователя (старые записи без пользователя — общие) или взятый им на себя. */
export const isOwnItem = (i: OutboxItem, userId: string): boolean => !i.user_id || i.user_id === userId || i.claimed_by === userId

/** Неотправленные чеки других пользователей: «N чеков <имя> ждут его входа» (SPEC-09). */
export function foreignWaiting(items: OutboxItem[], userId: string): { user_id: string; user_name: string; count: number }[] {
  const by = new Map<string, { user_id: string; user_name: string; count: number }>()
  for (const i of items) {
    if (isOwnItem(i, userId) || (i.status !== 'pending' && i.status !== 'sending') || !i.user_id) continue
    const cur = by.get(i.user_id) ?? { user_id: i.user_id, user_name: i.user_name ?? '', count: 0 }
    cur.count += 1
    by.set(i.user_id, cur)
  }
  return [...by.values()]
}

/** «1 чек», «3 чека», «5 чеков». */
export function receiptsWord(n: number): string {
  const m10 = n % 10
  const m100 = n % 100
  if (m10 === 1 && m100 !== 11) return 'чек'
  if (m10 >= 2 && m10 <= 4 && (m100 < 12 || m100 > 14)) return 'чека'
  return 'чеков'
}

/** Пауза перед повтором после сбоя: 30 с → 1 мин → 5 мин (SPEC-09). */
export function retryDelayMs(attempts: number): number {
  if (attempts <= 1) return 30_000
  if (attempts === 2) return 60_000
  return 5 * 60_000
}

/** Отправленные чеки хранятся неделю для повторной печати (SPEC-09). */
const KEEP_DONE_MS = 7 * 24 * 60 * 60 * 1000

export function expiredDone(items: OutboxItem[], now: number = Date.now()): OutboxItem[] {
  return items.filter((i) => i.status === 'done' && now - new Date(i.created_at).getTime() > KEEP_DONE_MS)
}

/** Чек из записи очереди для печати: номер сервера, если он уже есть, иначе временный. */
export function receiptOf(i: OutboxItem): Sale | null {
  if (!i.receipt) return null
  return { ...i.receipt, number: (i.server_number ?? i.temp_no) as unknown as number }
}

const TEMP_COUNTER_KEY = 'avtodom.temp_no'

/** Временный номер из хвоста устройства и порядкового номера: «Ч-1A2B-0042». */
export function formatTempNumber(device: string, seq: string): string {
  const tail = device.replace(/-/g, '').slice(-4).toUpperCase()
  return `Ч-${tail}-${seq}`
}

/**
 * Временный номер чека: печатается, пока сервер не дал свой. Счётчик лежит рядом
 * с номером устройства, поэтому номера на устройстве не повторяются; без хранилища —
 * время и случайный хвост.
 */
function tempNumber(): string {
  try {
    const n = Number(localStorage.getItem(TEMP_COUNTER_KEY) ?? '0') + 1
    if (!Number.isSafeInteger(n)) throw new Error('счётчик')
    localStorage.setItem(TEMP_COUNTER_KEY, String(n))
    return formatTempNumber(deviceId, String(n).padStart(4, '0'))
  } catch {
    const seq = Date.now().toString(36).slice(-4) + crypto.randomUUID().slice(0, 2)
    return formatTempNumber(deviceId, seq.toUpperCase())
  }
}

/** Чеки, оставшиеся «в отправке» после закрытой вкладки, снова ждут отправки. */
export function staleSending(items: OutboxItem[]): OutboxItem[] {
  return items.filter((i) => i.status === 'sending').map((i) => ({ ...i, status: 'pending' as const }))
}

let recovered: Promise<void> | null = null

/** Один раз за запуск: повтор безопасен, сервер узнает чек по `op_id`. Отправленные старше недели убираются. */
function recover(): Promise<void> {
  recovered ??= (async () => {
    try {
      const d = await db()
      const items = await allItems()
      for (const item of staleSending(items)) await d.put(OUTBOX, item)
      for (const item of expiredDone(items)) await d.delete(OUTBOX, item.op_id)
    } catch {
      // Без IndexedDB восстанавливать нечего.
    }
  })()
  return recovered
}

/**
 * Кладёт чек в очередь со статусом «отправляется»: касса сама шлёт его на сервер,
 * фоновая отправка его не трогает. Возвращает запись с временным номером.
 */
export async function enqueueSale(
  op_id: string,
  body: unknown,
  who?: { user?: QueueUser | null; receipt?: (temp_no: string) => Sale },
): Promise<OutboxItem> {
  await recover()
  const temp_no = tempNumber()
  const item: OutboxItem = {
    op_id,
    temp_no,
    created_at: new Date().toISOString(),
    status: 'sending',
    attempts: 0,
    last_error: '',
    body,
    user_id: who?.user?.id,
    user_name: who?.user?.full_name,
    receipt: who?.receipt?.(temp_no),
  }
  await (await db()).put(OUTBOX, item)
  await emit()
  return item
}

async function update(op_id: string, patch: Partial<OutboxItem>): Promise<void> {
  const d = await db()
  const item: OutboxItem | undefined = await d.get(OUTBOX, op_id)
  if (!item) return
  await d.put(OUTBOX, { ...item, ...patch })
  await emit()
}

/** Сервер принял чек. */
export const markSent = (op_id: string, server_number: number): Promise<void> =>
  update(op_id, { status: 'done', server_number, last_error: '' })

/**
 * Запись кассы, которая не чек (отменённые строки, SPEC-30 §8): ложится в очередь и уходит той же
 * отправкой, что чеки; повтор безопасен — сервер узнаёт её по `op_id`.
 */
export async function enqueueOp(op_id: string, path: string, body: unknown, user?: QueueUser | null): Promise<void> {
  try {
    await recover()
    const item: OutboxItem = {
      op_id,
      temp_no: '',
      created_at: new Date().toISOString(),
      status: 'pending',
      attempts: 0,
      last_error: '',
      body,
      user_id: user?.id,
      user_name: user?.full_name,
      path,
    }
    await (await db()).put(OUTBOX, item)
  } catch {
    // Без хранилища запись пропадёт — это след для владельца, а не чек: касса не встаёт.
    return
  }
  void syncOutbox()
}

/** Связи нет: чек ждёт фоновой отправки. */
export const markPending = (op_id: string, last_error: string): Promise<void> =>
  update(op_id, { status: 'pending', attempts: 1, last_error })

/** Сервер отказал, кассир видит ошибку: чек убирается, сам он не уйдёт. */
export async function removeSale(op_id: string): Promise<void> {
  await (await db()).delete(OUTBOX, op_id)
  await emit()
}

async function put(item: OutboxItem): Promise<void> {
  await (await db()).put(OUTBOX, item)
  await emit()
}

/** Убрать отклонённый чек с устройства: владелец о нём уже знает (запись принята сервером). */
export async function removeRejected(op_id: string): Promise<void> {
  const d = await db()
  const item: OutboxItem | undefined = await d.get(OUTBOX, op_id)
  if (item?.status !== 'rejected') return
  await d.delete(OUTBOX, op_id)
  await emit()
}

/** Владелец отправит чеки другого пользователя от себя (SPEC-09): сервер запишет, чьи они. */
export async function claimForeign(user_id: string, owner: QueueUser): Promise<void> {
  if (owner.role !== 'owner') return
  const d = await db()
  for (const item of await allItems()) {
    if (item.user_id === user_id && (item.status === 'pending' || item.status === 'sending')) {
      await d.put(OUTBOX, { ...item, claimed_by: owner.id, next_try_at: undefined })
    }
  }
  await emit()
}

export async function dropDone(): Promise<void> {
  const d = await db()
  for (const item of await allItems()) {
    // Отклонённые остаются: их надо разобрать, а не потерять.
    if (item.status === 'done') await d.delete(OUTBOX, item.op_id)
  }
  await emit()
}

/** Сколько раз повторяем чек, на который сервер отвечает «конфликт» (409). */
const MAX_CONFLICTS = 5

let syncing = false

/** Кто вошёл: очередь отправляет только его чеки (SPEC-09). Ставит фоновая работа кассы. */
let queueUser: QueueUser | null = null

export function setQueueUser(u: QueueUser | null): void {
  queueUser = u
}

/**
 * Ответ сервера не значит, что чек отклонён по существу: связи нет, сервер упал (5xx) или
 * истёк вход (401). Такой чек остаётся в очереди и уйдёт позже — удалять его нельзя (ADR-062).
 */
export function keepInQueue(e: unknown): boolean {
  if (!(e instanceof ApiError)) return true
  return noServerAnswer(e) || e.status >= 500 || e.status === 401
}

/** Что делать с чеком после ошибки отправки из очереди. */
export function sendOutcome(e: unknown, conflicts: number): 'conflict_retry' | 'reject' | 'keep' {
  // 409 — обычно гонка (карточку только что объединили): повторяем, но не вечно —
  // после MAX_CONFLICTS чек уходит владельцу как отклонённый, очередь не встаёт.
  if (e instanceof ApiError && e.status === 409 && conflicts + 1 < MAX_CONFLICTS) return 'conflict_retry'
  if (e instanceof ApiError && !keepInQueue(e) && e.status >= 400) return 'reject'
  return 'keep'
}

/**
 * Сервер отказал по существу. «Отклонён» чек становится, только когда сервер принял отчёт
 * об отказе (о нём узнал владелец); иначе чек ждёт и отчёт повторится (ADR-062).
 */
export function afterRejection(item: OutboxItem, error: string, reported: boolean, now: number = Date.now()): OutboxItem {
  if (reported) return { ...item, status: 'rejected', last_error: error, next_try_at: undefined }
  return {
    ...item,
    status: 'pending',
    attempts: item.attempts + 1,
    next_try_at: new Date(now + retryDelayMs(item.attempts + 1)).toISOString(),
    last_error: `${error} — отказ ещё не передан владельцу, повторим`,
  }
}

/** Чек ждёт паузы после сбоя — сейчас его не трогаем (кроме «Отправить сейчас» и появления сети). */
export const waitsRetry = (i: OutboxItem, now: number = Date.now()): boolean =>
  Boolean(i.next_try_at && new Date(i.next_try_at).getTime() > now)

/** Тело запроса из очереди: признак «без сети» (продажа уже состоялась, ADR-042, ADR-093) и, если
 *  владелец отправляет чужой чек, чей он. */
export function queuedBody(item: OutboxItem): Record<string, unknown> {
  const body: Record<string, unknown> = { ...(item.body as object), offline: true }
  if (item.claimed_by && item.user_id && item.user_id !== item.claimed_by) body.claimed_user_id = item.user_id
  return body
}

/** Проведён ли уже чек с этим op_id — например, от имени другого пользователя кассы. */
async function postedNumber(op_id: string): Promise<number | null> {
  try {
    return (await api<{ number: number }>('GET', `/sales/by-op/${op_id}`)).number
  } catch {
    return null
  }
}

/** Сообщить владельцу об отказе: да — принято (или уже было принято), нет — повторить позже. */
async function reportRejection(item: OutboxItem, error: string): Promise<'reported' | 'posted' | 'later' | 'no_server'> {
  try {
    await api('POST', '/sales/offline-rejected', { op_id: item.op_id, error, body: item.body })
    return 'reported'
  } catch (e) {
    // «op_id уже использован»: отказ уже сообщили (другой пользователь кассы) или чек проведён —
    // очередь не встаёт на этом чеке (ADR-093).
    if (e instanceof ApiError && e.status === 409) return (await postedNumber(item.op_id)) !== null ? 'posted' : 'reported'
    if (e instanceof ApiError && noServerAnswer(e)) return 'no_server'
    return 'later'
  }
}

const backoff = (item: OutboxItem, error: string): OutboxItem => ({
  ...item,
  status: 'pending',
  attempts: item.attempts + 1,
  next_try_at: new Date(Date.now() + retryDelayMs(item.attempts + 1)).toISOString(),
  last_error: error,
})

/**
 * Отправляет накопившиеся чеки вошедшего пользователя по одному. Повтор с тем же `op_id`
 * безопасен: сервер вернёт уже проведённый чек (ADR-013). Один чек не держит очередь: сбой
 * сервера по нему — пауза для этого чека, остальные идут дальше; нет связи — ждут все.
 */
export async function syncOutbox(force = false): Promise<void> {
  if (syncing || !navigator.onLine || !queueUser) return
  const me = queueUser
  syncing = true
  try {
    await recover()
    for (const item of await allItems()) {
      if (item.status !== 'pending' || !isOwnItem(item, me.id)) continue
      if (!force && waitsRetry(item)) continue
      if (item.path) {
        // Не чек: принят — удаляется; отказ по существу или «op_id уже был» — тоже (это след, а
        // не продажа, и повтор его не исправит); нет связи — ждёт, как чеки.
        try {
          await api('POST', item.path, item.body)
          await (await db()).delete(OUTBOX, item.op_id)
        } catch (e) {
          if (!keepInQueue(e)) {
            await (await db()).delete(OUTBOX, item.op_id)
            continue
          }
          await (await db()).put(OUTBOX, backoff(item, e instanceof Error ? e.message : 'нет связи'))
          if (!(e instanceof ApiError) || noServerAnswer(e) || e.status === 401) break
        }
        continue
      }
      await put({ ...item, status: 'sending' })
      try {
        const sale = await api<{ number: number }>('POST', '/sales', queuedBody(item))
        await put({ ...item, status: 'done', server_number: sale.number, last_error: '', next_try_at: undefined })
      } catch (e) {
        // «op_id уже использован»: чек мог уйти раньше от другого пользователя этой кассы —
        // тогда он проведён, а не отклонён.
        if (e instanceof ApiError && e.status === 409) {
          const number = await postedNumber(item.op_id)
          if (number !== null) {
            await put({ ...item, status: 'done', server_number: number, last_error: '', next_try_at: undefined })
            continue
          }
        }
        const message = e instanceof Error ? e.message : 'нет связи'
        const outcome = sendOutcome(e, item.conflicts ?? 0)
        if (outcome === 'conflict_retry') {
          await put({ ...item, status: 'pending', conflicts: (item.conflicts ?? 0) + 1, last_error: message })
          continue
        }
        if (outcome === 'reject') {
          // Сервер отказал по существу: чек не исчезает, о нём узнаёт владелец.
          const report = await reportRejection(item, message)
          if (report === 'posted') {
            const number = await postedNumber(item.op_id)
            await put({ ...item, status: 'done', server_number: number ?? undefined, last_error: '', next_try_at: undefined })
            continue
          }
          await put(afterRejection(item, message, report === 'reported'))
          if (report === 'no_server') break
          continue
        }
        await put(backoff(item, message))
        // Нет связи или истёк вход — остальные чеки тоже не уйдут; сбой сервера по одному чеку
        // остальных не держит.
        if (!(e instanceof ApiError) || noServerAnswer(e) || e.status === 401) break
      }
    }
  } finally {
    syncing = false
  }
}

/** Фоновая работа кассы: обновление снимка и отправка очереди вошедшего пользователя. */
export function startOfflineLoop(user: QueueUser): () => void {
  setQueueUser(user)
  const tick = () => {
    // Сервер не отвечал — проверяем его дешёвым запросом снимка (304), а не ждём 10 минут.
    if (!serverReachable()) void refreshSnapshot()
    void syncOutbox()
  }
  const back = () => {
    void refreshSnapshot()
    void syncOutbox(true)
  }
  const refresh = () => {
    void refreshSnapshot()
  }
  refresh()
  tick()
  const t1 = setInterval(tick, 30_000)
  const t2 = setInterval(refresh, 10 * 60_000)
  window.addEventListener('online', back)
  return () => {
    clearInterval(t1)
    clearInterval(t2)
    window.removeEventListener('online', back)
    setQueueUser(null)
  }
}

/**
 * Предупреждение перед выходом (SPEC-09): неотправленные чеки этого пользователя останутся на
 * устройстве и уйдут, когда он снова войдёт; без связи войти снова не получится, пока она не появится.
 * `null` — выходить можно без вопросов.
 */
export function logoutWarning(items: OutboxItem[], userId: string, online: boolean): string | null {
  const n = pendingCount(items.filter((i) => isOwnItem(i, userId)))
  const parts: string[] = []
  if (n > 0) parts.push(`${n} ${receiptsWord(n)} ещё не ${n === 1 ? 'отправлен' : 'отправлены'} на сервер. Они останутся на этом устройстве и уйдут, когда вы снова войдёте (или владелец отправит их от себя).`)
  if (!online) parts.push('Нет связи с сервером: войти снова получится только при связи.')
  return parts.length > 0 ? parts.join(' ') : null
}

/** Все чеки очереди — для проверки перед выходом. */
export const outboxItems = async (): Promise<OutboxItem[]> => (await allItems()).filter(isSaleItem)
