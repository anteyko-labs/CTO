import { useState } from 'react'
import { get, patch, post, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Party, PartyContact, PartyVehicle } from '../lib/types'
import { Badge, Button, ErrorBox, Field, Missing, Modal } from './ui'
import { missingWithFocus } from '../lib/forms'

/** Что набрали в поиске: ПИН/ИНН, телефон или имя — в какое поле подставить при создании.
 *  Физлицо это или фирма, по цифрам не понять (ПИН и ИНН оба по 14 цифр): выбирает кассир. */
function guessField(text: string): 'inn' | 'phone' | 'name' {
  const digits = text.replace(/[^\d]/g, '')
  const onlyDigits = /^[\d\s+()-]+$/.test(text)
  if (onlyDigits && digits.length >= 12) return 'inn'
  if (onlyDigits && digits.length >= 9) return 'phone'
  return 'name'
}

/** Долг клиента словами: кто кому должен. */
export function balanceText(balance: number): string {
  if (balance === 0) return 'долга нет'
  return balance > 0 ? `долг ${formatSom(balance)}` : `аванс ${formatSom(-balance)}`
}

/** Новый клиент прямо из кассы: минимум полей, остальное потом в карточке. */
function NewClientModal({ initial, onClose, onCreated }: { initial: string; onClose: () => void; onCreated: (p: Party) => void }) {
  const field = guessField(initial)
  const [form, setForm] = useState({
    kind: null as Party['kind'] | null,
    name: field === 'name' ? initial : '',
    phone: field === 'phone' ? initial : '',
    inn: field === 'inn' ? initial : '',
  })
  const { busy, error, run } = useAction()
  const company = form.kind === 'company'
  const notFilled = missingWithFocus(
    [form.kind !== null, 'физлицо или юрлицо', '[data-kind]'],
    [Boolean(form.name.trim()), company ? 'название фирмы' : 'ФИО', '#client-name'],
    [!company || Boolean(form.inn.trim()), 'ИНН фирмы', '#client-inn'],
  )

  const save = () =>
    run(async () => {
      if (!form.kind) return
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
        <div className="text-sm font-medium">Кто клиент?</div>
        <div data-kind tabIndex={-1} className="grid grid-cols-2 gap-2 text-sm">
          {(['person', 'company'] as const).map((k) => (
            <button
              key={k}
              type="button"
              aria-pressed={form.kind === k}
              className={`flex min-h-[56px] flex-col items-start justify-center rounded-md border px-3 py-2 text-left ${form.kind === k ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
              onClick={() => setForm({ ...form, kind: k })}
            >
              <span className="font-medium">{k === 'person' ? 'Физлицо' : 'Юрлицо'}</span>
              <span className={`text-xs ${form.kind === k ? 'text-sky-100' : 'text-slate-500'}`}>
                {k === 'person' ? 'человек, документ с ПИН' : 'фирма, ИП, документ с ИНН'}
              </span>
            </button>
          ))}
        </div>
        <Field label={company ? 'Название фирмы' : 'ФИО'} required>
          <input id="client-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
        </Field>
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
        <Field
          label={company ? 'ИНН фирмы' : 'ПИН (14 цифр с паспорта)'}
          required={company}
          hint={company ? 'Нужен для документа о долге' : 'Нужен, если берёт в долг'}
        >
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
  // Новый клиент: что набрали в поиске, то и подставим; тип выбирает кассир.
  const [creating, setCreating] = useState<string | null>(null)
  const kindAct = useAction()
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
              <span className="font-medium">{party.kind === 'company' ? 'Юрлицо' : 'Физлицо'}</span>{' '}
              <button
                type="button"
                className="text-sky-700 underline"
                disabled={kindAct.busy}
                onClick={() =>
                  void kindAct.run(async () => {
                    const kind = party.kind === 'company' ? 'person' : 'company'
                    const saved = await patch<Party>(`/parties/${party.id}`, { kind })
                    onParty({ ...party, kind: saved.kind })
                  })
                }
              >
                {party.kind === 'company' ? 'это физлицо' : 'это юрлицо'}
              </button>
              {party.inn && ` · ${party.kind === 'company' ? 'ИНН' : 'ПИН'} ${party.inn}`} · {balanceText(party.balance_tyiyn)}
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
        <ErrorBox error={add.error ?? card.error ?? kindAct.error} />
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

  const list = (found.data ?? []).slice(0, 5)

  return (
    <div className="relative flex flex-col gap-1">
      <Field label="Клиент" hint="Можно не указывать. Для продажи в долг — обязателен">
        <input
          data-client-input
          placeholder="ИНН, имя или телефон"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              if (list[0]) {
                onParty(list[0])
                setQ('')
              } else if (query.length >= 2) {
                setCreating(query)
              }
            }
          }}
        />
      </Field>
      {query.length >= 2 && (
        <ul className="absolute top-full z-30 mt-1 w-full overflow-hidden rounded-md border border-slate-200 bg-white shadow-lg">
          {list.map((p) => (
            <li key={p.id}>
              <button
                type="button"
                className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50"
                onClick={() => {
                  onParty(p)
                  setQ('')
                }}
              >
                <span className="min-w-0">
                  <span className="block truncate font-medium">{p.name}</span>
                  <span className="text-xs text-slate-500">
                    {[p.phone, p.inn && `ИНН ${p.inn}`].filter(Boolean).join(' · ') ||
                      (p.kind === 'company' ? 'юрлицо' : 'физлицо')}
                  </span>
                </span>
                {p.balance_tyiyn !== 0 && (
                  <Badge tone={p.balance_tyiyn > 0 ? 'amber' : 'sky'}>{balanceText(p.balance_tyiyn)}</Badge>
                )}
              </button>
            </li>
          ))}
          {list.length === 0 && !found.loading && (
            <li className="px-3 py-2 text-sm text-slate-500">Никого не нашли по «{query}»</li>
          )}
          <li className="flex items-center justify-between gap-2 border-t border-slate-200 px-3 py-2">
            <button type="button" className="text-left text-sm font-medium text-sky-700 hover:underline" onClick={() => setCreating(query)}>
              + Новый клиент «{query}»
            </button>
          </li>
        </ul>
      )}
      {creating !== null && (
        <NewClientModal
          initial={creating}
          onClose={() => setCreating(null)}
          onCreated={(p) => {
            setCreating(null)
            setQ('')
            onParty(p)
          }}
        />
      )}
    </div>
  )
}
