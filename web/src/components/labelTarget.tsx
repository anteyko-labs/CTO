// Куда печатать этикетки: «Здесь» — окно печати этого устройства, «На складе» — задание в
// очередь станции этикеток (задача 006, ADR-177). Выбор общий для всех экранов; плашка статуса
// задания — одна, внизу экрана.
import { useEffect, useState, useSyncExternalStore } from 'react'
import { get, newOpId, post, serverReachable, watchServerReachable } from '../lib/api'
import { defaultTarget, isPhoneDevice, pollJob, type LabelJob, type LabelTarget, type Plate } from '../lib/labelJob'
import type { LabelSettings } from '../lib/types'
import { printLabels, type LabelItem } from './labels'

type Listener = () => void

let target: LabelTarget | null = null
let chosen = false
let loading: Promise<void> | null = null
const targetListeners = new Set<Listener>()

function setTargetValue(t: LabelTarget, byUser: boolean) {
  if (!byUser && chosen) return
  if (byUser) chosen = true
  target = t
  for (const l of targetListeners) l()
}

/** Значение по умолчанию — из «Настройки → Этикетки» (на телефоне) или «Здесь» (компьютер). */
function ensureTarget(): void {
  if (target !== null || loading) return
  const phone = isPhoneDevice()
  target = defaultTarget(undefined, phone)
  if (!phone) return
  loading = get<LabelSettings>('/settings/labels')
    .then((s) => setTargetValue(defaultTarget(s.remote_default, phone), false))
    .catch(() => undefined)
}

export function currentTarget(): LabelTarget {
  ensureTarget()
  return target ?? 'here'
}

function useOnline(): boolean {
  const [online, setOnline] = useState(() => navigator.onLine && serverReachable())
  useEffect(() => {
    const upd = () => setOnline(navigator.onLine && serverReachable())
    window.addEventListener('online', upd)
    window.addEventListener('offline', upd)
    const off = watchServerReachable(upd)
    return () => {
      window.removeEventListener('online', upd)
      window.removeEventListener('offline', upd)
      off()
    }
  }, [])
  return online
}

/** Выбор «Здесь» / «На складе» рядом с кнопкой печати этикеток. */
export function LabelTargetSwitch() {
  const t = useSyncExternalStore(
    (l) => {
      targetListeners.add(l)
      return () => targetListeners.delete(l)
    },
    currentTarget,
  )
  const online = useOnline()
  const effective = online ? t : 'here'
  const btn = (value: LabelTarget, text: string, disabled = false) => (
    <button
      type="button"
      aria-pressed={effective === value}
      disabled={disabled}
      onClick={() => setTargetValue(value, true)}
      className={`min-h-[36px] px-3 text-sm ${effective === value ? 'bg-slate-800 text-white' : 'bg-white text-slate-700'} disabled:cursor-not-allowed disabled:opacity-50`}
    >
      {text}
    </button>
  )
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Куда отправить этикетки">
      <div className="inline-flex overflow-hidden rounded-md border border-slate-300">
        {btn('here', 'Здесь')}
        {btn('station', 'На складе', !online)}
      </div>
      {!online && <span className="text-xs text-slate-500">Без сети печать на складе недоступна — только «Здесь»</span>}
    </div>
  )
}

// ---------- Плашка статуса ----------

interface PlateState {
  id: string
  plate: Plate
  items: LabelItem[]
}

let plate: PlateState | null = null
let pollToken = 0
const plateListeners = new Set<Listener>()

function setPlate(p: PlateState | null) {
  plate = p
  for (const l of plateListeners) l()
}

function watchJob(id: string, items: LabelItem[]) {
  const token = ++pollToken
  setPlate({ id, items, plate: { kind: 'queued', text: 'В очереди…', final: false } })
  void pollJob(id, {
    fetchJob: (jid) => get<LabelJob>(`/label-jobs/${jid}`),
    onPlate: (p) => {
      if (token === pollToken) setPlate({ id, items, plate: p })
    },
    stopped: () => token !== pollToken,
  })
}

export async function sendToStation(items: LabelItem[]): Promise<number> {
  const printable = items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
  const count = printable.reduce((acc, i) => acc + i.copies, 0)
  if (count === 0) return 0
  const job = await post<{ id: string; status: string }>('/label-jobs', {
    op_id: newOpId(),
    items: printable.map((i) => ({ product_id: i.product.id, copies: i.copies })),
  })
  watchJob(job.id, printable)
  return count
}

/** Печать по выбору «Здесь» / «На складе». Возвращает число этикеток. */
export async function sendLabels(items: LabelItem[]): Promise<number> {
  const online = navigator.onLine && serverReachable()
  if (currentTarget() === 'station' && online) return sendToStation(items)
  return printLabels(items)
}

const PLATE_STYLE: Record<Plate['kind'], string> = {
  queued: 'border-sky-200 bg-sky-50 text-sky-900',
  printing: 'border-sky-200 bg-sky-50 text-sky-900',
  done: 'border-emerald-200 bg-emerald-50 text-emerald-900',
  failed: 'border-red-200 bg-red-50 text-red-900',
  offline: 'border-amber-200 bg-amber-50 text-amber-900',
  timeout: 'border-amber-200 bg-amber-50 text-amber-900',
}

/** Плашка «В очереди → Печатается → Напечатано» внизу экрана. */
export function LabelJobPlate() {
  const p = useSyncExternalStore(
    (l) => {
      plateListeners.add(l)
      return () => plateListeners.delete(l)
    },
    () => plate,
  )
  if (!p) return null
  const close = () => {
    pollToken++
    setPlate(null)
  }
  return (
    <div className="fixed inset-x-2 bottom-20 z-50 md:bottom-2 flex justify-center print:hidden" role="status" aria-label="Печать на складе">
      <div className={`flex max-w-md flex-1 flex-wrap items-center gap-2 rounded-md border px-3 py-2 text-sm shadow ${PLATE_STYLE[p.plate.kind]}`}>
        <span className="min-w-0 flex-1">
          <span className="font-medium">Склад: </span>
          {p.plate.text}
        </span>
        {p.plate.kind === 'failed' && (
          <button type="button" className="rounded border border-current px-2 py-1" onClick={() => void sendToStation(p.items).catch(() => undefined)}>
            Повторить
          </button>
        )}
        <button type="button" className="px-2 py-1" aria-label="Закрыть" onClick={close}>
          ✕
        </button>
      </div>
    </div>
  )
}
