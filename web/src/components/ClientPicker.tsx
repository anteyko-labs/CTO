import { useEffect, useRef, useState } from 'react'
import { get, noServerAnswer, patch, post, qs } from '../lib/api'
import { readSnapshot, searchSnapshotClients } from '../lib/offline'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import type { Party, PartyContact, PartyVehicle } from '../lib/types'
import { Badge, Button, ErrorBox, Field, Missing, Modal } from './ui'
import { missingWithFocus } from '../lib/forms'
import { looksLikePlate } from '../lib/serviceSale'
import { VehicleFormModal } from './VehicleFormModal'

/** Что набрали в поиске: ПИН/ИНН, телефон или имя — в какое поле подставить при создании.
 *  Физлицо это или фирма, по цифрам не понять (ПИН и ИНН оба по 14 цифр): выбирает кассир. */
function guessField(text: string): 'inn' | 'phone' | 'name' {
  const digits = text.replace(/[^\d]/g, '')
  const onlyDigits = /^[\d\s+()-]+$/.test(text)
  if (onlyDigits && digits.length >= 12) return 'inn'
  if (onlyDigits && digits.length >= 9) return 'phone'
  return 'name'
}

/** Госномер без пробелов и заглавными — для сравнения. */
const plateKey = (v: string) => v.replace(/\s/g, '').toUpperCase()

/** Долг клиента словами: кто кому должен. */
export function balanceText(balance: number): string {
  if (balance === 0) return 'долга нет'
  return balance > 0 ? `долг ${formatSom(balance)}` : `аванс ${formatSom(-balance)}`
}

/** Новый клиент прямо из кассы: минимум полей, остальное потом в карточке. */
function NewClientModal({ initial, onClose, onCreated }: { initial: string; onClose: () => void; onCreated: (p: Party) => void }) {
  const field = guessField(initial)
  // Набранное слово — в название фирмы; у человека — в имя, фамилию кассир допишет (ADR-083).
  const [form, setForm] = useState({
    kind: null as Party['kind'] | null,
    name: field === 'name' ? initial : '',
    first: field === 'name' ? initial : '',
    last: '',
    phone: field === 'phone' ? initial : '',
    inn: field === 'inn' ? initial : '',
  })
  const { busy, error, run } = useAction()
  const company = form.kind === 'company'
  const notFilled = missingWithFocus(
    [form.kind !== null, 'физлицо или юрлицо', '[data-kind]'],
    [form.kind !== 'company' || Boolean(form.name.trim()), 'название фирмы', '#client-name'],
    [form.kind !== 'person' || Boolean(form.first.trim()), 'имя', '#client-first'],
    [form.kind !== 'person' || Boolean(form.last.trim()), 'фамилию', '#client-last'],
    [!company || Boolean(form.inn.trim()), 'ИНН фирмы', '#client-inn'],
  )

  const save = () =>
    run(async () => {
      if (!form.kind) return
      const p = await post<Party>('/parties', {
        role: 'customer',
        kind: form.kind,
        ...(company ? { name: form.name.trim() } : { first_name: form.first.trim(), last_name: form.last.trim() }),
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
        {company || form.kind === null ? (
          <Field label={company ? 'Название фирмы' : 'Имя или название'} required>
            <input id="client-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value, first: e.target.value })} />
          </Field>
        ) : (
          <div className="grid grid-cols-2 gap-3">
            <Field label="Имя" required>
              <input id="client-first" value={form.first} onChange={(e) => setForm({ ...form, first: e.target.value })} />
            </Field>
            <Field label="Фамилия" required>
              <input id="client-last" autoFocus value={form.last} onChange={(e) => setForm({ ...form, last: e.target.value })} />
            </Field>
          </div>
        )}
        <Field label="Телефон">
          <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
        </Field>
        <Field
          label={company ? 'ИНН фирмы' : 'ПИН (14 цифр с паспорта)'}
          required={company}
          hint={company ? 'Нужен для документа о долге' : 'Нужен, если берёт в долг'}
        >
          <input
            id="client-inn"
            inputMode="numeric"
            value={form.inn}
            onChange={(e) => setForm({ ...form, inn: e.target.value })}
            onKeyDown={(e) => {
              // Enter в последнем поле — как «Создать» (ui-rules §4).
              if (e.key === 'Enter' && !busy && notFilled.length === 0) {
                e.preventDefault()
                void save()
              }
            }}
          />
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
  service = false,
}: {
  party: Party | null
  contactId: string
  vehicleId: string
  /** Чек «В сервис»: клиент обязателен (ADR-100). */
  service?: boolean
  onParty: (p: Party | null) => void
  onContact: (id: string) => void
  onVehicle: (id: string) => void
}) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 250)
  // Заводить и править клиентов и машины — право администратора «Клиенты и машины» (ADR-097).
  const can = useCan()
  const mayEdit = can('clients')
  // Новый клиент: что набрали в поиске, то и подставим; тип выбирает кассир.
  const [creating, setCreating] = useState<string | null>(null)
  // Нашли клиента по госномеру — эту машину ставим, как загрузится карточка; искали по имени или
  // телефону — ставим единственную машину клиента (ниже).
  const [platePick, setPlatePick] = useState<string | null>(null)
  const choose = (p: Party) => {
    onParty(p)
    setPlatePick(looksLikePlate(query) ? query : null)
    setQ('')
  }
  const kindAct = useAction()
  const [adding, setAdding] = useState<'contact' | 'vehicle' | null>(null)
  const add = useAction()
  const found = useLoad(
    () =>
      query.length < 2
        ? Promise.resolve<Party[]>([])
        : get<Party[]>(`/parties${qs({ role: 'customer', q: query, limit: 8 })}`).catch(async (e: unknown) => {
            // Без связи — клиенты из снимка кассы: должники и недавние покупатели с машинами (ADR-093).
            if (!noServerAnswer(e)) throw e
            return searchSnapshotClients(await readSnapshot(), query)
          }),
    [query],
  )
  const card = useLoad(
    () =>
      party
        ? get<{ contacts: PartyContact[]; vehicles: PartyVehicle[] }>(`/parties/${party.id}/card`)
            .catch(async (e: unknown) => {
              const known = noServerAnswer(e) ? (await readSnapshot())?.clients?.find((c) => c.id === party.id) : undefined
              if (!known) throw e
              return { contacts: known.contacts as PartyContact[], vehicles: known.vehicles }
            })
            .then((c) => ({ ...c, partyId: party.id }))
        : Promise.resolve(null),
    [party?.id],
  )

  const addRow = (value: string) =>
    void add.run(async () => {
      if (!party || !adding) return
      const c = await post<PartyContact>(`/parties/${party.id}/contacts`, { full_name: value })
      onContact(c.id)
      setAdding(null)
      card.reload()
    })

  // Машина сама: совпала с набранным госномером — она; иначе единственная машина клиента.
  // Один раз на выбранного клиента: если кассир убрал машину, обратно не подставляем.
  const autoPicked = useRef<string | null>(null)
  useEffect(() => {
    if (!party) {
      autoPicked.current = null
      return
    }
    if (!card.data || card.data.partyId !== party.id || autoPicked.current === party.id) return
    autoPicked.current = party.id
    const active = card.data.vehicles.filter((v) => v.active)
    const byPlate = platePick ? active.find((x) => plateKey(x.plate).includes(plateKey(platePick))) : undefined
    setPlatePick(null)
    if (vehicleId) return
    if (byPlate) onVehicle(byPlate.id)
    else if (active.length === 1) onVehicle(active[0].id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [party?.id, card.data])

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
              {mayEdit && (
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
              )}
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
                mayEdit && (
                  <button type="button" className="text-sky-700 underline" onClick={() => setAdding('contact')}>
                    + новый работник
                  </button>
                )
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
          </>
        )}
        <Field
          label="Машина"
          hint={
            mayEdit && (
              <button type="button" className="text-sky-700 underline" onClick={() => setAdding('vehicle')}>
                + новая машина
              </button>
            )
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
        <ErrorBox error={add.error ?? card.error ?? kindAct.error} />
        {adding === 'vehicle' && (
          <VehicleFormModal
            partyId={party.id}
            onClose={() => setAdding(null)}
            onSaved={(v) => {
              setAdding(null)
              onVehicle(v.id)
              card.reload()
            }}
          />
        )}
        {adding === 'contact' && (
          <NewRowModal
            title="Новый работник"
            label="ФИО"
            hint="Телефон и должность можно добавить в карточке"
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
      <Field
        label="Клиент"
        required={service}
        hint={service ? 'Для чека «В сервис» обязателен' : can('sale_debt') ? 'Можно не указывать. Для продажи в долг — обязателен' : 'Можно не указывать'}
      >
        <input
          data-client-input
          placeholder="Госномер, телефон, имя или ИНН"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              if (list[0]) {
                choose(list[0])
              } else if (query.length >= 2 && mayEdit) {
                setCreating(query)
              }
            }
          }}
        />
      </Field>
      {query.length >= 2 && (
        <ul className="absolute top-full z-30 mt-1 max-h-72 w-full overflow-y-auto overscroll-contain rounded-md border border-slate-200 bg-white shadow-lg">
          {list.map((p) => (
            <li key={p.id}>
              <button
                type="button"
                className="flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-sm hover:bg-slate-50"
                onClick={() => choose(p)}
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
          {mayEdit && (
            <li className="flex items-center justify-between gap-2 border-t border-slate-200 px-3 py-2">
              <button type="button" className="text-left text-sm font-medium text-sky-700 hover:underline" onClick={() => setCreating(query)}>
                + Новый клиент «{query}»
              </button>
            </li>
          )}
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
