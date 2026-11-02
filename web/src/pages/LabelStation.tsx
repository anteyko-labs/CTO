// Станция этикеток на ПК склада (задача 006, ADR-177): раз в 3 с забирает задание из очереди
// филиала и печатает его на принтер по умолчанию. Chrome с --kiosk-printing печатает без окна.
import { useEffect, useRef, useState } from 'react'
import { labelsHtml, type LabelItem } from '../components/labels'
import { Button, Card, CardTitle, ErrorBox, PageHeader } from '../components/ui'
import { get, post, uuidv7 } from '../lib/api'
import { STATION_TICK_MS, stationId, type LabelJob } from '../lib/labelJob'
import { printInFrame } from '../lib/printFrame'
import type { LabelSettings, Product } from '../lib/types'

interface Taken {
  id: string
  copies: number
  created_by_name: string
  items: LabelItem[]
}

const STATUS: Record<LabelJob['status'], string> = {
  queued: 'в очереди',
  printing: 'печатается',
  done: 'напечатано',
  failed: 'ошибка',
}

const SAMPLE: Product = {
  id: 'sample',
  category_id: 'sample',
  category_kind: 'other',
  name: 'Пробная этикетка',
  brand: null,
  article: null,
  unit: 'piece',
  container_ml: null,
  attrs: {},
  archived: false,
  barcodes: ['2200000000019'],
  sale_price_tyiyn: 100,
  pour_price_per_l_tyiyn: null,
  min_stock: 0,
  stock_qty: 0,
  needs_review: false,
  last_purchase_price_tyiyn: null,
} as unknown as Product

const time = (iso: string) => new Date(iso).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' })

export default function LabelStation() {
  const [jobs, setJobs] = useState<LabelJob[]>([])
  const [error, setError] = useState<string | null>(null)
  const [last, setLast] = useState<string | null>(null)
  const busy = useRef(false)
  const station = useRef<string>('')
  if (!station.current) station.current = stationId(localStorage, uuidv7)

  useEffect(() => {
    let alive = true
    const tick = async () => {
      if (busy.current) return
      busy.current = true
      try {
        const res = await post<{ job: Taken | null }>('/label-jobs/take', { station_id: station.current })
        const job = res.job
        if (job) {
          try {
            const settings = await get<LabelSettings>('/settings/labels')
            const items = job.items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
            if (items.length === 0) throw new Error('у товаров нет штрихкода')
            printInFrame(labelsHtml(items, settings))
            await post(`/label-jobs/${job.id}/done`, { station_id: station.current })
            setLast(`${job.created_by_name}: ${job.copies} шт.`)
          } catch (e) {
            await post(`/label-jobs/${job.id}/failed`, { station_id: station.current, error: e instanceof Error ? e.message : String(e) })
          }
        }
        const list = await get<{ jobs: LabelJob[] }>('/label-jobs')
        if (alive) {
          setJobs(list.jobs)
          setError(null)
        }
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e))
      } finally {
        busy.current = false
      }
    }
    void tick()
    const t = setInterval(() => void tick(), STATION_TICK_MS)
    return () => {
      alive = false
      clearInterval(t)
    }
  }, [])

  const test = async () => {
    try {
      const settings = await get<LabelSettings>('/settings/labels')
      printInFrame(labelsHtml([{ product: SAMPLE, copies: 1 }], settings))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div>
      <PageHeader title="Станция этикеток" />
      <Card className="mb-4">
        <div className="flex flex-wrap items-center gap-3">
          <span className={`text-lg font-semibold ${error ? 'text-red-700' : 'text-emerald-700'}`}>{error ? 'Принтер: нет связи с сервером' : 'Принтер: готов'}</span>
          <Button variant="secondary" onClick={() => void test()}>
            Пробная этикетка
          </Button>
        </div>
        {last && <div className="mt-2 text-sm text-slate-600">Последнее: {last}</div>}
        <ErrorBox error={error} />
        <p className="mt-2 text-xs text-slate-500">Держите эту страницу открытой: задания с телефонов печатаются сами.</p>
      </Card>
      <Card>
        <CardTitle>Последние задания</CardTitle>
        {jobs.length === 0 ? (
          <div className="text-sm text-slate-500">Заданий пока нет</div>
        ) : (
          <ul className="flex flex-col divide-y divide-slate-100 text-sm" aria-label="Задания печати">
            {jobs.map((j) => (
              <li key={j.id} className="flex flex-wrap gap-x-3 py-1.5">
                <span className="text-slate-500">{time(j.created_at)}</span>
                <span>{j.created_by_name}</span>
                <span className="min-w-0 flex-1">{j.what}</span>
                <span>{j.copies} шт.</span>
                <span className={j.status === 'failed' ? 'text-red-700' : j.status === 'done' ? 'text-emerald-700' : 'text-sky-700'}>
                  {STATUS[j.status]}
                  {j.error ? `: ${j.error}` : ''}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  )
}
