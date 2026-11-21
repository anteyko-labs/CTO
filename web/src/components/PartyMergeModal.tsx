// Объединение карточек клиентов или поставщиков (задача 043): выбранные карточки вливаются в эту —
// долг общий, люди становятся агентами фирмы, машины и кабинет переходят, влитые — в архив.
import { useState } from 'react'
import { Button, ErrorBox, Field, Modal, toast } from './ui'
import { get, newOpId, post, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { becomesAgents, mergedBalance } from '../lib/partyMerge'
import type { Party } from '../lib/types'

export function PartyMergeModal({ party, onClose, onDone }: { party: Party; onClose: () => void; onDone: () => void }) {
  const [query, setQuery] = useState('')
  const [picked, setPicked] = useState<Party[]>([])
  const [kind, setKind] = useState<'person' | 'company'>(party.kind === 'company' ? 'company' : 'person')
  const [name, setName] = useState(party.kind === 'company' ? party.name : '')
  const [first, setFirst] = useState(party.first_name ?? '')
  const [last, setLast] = useState(party.last_name ?? '')
  // Один op_id на окно: повторное нажатие после обрыва связи не сливает дважды (I8).
  const [opId] = useState(newOpId)
  const act = useAction()
  const found = useLoad(
    () =>
      query.trim().length < 2
        ? Promise.resolve<Party[]>([])
        : get<Party[]>(`/parties${qs({ role: party.role, q: query.trim(), limit: 10 })}`),
    [query, party.role],
  )
  const options = (found.data ?? []).filter((p) => p.id !== party.id && !picked.some((x) => x.id === p.id))
  const total = mergedBalance(party, picked)
  const agents = becomesAgents(kind, picked)
  const client = party.role === 'customer'

  const submit = () =>
    void act.run(async () => {
      if (picked.length === 0) throw new Error('Выберите карточки для объединения')
      if (kind === 'company' && !name.trim()) throw new Error('Укажите название')
      if (kind === 'person' && (!first.trim() || !last.trim())) throw new Error('Укажите имя и фамилию')
      await post(`/parties/${party.id}/merge`, {
        op_id: opId,
        duplicate_ids: picked.map((p) => p.id),
        kind,
        ...(kind === 'company' ? { name: name.trim() } : { first_name: first.trim(), last_name: last.trim() }),
      })
      toast('Карточки объединены')
      onDone()
    })

  return (
    <Modal title="Объединить карточки" onClose={onClose} wide>
      <div className="flex flex-col gap-3 text-sm">
        <Field label={client ? 'Найти клиента, которого влить сюда' : 'Найти поставщика, которого влить сюда'}>
          <input
            data-testid="merge-search"
            autoFocus
            value={query}
            placeholder="имя, телефон, госномер"
            onChange={(e) => setQuery(e.target.value)}
          />
        </Field>
        {options.length > 0 && (
          <div className="flex flex-col divide-y rounded border">
            {options.map((p) => (
              <button
                key={p.id}
                type="button"
                data-testid="merge-option"
                className="flex justify-between px-3 py-2 text-left hover:bg-slate-50"
                onClick={() => setPicked([...picked, p])}
              >
                <span>
                  {p.name}
                  {p.phone && <span className="text-slate-500"> · {p.phone}</span>}
                </span>
                <span>{formatSom(p.balance_tyiyn)}</span>
              </button>
            ))}
          </div>
        )}
        <div>
          <div className="text-xs text-slate-500">Вливаются в эту карточку</div>
          {picked.length === 0 && <div className="text-slate-500">пока никого</div>}
          {picked.map((p) => (
            <div key={p.id} className="flex items-center justify-between py-1" data-testid="merge-picked">
              <span>
                {p.name} — {formatSom(p.balance_tyiyn)}
              </span>
              <button type="button" className="text-xs text-rose-700 underline" onClick={() => setPicked(picked.filter((x) => x.id !== p.id))}>
                убрать
              </button>
            </div>
          ))}
        </div>
        {client && (
          <Field label="Кто это после объединения">
            <select value={kind} onChange={(e) => setKind(e.target.value as 'person' | 'company')}>
              <option value="company">Фирма</option>
              <option value="person">Человек</option>
            </select>
          </Field>
        )}
        {kind === 'company' ? (
          <Field label="Название после объединения" required>
            <input data-testid="merge-name" value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
        ) : (
          <div className="grid grid-cols-2 gap-2">
            <Field label="Фамилия" required>
              <input value={last} onChange={(e) => setLast(e.target.value)} />
            </Field>
            <Field label="Имя" required>
              <input value={first} onChange={(e) => setFirst(e.target.value)} />
            </Field>
          </div>
        )}
        <div className="rounded bg-slate-50 p-3" data-testid="merge-total">
          Долг: {formatSom(party.balance_tyiyn)}
          {picked.map((p) => ` + ${formatSom(p.balance_tyiyn)}`)} → станет <b>{formatSom(total)}</b>
          {agents.length > 0 && <div className="mt-1 text-slate-600">Станут агентами фирмы: {agents.join(', ')}</div>}
          <div className="mt-1 text-xs text-slate-500">
            Машины, работники, кабинет и Telegram перейдут сюда. Влитые карточки уйдут в архив; их чеки будут видны здесь.
            Отменить объединение нельзя.
          </div>
        </div>
        {act.error && <ErrorBox error={act.error} />}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button data-testid="merge-submit" disabled={act.busy || picked.length === 0} onClick={submit}>
            Объединить
          </Button>
        </div>
      </div>
    </Modal>
  )
}
