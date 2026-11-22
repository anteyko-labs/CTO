// Данные клиента для долга (задача 048): физлицо — фамилия, имя, телефон, ПИН (14 цифр), как в
// расписке; фирма — название, телефон, ИНН. Правка на месте из «Долгов» и с кассы перед долгом.
import { useState } from 'react'
import { Button, ErrorBox, Modal, toast } from './ui'
import { patch } from '../lib/api'
import { useAction } from '../lib/hooks'
import type { Party } from '../lib/types'

type PartyData = Pick<Party, 'kind' | 'name' | 'first_name' | 'last_name' | 'phone' | 'inn'>

/** Чего не хватает для долга — то же правило, что на сервере (`debt_data_missing`). */
export function debtDataMissing(p: PartyData): string[] {
  const inn = p.inn.trim()
  const phone = p.phone.trim() !== ''
  if (p.kind === 'company') {
    return [
      ...(p.name.trim() ? [] : ['название']),
      ...(phone ? [] : ['телефон']),
      ...(/^\d{10,14}$/.test(inn) ? [] : ['ИНН']),
    ]
  }
  return [
    ...((p.last_name ?? '').trim() ? [] : ['фамилия']),
    ...((p.first_name ?? '').trim() ? [] : ['имя']),
    ...(phone ? [] : ['телефон']),
    ...(/^\d{14}$/.test(inn) ? [] : ['ПИН']),
  ]
}

export function PartyDataModal({ party, onClose, onSaved }: { party: Party; onClose: () => void; onSaved: (p: Party) => void }) {
  const company = party.kind === 'company'
  const [f, setF] = useState({
    name: party.name,
    last_name: party.last_name ?? '',
    first_name: party.first_name ?? '',
    phone: party.phone,
    inn: party.inn,
  })
  const act = useAction()
  const draft: PartyData = { ...party, ...f }
  const missing = debtDataMissing(draft)
  const field = (key: keyof typeof f, label: string, extra: Record<string, string> = {}) => (
    <label className="flex flex-col gap-1 text-sm">
      <span className="text-slate-600">{label}</span>
      <input data-testid={`party-data-${key}`} value={f[key]} onChange={(e) => setF({ ...f, [key]: e.target.value })} {...extra} />
    </label>
  )
  return (
    <Modal title={`Данные клиента: ${party.name}`} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="party-data-modal">
        {company ? (
          field('name', 'Название')
        ) : (
          <>
            {field('last_name', 'Фамилия')}
            {field('first_name', 'Имя')}
          </>
        )}
        {field('phone', 'Телефон', { inputMode: 'tel' })}
        {field('inn', company ? 'ИНН фирмы' : 'ПИН (14 цифр с паспорта)', { inputMode: 'numeric' })}
        {missing.length > 0 && <div className="text-sm text-amber-800">Для долга не хватает: {missing.join(', ')}</div>}
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button
            data-testid="party-data-save"
            disabled={act.busy}
            onClick={() =>
              void act.run(async () => {
                const body = company
                  ? { name: f.name.trim(), phone: f.phone.trim(), inn: f.inn.trim() }
                  : { first_name: f.first_name.trim(), last_name: f.last_name.trim(), phone: f.phone.trim(), inn: f.inn.trim() }
                const saved = await patch<Party>(`/parties/${party.id}`, body)
                toast('Записано в карточку клиента')
                onSaved(saved)
              })
            }
          >
            Сохранить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
