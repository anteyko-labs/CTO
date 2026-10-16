// «Обзор счетов» (SPEC-28 §5, ADR-076): кассы и безналичные счета как в старой программе — счёт,
// тип, сумма, отрицательный баланс, правка и история. Заводит и правит счета только владелец;
// администратор видит счета без «только владелец», без правки. Остатки и права — на сервере.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Icon } from '../components/icons'
import { Pager, usePageState, usePager } from '../components/Pager'
import { PeriodPicker } from '../components/PeriodPicker'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, PageHeader, Table, toast } from '../components/ui'
import { accountNote, accountTotals, accountType, isBank, isTill } from '../lib/accounts'
import { get, getPage, newOpId, patch, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatSom, shiftDate, todayBishkek } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { MOVEMENT_LABELS, type Range } from '../lib/income'
import type { AccountFlow, CashAccount, CashMovement } from '../lib/types'

/** Вид нового счёта: наличный без смены, сейф или безналичный. */
type NewKind = 'other' | 'safe' | 'bank'

const NEW_KINDS: [NewKind, string][] = [
  ['other', 'Наличный — касса без смены'],
  ['safe', 'Наличный — сейф'],
  ['bank', 'Безналичный — банк'],
]

type Form = {
  /** Пусто — новый счёт. */
  id: string
  name: string
  kind: NewKind | CashAccount['kind']
  allow_negative: boolean
  owner_only: boolean
  active: boolean
  is_default: boolean
  opId: string
}

export default function Accounts() {
  const owner = useUser().role === 'owner'
  const [archived, setArchived] = useState(false)
  const list = useLoad(() => get<CashAccount[]>(`/cash/accounts${qs({ archived: archived || undefined })}`), [archived])
  const rows = list.data ?? []
  const pager = usePager(rows, 'accounts', String(archived))
  const totals = accountTotals(rows)
  const [form, setForm] = useState<Form | null>(null)
  const [history, setHistory] = useState<CashAccount | null>(null)
  const act = useAction()

  const edit = (a: CashAccount) => {
    act.setError(null)
    setForm({ id: a.id, name: a.name, kind: a.kind, allow_negative: a.allow_negative, owner_only: a.owner_only, active: a.active, is_default: a.is_default, opId: newOpId() })
  }
  const create = () => {
    act.setError(null)
    setForm({ id: '', name: '', kind: 'other', allow_negative: false, owner_only: false, active: true, is_default: false, opId: newOpId() })
  }
  const current = form?.id ? rows.find((a) => a.id === form.id) : undefined
  const till = current ? isTill(current) : false
  const mainBank = current ? isBank(current) && current.is_default : false
  const missing = form ? missingWithFocus([Boolean(form.name.trim()), 'название', '#account-name']) : []

  const save = () =>
    void act.run(async () => {
      if (!form) return
      if (form.id) {
        const before = rows.find((a) => a.id === form.id)
        await patch(`/cash/accounts/${form.id}`, {
          name: form.name,
          allow_negative: form.allow_negative,
          owner_only: form.owner_only,
          active: form.active,
          // Основным счёт делают отметкой; снять её можно только выбором другого основного.
          is_default: form.is_default && !before?.is_default ? true : undefined,
        })
        toast('Счёт сохранён')
      } else {
        await post('/cash/accounts', {
          op_id: form.opId,
          name: form.name,
          kind: form.kind,
          allow_negative: form.allow_negative,
          owner_only: form.owner_only,
        })
        toast('Счёт заведён')
      }
      setForm(null)
      list.reload()
    })

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Обзор счетов"
        subtitle="Кассы, сейфы и банковские счета: остатки, отрицательный баланс, история движений"
        actions={
          owner && (
            <Button onClick={create}>
              <Icon name="plus" className="h-4 w-4" />
              Счёт
            </Button>
          )
        }
      />
      <Card>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <Checkbox label="Показать архивные" checked={archived} onChange={setArchived} />
          {!owner && <span className="text-xs text-slate-500">Счета «только владелец» администратору не показываются; правит счета владелец.</span>}
        </div>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty>Счетов нет</Empty>
        ) : (
          <>
            <Table head={['Счёт', 'Тип', 'Сумма, сом', 'Отрицательный баланс', '']}>
              {pager.shown.map((a) => (
                <tr key={a.id} data-testid="account-row" className={a.active ? '' : 'text-slate-400'}>
                  <td className="px-2 py-2">
                    <span className="font-medium">{a.name}</span>
                    {accountNote(a) && <span className="ml-2 text-xs text-slate-500">{accountNote(a)}</span>}
                    {a.owner_only && (
                      <span className="ml-2">
                        <Badge tone="sky">только владелец</Badge>
                      </span>
                    )}
                    {!a.active && (
                      <span className="ml-2">
                        <Badge>в архиве</Badge>
                      </span>
                    )}
                  </td>
                  <td className="px-2 py-2">{accountType(a)}</td>
                  <td className="px-2 py-2 text-right">
                    <Money value={a.balance_tyiyn} tone="negative" className="font-semibold" />
                  </td>
                  <td className="px-2 py-2">{a.allow_negative ? 'Разрешён' : 'Не разрешён'}</td>
                  <td className="whitespace-nowrap px-2 py-2 text-right">
                    {owner && (
                      <Button variant="ghost" className="px-2" aria-label={`Изменить «${a.name}»`} title="Изменить" onClick={() => edit(a)}>
                        <Icon name="edit" className="h-4 w-4" />
                      </Button>
                    )}
                    <Button variant="ghost" className="px-2" aria-label={`История «${a.name}»`} title="История" onClick={() => setHistory(a)}>
                      <Icon name="history" className="h-4 w-4" />
                    </Button>
                  </td>
                </tr>
              ))}
            </Table>
            {/* Итоги — по всем действующим счетам (не по странице); на компьютере прижаты к низу экрана. */}
            <div className="-mx-4 -mb-4 mt-3 rounded-b-lg border-t-2 border-slate-300 bg-white px-4 pb-3 pt-2 md:sticky md:bottom-0 md:z-10 md:shadow-[0_-6px_12px_-10px_rgba(15,23,42,0.35)]">
              <div className="flex flex-wrap items-baseline justify-end gap-x-2 gap-y-1 text-base font-semibold" data-testid="account-totals">
                <span>
                  Итого: наличные <span className="tabular-nums">{formatSom(totals.cash)}</span>
                </span>
                <span>
                  · безнал <span className="tabular-nums">{formatSom(totals.bank)}</span>
                </span>
                <span>
                  · всего <span className="tabular-nums">{formatSom(totals.total)}</span>
                </span>
              </div>
              <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы счетов" />
            </div>
          </>
        )}
      </Card>

      {form && (
        <Modal title={form.id ? `Счёт «${current?.name ?? form.name}»` : 'Новый счёт'} onClose={() => setForm(null)}>
          <div className="flex flex-col gap-3">
            <Field label="Название" required>
              <input id="account-name" autoFocus value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
            </Field>
            {form.id ? (
              <div className="text-sm text-slate-600">
                Тип: {accountType({ kind: form.kind as CashAccount['kind'] })}
                {current && accountNote(current) ? ` · ${accountNote(current)}` : ''} — тип после создания не меняется.
              </div>
            ) : (
              <Field label="Тип">
                <select id="account-kind" value={form.kind} onChange={(e) => setForm({ ...form, kind: e.target.value as NewKind })}>
                  {NEW_KINDS.map(([k, label]) => (
                    <option key={k} value={k}>
                      {label}
                    </option>
                  ))}
                </select>
              </Field>
            )}
            {!till && (
              <Checkbox label="Отрицательный баланс разрешён" checked={form.allow_negative} onChange={(v) => setForm({ ...form, allow_negative: v })} />
            )}
            {!till && !mainBank && (
              <Checkbox label="Только владелец (администратор не видит)" checked={form.owner_only} onChange={(v) => setForm({ ...form, owner_only: v })} />
            )}
            {form.id && isBank({ kind: form.kind as CashAccount['kind'] }) && !mainBank && form.active && !form.owner_only && (
              <Checkbox label="Основной безналичный (по умолчанию для карты и QR)" checked={form.is_default} onChange={(v) => setForm({ ...form, is_default: v })} />
            )}
            {form.id && !till && !mainBank && (
              <Checkbox label="В архиве (только с нулевым остатком)" checked={!form.active} onChange={(v) => setForm({ ...form, active: !v })} />
            )}
            {till && <div className="text-xs text-slate-500">Касса смены: наличных в ней не бывает меньше нуля, в архив и «только владелец» её не убрать.</div>}
            {mainBank && <div className="text-xs text-slate-500">Основной безналичный счёт: чтобы убрать его в архив, сделайте основным другой.</div>}
            <Missing items={missing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setForm(null)}>
                Отмена
              </Button>
              <Button disabled={act.busy || missing.length > 0} onClick={save}>
                Сохранить
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {history && <AccountHistory account={history} onClose={() => setHistory(null)} owner={owner} />}
    </div>
  )
}

/** История счёта за период: на начало, по видам движений, на конец и движения по страницам. */
function AccountHistory({ account, owner, onClose }: { account: CashAccount; owner: boolean; onClose: () => void }) {
  const today = todayBishkek()
  const [range, setRange] = useState<Range>({ from: shiftDate(today, -29), to: today })
  const period = qs({ from: range.from, to: range.to })
  const sum = useLoad(() => get<AccountFlow>(`/cash/accounts/${account.id}/summary${period}`), [account.id, period])
  const pager = usePageState('accounts.history', `${account.id}:${period}`)
  const moves = useLoad(
    () => getPage<CashMovement>(`/cash/accounts/${account.id}${period}`, { limit: pager.size, offset: pager.page * pager.size }),
    [account.id, period, pager.page, pager.size],
  )
  const s = sum.data
  return (
    <Modal title={`История: ${account.name}`} full onClose={onClose}>
      <div className="flex flex-col gap-4">
        <PeriodPicker value={range} today={today} onChange={setRange} />
        <ErrorBox error={sum.error ?? moves.error} />
        {s && (
          <div className="grid grid-cols-2 gap-4 sm:grid-cols-4" data-testid="account-history-totals">
            <Stat label="На начало" value={s.opening_tyiyn} />
            <Stat label="Пришло" value={s.in_tyiyn} />
            <Stat label="Ушло" value={-s.out_tyiyn} />
            <Stat label="На конец" value={s.closing_tyiyn} />
          </div>
        )}
        {s && s.kinds.length > 0 && (
          <Table head={['Вид', 'Пришло', 'Ушло']}>
            {s.kinds.map((k) => (
              <tr key={k.kind}>
                <td className="px-2 py-2">{MOVEMENT_LABELS[k.kind] ?? k.kind}</td>
                <td className="px-2 py-2 text-right">
                  <Money value={k.in_tyiyn} zero="—" />
                </td>
                <td className="px-2 py-2 text-right">
                  <Money value={k.out_tyiyn} zero="—" />
                </td>
              </tr>
            ))}
          </Table>
        )}
        {owner && isBank(account) && (
          <Link className="text-sm text-sky-700 hover:underline" to={`/bank?account=${account.id}`}>
            Подробно по безналу: картой, QR, комиссия банка →
          </Link>
        )}
        {moves.loading && !moves.data ? (
          <Loading />
        ) : (moves.data?.total ?? 0) === 0 ? (
          <Empty>За период движений нет</Empty>
        ) : (
          <>
            <Table head={['Когда', 'Что', 'Комментарий', 'Кто', 'Сумма']}>
              {(moves.data?.items ?? []).map((m) => (
                <tr key={m.id} data-testid="account-movement">
                  <td className="whitespace-nowrap px-2 py-2 text-slate-500">{formatDateTime(m.created_at)}</td>
                  <td className="px-2 py-2">{MOVEMENT_LABELS[m.kind] ?? m.kind}</td>
                  <td className="px-2 py-2 text-slate-600">{m.comment || '—'}</td>
                  <td className="px-2 py-2 text-slate-600">{m.user_name}</td>
                  <td className="px-2 py-2 text-right font-medium">
                    <Money value={m.amount_tyiyn} tone="sign" />
                  </td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={moves.data?.total ?? 0} onPage={pager.setPage} onSize={pager.setSize} label="Страницы истории счёта" />
          </>
        )}
      </div>
    </Modal>
  )
}

function Stat({ label, value }: { label: string; value: number }) {
  return (
    <div>
      <div className="text-xs text-slate-500">{label}</div>
      <Money value={value} tone="negative" className="text-xl font-bold" />
    </div>
  )
}
