// Карточка клиента: работники, машины, покупки, долг и погашения (SPEC-10).
import { useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { balanceText } from '../components/ClientPicker'
import { OilBookList } from '../components/OilBook'
import { ReconciliationButton, ReconciliationModal } from '../components/ReconciliationButton'
import { RepaymentModal } from '../components/RepaymentModal'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Modal, Money, PageHeader, RowMenu, Table, toast, type MenuItem } from '../components/ui'
import { get, newOpId, patch, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Party, PartyCard, PartyTimelineItem } from '../lib/types'

interface CabinetState {
  available: boolean
  login: string
  has_account: boolean
  must_change: boolean
  last_login_at: string | null
}

/** Адрес кабинета с кнопкой «Копировать»: без доступа к буферу — выделяет текст для ручного копирования. */
function CopyAddress({ url }: { url: string }) {
  const [copied, setCopied] = useState(false)
  const field = useRef<HTMLInputElement>(null)
  const copy = async () => {
    const input = field.current
    try {
      await navigator.clipboard.writeText(url)
      setCopied(true)
      toast('Адрес скопирован')
    } catch {
      // Без HTTPS буфер обмена недоступен: выделяем адрес и пробуем старый способ копирования.
      input?.focus()
      input?.select()
      if (input && document.execCommand('copy')) {
        setCopied(true)
        toast('Адрес скопирован')
      } else toast('Скопируйте выделенный адрес', 'error')
    }
  }
  return (
    <div className="flex flex-wrap items-center gap-2">
      <input
        ref={field}
        readOnly
        aria-label="Адрес кабинета"
        className="w-full max-w-xs bg-slate-50 font-mono text-xs sm:w-72"
        value={url}
        onFocus={(e) => e.target.select()}
      />
      <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={() => void copy()}>
        {copied ? 'Скопировано' : 'Копировать'}
      </Button>
    </div>
  )
}

/** Кабинет юрлица: логин — ИНН, пароль сбрасывает только владелец (ADR-049). */
function CabinetBox({ partyId }: { partyId: string }) {
  const st = useLoad(() => get<CabinetState>(`/parties/${partyId}/cabinet`), [partyId])
  const act = useAction()
  const d = st.data
  if (!d) return null
  if (!d.available)
    return (
      <Card className="text-sm text-slate-600">
        Кабинет клиента: у фирмы нет ИНН — впишите его в карточке, и ИНН станет логином для входа.
      </Card>
    )
  const url = `${window.location.origin}/cabinet`
  return (
    <Card className="flex flex-wrap items-center justify-between gap-3 text-sm">
      <div className="flex min-w-0 flex-col gap-1">
        <div className="font-medium">Кабинет клиента</div>
        <CopyAddress url={url} />
        <div className="text-slate-600">
          Логин {d.login} ·{' '}
          {!d.has_account
            ? 'ещё не входили, начальный пароль avtodom2026'
            : d.must_change
              ? 'пароль сброшен — при входе попросит сменить'
              : 'клиент сменил пароль'}
          {d.last_login_at && ` · последний вход ${formatDateTime(d.last_login_at)}`}
        </div>
        <ErrorBox error={act.error ?? st.error} />
      </div>
      <Button
        variant="secondary"
        disabled={act.busy}
        onClick={() =>
          void act.run(async () => {
            if (!window.confirm('Сбросить пароль кабинета на avtodom2026? Все входы фирмы закроются.')) return
            await post(`/parties/${partyId}/cabinet/reset`, {})
            st.reload()
            toast('Пароль сброшен на avtodom2026')
          })
        }
      >
        Сбросить пароль
      </Button>
    </Card>
  )
}

interface HistoryRow {
  item: PartyTimelineItem
  /** Сколько из покупки взято в долг (запись долга по тому же чеку слита в строку покупки). */
  debt: number | null
}

/** «Покупка» и «Взял в долг» по одному чеку — одной строкой с отметкой «в долг». */
function mergeDebts(timeline: PartyTimelineItem[]): HistoryRow[] {
  const debtBySale = new Map<string, PartyTimelineItem>()
  for (const t of timeline) if (t.kind === 'debt' && t.amount_tyiyn > 0 && t.doc_id && !t.reversible) debtBySale.set(t.doc_id, t)
  const merged = new Set<PartyTimelineItem>()
  const rows: HistoryRow[] = []
  for (const t of timeline) {
    if (t.kind === 'sale' && t.doc_id) {
      const d = debtBySale.get(t.doc_id)
      if (d && d.amount_tyiyn <= t.amount_tyiyn) {
        merged.add(d)
        rows.push({ item: t, debt: d.amount_tyiyn })
        continue
      }
    }
    rows.push({ item: t, debt: null })
  }
  return rows.filter((r) => !merged.has(r.item))
}

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
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит операцию.
  const [opId] = useState(newOpId)

  const save = () =>
    run(async () => {
      const v = parseSom(sum)
      if (v === null || v <= 0) throw new Error('Неверная сумма')
      if (adjust) {
        if (!comment.trim()) throw new Error('Укажите причину правки')
        await post('/debts/adjust', { op_id: opId, party_id: party.id, amount_tyiyn: sign * v, comment })
      } else {
        await post('/debts/repayments', { op_id: opId, party_id: party.id, amount_tyiyn: v, comment })
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
/** Баллы клиента из Телеграм-бота (SPEC-19). */
function BonusBox({ partyId }: { partyId: string }) {
  const h = useLoad(
    () =>
      get<{ member: boolean; balance_tyiyn: number; rows: { kind: string; amount_tyiyn: number; sale_number: number | null; created_at: string }[] }>(
        `/parties/${partyId}/loyalty`,
      ),
    [partyId],
  )
  if (!h.data || (!h.data.member && h.data.rows.length === 0)) return null
  return (
    <Card className="text-sm">
      <div className="font-medium">
        Баллы: {formatSom(h.data.balance_tyiyn).replace(/\s*с$/, '')}
        {!h.data.member && <span className="ml-2 font-normal text-slate-500">(отключился от бота — не начисляются)</span>}
      </div>
      {h.data.rows.length > 0 && (
        <div className="mt-1 text-xs text-slate-500">
          Последнее: {h.data.rows[0].amount_tyiyn > 0 ? '+' : '−'}
          {formatSom(Math.abs(h.data.rows[0].amount_tyiyn)).replace(/\s*с$/, '')}
          {h.data.rows[0].sale_number !== null && `, чек № ${h.data.rows[0].sale_number}`}
        </div>
      )}
    </Card>
  )
}

export default function ClientCard() {
  const { id = '' } = useParams()
  const owner = useUser().role === 'owner'
  const [money, setMoney] = useState<null | 'repay' | 'adjust'>(null)
  const [reversing, setReversing] = useState<{ item: PartyTimelineItem; comment: string; opId: string } | null>(null)
  const revAct = useAction()
  const [adding, setAdding] = useState<'contact' | 'vehicle' | null>(null)
  const [limits, setLimits] = useState<{ limit: string; days: string } | null>(null)
  const [value, setValue] = useState('')
  const [reconciling, setReconciling] = useState(false)
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
      toast(party.active ? 'Клиент отключён' : 'Клиент включён')
    })

  // На телефоне в шапке остаётся «Погасить», остальное — в меню «⋯».
  const toggleItem: MenuItem = party.active
    ? { label: 'Отключить клиента', danger: true, onClick: toggleActive, disabled: row.busy }
    : { label: 'Включить клиента', onClick: toggleActive, disabled: row.busy }
  const phoneItems: MenuItem[] = [
    { label: 'Акт сверки', onClick: () => setReconciling(true) },
    { label: 'Правка баланса', onClick: () => setMoney('adjust'), disabled: !owner },
    toggleItem,
  ]
  const history = mergeDebts(timeline)

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={party.name}
        back={
          <Link className="text-sky-700 hover:underline" to="/clients">
            ← Клиенты
          </Link>
        }
        subtitle={!party.active && <Badge tone="rose">клиент отключён</Badge>}
        actions={
          <>
            <span className="hidden gap-2 md:flex">
              <ReconciliationButton partyId={party.id} />
              {owner && (
                <Button variant="secondary" onClick={() => setMoney('adjust')}>
                  Правка баланса
                </Button>
              )}
            </span>
            <Button disabled={party.balance_tyiyn <= 0} onClick={() => setMoney('repay')}>
              Погасить
            </Button>
            <span className="md:hidden">
              <RowMenu items={phoneItems} />
            </span>
            <span className="hidden md:inline-block">
              <RowMenu items={[toggleItem]} />
            </span>
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
          {party.overdue_tyiyn > 0 && (
            <div className="mt-1">
              <Badge tone="rose">просрочено {formatSom(party.overdue_tyiyn)}</Badge>
            </div>
          )}
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
        {party.comment && <div className="w-full text-slate-600">{party.comment}</div>}
      </Card>

      {party.kind === 'company' && (
        <Card>
          <CardTitle
            actions={
              <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={() => setAdding('contact')}>
                + работник
              </Button>
            }
          >
            Работники
          </CardTitle>
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
      )}

      {owner && party.kind === 'company' && party.role === 'customer' && <CabinetBox partyId={party.id} />}

      {party.role === 'customer' && <BonusBox partyId={party.id} />}

      {/* Машины клиента и их масляная книжка — одним блоком: у каждой машины свои замены. */}
      <section className="rounded-lg border border-slate-200 bg-slate-50 p-4">
        <CardTitle
          actions={
            <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={() => setAdding('vehicle')}>
              + машина
            </Button>
          }
        >
          Машины и масляная книжка
        </CardTitle>
        <OilBookList key={vehicles.length} path={`/parties/${party.id}/oil-book`} editable />
      </section>

      <Card>
        <CardTitle>История</CardTitle>
        <ErrorBox error={card.error ?? row.error} />
        {timeline.length === 0 ? (
          <Empty>Пока ничего не было</Empty>
        ) : (
          <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', '']}>
            {history.map(({ item: t, debt }, i) => (
              <tr key={`${t.at}-${i}`} className="hover:bg-slate-50">
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(t.at)}</td>
                <td className="px-2 py-2">
                  <Badge tone={KIND_TONE[t.kind]}>{t.title}</Badge>
                  {debt !== null && (
                    <span className="ml-1">
                      <Badge tone="amber">{debt === t.amount_tyiyn ? 'в долг' : `в долг ${formatSom(debt)}`}</Badge>
                    </span>
                  )}
                  {t.number !== null && <span className="ml-2 whitespace-nowrap text-slate-500">№ {t.number}</span>}
                </td>
                <td className="px-2 py-2 md:text-right">
                  <Money value={t.amount_tyiyn} />
                </td>
                <td className="px-2 py-2 text-slate-600">{t.comment || '—'}</td>
                <td className="px-2 py-2 text-right">
                  {(t.kind === 'sale' || t.kind === 'sale_return') && t.doc_id && (
                    <Link className="text-sky-700 underline" to={`/sales/${t.doc_id}`}>
                      Чек
                    </Link>
                  )}
                  {owner && t.reversible && (
                    <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setReversing({ item: t, comment: '', opId: newOpId() })}>
                      Сторно
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {reversing && (
        <Modal title={`Сторно: ${reversing.item.title.toLowerCase()} ${formatSom(Math.abs(reversing.item.amount_tyiyn))}`} onClose={() => setReversing(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Запись останется в истории, рядом появится обратная. Деньги погашения уйдут из той кассы, куда пришли.
            </div>
            <Field label="Причина" required>
              <input autoFocus value={reversing.comment} onChange={(e) => setReversing({ ...reversing, comment: e.target.value })} />
            </Field>
            <ErrorBox error={revAct.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setReversing(null)}>
                Отмена
              </Button>
              <Button
                variant="danger"
                disabled={revAct.busy || !reversing.comment.trim()}
                onClick={() =>
                  void revAct.run(async () => {
                    await post(`/debts/ledger/${reversing.item.ledger_id}/reverse`, { op_id: reversing.opId, comment: reversing.comment })
                    setReversing(null)
                    card.reload()
                    toast('Сторно проведено')
                  })
                }
              >
                Провести сторно
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {reconciling && <ReconciliationModal partyId={party.id} onClose={() => setReconciling(false)} />}
      {money === 'repay' && (
        <RepaymentModal
          party={party}
          onClose={() => setMoney(null)}
          onDone={() => {
            setMoney(null)
            card.reload()
          }}
        />
      )}
      {money === 'adjust' && (
        <MoneyModal
          title="Правка баланса"
          party={party}
          adjust
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
