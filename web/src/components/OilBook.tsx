// Масляная книжка: замены по машинам, строки по фильтрам, следующая замена по каждой строке,
// правка пробега и интервала (SPEC-16, SPEC-27).
import { useState } from 'react'
import { ApiError, get, patch, post } from '../lib/api'
import { activeNotes, cashierNotesText, type ClientNote } from '../lib/clientNotes'
import { formatDate, formatDateTime, formatSom, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { fromKm, mileageText, toKm, unitLabel, type MileageUnit } from '../lib/mileage'
import { lineNextText, lineSlotLabel, lineTitle, nextText, recordLines, recordMileage, recordText, slotKey, slotLabel, slotNextText, slotsWithNext, slotTitle, type CustomSlot, type OilRecord, type SlotNext, type VehicleBook } from '../lib/oilBook'
import { printOilBook } from '../lib/oilBookPrint'
import { BOOK_SLOT_LABELS, type BookSlot } from '../lib/types'
import { Pager, usePager } from './Pager'
import { ExportButton } from './ExportButton'
import { exportTitle, listSheet, xl, type ExportCol } from '../lib/exportTable'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, toast } from './ui'
import { VehicleFormModal } from './VehicleFormModal'

/** Касса: под выбранной машиной — прошлая замена и следующая по каждой строке книжки. Книжку
 *  касса грузит сама один раз на машину и передаёт сюда (SPEC-36 §16): окно клиента её не
 *  перезапрашивает. */
export function OilHint({ book: b }: { book: VehicleBook | null }) {
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
  const next = slotsWithNext(b)
  return (
    <div className={`rounded-md px-3 py-2 text-xs bg-slate-50 text-slate-700`} data-testid="oil-hint">
      Прошлая замена {formatDate(last.change_date)}: {recordText(last) || '—'}
      {recordMileage(last) !== null && `, ${recordMileage(last)}`}.
      {next.length > 0 && (
        <ul className="mt-1">
          {next.map((n) => (
            <li key={slotKey(n)}>
              {slotTitle(n)}: следующая {slotNextText(n)}
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

/** Правка записи книжки (задача 013): пробег, строки и комментарий; даты нет. `index` — номер
 *  строки в записи на сервере, у новой строки — нет. */
type RecordEdit = {
  id: string
  mileage: string
  comment: string
  lines: { index: number | null; slot: BookSlot; text: string; custom_slot_id: string | null; custom_slot_name: string | null }[]
}

/** Значение строки в списке «Что меняли»: встроенная строка или своя строка точки (задача 016). */
const CUSTOM = 'custom:'
const NEW_SLOT = '__new'
const lineValue = (l: { slot: BookSlot; custom_slot_id: string | null }): string => (l.custom_slot_id ? `${CUSTOM}${l.custom_slot_id}` : l.slot)

/** Новая своя строка из окна записи: к какой строке записи и поля формы. */
type NewSlotForm = { line: number; name: string; km: string; days: string }

/** Отказ сервера при создании своей строки — понятным текстом (задача 016). */
function customSlotError(e: unknown): string {
  if (e instanceof ApiError && e.status === 409) return 'Строка с таким названием уже есть — выберите её в списке или назовите иначе'
  if (e instanceof ApiError && (e.status === 400 || e.status === 422)) return 'Название — от 2 до 40 символов; интервал — от 500 до 200 000 км и от 1 до 1095 дней'
  return e instanceof Error ? e.message : String(e)
}

function recordEdit(r: OilRecord, unit: MileageUnit): RecordEdit {
  const before = r.mileage ?? r.mileage_km
  // У старой записи без строк книжки строки — из текста масла и фильтра: на сервере их нет, это новые.
  const own = Boolean(r.lines && r.lines.length > 0)
  return {
    id: r.id,
    mileage: before !== null && before !== undefined && (r.mileage_unit ?? 'km') === unit ? String(before) : '',
    comment: r.comment ?? '',
    lines: recordLines(r).map((l, i) => ({
      index: own ? i : null,
      slot: l.slot,
      text: l.text,
      custom_slot_id: l.custom_slot_id ?? null,
      custom_slot_name: l.custom_slot_name ?? null,
    })),
  }
}

const emptyManual = (): ManualForm => ({
  date: todayBishkek(),
  mileage: '',
  ...(Object.fromEntries(SLOT_ORDER.map((s) => [s, ''])) as Record<BookSlot, string>),
})

/** Интервал строки в единице машины: «каждые 8 000 км», «каждые 3 000 миль»;
 *  у прочих работ без интервала — «срок не задан». Интервал хранится в км (ADR-084). */
function intervalText(n: SlotNext, unit: MileageUnit): string {
  if (n.interval_km === null) return 'срок не задан'
  return `каждые ${mileageText(fromKm(n.interval_km, unit), unit)}${n.own_interval ? ', у этой машины' : ''}`
}

function VehicleCard({ b, editable, partyId, onChanged }: { b: VehicleBook; editable: boolean; partyId?: string; onChanged: () => void }) {
  const [interval, setInterval] = useState<{ slot: BookSlot; km: string; days: string; own: boolean } | null>(null)
  const [manual, setManual] = useState<ManualForm | null>(null)
  const [editing, setEditing] = useState(false)
  const [recEdit, setRecEdit] = useState<RecordEdit | null>(null)
  // Свои строки точки для списка «Что меняли» — грузятся при открытии окна записи (задача 016).
  const [customSlots, setCustomSlots] = useState<CustomSlot[]>([])
  const [newSlot, setNewSlot] = useState<NewSlotForm | null>(null)
  const [newSlotError, setNewSlotError] = useState<string | null>(null)
  const [newSlotBusy, setNewSlotBusy] = useState(false)
  const act = useAction()
  // Пробег машины — в км или милях; интервалы в книжке хранятся в км (ADR-084).
  const unit: MileageUnit = b.mileage_unit ?? 'km'
  // Замены машины — по страницам, новые сверху.
  const records = usePager(b.records, 'oilbook.records', b.vehicle_id)
  const removed = b.active === false
  const slots = b.slots ?? []
  // Строки без замен тоже можно настроить: интервал у машины задают заранее. Услуги «прочее» — со
  // своим интервалом у услуги (ADR-131), «Прочие работы» из тетради — строкой книжки.
  const unused = SLOT_ORDER.filter((s) => !slots.some((n) => n.slot === s && !n.service_id))
  const openRecEdit = (r: OilRecord) => {
    setRecEdit(recordEdit(r, unit))
    setNewSlot(null)
    setNewSlotError(null)
    get<CustomSlot[]>('/book-slots')
      .then(setCustomSlots)
      .catch(() => setCustomSlots([]))
  }
  const setLine = (i: number, patchLine: Partial<RecordEdit['lines'][number]>) =>
    setRecEdit((cur) => (cur ? { ...cur, lines: cur.lines.map((x, j) => (j === i ? { ...x, ...patchLine } : x)) } : cur))
  const pickSlot = (i: number, value: string) => {
    if (value === NEW_SLOT) {
      setNewSlotError(null)
      setNewSlot({ line: i, name: '', km: '', days: '' })
      return
    }
    if (value.startsWith(CUSTOM)) {
      const c = customSlots.find((x) => x.id === value.slice(CUSTOM.length))
      setLine(i, { slot: 'other', custom_slot_id: value.slice(CUSTOM.length), custom_slot_name: c?.name ?? null })
    } else setLine(i, { slot: value as BookSlot, custom_slot_id: null, custom_slot_name: null })
  }
  const createSlot = async (f: NewSlotForm) => {
    setNewSlotBusy(true)
    setNewSlotError(null)
    try {
      const c = await post<CustomSlot>('/book-slots', {
        name: f.name.trim(),
        ...(f.km.trim() ? { interval_km: toKm(Number(f.km.trim()), unit) } : {}),
        ...(f.days.trim() ? { interval_days: Number(f.days.trim()) } : {}),
      })
      setCustomSlots((cur) => [...cur.filter((x) => x.id !== c.id), c])
      setLine(f.line, { slot: 'other', custom_slot_id: c.id, custom_slot_name: c.name })
      setNewSlot(null)
      toast(`Строка «${c.name}» создана`)
    } catch (e) {
      setNewSlotError(customSlotError(e))
    } finally {
      setNewSlotBusy(false)
    }
  }
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
              return (
                <tr key={slotKey(n)}>
                  <td className="py-1 pr-2">
                    {/* Что залили в последний раз (задача 003 Н5); строка книжки — мелко: интервал у неё. */}
                    <span className="font-medium">{slotTitle(n)}</span>
                    {slotTitle(n) !== n.label && <span className="block text-xs text-slate-500">{n.label}</span>}
                  </td>
                  <td className="py-1 pr-2">{slotNextText(n) || '—'}</td>
                  <td className="py-1 pr-2 text-xs text-slate-500">{intervalText(n, unit)}</td>
                  <td className="py-1 text-right">
                    {/* У услуги «прочее» интервал — у самой услуги (экран «Услуги»), не у машины. */}
                    {editable && !removed && !n.service_id && (
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
                      {lineTitle(l)}
                      {lineNextText(l) && <span className="text-sky-800"> — {lineNextText(l)}</span>}
                    </span>
                  ))}
                </span>
              </span>
              {editable && (
                <Button variant="ghost" className="px-2 py-1 text-xs" data-testid="book-record-edit" onClick={() => openRecEdit(r)}>
                  Изменить
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
              <Field label="Напоминать клиенту через, дней" hint="Только для напоминаний в боте: на экране и в печати дат нет">
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
          onMoved={() => {
            setEditing(false)
            onChanged()
          }}
        />
      )}
      {recEdit && (
        <Modal title={`Запись книжки · ${b.plate}`} onClose={() => setRecEdit(null)}>
          <div className="flex flex-col gap-3">
            <Field label={`Пробег, ${unitLabel(unit)}`}>
              <input id="book-edit-mileage" inputMode="numeric" value={recEdit.mileage} onChange={(e) => setRecEdit({ ...recEdit, mileage: e.target.value })} />
            </Field>
            {recEdit.lines.map((l, i) => (
              <div key={i} className="flex gap-2" data-testid="book-edit-line">
                <select aria-label="Что меняли" value={lineValue(l)} onChange={(e) => pickSlot(i, e.target.value)}>
                  {SLOT_ORDER.map((s) => (
                    <option key={s} value={s}>
                      {slotLabel(s)}
                    </option>
                  ))}
                  {/* Свои строки точки (задача 016): видимые, и у старой записи — её строка, даже скрытая. */}
                  {customSlots
                    .filter((c) => !c.hidden || c.id === l.custom_slot_id)
                    .map((c) => (
                      <option key={c.id} value={`${CUSTOM}${c.id}`}>
                        {c.name}
                      </option>
                    ))}
                  {l.custom_slot_id && !customSlots.some((c) => c.id === l.custom_slot_id) && (
                    <option value={lineValue(l)}>{lineSlotLabel(l)}</option>
                  )}
                  <option value={NEW_SLOT}>+ Новая строка…</option>
                </select>
                <input
                  className="min-w-0 flex-1"
                  aria-label="Что залили или поставили"
                  value={l.text}
                  onChange={(e) => setLine(i, { text: e.target.value })}
                />
                <Button variant="secondary" aria-label="Убрать строку" onClick={() => setRecEdit({ ...recEdit, lines: recEdit.lines.filter((_, j) => j !== i) })}>
                  ×
                </Button>
              </div>
            ))}
            {newSlot && (
              <div className="flex flex-col gap-2 rounded-md bg-slate-50 p-3" data-testid="book-new-slot">
                <div className="text-sm font-medium">Новая строка книжки — появится у всех машин точки</div>
                <Field label="Название">
                  <input value={newSlot.name} maxLength={40} onChange={(e) => setNewSlot({ ...newSlot, name: e.target.value })} placeholder="Свечи, ремень ГРМ…" />
                </Field>
                <div className="grid grid-cols-2 gap-3">
                  <Field label={`Интервал, ${unitLabel(unit)}`} hint="Необязательно: без интервала — без срока">
                    <input inputMode="numeric" value={newSlot.km} onChange={(e) => setNewSlot({ ...newSlot, km: e.target.value })} />
                  </Field>
                  <Field label="Интервал, дней" hint="Необязательно: для напоминаний в боте">
                    <input inputMode="numeric" value={newSlot.days} onChange={(e) => setNewSlot({ ...newSlot, days: e.target.value })} />
                  </Field>
                </div>
                <ErrorBox error={newSlotError} />
                <div className="flex justify-end gap-2">
                  <Button variant="secondary" className="text-xs" onClick={() => setNewSlot(null)}>
                    Отмена
                  </Button>
                  <Button
                    className="text-xs"
                    disabled={
                      newSlotBusy ||
                      newSlot.name.trim().length < 2 ||
                      (newSlot.km.trim() !== '' && !/^\d+$/.test(newSlot.km.trim())) ||
                      (newSlot.days.trim() !== '' && !/^\d+$/.test(newSlot.days.trim()))
                    }
                    onClick={() => void createSlot(newSlot)}
                  >
                    Создать
                  </Button>
                </div>
              </div>
            )}
            <div>
              <Button
                variant="secondary"
                className="text-xs"
                onClick={() => setRecEdit({ ...recEdit, lines: [...recEdit.lines, { index: null, slot: 'other', text: '', custom_slot_id: null, custom_slot_name: null }] })}
              >
                + строка
              </Button>
            </div>
            <Field label="Комментарий">
              <input value={recEdit.comment} onChange={(e) => setRecEdit({ ...recEdit, comment: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setRecEdit(null)}>
                Отмена
              </Button>
              <Button
                disabled={
                  act.busy ||
                  newSlot !== null ||
                  !recEdit.lines.some((l) => l.text.trim()) ||
                  (recEdit.mileage.trim() !== '' && !/^\d+$/.test(recEdit.mileage.trim()))
                }
                onClick={() =>
                  void act.run(async () => {
                    await patch(`/oil-changes/${recEdit.id}`, {
                      ...(recEdit.mileage.trim() ? { mileage: Number(recEdit.mileage.trim()), mileage_unit: unit } : {}),
                      comment: recEdit.comment,
                      lines: recEdit.lines.map((l) => ({
                        ...(l.index !== null ? { index: l.index } : {}),
                        slot: l.slot,
                        text: l.text,
                        ...(l.custom_slot_id ? { custom_slot_id: l.custom_slot_id } : {}),
                      })),
                    })
                    setRecEdit(null)
                    onChanged()
                    toast('Запись изменена')
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
export function OilBookList({
  path,
  editable,
  partyId,
  onAdd,
  name = 'Масляная книжка',
}: {
  path: string
  editable: boolean
  partyId?: string
  onAdd?: () => void
  /** Название файла выгрузки: «Масляная книжка <клиент>». */
  name?: string
}) {
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
      <div className="flex justify-end">
        {/* Машины и все записи книжки — двумя листами (задача 003 Н13). */}
        <ExportButton file={name} sheets={() => oilBookSheets(name, list)} />
      </div>
      {pager.shown.map((b) => (
        <VehicleCard key={b.vehicle_id} b={b} editable={editable} partyId={partyId} onChanged={books.reload} />
      ))}
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы машин" />
      {toggle}
    </div>
  )
}

const VEHICLE_COLS: ExportCol<VehicleBook>[] = [
  { label: 'Госномер', value: (b) => b.plate },
  { label: 'Марка', value: (b) => b.brand },
  { label: 'Модель', value: (b) => b.model },
  { label: 'Следующая замена масла', value: (b) => nextText(b) },
  { label: 'Следующая, км', value: (b) => (b.next_km !== null ? xl.int(b.next_km) : null) },
  { label: 'Интервал, км', value: (b) => xl.int(b.interval_km) },
  { label: 'Пометка', value: (b) => b.comment ?? '' },
  { label: 'Состояние', value: (b) => (b.active === false ? 'убрана клиентом' : '') },
]

type RecordRow = { b: VehicleBook; r: OilRecord }

const RECORD_COLS: ExportCol<RecordRow>[] = [
  { label: 'Госномер', value: ({ b }) => b.plate },
  { label: 'Дата', value: ({ r }) => xl.date(r.change_date) },
  { label: 'Пробег', value: ({ r }) => recordMileage(r) ?? '' },
  { label: 'Пробег, км', value: ({ r }) => (r.mileage_km !== null ? xl.int(r.mileage_km) : null) },
  { label: 'Что меняли', value: ({ r }) => recordLines(r).map((l) => [`${lineSlotLabel(l)}: ${l.text}`, lineNextText(l)].filter(Boolean).join(', ')).join('; ') },
  { label: 'Чек', value: ({ r }) => (r.sale_number !== null ? xl.int(r.sale_number) : null) },
  { label: 'Сумма чека', value: ({ r }) => xl.maybeMoney(r.sale_total_tyiyn) },
  { label: 'Комментарий', value: ({ r }) => r.comment },
  { label: 'Отказался', value: ({ r }) => r.declined ?? '' },
]

/** Книжка в Excel: машины со следующей заменой и все записи замен. */
function oilBookSheets(name: string, list: VehicleBook[]) {
  const records = list.flatMap((b) => b.records.map((r) => ({ b, r })))
  return [listSheet(name, VEHICLE_COLS, list, { sheet: 'Машины' }), { ...listSheet(name, RECORD_COLS, records, { sheet: 'Замены' }), title: exportTitle(`${name}: замены`) }]
}
