// Печать этикеток — только на складе (задача 014): задание в очередь станции этикеток (задача 006,
// ADR-177). Две кнопки — «Большой штрихкод» (прежний принтер) и «Маленький штрихкод» (30×20 мм);
// плашка статуса задания — одна, внизу экрана. Задача 021: над кнопками — «Цена: за шт · за литр ·
// за метр», по умолчанию — из «Настройки → Этикетки».
import { useEffect, useState, useSyncExternalStore } from 'react'
import { get, newOpId, post, serverReachable, watchServerReachable } from '../lib/api'
import { pollJob, SIZE_BUTTON, type LabelJob, type LabelSize, type Plate } from '../lib/labelJob'
import { isPricePer, PRICE_PER, type PricePer } from '../lib/labelText'
import type { LabelSettings } from '../lib/types'
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

// ---------- За что цена (задача 021) ----------

let defaultPer: Promise<PricePer> | null = null

/** Цена на этикетке по умолчанию — из настроек филиала; один запрос на вкладку. */
function loadDefaultPer(): Promise<PricePer> {
  defaultPer ??= get<LabelSettings>('/settings/labels')
    .then((s) => (isPricePer(s.price_per_default) ? s.price_per_default : 'piece'))
    .catch(() => {
      defaultPer = null
      return 'piece' as const
    })
  return defaultPer
}

/** Настройку поменяли — следующий выбор при печати откроется с новой. */
export function forgetDefaultPer() {
  defaultPer = null
}

/** Выбор «за что цена» при печати: сначала «за шт», затем — настройка, если ещё не меняли руками. */
export function usePricePer(): [PricePer, (v: PricePer) => void] {
  const [per, setPer] = useState<PricePer>('piece')
  const [touched, setTouched] = useState(false)
  useEffect(() => {
    let alive = true
    void loadDefaultPer().then((v) => {
      if (alive && !touched) setPer(v)
    })
    return () => {
      alive = false
    }
  }, [touched])
  return [
    per,
    (v) => {
      setTouched(true)
      setPer(v)
    },
  ]
}

/** «Цена: за шт · за литр · за метр» — выбор не подходит товару, он печатается по своей единице. */
export function PricePerSwitch({ value, onChange }: { value: PricePer; onChange: (v: PricePer) => void }) {
  return (
    <div className="flex flex-wrap items-center gap-2 text-sm" role="group" aria-label="Цена на этикетке">
      <span className="text-slate-600">Цена:</span>
      {(['piece', 'liter', 'meter'] as PricePer[]).map((k) => (
        <button
          key={k}
          type="button"
          aria-pressed={value === k}
          data-price-per={k}
          className={`min-h-[40px] rounded-md border px-3 py-1 ${value === k ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
          onClick={() => onChange(k)}
        >
          {PRICE_PER[k]}
        </button>
      ))}
    </div>
  )
}

/** Выбор цены и кнопки «Большой штрихкод» / «Маленький штрихкод»; без сети недоступны с подсказкой. */
export function LabelSizeButtons({
  onPrint,
  disabled = false,
  suffix = '',
  variant = 'secondary',
}: {
  onPrint: (size: LabelSize, per: PricePer) => void
  disabled?: boolean
  /** Дописать к подписи, например число этикеток. */
  suffix?: string
  variant?: 'primary' | 'secondary'
}) {
  const online = useOnline()
  const [per, setPer] = usePricePer()
  return (
    <div className="flex flex-col gap-2">
      <PricePerSwitch value={per} onChange={setPer} />
      <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Печать этикеток на складе">
        {(['big', 'small'] as LabelSize[]).map((size) => (
          <Button key={size} variant={variant} disabled={disabled || !online} onClick={() => onPrint(size, per)} data-label-size={size}>
            {SIZE_BUTTON[size]}
            {suffix}
          </Button>
        ))}
        {!online && <span className="text-xs text-slate-500">{OFFLINE_HINT}</span>}
      </div>
    </div>
  )
}

// ---------- Плашка статуса ----------

interface PlateState {
  id: string
  plate: Plate
  items: LabelItem[]
  size: LabelSize
  per: PricePer
}

let plate: PlateState | null = null
let pollToken = 0
const plateListeners = new Set<Listener>()

function setPlate(p: PlateState | null) {
  plate = p
  for (const l of plateListeners) l()
}

function watchJob(id: string, items: LabelItem[], size: LabelSize, per: PricePer) {
  const token = ++pollToken
  setPlate({ id, items, size, per, plate: { kind: 'queued', text: 'В очереди…', final: false } })
  void pollJob(id, {
    fetchJob: (jid) => get<LabelJob>(`/label-jobs/${jid}`),
    onPlate: (p) => {
      if (token === pollToken) setPlate({ id, items, size, per, plate: p })
    },
    stopped: () => token !== pollToken,
  })
}

/** Задание на склад: принтер по размеру, цена — `per`. Без сети — ошибка с подсказкой. Возвращает число этикеток. */
export async function sendToStation(items: LabelItem[], size: LabelSize, per: PricePer = 'piece'): Promise<number> {
  if (!(navigator.onLine && serverReachable())) throw new Error(OFFLINE_HINT)
  const printable = items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
  const count = printable.reduce((acc, i) => acc + i.copies, 0)
  if (count === 0) return 0
  const job = await post<{ id: string; status: string }>('/label-jobs', {
    op_id: newOpId(),
    size,
    price_per: per,
    items: printable.map((i) => ({ product_id: i.product.id, copies: i.copies })),
  })
  watchJob(job.id, printable, size, per)
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
          <button type="button" className="rounded border border-current px-2 py-1" onClick={() => void sendToStation(p.items, p.size, p.per).catch(() => undefined)}>
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
