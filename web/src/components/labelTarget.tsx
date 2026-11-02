// Печать этикеток — только на складе (задача 014): задание в очередь станции этикеток (задача 006,
// ADR-177). Две кнопки — «Большой штрихкод» (прежний принтер) и «Маленький штрихкод» (30×20 мм);
// плашка статуса задания — одна, внизу экрана.
import { useEffect, useState, useSyncExternalStore } from 'react'
import { get, newOpId, post, serverReachable, watchServerReachable } from '../lib/api'
import { pollJob, SIZE_BUTTON, type LabelJob, type LabelSize, type Plate } from '../lib/labelJob'
import type { LabelItem } from './labels'
import { Button } from './ui'

type Listener = () => void

export const OFFLINE_HINT = 'Печать — только на складе, нужна сеть'

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

/** Кнопки «Большой штрихкод» / «Маленький штрихкод»; без сети недоступны с подсказкой. */
export function LabelSizeButtons({
  onPrint,
  disabled = false,
  suffix = '',
  variant = 'secondary',
}: {
  onPrint: (size: LabelSize) => void
  disabled?: boolean
  /** Дописать к подписи, например число этикеток. */
  suffix?: string
  variant?: 'primary' | 'secondary'
}) {
  const online = useOnline()
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Печать этикеток на складе">
      {(['big', 'small'] as LabelSize[]).map((size) => (
        <Button key={size} variant={variant} disabled={disabled || !online} onClick={() => onPrint(size)} data-label-size={size}>
          {SIZE_BUTTON[size]}
          {suffix}
        </Button>
      ))}
      {!online && <span className="text-xs text-slate-500">{OFFLINE_HINT}</span>}
    </div>
  )
}

// ---------- Плашка статуса ----------

interface PlateState {
  id: string
  plate: Plate
  items: LabelItem[]
  size: LabelSize
}

let plate: PlateState | null = null
let pollToken = 0
const plateListeners = new Set<Listener>()

function setPlate(p: PlateState | null) {
  plate = p
  for (const l of plateListeners) l()
}

function watchJob(id: string, items: LabelItem[], size: LabelSize) {
  const token = ++pollToken
  setPlate({ id, items, size, plate: { kind: 'queued', text: 'В очереди…', final: false } })
  void pollJob(id, {
    fetchJob: (jid) => get<LabelJob>(`/label-jobs/${jid}`),
    onPlate: (p) => {
      if (token === pollToken) setPlate({ id, items, size, plate: p })
    },
    stopped: () => token !== pollToken,
  })
}

/** Задание на склад: принтер по размеру. Без сети — ошибка с подсказкой. Возвращает число этикеток. */
export async function sendToStation(items: LabelItem[], size: LabelSize): Promise<number> {
  if (!(navigator.onLine && serverReachable())) throw new Error(OFFLINE_HINT)
  const printable = items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
  const count = printable.reduce((acc, i) => acc + i.copies, 0)
  if (count === 0) return 0
  const job = await post<{ id: string; status: string }>('/label-jobs', {
    op_id: newOpId(),
    size,
    items: printable.map((i) => ({ product_id: i.product.id, copies: i.copies })),
  })
  watchJob(job.id, printable, size)
  return count
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
          <span className="font-medium">Склад ({p.size === 'small' ? 'маленький' : 'большой'}): </span>
          {p.plate.text}
        </span>
        {p.plate.kind === 'failed' && (
          <button type="button" className="rounded border border-current px-2 py-1" onClick={() => void sendToStation(p.items, p.size).catch(() => undefined)}>
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
