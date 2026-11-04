// Незавершённые приход и оприходование (задача 030, ADR-190): поля формы — на сервере, как у
// ревизии, и копия на устройстве, пока сервер не подтвердил запись: обрыв связи и закрытая вкладка
// ввод не стирают. Суммы здесь не считаются — поля хранятся как введены, считает проведение (I1).
import { ApiError, deviceId, noServerAnswer, put } from './api'
import { branchHeaders } from './branch'
import type { Product } from './types'

export type EntryKind = 'receipt' | 'stock_in'

export interface EntryDraftLine {
  product_id: string
  /** Название на случай, если товара больше нет: чтобы сказать, что убрано. */
  name: string
  count: string
  mode?: 'can' | 'l'
  price: string
  markup: string
  salePrice: string
}

export interface EntryDraftBody {
  supplierId: string
  supplierDoc: string
  comment: string
  payMode: 'paid' | 'part' | 'debt'
  partText: string
  paidFrom: string
  lines: EntryDraftLine[]
}

export interface EntryDraft {
  kind: EntryKind
  op_id: string
  seq: number
  body: EntryDraftBody
}

export type EntrySaveStatus = 'idle' | 'saving' | 'saved' | 'offline' | 'failed'

export const entrySaveText: Record<EntrySaveStatus, string> = {
  idle: '',
  saving: 'Сохраняется…',
  saved: 'Сохранено',
  offline: 'Нет связи — сохранено на этом устройстве',
  failed: 'Не удалось сохранить на сервере — копия на этом устройстве',
}

/** Форма пуста — хранить нечего. */
export const entryEmpty = (b: EntryDraftBody): boolean =>
  b.lines.length === 0 && !b.comment.trim() && !b.supplierDoc.trim() && !b.supplierId && !b.partText.trim()

const localKey = (user: string, branch: string, kind: EntryKind) => `avtodom.entryDraft.${user}.${branch}.${kind}`

export const entryStorageKey = (user: string, kind: EntryKind): string =>
  localKey(user, branchHeaders()['X-Branch-Id'] ?? 'own', kind)

export function readLocalDraft(key: string): EntryDraft | null {
  try {
    const text = localStorage.getItem(key)
    return text ? (JSON.parse(text) as EntryDraft) : null
  } catch {
    return null
  }
}

export function clearLocalDraft(key: string): void {
  try {
    localStorage.removeItem(key)
  } catch {
    // Без хранилища копии и не было.
  }
}

function writeLocalDraft(key: string, d: EntryDraft): void {
  try {
    localStorage.setItem(key, JSON.stringify(d))
  } catch {
    // Хранилище недоступно или полно: остаётся запись на сервер.
  }
}

/** Что продолжать: копия устройства, если сервер её ещё не получил, иначе черновик сервера. */
export const pickDraft = (server: EntryDraft | null, local: EntryDraft | null): EntryDraft | null =>
  local && (!server || local.seq > server.seq) ? local : server

/** Строки черновика с текущими карточками: товара нет или он в архиве — строка убирается, с названием. */
export async function resolveLines(
  lines: EntryDraftLine[],
  load: (id: string) => Promise<Product>,
): Promise<{ lines: { line: EntryDraftLine; product: Product }[]; removed: string[] }> {
  const found = await Promise.all(
    lines.map(async (line) => {
      try {
        const product = await load(line.product_id)
        return product.archived ? null : { line, product }
      } catch (e) {
        // Сбой связи — не повод терять строку: экран покажет ошибку и черновик не перезапишет.
        if (e instanceof ApiError && e.status === 404) return null
        throw e
      }
    }),
  )
  return {
    lines: found.filter((f) => f !== null),
    removed: lines.filter((_, i) => found[i] === null).map((l) => l.name),
  }
}

const sendDraft = (d: EntryDraft) => put(`/entry-drafts/${d.kind}`, { op_id: d.op_id, seq: d.seq, body: d.body })

/** Запись при закрытии вкладки: обычный запрос браузер оборвёт. Не дошла — останется копия устройства. */
function sendOnClose(d: EntryDraft): void {
  void fetch(`/api/v1/entry-drafts/${d.kind}`, {
    method: 'PUT',
    keepalive: true,
    credentials: 'same-origin',
    headers: { 'X-Device-Id': deviceId, ...branchHeaders(), 'Content-Type': 'application/json' },
    body: JSON.stringify({ op_id: d.op_id, seq: d.seq, body: d.body }),
  }).catch(() => {})
}

export interface EntrySaverOptions {
  storageKey: (kind: EntryKind) => string
  onStatus: (s: EntrySaveStatus) => void
  /** Документ с этим `op_id` уже проведён (ответ на него потерялся): черновик устарел. */
  onPosted: () => void
  send?: (d: EntryDraft) => Promise<unknown>
  sendClosing?: (d: EntryDraft) => void
  delayMs?: number
  retryMs?: number
}

/** Запись по очереди: новая не обгоняет старую, ответы приходят по порядку. */
export function createEntrySaver(o: EntrySaverOptions) {
  const send = o.send ?? sendDraft
  const sendClosing = o.sendClosing ?? sendOnClose
  const delay = o.delayMs ?? 1000
  const retry = o.retryMs ?? 5000
  let pending: EntryDraft | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let chain: Promise<unknown> = Promise.resolve()
  let lastSeq = 0
  let stopped = false

  const clearTimer = () => {
    if (timer) clearTimeout(timer)
    timer = null
  }

  const flush = (): Promise<unknown> => {
    clearTimer()
    const item = pending
    if (!item || stopped) return chain
    pending = null
    chain = chain.then(async () => {
      if (stopped) return
      const key = o.storageKey(item.kind)
      try {
        await send(item)
        if (stopped) return
        if (readLocalDraft(key)?.seq === item.seq) clearLocalDraft(key)
        if (!pending) o.onStatus('saved')
      } catch (e) {
        if (stopped) return
        if (e instanceof ApiError && e.status === 409) {
          pending = null
          clearLocalDraft(key)
          o.onPosted()
        } else if (noServerAnswer(e)) {
          pending ??= item
          o.onStatus('offline')
          clearTimer()
          timer = setTimeout(() => void flush(), retry)
        } else {
          o.onStatus('failed')
        }
      }
    })
    return chain
  }

  return {
    /** Форма изменилась: копия устройства — сразу, сервер — чуть позже. */
    change(kind: EntryKind, opId: string, body: EntryDraftBody) {
      if (stopped) return
      lastSeq = Math.max(Date.now(), lastSeq + 1)
      pending = { kind, op_id: opId, seq: lastSeq, body }
      writeLocalDraft(o.storageKey(kind), pending)
      o.onStatus('saving')
      clearTimer()
      timer = setTimeout(() => void flush(), delay)
    },
    /** Записать сейчас: уход с экрана, смена вида. */
    flush,
    /** Закрытие вкладки: запрос, который браузер не оборвёт. Копия остаётся до подтверждения. */
    closing() {
      if (pending && !stopped) sendClosing(pending)
    },
    /** Убрать черновик вида: «Начать заново», перенос на другой вид, пустая форма. */
    discard(kind: EntryKind, remove: () => Promise<unknown>): Promise<unknown> {
      clearTimer()
      if (pending?.kind === kind) pending = null
      clearLocalDraft(o.storageKey(kind))
      o.onStatus('idle')
      chain = chain.then(remove).catch(() => {})
      return chain
    },
    /** Документ проведён: больше ничего не писать (черновик сервер убрал в той же транзакции). */
    finish(kind: EntryKind) {
      stopped = true
      clearTimer()
      pending = null
      clearLocalDraft(o.storageKey(kind))
    },
  }
}

export type EntrySaver = ReturnType<typeof createEntrySaver>
