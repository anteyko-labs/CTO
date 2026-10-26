// Масляная книжка: замены по машинам, строки по фильтрам, следующая замена по каждой строке,
// правка пробега и интервала (SPEC-16, SPEC-27).
import { useState } from 'react'
import { get, patch, post } from '../lib/api'
import { activeNotes, cashierNotesText, type ClientNote } from '../lib/clientNotes'
import { formatDate, formatDateTime, formatSom, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { fromKm, mileageText, toKm, unitLabel, type MileageUnit } from '../lib/mileage'
import { lineNextText, nextText, overdueSlots, recordLines, recordMileage, recordText, slotLabel, slotNextText, slotsWithNext, type SlotNext, type VehicleBook } from '../lib/oilBook'
import { printOilBook } from '../lib/oilBookPrint'
import { BOOK_SLOT_LABELS, type BookSlot } from '../lib/types'
import { Pager, usePager } from './Pager'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, toast } from './ui'
import { VehicleFormModal } from './VehicleFormModal'

/** Касса: под выбранной машиной — прошлая замена и следующая по каждой строке книжки. */
export function OilHint({ vehicleId }: { vehicleId: string }) {
  const book = useLoad(() => get<VehicleBook>(`/vehicles/${vehicleId}/oil-book`), [vehicleId])
  const b = book.data
  if (!b) return null
  const last = b.records[0]
  const notes = cashierNotesText(b.client_notes)
  const notesLine = notes && <div className="mt-1 font-medium text-sky-800">{notes}</div>
  if (!last)
    return (
      <div className="text-xs text-slate-500">
        В масляной книжке этой машины пока пусто.
        {notesLine}
      </div>
    )
  const overdue = overdueSlots(b, todayBishkek())
  const next = slotsWithNext(b)
  return (
    <div className={`rounded-md px-3 py-2 text-xs ${overdue.length ? 'bg-amber-50 text-amber-900' : 'bg-slate-50 text-slate-700'}`} data-testid="oil-hint">
      Прошлая замена {formatDate(last.change_date)}: {recordText(last) || '—'}
      {recordMileage(last) !== null && `, ${recordMileage(last)}`}.
      {next.length > 0 && (
        <ul className="mt-1">
          {next.map((n) => (
            <li key={n.slot} className={overdue.includes(n) ? 'font-medium' : ''}>
              {n.label}: следующая {slotNextText(n)}
              {overdue.includes(n) && ' — пора менять'}
            </li>
          ))}
        </ul>
      )}
      {last.declined?.trim() && <div className="mt-1 font-medium text-amber-800">В прошлый раз рекомендовали: {last.declined}</div>}
      {notesLine}
    </div>
  )
}

/** Пометки клиента из бота: текстом, без правки; удалённые (их видит только владелец) — зачёркнуты. */
function ClientNotes({ notes }: { notes: ClientNote[] }) {
  if (notes.length === 0) return null
  const active = activeNotes(notes).length
  return (
    <div className="rounded-md bg-sky-50 px-3 py-2 text-sm">
      <div className="mb-1 text-xs font-medium text-sky-900">Пометки клиента из бота{active ? '' : ' — все удалены'}</div>
      <ul className="flex flex-col gap-0.5">
        {notes.map((n) => (
          <li key={n.id} className={n.deleted ? 'text-slate-400 line-through' : 'text-slate-800'}>
            <span className="mr-2 text-xs text-slate-500">{formatDateTime(n.created_at)}</span>
            {n.text}
            {n.deleted && <span className="ml-2 text-xs no-underline">(удалена клиентом)</span>}
          </li>
        ))}
      </ul>
    </div>
  )
}

const SLOT_ORDER = Object.keys(BOOK_SLOT_LABELS) as BookSlot[]

type ManualForm = { date: string; mileage: string } & Record<BookSlot, string>

const emptyManual = (): ManualForm => ({
  date: todayBishkek(),
  mileage: '',
  ...(Object.fromEntries(SLOT_ORDER.map((s) => [s, ''])) as Record<BookSlot, string>),
})

/** Интервал строки в единице машины: «каждые 8 000 км или 31 дн.», «каждые 3 000 миль …»;
 *  у прочих работ без интервала — «срок не задан». Интервал хранится в км (ADR-084). */
function intervalText(n: SlotNext, unit: MileageUnit): string {
  if (n.interval_km === null || n.interval_days === null) return 'срок не задан'
  return `каждые ${mileageText(fromKm(n.interval_km, unit), unit)} или ${n.interval_days} дн.${n.own_interval ? ', у этой машины' : ''}`
}

function VehicleCard({ b, editable, partyId, onChanged }: { b: VehicleBook; editable: boolean; partyId?: string; onChanged: () => void }) {
  const [interval, setInterval] = useState<{ slot: BookSlot; km: string; days: string; own: boolean } | null>(null)
  const [manual, setManual] = useState<ManualForm | null>(null)
  const [editing, setEditing] = useState(false)
  const act = useAction()
  // Пробег машины — в км или милях; интервалы в книжке хранятся в км (ADR-084).
  const unit: MileageUnit = b.mileage_unit ?? 'km'
  // Замены машины — по страницам, новые сверху.
  const records = usePager(b.records, 'oilbook.records', b.vehicle_id)
  const removed = b.active === false
  const slots = b.slots ?? []
  // Строки без замен тоже можно настроить: интервал у машины задают заранее.
  const unused = SLOT_ORDER.filter((s) => !slots.some((n) => n.slot === s))
  const manualLines = manual ? SLOT_ORDER.filter((s) => manual[s].trim()).map((s) => ({ slot: s, text: manual[s].trim() })) : []
  return (
    <Card className={`flex flex-col gap-2 ${removed ? 'opacity-75' : ''}`}>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <div>
          <span className="font-semibold">{b.plate}</span>
          {(b.brand || b.model) && <span className="ml-2 text-slate-500">{[b.brand, b.model].filter(Boolean).join(' ')}</span>}
          {unit === 'mi' && (
            <span className="ml-2">
              <Badge tone="sky">пробег в милях</Badge>
            </span>
          )}
          {editable && partyId && !removed && (
            <Button variant="ghost" className="ml-1 px-2 py-0.5 text-xs" onClick={() => setEditing(true)}>
              Изменить
            </Button>
          )}
          {removed && (
            <span className="ml-2">
              <Badge tone="slate">убрана из списка</Badge>
            </span>
          )}
          {removed && editable && partyId && (
            <Button
              variant="secondary"
              className="ml-2 px-2 py-1 text-xs"
              disabled={act.busy}
              onClick={() =>
                void act.run(async () => {
                  await post(`/parties/${partyId}/vehicles/${b.vehicle_id}/restore`)
                  onChanged()
                  toast(`Машина ${b.plate} снова в списке клиента`)
                })
              }
            >
              Вернуть
            </Button>
          )}
        </div>
        {slots.length === 0 && (
          <div className="text-sm">
            Следующая замена масла: <b>{nextText(b)}</b>
          </div>
        )}
      </div>
      {slots.length > 0 && (
        <table className="w-full text-sm" data-testid="book-slots">
          <tbody className="divide-y divide-slate-100">
            {slots.map((n) => {
              const late = n.next_date !== null && n.next_date < todayBishkek()
              return (
                <tr key={n.slot}>
                  <td className="py-1 pr-2 font-medium">{n.label}</td>
                  <td className={`py-1 pr-2 ${late ? 'font-medium text-amber-800' : ''}`}>
                    {slotNextText(n) || '—'}
                    {late && ' — пора менять'}
                  </td>
                  <td className="py-1 pr-2 text-xs text-slate-500">{intervalText(n, unit)}</td>
                  <td className="py-1 text-right">
                    {editable && !removed && (
                      <Button
                        variant="ghost"
                        className="px-2 py-0.5 text-xs"
                        onClick={() =>
                          setInterval({
                            slot: n.slot,
                            km: n.interval_km !== null ? String(fromKm(n.interval_km, unit)) : '',
                            days: n.interval_days !== null ? String(n.interval_days) : '',
                            own: n.own_interval,
                          })
                        }
                      >
                        Интервал
                      </Button>
                    )}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
      {b.records.length === 0 ? (
        <div className="text-sm text-slate-500">Замен ещё не было</div>
      ) : (
        <ul className="divide-y divide-slate-100 text-sm">
          {records.shown.map((r) => (
            <li key={r.id} className="flex flex-wrap items-baseline justify-between gap-2 py-1.5">
              <span>
                <span className="font-medium">{formatDate(r.change_date)}</span>
                {recordMileage(r) !== null && <span className="ml-2">{recordMileage(r)}</span>}
                <span className="ml-2 text-slate-600">{r.sale_total_tyiyn != null ? formatSom(r.sale_total_tyiyn) : '—'}</span>
                {r.comment && <span className="ml-2 text-slate-500">— {r.comment}</span>}
                {r.sale_number === null && (
                  <span className="ml-2">
                    <Badge tone="slate">из тетради</Badge>
                  </span>
                )}
                <span className="block text-slate-600">
                  {recordLines(r).map((l, i) => (
                    <span key={i} className="mr-3 inline-block">
                      <span className="text-slate-500">{slotLabel(l.slot)}:</span> {l.text}
                      {lineNextText(l) && <span className="text-sky-800"> — {lineNextText(l)}</span>}
                    </span>
                  ))}
                </span>
              </span>
              {editable && (
                <Button
                  variant="ghost"
                  className="px-2 py-1 text-xs"
                  onClick={() => {
                    const before = r.mileage ?? r.mileage_km
                    const v = window.prompt(`Пробег, ${unitLabel(unit)}`, before !== null && before !== undefined && (r.mileage_unit ?? 'km') === unit ? String(before) : '')
                    if (v === null || !/^\d+$/.test(v.trim())) return
                    void act.run(async () => {
                      await patch(`/oil-changes/${r.id}`, { mileage: Number(v.trim()), mileage_unit: unit })
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
      <Pager page={records.page} size={records.size} total={records.total} onPage={records.setPage} onSize={records.setSize} label={`Страницы замен ${b.plate}`} />
      <ClientNotes notes={b.client_notes ?? []} />
      <ErrorBox error={act.error} />
      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => void printOilBook(b)}>
          Печать книжки
        </Button>
        {editable && !removed && (
          <>
            {unused.length > 0 && (
              <select
                aria-label="Интервал другой строки"
                className="w-auto py-1 text-xs"
                value=""
                onChange={(e) => {
                  const slot = e.target.value as BookSlot
                  if (slot) setInterval({ slot, km: String(fromKm(b.interval_km, unit)), days: String(b.interval_days), own: false })
                }}
              >
                <option value="">Интервал строки…</option>
                {unused.map((s) => (
                  <option key={s} value={s}>
                    {slotLabel(s)}
                  </option>
                ))}
              </select>
            )}
            <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setManual(emptyManual())}>
              Запись из тетради
            </Button>
          </>
        )}
      </div>
      {interval && (
        <Modal title={`Интервал · ${slotLabel(interval.slot)} · ${b.plate}`} onClose={() => setInterval(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              По умолчанию интервал строки задан у категории товара. Здесь — только для этой машины.
            </div>
            <div className="grid grid-cols-2 gap-3">
              <Field label={`Каждые, ${unitLabel(unit)}`}>
                <input inputMode="numeric" value={interval.km} onChange={(e) => setInterval({ ...interval, km: e.target.value })} />
              </Field>
              <Field label="Или каждые, дней">
                <input inputMode="numeric" value={interval.days} onChange={(e) => setInterval({ ...interval, days: e.target.value })} />
              </Field>
            </div>
            <ErrorBox error={act.error} />
            <div className="flex flex-wrap justify-end gap-2">
              {interval.own && (
                <Button
                  variant="ghost"
                  disabled={act.busy}
                  onClick={() =>
                    void act.run(async () => {
                      await patch(`/vehicles/${b.vehicle_id}/oil-settings`, { slot: interval.slot, reset: true })
                      setInterval(null)
                      onChanged()
                    })
                  }
                >
                  Как у категории
                </Button>
              )}
              <Button variant="secondary" onClick={() => setInterval(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || !/^\d+$/.test(interval.km) || !/^\d+$/.test(interval.days)}
                onClick={() =>
                  void act.run(async () => {
                    await patch(`/vehicles/${b.vehicle_id}/oil-settings`, {
                      slot: interval.slot,
                      interval_km: toKm(Number(interval.km), unit),
                      interval_days: Number(interval.days),
                    })
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
      {editing && partyId && (
        <VehicleFormModal
          partyId={partyId}
          vehicle={{ id: b.vehicle_id, plate: b.plate, brand: b.brand, model: b.model, comment: b.comment ?? '', mileage_unit: unit }}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false)
            onChanged()
          }}
        />
      )}
      {manual && (
        <Modal title={`Запись из тетради · ${b.plate}`} onClose={() => setManual(null)}>
          <div className="flex flex-col gap-3">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Когда меняли">
                <input type="date" value={manual.date} onChange={(e) => setManual({ ...manual, date: e.target.value })} />
              </Field>
              <Field label={`Пробег, ${unitLabel(unit)}`}>
                <input inputMode="numeric" value={manual.mileage} onChange={(e) => setManual({ ...manual, mileage: e.target.value })} />
              </Field>
            </div>
            {SLOT_ORDER.map((s) => (
              <Field key={s} label={slotLabel(s)}>
                <input
                  value={manual[s]}
                  onChange={(e) => setManual({ ...manual, [s]: e.target.value })}
                  placeholder={s === 'oil' ? 'Totachi 0W-20, 4 л' : s === 'other' ? 'антифриз, свечи…' : ''}
                />
              </Field>
            ))}
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setManual(null)}>
                Отмена
              </Button>
              <Button
                disabled={act.busy || manualLines.length === 0 || (manual.mileage.trim() !== '' && !/^\d+$/.test(manual.mileage.trim()))}
                onClick={() =>
                  void act.run(async () => {
                    await post(`/vehicles/${b.vehicle_id}/oil-changes`, {
                      change_date: manual.date,
                      mileage: manual.mileage.trim() ? Number(manual.mileage) : null,
                      mileage_unit: unit,
                      lines: manualLines,
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

/**
 * Книжка всех машин: карточка клиента (с правкой) или кабинет фирмы (только просмотр).
 * В карточке клиента (`partyId`) можно показать и машины, которые клиент убрал в боте,
 * с их историей, и вернуть их (ADR-062).
 */
export function OilBookList({ path, editable, partyId, onAdd }: { path: string; editable: boolean; partyId?: string; onAdd?: () => void }) {
  const [showRemoved, setShowRemoved] = useState(false)
  const url = partyId && showRemoved ? `${path}?removed=true` : path
  const books = useLoad(() => get<VehicleBook[]>(url), [url])
  const pager = usePager(books.data ?? [], 'oilbook.vehicles', url)
  const toggle = partyId && (
    <div>
      <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setShowRemoved(!showRemoved)}>
        {showRemoved ? 'Скрыть убранные' : 'Показать убранные'}
      </Button>
    </div>
  )
  if (books.loading && !books.data) return <Loading />
  if (books.error) return <ErrorBox error={books.error} />
  const list = books.data ?? []
  if (list.length === 0)
    return (
      <div className="flex flex-col gap-2">
        <Empty
          action={
            !showRemoved &&
            onAdd && (
              <Button variant="secondary" onClick={onAdd}>
                + Добавить машину
              </Button>
            )
          }
        >
          {showRemoved
            ? 'Машин нет, убранных тоже'
            : onAdd
              ? 'Машин пока нет. Добавьте машину — по ней ведётся книжка замен масла; в кассе её тоже можно добавить при продаже.'
              : 'Машин пока нет'}
        </Empty>
        {toggle}
      </div>
    )
  return (
    <div className="flex flex-col gap-3">
      {pager.shown.map((b) => (
        <VehicleCard key={b.vehicle_id} b={b} editable={editable} partyId={partyId} onChanged={books.reload} />
      ))}
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы машин" />
      {toggle}
    </div>
  )
}
