// Касса без сети: снимок каталога и очередь чеков на устройстве (SPEC-09, ADR-013).
// Каждый чек сначала ложится в очередь и только потом уходит на сервер — один путь
// кода и для онлайна, и для офлайна, поэтому обрыв связи ничего не теряет.

import { openDB, type IDBPDatabase } from 'idb'
import { ApiError, api, deviceId, noServerAnswer } from './api'
import type { Product } from './types'

export interface SnapshotProduct extends Product {
  barcodes: string[]
}

export interface Snapshot {
  version: string
  server_time: string
  products: SnapshotProduct[]
  employees: { id: string; full_name: string; is_cashier: boolean; is_master: boolean }[]
  /** Безналичные счета для карты и перевода без сети (ADR-076); у старого снимка — нет. */
  bank_accounts?: { id: string; name: string; default_card?: boolean; default_transfer?: boolean }[]
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

/** Тянет снимок с сервера; 304 оставляет прежний. Без сети молча пропускает. */
export async function refreshSnapshot(): Promise<Snapshot | null> {
  const current = await readSnapshot()
  try {
    const res = await fetch('/api/v1/offline/snapshot', {
      headers: current?.version ? { 'if-none-match': `"${current.version}"` } : {},
      credentials: 'same-origin',
    })
    if (res.status === 304) return current
    if (!res.ok) return current
    const snap = (await res.json()) as Snapshot
    snap.saved_at = new Date().toISOString()
    await (await db()).put(SNAP, snap, 'current')
    return snap
  } catch {
    return current
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

async function emit(): Promise<void> {
  const items = await allItems()
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

/** Один раз за запуск: повтор безопасен, сервер узнает чек по `op_id`. */
function recover(): Promise<void> {
  recovered ??= (async () => {
    try {
      const d = await db()
      for (const item of staleSending(await allItems())) await d.put(OUTBOX, item)
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
export async function enqueueSale(op_id: string, body: unknown): Promise<OutboxItem> {
  await recover()
  const item: OutboxItem = {
    op_id,
    temp_no: tempNumber(),
    created_at: new Date().toISOString(),
    status: 'sending',
    attempts: 0,
    last_error: '',
    body,
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
export function afterRejection(item: OutboxItem, error: string, reported: boolean): OutboxItem {
  if (reported) return { ...item, status: 'rejected', last_error: error }
  return {
    ...item,
    status: 'pending',
    attempts: item.attempts + 1,
    last_error: `${error} — отказ ещё не передан владельцу, повторим`,
  }
}

/** Проведён ли уже чек с этим op_id — например, от имени другого пользователя кассы. */
async function postedNumber(op_id: string): Promise<number | null> {
  try {
    return (await api<{ number: number }>('GET', `/sales/by-op/${op_id}`)).number
  } catch {
    return null
  }
}

/**
 * Отправляет накопившиеся чеки по одному. Повтор с тем же `op_id` безопасен:
 * сервер вернёт уже проведённый чек (ADR-013).
 */
export async function syncOutbox(): Promise<void> {
  if (syncing || !navigator.onLine) return
  syncing = true
  try {
    await recover()
    for (const item of await allItems()) {
      if (item.status !== 'pending') continue
      await put({ ...item, status: 'sending' })
      try {
        // Чек из очереди проведён без сети: продажа уже была, сервер не отклонит его за цену (ADR-042).
        const sale = await api<{ number: number }>('POST', '/sales', { ...(item.body as object), offline: true })
        await put({ ...item, status: 'done', server_number: sale.number, last_error: '' })
      } catch (e) {
        // «op_id уже использован»: чек мог уйти раньше от другого пользователя этой кассы —
        // тогда он проведён, а не отклонён.
        if (e instanceof ApiError && e.status === 409) {
          const number = await postedNumber(item.op_id)
          if (number !== null) {
            await put({ ...item, status: 'done', server_number: number, last_error: '' })
            continue
          }
        }
        const outcome = sendOutcome(e, item.conflicts ?? 0)
        if (outcome === 'conflict_retry') {
          await put({ ...item, status: 'pending', conflicts: (item.conflicts ?? 0) + 1, last_error: e instanceof Error ? e.message : '' })
          continue
        }
        if (outcome === 'reject' && e instanceof ApiError) {
          // Сервер отказал по существу: чек не исчезает, о нём узнаёт владелец.
          const reported = await api('POST', '/sales/offline-rejected', {
            op_id: item.op_id,
            error: e.message,
            body: item.body,
          }).then(
            () => true,
            () => false,
          )
          await put(afterRejection(item, e.message, reported))
          if (!reported) break
        } else {
          await put({
            ...item,
            status: 'pending',
            attempts: item.attempts + 1,
            last_error: e instanceof Error ? e.message : 'нет связи',
          })
          break
        }
      }
    }
  } finally {
    syncing = false
  }
}

/** Фоновая работа кассы: обновление снимка и отправка очереди. */
export function startOfflineLoop(): () => void {
  const tick = () => {
    void syncOutbox()
  }
  const refresh = () => {
    void refreshSnapshot()
  }
  refresh()
  tick()
  const t1 = setInterval(tick, 30_000)
  const t2 = setInterval(refresh, 10 * 60_000)
  window.addEventListener('online', tick)
  window.addEventListener('online', refresh)
  return () => {
    clearInterval(t1)
    clearInterval(t2)
    window.removeEventListener('online', tick)
    window.removeEventListener('online', refresh)
  }
}
