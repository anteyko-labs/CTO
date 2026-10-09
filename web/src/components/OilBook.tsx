// Масляная книжка: замены по машинам, следующая замена, правка пробега и интервала (SPEC-16).
import { useState } from 'react'
import { get, patch, post } from '../lib/api'
import { formatDate, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, toast } from './ui'

export interface OilRecord {
  id: string
  change_date: string
  mileage_km: number | null
  oil_text: string
  filter_text: string
  comment: string
  sale_id: string | null
  sale_number: number | null
}

export interface VehicleBook {
  vehicle_id: string
  plate: string
  brand: string
  model: string
  interval_km: number
  interval_days: number
  next_km: number | null
  next_date: string | null
  records: OilRecord[]
}

const km = (v: number) => `${v.toLocaleString('ru-RU').replace(/ /g, ' ')} км`

/** Следующая замена одной строкой: «на 93 000 км или до 12.04.2027». */
export function nextText(b: VehicleBook): string {
  const parts = [b.next_km !== null ? `на ${km(b.next_km)}` : null, b.next_date ? `до ${formatDate(b.next_date)}` : null].filter(Boolean)
  return parts.length ? parts.join(' или ') : 'пока не с чего считать'
}

/** Касса: под выбранной машиной — прошлая и следующая замена. */
export function OilHint({ vehicleId }: { vehicleId: string }) {
  const book = useLoad(() => get<VehicleBook>(`/vehicles/${vehicleId}/oil-book`), [vehicleId])
  const b = book.data
  if (!b) return null
  const last = b.records[0]
  if (!last) return <div className="text-xs text-slate-500">В масляной книжке этой машины пока пусто.</div>
  const overdue = b.next_date !== null && b.next_date < todayBishkek()
  return (
    <div className={`rounded-md px-3 py-2 text-xs ${overdue ? 'bg-amber-50 text-amber-900' : 'bg-slate-50 text-slate-700'}`}>
      Прошлая замена {formatDate(last.change_date)}: {last.oil_text || last.filter_text || '—'}
      {last.mileage_km !== null && `, ${km(last.mileage_km)}`}. Следующая {nextText(b)}.
    </div>
  )
}

function VehicleCard({ b, editable, onChanged }: { b: VehicleBook; editable: boolean; onChanged: () => void }) {
  const [interval, setInterval] = useState<{ km: string; days: string } | null>(null)
  const [manual, setManual] = useState<{ date: string; mileage: string; oil: string; filter: string } | null>(null)
  const act = useAction()
  return (
    <Card className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <span className="font-semibold">{b.plate}</span>
          {(b.brand || b.model) && <span className="ml-2 text-slate-500">{[b.brand, b.model].filter(Boolean).join(' ')}</span>}
        </div>
        <div className="text-sm">
          Следующая замена: <b>{nextText(b)}</b>
          <span className="ml-2 text-xs text-slate-500">
            (каждые {km(b.interval_km)} или {b.interval_days} дн.)
          </span>
        </div>
      </div>
      {b.records.length === 0 ? (
        <div className="text-sm text-slate-500">Замен ещё не было</div>
      ) : (
        <ul className="divide-y divide-slate-100 text-sm">
          {b.records.map((r) => (
            <li key={r.id} className="flex flex-wrap items-baseline justify-between gap-2 py-1.5">
              <span>
                <span className="font-medium">{formatDate(r.change_date)}</span>
                {r.mileage_km !== null && <span className="ml-2">{km(r.mileage_km)}</span>}
                <span className="ml-2 text-slate-600">{[r.oil_text, r.filter_text].filter(Boolean).join(' · ')}</span>
                {r.comment && <span className="ml-2 text-slate-500">— {r.comment}</span>}
                {r.sale_number === null && (
                  <span className="ml-2">
                    <Badge tone="slate">из тетради</Badge>
                  </span>
                )}
              </span>
              {editable && (
                <Button
                  variant="ghost"
                  className="px-2 py-1 text-xs"
                  onClick={() => {
                    const v = window.prompt('Пробег, км', r.mileage_km !== null ? String(r.mileage_km) : '')
                    if (v === null || !/^\d+$/.test(v.trim())) return
                    void act.run(async () => {
                      await patch(`/oil-changes/${r.id}`, { mileage_km: Number(v.trim()) })
                      onChanged()
                    })
                  }}
                >
                  Пробег
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <ErrorBox error={act.error} />
      {editable && (
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setInterval({ km: String(b.interval_km), days: String(b.interval_days) })}>
            Интервал
          </Button>
          <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setManual({ date: todayBishkek(), mileage: '', oil: '', filter: '' })}>
            Запись из тетради
          </Button>
        </div>
      )}
      {interval && (
        <Modal title={`Интервал замены · ${b.plate}`} onClose={() => setInterval(null)}>
          <div className="flex flex-col gap-3">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Каждые, км">
                <input inputMode="numeric" value={interval.km} onChange={(e) => setInterval({ ...interval, km: e.target.value })} />
              </Field>
              <Field label="Или каждые, дней">
                <input inputMode="numeric" value={interval.days} onChange={(e) => setInterval({ ...interval, days: e.target.value })} />
              </Field>
            </div>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setInterval(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !/^\d+$/.test(interval.km) || !/^\d+$/.test(interval.days)}
                onClick={() =>
                  void act.run(async () => {
                    await patch(`/vehicles/${b.vehicle_id}/oil-settings`, { interval_km: Number(interval.km), interval_days: Number(interval.days) })
                    setInterval(null)
                    onChanged()
                  })
                }
              >
                Сохранить
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {manual && (
        <Modal title={`Запись из тетради · ${b.plate}`} onClose={() => setManual(null)}>
          <div className="flex flex-col gap-3">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Когда меняли">
                <input type="date" value={manual.date} onChange={(e) => setManual({ ...manual, date: e.target.value })} />
              </Field>
              <Field label="Пробег, км">
                <input inputMode="numeric" value={manual.mileage} onChange={(e) => setManual({ ...manual, mileage: e.target.value })} />
              </Field>
            </div>
            <Field label="Какое масло" required>
              <input value={manual.oil} onChange={(e) => setManual({ ...manual, oil: e.target.value })} placeholder="Totachi 0W-20, 4 л" />
            </Field>
            <Field label="Фильтр">
              <input value={manual.filter} onChange={(e) => setManual({ ...manual, filter: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setManual(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !manual.oil.trim() || (manual.mileage.trim() !== '' && !/^\d+$/.test(manual.mileage.trim()))}
                onClick={() =>
                  void act.run(async () => {
                    await post(`/vehicles/${b.vehicle_id}/oil-changes`, {
                      change_date: manual.date,
                      mileage_km: manual.mileage.trim() ? Number(manual.mileage) : null,
                      oil_text: manual.oil,
                      filter_text: manual.filter,
                    })
                    setManual(null)
                    onChanged()
                    toast('Запись добавлена')
                  })
                }
              >
                Добавить
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </Card>
  )
}

/** Книжка всех машин: карточка клиента (с правкой) или кабинет фирмы (только просмотр). */
export function OilBookList({ path, editable }: { path: string; editable: boolean }) {
  const books = useLoad(() => get<VehicleBook[]>(path), [path])
  if (books.loading && !books.data) return <Loading />
  if (books.error) return <ErrorBox error={books.error} />
  const list = books.data ?? []
  if (list.length === 0) return <Empty>Машин нет — их добавляют в карточке клиента или прямо в кассе</Empty>
  return (
    <div className="flex flex-col gap-3">
      {list.map((b) => (
        <VehicleCard key={b.vehicle_id} b={b} editable={editable} onChanged={books.reload} />
      ))}
    </div>
  )
}
