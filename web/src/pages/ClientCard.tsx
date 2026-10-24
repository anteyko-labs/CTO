// Карточка клиента: работники, машины, покупки, долг и погашения (SPEC-10).
import { useMemo, useRef, useState } from 'react'
import { Link, useParams } from 'react-router-dom'
import { balanceText } from '../components/ClientPicker'
import { OilBookList } from '../components/OilBook'
import { Pager, usePager } from '../components/Pager'
import { ReconciliationButton, ReconciliationModal } from '../components/ReconciliationButton'
import { stockText } from '../components/ProductPicker'
import { VehicleFormModal } from '../components/VehicleFormModal'
import { RepaymentModal } from '../components/RepaymentModal'
import { CorrectionBadges, CorrectionBanner, FixButton, useCorrection } from '../components/Correction'
import { Badge, Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Modal, Money, PageHeader, RowMenu, Table, toast, type MenuItem } from '../components/ui'
import { get, newOpId, patch, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { adjustDoc, adjustFields, type AdjustDoc } from '../lib/moneyFix'
import type { Party, PartyCard, PartyTimelineItem, TopProducts } from '../lib/types'

interface CabinetState {
  available: boolean
  login: string
  has_account: boolean
  must_change: boolean
  last_login_at: string | null
}

/** Адрес кабинета с кнопкой «Копировать»: без доступа к буферу — выделяет текст для ручного копирования. */
function CopyAddress({ url, label = 'Адрес кабинета' }: { url: string; label?: string }) {
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
        aria-label={label}
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

/** Ссылка на бот со скрытым кодом клиента (SPEC-32): «Старт» по ней сразу подключает чат.
 *  Видит только владелец; «Новая ссылка» — прежняя перестаёт работать. */
function BotLinkBox({ partyId }: { partyId: string }) {
  const st = useLoad(() => get<{ url: string | null }>(`/parties/${partyId}/bot-link`), [partyId])
  const act = useAction()
  if (!st.data) return null
  return (
    <Card className="flex flex-wrap items-center justify-between gap-3 text-sm" data-testid="bot-link">
      <div className="flex min-w-0 flex-col gap-1">
        <div className="font-medium">Ссылка на бот для клиента</div>
        {st.data.url ? (
          <CopyAddress url={st.data.url} label="Ссылка на бот" />
        ) : (
          <div className="text-slate-600">Бот ещё не запускался — имя бота неизвестно.</div>
        )}
        <div className="text-slate-600">Нажал «Старт» по ссылке — бот подключён к этой карточке без номера телефона.</div>
        <ErrorBox error={act.error ?? st.error} />
      </div>
      <Button
        variant="secondary"
        disabled={act.busy}
        onClick={() =>
          void act.run(async () => {
            if (!window.confirm('Сделать новую ссылку? Прежняя перестанет работать; уже подключённый чат останется.')) return
            st.setData(await post<{ url: string | null }>(`/parties/${partyId}/bot-link/reset`, {}))
            toast('Новая ссылка готова')
          })
        }
      >
        Новая ссылка
      </Button>
    </Card>
  )
}

interface ContactForm {
  id: string | null
  full_name: string
  phone: string
  position: string
  is_head: boolean
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

/** Правка баланса владельцем (погашение — `RepaymentModal`). */
function AdjustModal({
  title,
  party,
  fixId,
  onClose,
  onDone,
}: {
  title: string
  party: Party
  /** «Исправить» проведённую правку баланса (SPEC-29 §24): id записи долга; окно заполняется ею. */
  fixId?: string
  onClose: () => void
  onDone: () => void
}) {
  const [sum, setSum] = useState('')
  const [comment, setComment] = useState('')
  const [sign, setSign] = useState<1 | -1>(1)
  const { busy, error, run } = useAction()
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит операцию.
  const [opId] = useState(newOpId)
  // Исправление: поля — из исходной правки, как только она пришла с сервера.
  const correction = useCorrection<AdjustDoc>('ledger', fixId ?? null)
  const fixDraft = fixId ? correction.draft.data : null
  const [filled, setFilled] = useState(false)
  if (fixDraft && !filled) {
    const f = adjustFields(fixDraft.doc)
    setSign(f.sign)
    setSum(f.sum)
    setComment(f.comment)
    setFilled(true)
  }
  const fixDoc = fixDraft ? adjustDoc(fixDraft.doc, { sign, sum, comment }) : null

  const saveFix = async () => {
    if (!fixDoc || !comment.trim()) return
    const c = await correction.save(fixDoc)
    if (!c) return
    toast(`Исправлено: правка баланса ${formatSom(fixDoc.amount_tyiyn)}`)
    onDone()
  }

  const save = () =>
    run(async () => {
      const v = parseSom(sum)
      if (v === null || v <= 0) throw new Error('Неверная сумма')
      if (!comment.trim()) throw new Error('Укажите причину правки')
      await post('/debts/adjust', { op_id: opId, party_id: party.id, amount_tyiyn: sign * v, comment })
      toast('Баланс исправлен')
      onDone()
    })

  return (
    <Modal title={title} onClose={onClose}>
      <div className="flex flex-col gap-3">
        {fixDraft && <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />}
        {fixId && !filled && correction.draft.loading && <Loading />}
        <div className="text-sm text-slate-600">
          {party.name}: {balanceText(party.balance_tyiyn)}
        </div>
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
        <Field label="Сумма, с" required>
          <input autoFocus inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
        </Field>
        <Field label="Комментарий" required>
          <input value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <ErrorBox error={fixId ? correction.error : error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          {fixId ? (
            <Button disabled={correction.busy || !fixDoc || !comment.trim() || !correction.comment.trim()} onClick={() => void saveFix()}>
              Сохранить исправление
            </Button>
          ) : (
            <Button disabled={busy || !sum.trim()} onClick={() => void save()}>
              Записать
            </Button>
          )}
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
  // Править клиента и машины и принимать оплату долга — права администратора (ADR-097).
  const can = useCan()
  const mayEdit = can('clients')
  const [money, setMoney] = useState<null | 'repay' | 'adjust'>(null)
  const [reversing, setReversing] = useState<{ item: PartyTimelineItem; comment: string; opId: string } | null>(null)
  // «Исправить» оплату долга или правку баланса — той же формой, что их записывает (SPEC-29 §24).
  const [fixing, setFixing] = useState<{ kind: 'repayment' | 'adjust'; ledgerId: string } | null>(null)
  const revAct = useAction()
  const [adding, setAdding] = useState<'vehicle' | null>(null)
  // Человек фирмы: новый или правка — имя, телефон, должность, руководитель (SPEC-32).
  const [contact, setContact] = useState<ContactForm | null>(null)
  const [limits, setLimits] = useState<{ limit: string; days: string } | null>(null)
  // Имя и фамилия человека (ADR-083): у прежних записей их нет — дописывают здесь.
  const [naming, setNaming] = useState<{ first: string; last: string } | null>(null)
  const [reconciling, setReconciling] = useState(false)
  const row = useAction()
  const card = useLoad(() => get<PartyCard>(`/parties/${id}/card`), [id])
  // Отметки «исправлен» / «исправление» у записей долга — одним запросом на историю.
  const corrections = useLoad(() => loadCorrections('ledger'), [])
  const marks = useMemo(() => correctionMarks(corrections.data ?? []), [corrections.data])
  // История — по страницам, новые сверху; другой клиент — первая страница.
  const history = useMemo(() => mergeDebts(card.data?.timeline ?? []), [card.data])
  const historyPager = usePager(history, 'client.history', id)

  if (card.loading && !card.data) return <Loading />
  if (!card.data) return <ErrorBox error={card.error ?? 'Клиент не найден'} />
  const { party, contacts, vehicles, timeline } = card.data

  const saveContact = () =>
    void row.run(async () => {
      if (!contact) return
      const body = {
        full_name: contact.full_name.trim(),
        phone: contact.phone.trim(),
        position: contact.position.trim(),
        is_head: contact.is_head,
      }
      if (!body.full_name) throw new Error('Заполните ФИО')
      if (contact.id) {
        const was = contacts.find((c) => c.id === contact.id)
        await patch(`/parties/${party.id}/contacts/${contact.id}`, { ...body, inn: was?.inn ?? '' })
      } else await post(`/parties/${party.id}/contacts`, body)
      setContact(null)
      card.reload()
    })

  const togglePurchaseMessages = () =>
    void row.run(async () => {
      await patch<Party>(`/parties/${party.id}`, { no_purchase_messages: !party.no_purchase_messages })
      card.reload()
      toast(party.no_purchase_messages ? 'Сообщения о закупах включены' : 'Сообщения о закупах отключены')
    })

  const saveName = () =>
    void row.run(async () => {
      if (!naming) return
      await patch<Party>(`/parties/${party.id}`, { first_name: naming.first.trim(), last_name: naming.last.trim() })
      setNaming(null)
      card.reload()
      toast('Имя сохранено')
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
    ...(mayEdit ? [toggleItem] : []),
  ]

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
            {can('debt_payment') && (
              <Button disabled={party.balance_tyiyn <= 0} onClick={() => setMoney('repay')}>
                Погасить
              </Button>
            )}
            <span className="md:hidden">
              <RowMenu items={phoneItems} />
            </span>
            {mayEdit && (
              <span className="hidden md:inline-block">
                <RowMenu items={[toggleItem]} />
              </span>
            )}
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
        {party.kind === 'person' && (
          <div>
            <div className="text-xs text-slate-500">Имя и фамилия</div>
            <div className="font-medium" data-testid="party-person-name">
              {party.first_name ? `${party.first_name} ${party.last_name ?? ''}` : <span className="text-amber-700">не заполнены</span>}
            </div>
            {mayEdit && (
              <button
                type="button"
                className="mt-1 text-xs text-sky-700 underline"
                onClick={() => setNaming({ first: party.first_name ?? '', last: party.last_name ?? '' })}
              >
                {party.first_name ? 'изменить' : 'заполнить'}
              </button>
            )}
          </div>
        )}
        <div>
          <div className="text-xs text-slate-500">Телефон</div>
          <div className="font-medium">{party.phone || '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">{party.kind === 'company' ? 'ИНН фирмы' : 'ПИН'}</div>
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
                    limit: party.credit_limit_tyiyn !== null ? somInput(party.credit_limit_tyiyn) : '',
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
              mayEdit && (
                <Button
                  variant="secondary"
                  className="min-h-[34px] px-3 py-1 text-xs"
                  onClick={() => setContact({ id: null, full_name: '', phone: '', position: '', is_head: false })}
                >
                  + работник
                </Button>
              )
            }
          >
            Работники
          </CardTitle>
          {contacts.length === 0 ? (
            <Empty>Никто не записан. Добавьте того, кто приезжает за товаром.</Empty>
          ) : (
            <ul className="flex flex-col gap-1 text-sm">
              {contacts.map((c) => (
                <li key={c.id} className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-100 py-1 last:border-0">
                  <span className={c.active ? '' : 'text-slate-400'}>
                    {c.full_name} {c.is_head && <Badge tone="sky">руководитель</Badge>}
                  </span>
                  <span className="flex items-center gap-2 text-slate-500">
                    {[c.position, c.phone, c.inn].filter(Boolean).join(' · ') || '—'}
                    {mayEdit && (
                      <button
                        type="button"
                        className="text-xs text-sky-700 underline"
                        onClick={() => setContact({ id: c.id, full_name: c.full_name, phone: c.phone, position: c.position, is_head: !!c.is_head })}
                      >
                        изменить
                      </button>
                    )}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {party.role === 'customer' && (
            <div className="mt-3 border-t border-slate-100 pt-2 text-sm">
              <label className="inline-flex items-center gap-2">
                <input
                  id="party-no-purchase-messages"
                  type="checkbox"
                  className="h-4 w-4"
                  checked={!!party.no_purchase_messages}
                  disabled={!mayEdit || row.busy}
                  onChange={togglePurchaseMessages}
                />
                Не присылать сообщения о закупах
              </label>
              <div className="text-xs text-slate-500">
                После каждого чека фирме и руководителю уходит сообщение: сумма, водитель, машина и общий долг (Телеграм или WhatsApp).
              </div>
            </div>
          )}
        </Card>
      )}

      {owner && party.kind === 'company' && party.role === 'customer' && <CabinetBox partyId={party.id} />}

      {owner && party.role === 'customer' && <BotLinkBox partyId={party.id} />}

      {party.role === 'customer' && <BonusBox partyId={party.id} />}

      {/* Машины клиента и их масляная книжка — одним блоком: у каждой машины свои замены. */}
      <section className="rounded-lg border border-slate-200 bg-slate-50 p-4">
        <CardTitle
          actions={
            mayEdit && (
              <Button variant="secondary" className="min-h-[34px] px-3 py-1 text-xs" onClick={() => setAdding('vehicle')}>
                + машина
              </Button>
            )
          }
        >
          Машины и масляная книжка
        </CardTitle>
        <OilBookList
          key={vehicles.length}
          path={`/parties/${party.id}/oil-book`}
          editable={mayEdit}
          partyId={party.id}
          onAdd={mayEdit ? () => setAdding('vehicle') : undefined}
        />
      </section>

      {party.role === 'customer' && <FrequentProducts partyId={party.id} />}

      <Card>
        <CardTitle>История</CardTitle>
        <ErrorBox error={card.error ?? row.error} />
        {timeline.length === 0 ? (
          <Empty>Пока ничего не было</Empty>
        ) : (
          <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', '']}>
            {historyPager.shown.map(({ item: t, debt }, i) => (
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
                  {t.ledger_id && (
                    <span className="ml-1 inline-flex flex-wrap gap-1">
                      <CorrectionBadges mark={marks.get(t.ledger_id)} />
                    </span>
                  )}
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
                  {owner && t.reversible && t.ledger_id && (t.kind === 'repayment' || t.kind === 'adjust') && (
                    <FixButton small className="ml-2" onClick={() => setFixing({ kind: t.kind === 'adjust' ? 'adjust' : 'repayment', ledgerId: t.ledger_id ?? '' })} />
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
        <Pager page={historyPager.page} size={historyPager.size} total={historyPager.total} onPage={historyPager.setPage} onSize={historyPager.setSize} label="Страницы истории клиента" />
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
      {fixing?.kind === 'repayment' && (
        <RepaymentModal
          key={fixing.ledgerId}
          party={party}
          fixId={fixing.ledgerId}
          onClose={() => setFixing(null)}
          onDone={() => {
            setFixing(null)
            card.reload()
            corrections.reload()
          }}
        />
      )}
      {fixing?.kind === 'adjust' && (
        <AdjustModal
          key={fixing.ledgerId}
          title="Исправить правку баланса"
          party={party}
          fixId={fixing.ledgerId}
          onClose={() => setFixing(null)}
          onDone={() => {
            setFixing(null)
            card.reload()
            corrections.reload()
          }}
        />
      )}
      {money === 'adjust' && (
        <AdjustModal
          title="Правка баланса"
          party={party}
          onClose={() => setMoney(null)}
          onDone={() => {
            setMoney(null)
            card.reload()
          }}
        />
      )}
      {adding === 'vehicle' && (
        <VehicleFormModal
          partyId={party.id}
          onClose={() => setAdding(null)}
          onSaved={() => {
            setAdding(null)
            card.reload()
          }}
        />
      )}
      {naming && (
        <Modal title="Имя и фамилия клиента" onClose={() => setNaming(null)}>
          <div className="flex flex-col gap-3">
            {!party.first_name && <div className="text-sm text-slate-600">Сейчас записан как «{party.name}».</div>}
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Имя" required>
                <input id="party-first" autoFocus value={naming.first} onChange={(e) => setNaming({ ...naming, first: e.target.value })} />
              </Field>
              <Field label="Фамилия" required>
                <input id="party-last" value={naming.last} onChange={(e) => setNaming({ ...naming, last: e.target.value })} />
              </Field>
            </div>
            <ErrorBox error={row.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setNaming(null)}>
                Отмена
              </Button>
              <Button disabled={row.busy || !naming.first.trim() || !naming.last.trim()} onClick={saveName}>
                Сохранить
              </Button>
            </div>
          </div>
        </Modal>
      )}
      {contact && (
        <Modal title={contact.id ? 'Работник фирмы' : 'Новый работник'} onClose={() => setContact(null)}>
          <div className="flex flex-col gap-3">
            <Field label="ФИО" required>
              <input id="contact-name" autoFocus value={contact.full_name} onChange={(e) => setContact({ ...contact, full_name: e.target.value })} />
            </Field>
            <Field label="Телефон">
              <input id="contact-phone" inputMode="tel" value={contact.phone} onChange={(e) => setContact({ ...contact, phone: e.target.value })} />
            </Field>
            <Field label="Должность">
              <input id="contact-position" value={contact.position} onChange={(e) => setContact({ ...contact, position: e.target.value })} />
            </Field>
            <Checkbox
              label="Руководитель — ему сообщения о закупах, его имя в обращении"
              checked={contact.is_head}
              onChange={(v) => setContact({ ...contact, is_head: v })}
            />
            <ErrorBox error={row.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setContact(null)}>
                Отмена
              </Button>
              <Button disabled={row.busy || !contact.full_name.trim()} onClick={saveContact}>
                {contact.id ? 'Сохранить' : 'Добавить'}
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}

/** Что клиент покупает чаще всего — за всё время, по числу чеков (SPEC-27 §4). */
function FrequentProducts({ partyId }: { partyId: string }) {
  const top = useLoad(() => get<TopProducts>(`/reports/top-products${qs({ party_id: partyId, sort: 'checks', limit: 5 })}`), [partyId])
  const items = top.data?.items ?? []
  if (!top.error && items.length === 0) return null
  return (
    <Card>
      <CardTitle>Что покупает чаще всего</CardTitle>
      <ErrorBox error={top.error} />
      <ul className="divide-y divide-slate-100 text-sm">
        {items.map((r) => (
          <li key={r.product_id} className="flex items-center justify-between gap-3 py-1.5">
            <span className="min-w-0 flex-1 truncate font-medium">{r.name}</span>
            <span className="whitespace-nowrap tabular-nums text-slate-600">
              {stockText({ unit: r.unit, container_ml: r.container_ml, stock_qty: r.qty })} · чеков {r.checks}
            </span>
          </li>
        ))}
      </ul>
    </Card>
  )
}
