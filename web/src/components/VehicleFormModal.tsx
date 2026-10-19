// Машина клиента в программе (SPEC-29 §21–22): госномер, марка и модель из справочника (своё
// слово — можно, сервер добавит его в справочник, ADR-083) и в чём пробег — км или мили (ADR-084).
// Одна форма для карточки клиента, кассы и правки машины в книжке.
import { useState } from 'react'
import { get, patch, post } from '../lib/api'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { modelsOf, type MileageUnit } from '../lib/carDict'
import type { CarMake, PartyVehicle } from '../lib/types'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

export function VehicleFormModal({
  partyId,
  vehicle,
  onClose,
  onSaved,
}: {
  partyId: string
  /** Правка машины; без неё — новая. */
  vehicle?: Pick<PartyVehicle, 'id' | 'plate' | 'brand' | 'model' | 'comment' | 'mileage_unit'>
  onClose: () => void
  onSaved: (v: PartyVehicle) => void
}) {
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
      const body = { plate: form.plate.trim(), brand: form.brand.trim(), model: form.model.trim(), mileage_unit: form.unit }
      const v = vehicle
        ? await patch<PartyVehicle>(`/parties/${partyId}/vehicles/${vehicle.id}`, { ...body, comment: vehicle.comment })
        : await post<PartyVehicle>(`/parties/${partyId}/vehicles`, body)
      toast(vehicle ? 'Машина сохранена' : `Машина ${v.plate} добавлена`)
      onSaved(v)
    })

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
            />
          </Field>
          <Field label="Модель">
            <input
              id="vehicle-model"
              list="car-models"
              autoComplete="off"
              value={form.model}
              onChange={(e) => setForm({ ...form, model: e.target.value })}
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
        <div className="flex justify-end gap-2">
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
