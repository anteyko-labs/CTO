// Оплата долга: клиент гасит нам или мы платим поставщику, деньги проходят через кассу (SPEC-10).
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Party } from '../lib/types'
import { Button, Empty, ErrorBox, Field, Loading, Missing, Modal, toast } from './ui'

type Method = 'cash' | 'card' | 'transfer' | 'outside'

const METHOD_LABELS: Record<Method, string> = {
  cash: 'Наличные',
  card: 'Карта',
  transfer: 'QR',
  outside: 'Вне кассы',
}

const METHOD_HINTS: Record<Method, string> = {
  cash: 'Деньги в кассе смены: смена должна быть открыта.',
  card: 'Оплата картой через терминал: деньги на «Счёт», банк возьмёт 0,5 %.',
  transfer: 'Оплата QR через терминал: деньги на «Счёт», банк возьмёт 0,5 %.',
  outside: 'Мимо денег точки: например, владелец заплатил из своих.',
}

export function RepaymentModal({
  party,
  onClose,
  onDone,
}: {
  party: Pick<Party, 'id' | 'name' | 'balance_tyiyn' | 'role'>
  onClose: () => void
  onDone: (balance: number) => void
}) {
  const owner = useUser().role === 'owner'
  const supplier = party.role === 'supplier'
  const owed = Math.abs(party.balance_tyiyn)
  const [sum, setSum] = useState(owed > 0 ? somInput(owed) : '')
  const [method, setMethod] = useState<Method>('cash')
  const [comment, setComment] = useState('')
  const { busy, error, run } = useAction()
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит оплату.
  const [opId] = useState(newOpId)
  // Администратор поставщику платит только из кассы; мимо кассы — только владелец.
  const methods: Method[] = owner ? ['cash', 'card', 'transfer', 'outside'] : supplier ? ['cash'] : ['cash', 'card', 'transfer']
  const value = parseSom(sum)
  const notFilled = missingWithFocus([value !== null && value > 0, 'сумму', '#repay-sum'])

  const save = () =>
    void run(async () => {
      if (value === null) return
      const res = await post<{ balance_tyiyn: number }>('/debts/repayments', {
        op_id: opId,
        party_id: party.id,
        amount_tyiyn: value,
        method,
        comment,
      })
      toast(supplier ? 'Оплата поставщику записана' : `Оплата долга принята: ${formatSom(value)}`)
      onDone(res.balance_tyiyn)
    })

  return (
    <Modal title={supplier ? `Оплата поставщику: ${party.name}` : `Оплата долга: ${party.name}`} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="text-sm text-slate-600">
          {party.balance_tyiyn > 0 ? 'Должен нам' : party.balance_tyiyn < 0 ? 'Должны мы' : 'Долга нет'}
          {owed > 0 && `: ${formatSom(owed)}`}
        </div>
        <Field label="Сумма, с" required>
          <input id="repay-sum" autoFocus inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
        </Field>
        {value !== null && value > owed && owed > 0 && (
          <div className="text-xs text-amber-700">Больше долга: разница станет {supplier ? 'нашим авансом поставщику' : 'авансом клиента'}.</div>
        )}
        <div className={`grid overflow-hidden rounded-md border border-slate-300 text-sm`} style={{ gridTemplateColumns: `repeat(${methods.length}, minmax(0, 1fr))` }}>
          {methods.map((m) => (
            <button
              key={m}
              type="button"
              className={`min-h-[42px] py-2 ${method === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
              onClick={() => setMethod(m)}
            >
              {METHOD_LABELS[m]}
            </button>
          ))}
        </div>
        <div className="text-xs text-slate-500">
          {METHOD_HINTS[method]} В прибыль оплата долга не идёт: выручка и прибыль по чеку учтены в день продажи.
        </div>
        <Field label="Комментарий">
          <input value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <Missing items={notFilled} />
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || notFilled.length > 0} onClick={save}>
            {supplier ? 'Оплатить' : 'Принять оплату'}
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Касса: найти должника и принять от него оплату. */
export function DebtorPayment({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q)
  const list = useLoad(() => get<Party[]>(`/parties${qs({ role: 'customer', only_debtors: true, q: query })}`), [query])
  const [party, setParty] = useState<Party | null>(null)
  const debtors = (list.data ?? []).filter((p) => p.balance_tyiyn > 0)

  if (party) return <RepaymentModal party={party} onClose={onClose} onDone={onClose} />
  return (
    <Modal title="Оплата долга" onClose={onClose}>
      <div className="flex flex-col gap-3">
        <input autoFocus placeholder="Имя, телефон или ИНН должника" value={q} onChange={(e) => setQ(e.target.value)} />
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : debtors.length === 0 ? (
          <Empty>{query ? 'Должник не найден' : 'Должников нет'}</Empty>
        ) : (
          <ul className="flex max-h-80 flex-col divide-y divide-slate-100 overflow-y-auto">
            {debtors.map((p) => (
              <li key={p.id}>
                <button type="button" className="flex w-full items-center justify-between gap-3 px-2 py-2 text-left hover:bg-slate-50" onClick={() => setParty(p)}>
                  <span>
                    <span className="font-medium">{p.name}</span>
                    {p.phone && <span className="ml-2 text-xs text-slate-500">{p.phone}</span>}
                  </span>
                  <span className="font-semibold text-rose-700">{formatSom(p.balance_tyiyn)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Modal>
  )
}
