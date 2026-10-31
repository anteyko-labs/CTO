// Машина клиента в программе (SPEC-29 §21–22): госномер, марка и модель из справочника (своё
// слово — можно, сервер добавит его в справочник, ADR-083) и в чём пробег — км или мили (ADR-084).
// Одна форма для карточки клиента, кассы и правки машины в книжке.
import { useState } from 'react'
import { get, newOpId, patch, post, qs } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { carName, modelsOf, type MileageUnit } from '../lib/carDict'
import { useCan } from '../lib/permissions'
import type { CarMake, Party, PartyVehicle } from '../lib/types'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

/** «Перенести к другому клиенту» (задача 003 Н10): продали машину или ошиблись клиентом. Книжка ТО
 *  и напоминания идут с машиной, прежние чеки остаются у прежнего клиента. Право «Клиенты и машины». */
export function VehicleMoveModal({
  partyId,
  vehicle,
  onClose,
  onMoved,
}: {
  partyId: string
  vehicle: Pick<PartyVehicle, 'id' | 'plate'>
  onClose: () => void
  onMoved: (to: Party) => void
}) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 250)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve([] as Party[]) : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`)),
    [query],
  )
  const [to, setTo] = useState<Party | null>(null)
  // op_id окна: повтор после потерянного ответа не перенесёт машину дважды.
  const [opId] = useState(newOpId)
  const act = useAction()
  const list = (found.data ?? []).filter((p) => p.id !== partyId)

  const move = () =>
    void act.run(async () => {
      if (!to) return
      await post(`/parties/${partyId}/vehicles/${vehicle.id}/move`, { op_id: opId, to_party_id: to.id })
      toast(`Машина ${vehicle.plate} перенесена к клиенту ${to.name}`)
      onMoved(to)
    })

  return (
    <Modal title={`Перенести ${vehicle.plate} к другому клиенту`} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="vehicle-move">
        <div className="text-sm text-slate-600">
          Масляная книжка, интервалы и напоминания перейдут к новому клиенту. Прежние чеки останутся у прежнего клиента.
        </div>
        {to ? (
          <div className="flex items-center justify-between gap-2 rounded-md border border-sky-300 bg-sky-50 px-3 py-2 text-sm">
            <span>
              Новый клиент: <b>{to.name}</b>
              {to.phone && <span className="text-slate-500"> · {to.phone}</span>}
            </span>
            <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setTo(null)}>
              Другой
            </Button>
          </div>
        ) : (
          <>
            <Field label="Клиент" hint="Имя, телефон или госномер">
              <input id="vehicle-move-q" autoFocus value={q} onChange={(e) => setQ(e.target.value)} />
            </Field>
            {query.length >= 2 && !found.loading && list.length === 0 && <div className="text-sm text-slate-500">Никого не нашли</div>}
            <div className="flex flex-col gap-1">
              {list.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  className="min-h-[44px] rounded-md border border-slate-200 px-3 py-2 text-left text-sm hover:bg-slate-50"
                  onClick={() => setTo(p)}
                >
                  <span className="font-medium">{p.name}</span>
                  {p.phone && <span className="text-slate-500"> · {p.phone}</span>}
                </button>
              ))}
            </div>
          </>
        )}
        <ErrorBox error={act.error ?? found.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || !to} onClick={move}>
            Перенести
          </Button>
        </div>
      </div>
    </Modal>
  )
}

export function VehicleFormModal({
  partyId,
  vehicle,
  onClose,
  onSaved,
  onMoved,
}: {
  partyId: string
  /** Правка машины; без неё — новая. */
  vehicle?: Pick<PartyVehicle, 'id' | 'plate' | 'brand' | 'model' | 'comment' | 'mileage_unit'>
  onClose: () => void
  onSaved: (v: PartyVehicle) => void
  /** Машину перенесли к другому клиенту: у этого её больше нет. Без него — просто закрыть. */
  onMoved?: (to: Party) => void
}) {
  const mayMove = useCan()('clients')
  const [moving, setMoving] = useState(false)
  const dict = useLoad(() => get<CarMake[]>('/car-makes'), [])
  const [form, setForm] = useState({
    plate: vehicle?.plate ?? '',
    brand: vehicle?.brand ?? '',
    model: vehicle?.model ?? '',
    unit: (vehicle?.mileage_unit ?? 'km') as MileageUnit,
  })
  const act = useAction()
  const makes = dict.data ?? []
  const models = modelsOf(makes, form.brand)
  const missing = missingWithFocus([Boolean(form.plate.trim()), 'госномер', '#vehicle-plate'])

  const save = () =>
    void act.run(async () => {
      // Написание — как в справочнике: «rx 350» не заводит в нём новую модель (SPEC-29 §21).
      const brand = carName(form.brand, makes.map((m) => m.name))
      const model = carName(form.model, modelsOf(makes, brand).map((m) => m.name))
      const body = { plate: form.plate.trim(), brand, model, mileage_unit: form.unit }
      const v = vehicle
        ? await patch<PartyVehicle>(`/parties/${partyId}/vehicles/${vehicle.id}`, { ...body, comment: vehicle.comment })
        : await post<PartyVehicle>(`/parties/${partyId}/vehicles`, body)
      toast(vehicle ? 'Машина сохранена' : `Машина ${v.plate} добавлена`)
      onSaved(v)
    })

  // Окно переноса — вместо формы, а не поверх неё.
  if (moving && vehicle)
    return (
      <VehicleMoveModal
        partyId={partyId}
        vehicle={vehicle}
        onClose={() => setMoving(false)}
        onMoved={(to) => {
          setMoving(false)
          if (onMoved) onMoved(to)
          else onClose()
        }}
      />
    )

  return (
    <Modal title={vehicle ? `Машина ${vehicle.plate}` : 'Новая машина'} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="vehicle-form">
        <Field label="Госномер" required>
          <input id="vehicle-plate" autoFocus value={form.plate} onChange={(e) => setForm({ ...form, plate: e.target.value })} />
        </Field>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label="Марка" hint="Из справочника; нет нужной — впишите, она добавится">
            <input
              id="vehicle-brand"
              list="car-makes"
              autoComplete="off"
              value={form.brand}
              onChange={(e) => setForm({ ...form, brand: e.target.value })}
              onBlur={() => setForm((f) => ({ ...f, brand: carName(f.brand, makes.map((m) => m.name)) }))}
            />
          </Field>
          <Field label="Модель">
            <input
              id="vehicle-model"
              list="car-models"
              autoComplete="off"
              value={form.model}
              onChange={(e) => setForm({ ...form, model: e.target.value })}
              onBlur={() => setForm((f) => ({ ...f, model: carName(f.model, modelsOf(makes, f.brand).map((m) => m.name)) }))}
            />
          </Field>
        </div>
        <datalist id="car-makes">
          {makes.map((m) => (
            <option key={m.id} value={m.name} />
          ))}
        </datalist>
        <datalist id="car-models">
          {models.map((m) => (
            <option key={m.id} value={m.name} />
          ))}
        </datalist>
        <div className="flex flex-col gap-1 text-sm">
          <span className="font-medium text-slate-700">Пробег в</span>
          <div className="inline-grid w-fit grid-cols-2 overflow-hidden rounded-md border border-slate-300" role="group" aria-label="Пробег в">
            {(['km', 'mi'] as const).map((u) => (
              <button
                key={u}
                type="button"
                aria-pressed={form.unit === u}
                className={`min-h-[44px] px-4 ${form.unit === u ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setForm({ ...form, unit: u })}
              >
                {u === 'km' ? 'километрах' : 'милях'}
              </button>
            ))}
          </div>
          <span className="text-xs text-slate-500">Мили — у машин из США и Японии со спидометром в милях. Интервалы замен считаются сами.</span>
        </div>
        <Missing items={missing} />
        <ErrorBox error={act.error ?? dict.error} />
        <div className="flex flex-wrap justify-end gap-2">
          {vehicle && mayMove && (
            <Button variant="ghost" className="mr-auto" onClick={() => setMoving(true)}>
              Перенести к другому клиенту
            </Button>
          )}
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || missing.length > 0} onClick={save}>
            {vehicle ? 'Сохранить' : 'Добавить'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}
