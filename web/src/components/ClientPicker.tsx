import { useState } from 'react'
import { get, post, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Party, PartyContact, PartyVehicle } from '../lib/types'
import { Badge, Button, ErrorBox, Field, Missing, Modal } from './ui'
import { missingWithFocus } from '../lib/forms'

/** Долг клиента словами: кто кому должен. */
export function balanceText(balance: number): string {
  if (balance === 0) return 'долга нет'
  return balance > 0 ? `долг ${formatSom(balance)}` : `аванс ${formatSom(-balance)}`
}

/** Новый клиент прямо из кассы: минимум полей, остальное потом в карточке. */
function NewClientModal({ onClose, onCreated }: { onClose: () => void; onCreated: (p: Party) => void }) {
  const [form, setForm] = useState({ kind: 'person' as Party['kind'], name: '', phone: '', inn: '' })
  const { busy, error, run } = useAction()
  const company = form.kind === 'company'
  const notFilled = missingWithFocus(
    [Boolean(form.name.trim()), company ? 'название фирмы' : 'ФИО', '#client-name'],
    [!company || Boolean(form.inn.trim()), 'ИНН фирмы', '#client-inn'],
  )

  const save = () =>
    run(async () => {
      const p = await post<Party>('/parties', {
        role: 'customer',
        kind: form.kind,
        name: form.name.trim(),
        phone: form.phone.trim(),
        inn: form.inn.trim(),
      })
      onCreated(p)
    })

  return (
    <Modal title="Новый клиент" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
          {(['person', 'company'] as const).map((k) => (
            <button
              key={k}
              type="button"
              className={`min-h-[42px] py-2 ${form.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
              onClick={() => setForm({ ...form, kind: k })}
            >
              {k === 'person' ? 'Физлицо' : 'Юрлицо'}
            </button>
          ))}
        </div>
        <Field label={company ? 'Название фирмы' : 'ФИО'} required>
          <input id="client-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
        <Field label="ИНН" required={company} hint={company ? 'Нужен для документа о долге' : 'Если есть'}>
          <input id="client-inn" inputMode="numeric" value={form.inn} onChange={(e) => setForm({ ...form, inn: e.target.value })} />
        </Field>
        <Missing items={notFilled} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || notFilled.length > 0} onClick={() => void save()}>
            Создать
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Новый работник или машина фирмы — не выходя из чека (ADR-030). */
function NewRowModal({
  title,
  label,
  hint,
  busy,
  error,
  onClose,
  onSave,
}: {
  title: string
  label: string
  hint?: string
  busy: boolean
  error: string | null
  onClose: () => void
  onSave: (value: string) => void
}) {
  const [value, setValue] = useState('')
  return (
    <Modal title={title} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <Field label={label} required hint={hint}>
          <input
            autoFocus
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && value.trim()) {
                e.preventDefault()
                onSave(value.trim())
              }
            }}
          />
        </Field>
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !value.trim()} onClick={() => onSave(value.trim())}>
            Добавить
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/**
 * Выбор покупателя в кассе: поиск по названию, телефону и ИНН, создание на месте.
 * Для фирмы под клиентом появляются «кто приехал» и «машина» (SPEC-10, ui-rules §8).
 */
export function ClientPicker({
  party,
  contactId,
  vehicleId,
  onParty,
  onContact,
  onVehicle,
}: {
  party: Party | null
  contactId: string
  vehicleId: string
  onParty: (p: Party | null) => void
  onContact: (id: string) => void
  onVehicle: (id: string) => void
}) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 250)
  const [creating, setCreating] = useState(false)
  const [adding, setAdding] = useState<'contact' | 'vehicle' | null>(null)
  const add = useAction()
  const found = useLoad(
    () =>
      query.length < 2
        ? Promise.resolve<Party[]>([])
        : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`),
    [query],
  )
  const card = useLoad(
    () =>
      party && party.kind === 'company'
        ? get<{ contacts: PartyContact[]; vehicles: PartyVehicle[] }>(`/parties/${party.id}/card`)
        : Promise.resolve(null),
    [party?.id],
  )

  const addRow = (value: string) =>
    void add.run(async () => {
      if (!party || !adding) return
      if (adding === 'contact') {
        const c = await post<PartyContact>(`/parties/${party.id}/contacts`, { full_name: value })
        onContact(c.id)
      } else {
        const v = await post<PartyVehicle>(`/parties/${party.id}/vehicles`, { plate: value })
        onVehicle(v.id)
      }
      setAdding(null)
      card.reload()
    })

  if (party) {
    const contacts = card.data?.contacts.filter((c) => c.active) ?? []
    const vehicles = card.data?.vehicles.filter((v) => v.active) ?? []
    return (
      <div className="flex flex-col gap-2">
        <div className="flex items-start justify-between gap-2 rounded-md bg-slate-50 px-3 py-2">
          <div className="min-w-0">
            <div className="truncate font-medium">{party.name}</div>
            <div className="text-xs text-slate-600">
              {party.kind === 'company' ? 'Юрлицо' : 'Физлицо'}
              {party.inn && ` · ИНН ${party.inn}`} · {balanceText(party.balance_tyiyn)}
            </div>
          </div>
          <button type="button" className="shrink-0 text-slate-400 hover:text-rose-600" aria-label="Убрать клиента" onClick={() => onParty(null)}>
            ✕
          </button>
        </div>
        {party.kind === 'company' && (
          <>
            <Field
              label="Кто приехал"
              hint={
                <button type="button" className="text-sky-700 underline" onClick={() => setAdding('contact')}>
                  + новый работник
                </button>
              }
            >
              <select value={contactId} onChange={(e) => onContact(e.target.value)}>
                <option value="">— не указан —</option>
                {contacts.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.full_name}
                    {c.position && ` · ${c.position}`}
                  </option>
                ))}
              </select>
            </Field>
            <Field
              label="Машина"
              hint={
                <button type="button" className="text-sky-700 underline" onClick={() => setAdding('vehicle')}>
                  + новая машина
                </button>
              }
            >
              <select value={vehicleId} onChange={(e) => onVehicle(e.target.value)}>
                <option value="">— не указана —</option>
                {vehicles.map((v) => (
                  <option key={v.id} value={v.id}>
                    {v.plate}
                    {v.brand && ` · ${v.brand} ${v.model}`}
                  </option>
                ))}
              </select>
            </Field>
          </>
        )}
        <ErrorBox error={add.error ?? card.error} />
        {adding && (
          <NewRowModal
            title={adding === 'contact' ? 'Новый работник' : 'Новая машина'}
            label={adding === 'contact' ? 'ФИО' : 'Госномер'}
            hint={adding === 'contact' ? 'Телефон и должность можно добавить в карточке' : 'Марку и модель можно добавить в карточке'}
            busy={add.busy}
            error={add.error}
            onClose={() => setAdding(null)}
            onSave={addRow}
          />
        )}
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-1">
      <Field label="Клиент" hint="Можно не указывать. Для продажи в долг — обязателен">
        <input placeholder="Название, телефон или ИНН" value={q} onChange={(e) => setQ(e.target.value)} />
      </Field>
      {query.length >= 2 && (
        <div className="flex flex-col gap-1">
          {(found.data ?? []).map((p) => (
            <button
              key={p.id}
              type="button"
              className="flex items-center justify-between gap-2 rounded-md border border-slate-200 px-2 py-1.5 text-left text-sm hover:bg-slate-50"
              onClick={() => {
                onParty(p)
                setQ('')
              }}
            >
              <span className="min-w-0">
                <span className="block truncate font-medium">{p.name}</span>
                <span className="text-xs text-slate-500">
                  {p.phone || p.inn || (p.kind === 'company' ? 'юрлицо' : 'физлицо')}
                </span>
              </span>
              {p.balance_tyiyn !== 0 && <Badge tone={p.balance_tyiyn > 0 ? 'amber' : 'sky'}>{balanceText(p.balance_tyiyn)}</Badge>}
            </button>
          ))}
          {!found.loading && (found.data ?? []).length === 0 && <div className="text-xs text-slate-500">Никого не нашли</div>}
          <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setCreating(true)}>
            + новый клиент
          </Button>
        </div>
      )}
      {creating && (
        <NewClientModal
          onClose={() => setCreating(false)}
          onCreated={(p) => {
            setCreating(false)
            setQ('')
            onParty(p)
          }}
        />
      )}
    </div>
  )
}
