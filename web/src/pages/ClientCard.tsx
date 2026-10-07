import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { balanceText } from '../components/ClientPicker'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Party, PartyCard, PartyTimelineItem } from '../lib/types'

const KIND_TONE: Record<PartyTimelineItem['kind'], 'slate' | 'green' | 'amber' | 'rose' | 'sky'> = {
  sale: 'slate',
  sale_return: 'rose',
  debt: 'amber',
  repayment: 'green',
  adjust: 'sky',
}

/** Погашение долга или правка баланса владельцем. */
function MoneyModal({
  title,
  party,
  adjust,
  onClose,
  onDone,
}: {
  title: string
  party: Party
  adjust: boolean
  onClose: () => void
  onDone: () => void
}) {
  const [sum, setSum] = useState('')
  const [comment, setComment] = useState('')
  const [sign, setSign] = useState<1 | -1>(adjust ? 1 : 1)
  const { busy, error, run } = useAction()

  const save = () =>
    run(async () => {
      const v = parseSom(sum)
      if (v === null || v <= 0) throw new Error('Неверная сумма')
      if (adjust) {
        if (!comment.trim()) throw new Error('Укажите причину правки')
        await post('/debts/adjust', { op_id: newOpId(), party_id: party.id, amount_tyiyn: sign * v, comment })
      } else {
        await post('/debts/repayments', { op_id: newOpId(), party_id: party.id, amount_tyiyn: v, comment })
      }
      toast(adjust ? 'Баланс исправлен' : 'Погашение записано')
      onDone()
    })

  return (
    <Modal title={title} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <div className="text-sm text-slate-600">
          {party.name}: {balanceText(party.balance_tyiyn)}
        </div>
        {adjust && (
          <div className="grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
            {(
              [
                [1, 'Увеличить долг'],
                [-1, 'Простить долг'],
              ] as const
            ).map(([v, label]) => (
              <button
                key={v}
                type="button"
                className={`min-h-[42px] py-2 ${sign === v ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setSign(v)}
              >
                {label}
              </button>
            ))}
          </div>
        )}
        <Field label="Сумма, с" required>
          <input autoFocus inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
        </Field>
        <Field label="Комментарий" required={adjust}>
          <input value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        {!adjust && (
          <div className="text-xs text-slate-500">
            Деньги попадут в кассу вместе со сменой на этапе «День»; сейчас записывается только долг.
          </div>
        )}
        <ErrorBox error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={busy || !sum.trim()} onClick={() => void save()}>
            Записать
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/** Карточка клиента: баланс, что покупал, когда брал в долг и когда гасил (SPEC-10). */
export default function ClientCard() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const owner = useUser().role === 'owner'
  const [money, setMoney] = useState<null | 'repay' | 'adjust'>(null)
  const [adding, setAdding] = useState<'contact' | 'vehicle' | null>(null)
  const [limits, setLimits] = useState<{ limit: string; days: string } | null>(null)
  const [value, setValue] = useState('')
  const row = useAction()
  const card = useLoad(() => get<PartyCard>(`/parties/${id}/card`), [id])

  if (card.loading && !card.data) return <Loading />
  if (!card.data) return <ErrorBox error={card.error ?? 'Клиент не найден'} />
  const { party, contacts, vehicles, timeline } = card.data

  const addRow = () =>
    void row.run(async () => {
      const v = value.trim()
      if (!v) throw new Error('Заполните поле')
      if (adding === 'contact') await post(`/parties/${party.id}/contacts`, { full_name: v })
      else await post(`/parties/${party.id}/vehicles`, { plate: v })
      setAdding(null)
      setValue('')
      card.reload()
    })

  /** Лимит долга и срок оплаты правит только владелец (SPEC-10). */
  const saveLimits = () =>
    void row.run(async () => {
      if (!limits) return
      const limit = limits.limit.trim() ? parseSom(limits.limit) : null
      if (limits.limit.trim() && limit === null) throw new Error('Неверная сумма лимита')
      const days = limits.days.trim() ? Number(limits.days) : null
      if (days !== null && (!Number.isInteger(days) || days < 0)) throw new Error('Неверный срок')
      await patch<Party>(`/parties/${party.id}`, { credit_limit_tyiyn: limit, due_days: days })
      setLimits(null)
      card.reload()
      toast('Сохранено')
    })

  const toggleActive = () =>
    void row.run(async () => {
      await patch<Party>(`/parties/${party.id}`, { active: !party.active })
      card.reload()
    })

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={party.name}
        actions={
          <>
            <Button variant="secondary" onClick={() => navigate('/clients')}>
              К списку
            </Button>
            {owner && (
              <Button variant="secondary" onClick={() => setMoney('adjust')}>
                Правка баланса
              </Button>
            )}
            <Button disabled={party.balance_tyiyn <= 0} onClick={() => setMoney('repay')}>
              Погасить
            </Button>
          </>
        }
      />

      <Card className="flex flex-wrap items-start gap-x-8 gap-y-3 text-sm">
        <div>
          <div className="text-xs text-slate-500">Баланс</div>
          <div className={`text-2xl font-bold ${party.balance_tyiyn > 0 ? 'text-amber-700' : 'text-slate-900'}`}>
            {formatSom(Math.abs(party.balance_tyiyn))}
          </div>
          <div className="text-xs text-slate-500">{balanceText(party.balance_tyiyn)}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Покупок</div>
          <div className="font-medium">
            {card.data.purchases} на {formatSom(card.data.purchases_tyiyn)}
          </div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Брал в долг</div>
          <div className="font-medium">{formatSom(card.data.debt_taken_tyiyn)}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Погасил</div>
          <div className="font-medium">{formatSom(card.data.repaid_tyiyn)}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Телефон</div>
          <div className="font-medium">{party.phone || '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">{party.kind === 'company' ? 'ИНН фирмы' : 'ИНН'}</div>
          <div className="font-medium">{party.inn || '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Лимит долга</div>
          <div className="font-medium">
            {party.credit_limit_tyiyn !== null ? formatSom(party.credit_limit_tyiyn) : 'без лимита'}
            {party.due_days !== null && <span className="text-xs text-slate-500"> · оплата {party.due_days} дн.</span>}
          </div>
          {owner &&
            (limits ? (
              <div className="mt-1 flex flex-wrap items-center gap-2">
                <input
                  className="w-28"
                  inputMode="decimal"
                  placeholder="лимит, с"
                  value={limits.limit}
                  onChange={(e) => setLimits({ ...limits, limit: e.target.value })}
                />
                <input
                  className="w-20"
                  inputMode="numeric"
                  placeholder="дней"
                  value={limits.days}
                  onChange={(e) => setLimits({ ...limits, days: e.target.value })}
                />
                <Button className="px-2 py-1 text-xs" disabled={row.busy} onClick={saveLimits}>
                  Сохранить
                </Button>
                <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setLimits(null)}>
                  Отмена
                </Button>
              </div>
            ) : (
              <button
                type="button"
                className="mt-1 text-xs text-sky-700 underline"
                onClick={() =>
                  setLimits({
                    limit: party.credit_limit_tyiyn !== null ? String(Math.trunc(party.credit_limit_tyiyn / 100)) : '',
                    days: party.due_days !== null ? String(party.due_days) : '',
                  })
                }
              >
                изменить лимит и срок
              </button>
            ))}
        </div>
        <div className="ml-auto flex items-center gap-2">
          {!party.active && <Badge tone="rose">отключён</Badge>}
          <Button variant="secondary" className="px-2 py-1 text-xs" disabled={row.busy} onClick={toggleActive}>
            {party.active ? 'Отключить' : 'Включить'}
          </Button>
        </div>
        {party.comment && <div className="w-full text-slate-600">{party.comment}</div>}
      </Card>

      {party.kind === 'company' && (
        <div className="grid gap-4 md:grid-cols-2">
          <Card>
            <div className="mb-3 flex items-center justify-between">
              <h2 className="font-semibold">Работники</h2>
              <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setAdding('contact')}>
                + работник
              </Button>
            </div>
            {contacts.length === 0 ? (
              <Empty>Никто не записан. Добавьте того, кто приезжает за товаром.</Empty>
            ) : (
              <ul className="flex flex-col gap-1 text-sm">
                {contacts.map((c) => (
                  <li key={c.id} className="flex justify-between gap-2 border-b border-slate-100 py-1 last:border-0">
                    <span className={c.active ? '' : 'text-slate-400'}>{c.full_name}</span>
                    <span className="text-slate-500">{[c.position, c.phone, c.inn].filter(Boolean).join(' · ') || '—'}</span>
                  </li>
                ))}
              </ul>
            )}
          </Card>
          <Card>
            <div className="mb-3 flex items-center justify-between">
              <h2 className="font-semibold">Машины</h2>
              <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setAdding('vehicle')}>
                + машина
              </Button>
            </div>
            {vehicles.length === 0 ? (
              <Empty>Машин нет. Их можно добавить и прямо в чеке.</Empty>
            ) : (
              <ul className="flex flex-col gap-1 text-sm">
                {vehicles.map((v) => (
                  <li key={v.id} className="flex justify-between gap-2 border-b border-slate-100 py-1 last:border-0">
                    <span className={v.active ? 'font-medium' : 'text-slate-400'}>{v.plate}</span>
                    <span className="text-slate-500">{[v.brand, v.model].filter(Boolean).join(' ') || '—'}</span>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </div>
      )}

      <Card>
        <h2 className="mb-3 font-semibold">История</h2>
        <ErrorBox error={card.error ?? row.error} />
        {timeline.length === 0 ? (
          <Empty>Пока ничего не было</Empty>
        ) : (
          <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', '']}>
            {timeline.map((t, i) => (
              <tr key={`${t.at}-${i}`} className="hover:bg-slate-50">
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(t.at)}</td>
                <td className="px-2 py-2">
                  <Badge tone={KIND_TONE[t.kind]}>{t.title}</Badge>
                  {t.number !== null && <span className="ml-2 text-slate-500">№ {t.number}</span>}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(t.amount_tyiyn)}</td>
                <td className="px-2 py-2 text-slate-600">{t.comment || '—'}</td>
                <td className="px-2 py-2 text-right">
                  {(t.kind === 'sale' || t.kind === 'sale_return') && t.doc_id && (
                    <Link className="text-sky-700 underline" to={`/sales/${t.doc_id}`}>
                      Чек
                    </Link>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {money && (
        <MoneyModal
          title={money === 'repay' ? 'Погашение долга' : 'Правка баланса'}
          party={party}
          adjust={money === 'adjust'}
          onClose={() => setMoney(null)}
          onDone={() => {
            setMoney(null)
            card.reload()
          }}
        />
      )}
      {adding && (
        <Modal title={adding === 'contact' ? 'Новый работник' : 'Новая машина'} onClose={() => setAdding(null)}>
          <div className="flex flex-col gap-3">
            <Field label={adding === 'contact' ? 'ФИО' : 'Госномер'} required>
              <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} />
            </Field>
            <ErrorBox error={row.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setAdding(null)}>
                Отмена
              </Button>
              <Button disabled={row.busy || !value.trim()} onClick={addRow}>
                Добавить
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
