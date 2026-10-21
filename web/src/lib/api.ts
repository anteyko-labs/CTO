// Обращение к серверу: устройство, сессия, единый формат ошибок.

const DEVICE_KEY = 'avtodom.device_id'

function readDeviceId(): string {
  try {
    const saved = localStorage.getItem(DEVICE_KEY)
    if (saved) return saved
    const id = crypto.randomUUID()
    localStorage.setItem(DEVICE_KEY, id)
    return id
  } catch {
    return crypto.randomUUID()
  }
}

export const deviceId = readDeviceId()

/**
 * UUID версии 7: первые 48 бит — время в миллисекундах, остальное случайно. Идентификаторы
 * операций с устройства идут по времени, как и созданные на сервере (стек: UUID v7).
 */
export function uuidv7(now: number = Date.now()): string {
  const b = new Uint8Array(16)
  crypto.getRandomValues(b)
  let ms = Math.max(0, Math.floor(now))
  for (let i = 5; i >= 0; i--) {
    b[i] = ms % 256
    ms = Math.floor(ms / 256)
  }
  b[6] = (b[6] & 0x0f) | 0x70
  b[8] = (b[8] & 0x3f) | 0x80
  const h = Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('')
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`
}

/** Новый идентификатор операции для защиты от повторной отправки. */
export const newOpId = (): string => uuidv7()

// ---------- Связь с сервером ----------

let reachable = true
const reachListeners = new Set<(up: boolean) => void>()

/** Сервер отвечает: по последнему запросу, а не по `navigator.onLine` — сеть может быть, а сервера нет. */
export const serverReachable = (): boolean => reachable

export function setServerReachable(up: boolean): void {
  if (up === reachable) return
  reachable = up
  for (const l of reachListeners) l(up)
}

export function watchServerReachable(fn: (up: boolean) => void): () => void {
  reachListeners.add(fn)
  return () => {
    reachListeners.delete(fn)
  }
}

export class ApiError extends Error {
  readonly status: number
  readonly code: string

  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

let onUnauthorized: () => void = () => {}

export function setUnauthorizedHandler(fn: () => void): void {
  onUnauthorized = fn
}

/** Текст ошибки, когда сервер не объяснил её сам. */
function statusMessage(status: number): string {
  if ([502, 503, 504, 530].includes(status)) return 'Сервер недоступен, попробуйте позже'
  return `Ошибка ${status}`
}

/** Ответа сервера нет: обрыв, таймаут или прокси без сервера (502/503/504). Такой отказ
 *  ничего не говорит о чеке — его нельзя считать отклонённым, он остаётся в очереди. */
export function noServerAnswer(e: unknown): boolean {
  return e instanceof ApiError && (e.status === 0 || [502, 503, 504, 530].includes(e.status) || e.code === 'bad_response')
}

/** Сколько ждём ответа: дольше — считаем, что связи нет (касса уходит в офлайн). */
const TIMEOUT_MS = 20_000

export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  return (await request<T>(method, path, body)).data
}

async function request<T>(method: string, path: string, body?: unknown): Promise<{ data: T; headers: Headers }> {
  let res: Response
  const abort = new AbortController()
  const timer = setTimeout(() => abort.abort(), TIMEOUT_MS)
  try {
    res = await fetch(`/api/v1${path}`, {
      method,
      credentials: 'same-origin',
      signal: abort.signal,
      headers: {
        'X-Device-Id': deviceId,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
  } catch {
    clearTimeout(timer)
    setServerReachable(false)
    throw new ApiError(0, 'network', 'Нет связи с сервером')
  }
  setServerReachable(![502, 503, 504, 530].includes(res.status))
  let text: string
  try {
    text = await res.text()
  } catch {
    throw new ApiError(0, 'network', 'Нет связи с сервером')
  } finally {
    clearTimeout(timer)
  }
  // Не JSON приходит, когда отвечает не наш сервер: страница ошибки туннеля или прокси.
  let data: unknown = null
  let parsed = true
  try {
    data = text ? JSON.parse(text) : null
  } catch {
    parsed = false
  }
  if (!res.ok) {
    if (res.status === 401 && !path.startsWith('/auth/')) onUnauthorized()
    const err = (data as { error?: { code?: string; message?: string } } | null)?.error
    throw new ApiError(res.status, err?.code ?? 'error', err?.message ?? statusMessage(res.status))
  }
  if (!parsed) throw new ApiError(res.status, 'bad_response', 'Сервер прислал непонятный ответ, попробуйте ещё раз')
  return { data: data as T, headers: res.headers }
}

/** Страница списка (SPEC-28 §2): строки и сколько их во всём отборе (заголовок `X-Total-Count`). */
export interface Page<T> {
  items: T[]
  total: number
}

/** Страница длинного списка: `limit`/`offset` добавляются к адресу, число строк — из заголовка ответа. */
export async function getPage<T>(path: string, page: { limit: number; offset: number }): Promise<Page<T>> {
  const sep = path.includes('?') ? '&' : '?'
  const { data, headers } = await request<T[]>('GET', `${path}${sep}limit=${page.limit}&offset=${page.offset}`)
  const items = Array.isArray(data) ? data : []
  const total = Number(headers.get('X-Total-Count'))
  return { items, total: Number.isSafeInteger(total) && total >= 0 ? total : items.length }
}

export const get = <T>(path: string) => api<T>('GET', path)
export const post = <T>(path: string, body: unknown = {}) => api<T>('POST', path, body)
export const patch = <T>(path: string, body: unknown) => api<T>('PATCH', path, body)
export const put = <T>(path: string, body: unknown) => api<T>('PUT', path, body)
export const del = <T>(path: string) => api<T>('DELETE', path)

/** Строка запроса без пустых параметров. */
export function qs(params: Record<string, string | number | boolean | undefined | null>): string {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== '') p.set(k, String(v))
  }
  const s = p.toString()
  return s ? `?${s}` : ''
}

/**
 * Файл телом запроса как есть (фото накладной, SPEC-29 §3): тип — в заголовке, повтор той же
 * загрузки сервер узнаёт по `op_id` в адресе. Ответ — JSON, ошибки — как у остальных запросов.
 */
export async function upload<T>(path: string, data: Blob, contentType: string, timeoutMs = 120_000): Promise<T> {
  let res: Response
  const abort = new AbortController()
  const timer = setTimeout(() => abort.abort(), timeoutMs)
  try {
    res = await fetch(`/api/v1${path}`, {
      method: 'POST',
      credentials: 'same-origin',
      signal: abort.signal,
      headers: { 'X-Device-Id': deviceId, 'Content-Type': contentType },
      body: data,
    })
  } catch {
    clearTimeout(timer)
    throw new ApiError(0, 'network', 'Нет связи с сервером')
  }
  clearTimeout(timer)
  const text = await res.text().catch(() => '')
  let body: unknown = null
  try {
    body = text ? JSON.parse(text) : null
  } catch {
    body = null
  }
  if (!res.ok) {
    if (res.status === 401) onUnauthorized()
    const err = (body as { error?: { code?: string; message?: string } } | null)?.error
    throw new ApiError(res.status, err?.code ?? 'error', err?.message ?? (res.status === 413 ? 'Файл слишком большой' : statusMessage(res.status)))
  }
  return body as T
}
