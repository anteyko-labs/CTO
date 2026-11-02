// Печать этикеток на складском принтере через очередь (задача 006, ADR-177): выбор «Здесь» /
// «На складе», плашка статуса задания и опрос сервера до итога.

export type LabelTarget = 'here' | 'station'

export type JobStatus = 'queued' | 'printing' | 'done' | 'failed'

/** Ответ `GET /label-jobs/{id}`. */
export interface LabelJob {
  id: string
  status: JobStatus
  error: string | null
  copies: number
  created_by_name: string
  what: string
  created_at: string
  done_at: string | null
  station_online: boolean
}

/** Опрос статуса — раз в 2 с, не дольше 2 мин. */
export const POLL_MS = 2_000
export const POLL_MAX_MS = 120_000
/** Станция забирает задания раз в 3 с. */
export const STATION_TICK_MS = 3_000
/** Копий в одном задании не больше (сервер отвечает 422). */
export const MAX_JOB_COPIES = 500

/** Телефон — узкий экран или сенсорный ввод без мыши. */
export function isPhoneDevice(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  return window.matchMedia('(max-width: 640px)').matches || window.matchMedia('(pointer: coarse)').matches
}

/** Кнопка по умолчанию: на телефоне — из настройки (по умолчанию «На складе»), на компьютере — «Здесь». */
export function defaultTarget(remoteDefault: string | undefined, phone: boolean): LabelTarget {
  if (!phone) return 'here'
  return remoteDefault === 'here' ? 'here' : 'station'
}

export type PlateKind = 'queued' | 'printing' | 'done' | 'failed' | 'offline' | 'timeout'

export interface Plate {
  kind: PlateKind
  text: string
  /** Итог: опрос останавливается. */
  final: boolean
}

const labelsWord = (n: number): string => {
  const d = n % 10
  const dd = n % 100
  if (d === 1 && dd !== 11) return 'этикетка'
  if (d >= 2 && d <= 4 && (dd < 12 || dd > 14)) return 'этикетки'
  return 'этикеток'
}

/** Плашка на телефоне по статусу задания и времени опроса. */
export function plateFor(job: Pick<LabelJob, 'status' | 'error' | 'copies' | 'station_online'>, elapsedMs: number): Plate {
  if (job.status === 'done') return { kind: 'done', text: `Напечатано (${job.copies} ${labelsWord(job.copies)})`, final: true }
  if (job.status === 'failed') return { kind: 'failed', text: `Не напечатано: ${job.error ?? 'ошибка печати'}`, final: true }
  if (job.status === 'queued' && !job.station_online)
    return { kind: 'offline', text: 'Склад не в сети: этикетки напечатаются, когда станция включится', final: elapsedMs >= POLL_MAX_MS }
  if (elapsedMs >= POLL_MAX_MS)
    return { kind: 'timeout', text: job.status === 'printing' ? 'Печатается — долго нет ответа станции' : 'В очереди — станция пока не взяла', final: true }
  if (job.status === 'printing') return { kind: 'printing', text: 'Печатается…', final: false }
  return { kind: 'queued', text: 'В очереди…', final: false }
}

export interface PollDeps {
  fetchJob: (id: string) => Promise<LabelJob>
  onPlate: (p: Plate, job: LabelJob | null) => void
  sleep?: (ms: number) => Promise<void>
  now?: () => number
  /** Остановить опрос снаружи (плашку закрыли). */
  stopped?: () => boolean
}

const defaultSleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))

/** Опрашивает задание до итога или 2 мин; ошибки сети не прерывают опрос. Возвращает последнюю плашку. */
export async function pollJob(id: string, deps: PollDeps): Promise<Plate | null> {
  const sleep = deps.sleep ?? defaultSleep
  const now = deps.now ?? Date.now
  const start = now()
  let last: Plate | null = null
  for (;;) {
    if (deps.stopped?.()) return last
    const elapsed = now() - start
    try {
      const job = await deps.fetchJob(id)
      last = plateFor(job, elapsed)
      deps.onPlate(last, job)
      if (last.final) return last
    } catch {
      if (elapsed >= POLL_MAX_MS) return last
    }
    await sleep(POLL_MS)
  }
}

/** Номер этой станции: один на браузер, хранится между перезапусками. */
export function stationId(storage: Pick<Storage, 'getItem' | 'setItem'>, make: () => string): string {
  const key = 'avtodom.labelStation.id'
  const have = storage.getItem(key)
  if (have) return have
  const id = make()
  storage.setItem(key, id)
  return id
}
