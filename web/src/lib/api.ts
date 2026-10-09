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

/** Новый идентификатор операции для защиты от повторной отправки. */
export const newOpId = (): string => crypto.randomUUID()

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

export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response
  try {
    res = await fetch(`/api/v1${path}`, {
      method,
      credentials: 'same-origin',
      headers: {
        'X-Device-Id': deviceId,
        ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
      },
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
  } catch {
    throw new ApiError(0, 'network', 'Нет связи с сервером')
  }
  let text: string
  try {
    text = await res.text()
  } catch {
    throw new ApiError(0, 'network', 'Нет связи с сервером')
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
  return data as T
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
