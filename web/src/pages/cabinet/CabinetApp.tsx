// Кабинет юрлица: вход по ИНН, смена начального пароля, покупки, работники, долг, акт сверки
// (SPEC-12, ADR-049). Только просмотр — ничего не проводится. Тот же вход у поставщика — его
// разделы в `SupplierCabinet.tsx` (задача 028).
import { DebtKindsLine } from '../../components/DebtKindsLine'
import { useEffect, useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Table, Toaster, toast } from '../../components/ui'
import { ApiError, get, post, qs } from '../../lib/api'
import { OilBookList } from '../../components/OilBook'
import { Pager, usePager } from '../../components/Pager'
import { ExportButton } from '../../components/ExportButton'
import { listSheet, periodText, xl, type ExportCol } from '../../lib/exportTable'
import { PURCHASE_COLS, cabinetQtyText, purchaseRows, purchaseTotal, type CabinetSale } from '../../lib/cabinetExport'
import { BLANK, type ActPeriod, type Seller } from '../../lib/debtDocs'
import { ReconciliationView } from '../../components/ReconciliationButton'
import { formatDateTime, formatSom, todayBishkek } from '../../lib/format'
import { useAction, useLoad } from '../../lib/hooks'
import { SUPPLIER_TABS, SupplierHeader, SupplierTabBody, type SupplierSummary, type SupplierTab } from './SupplierCabinet'

interface Me {
  /** `supplier` — кабинет поставщика (задача 028). */
  role?: 'customer' | 'supplier'
  supplier?: SupplierSummary | null
  name: string
  inn: string
  phone: string
  must_change: boolean
  balance_tyiyn: number
  overdue_tyiyn: number
  due_days: number | null
  credit_limit_tyiyn: number | null
  /** Долг по филиалам (SPEC-33): только ненулевые. */
  branch_balances?: { branch_id: string; name: string; balance_tyiyn: number }[]
  /** Из долга — за личные машины работников (задача 051). */
  personal_debt_tyiyn?: number
  seller: Partial<Seller>
  /** Филиалы для отбора «Все / …» (задача 036); один филиал — пусто. */
  branches?: { id: string; name: string }[]
  /** Период акта по умолчанию — как на экране точки («Документы о долге»). */
  act_period?: ActPeriod | null
}

interface Contact {
  full_name: string
  phone: string
  position: string
  visits: number
  last_at: string | null
  total_tyiyn: number
  debt_tyiyn: number
}

function LoginForm({ onDone }: { onDone: () => void }) {
  const [inn, setInn] = useState('')
  const [password, setPassword] = useState('')
  const { busy, error, run } = useAction()
  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-100 p-4">
      <Card className="flex w-full max-w-sm flex-col gap-3">
        <h1 className="text-lg font-semibold">Кабинет Avtodom</h1>
        <p className="text-sm text-slate-600">Для организаций: клиентам — покупки работников, долг и акт сверки; поставщикам — накладные, оплаты и акт сверки.</p>
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault()
            void run(async () => {
              await post('/client/login', { inn, password })
              onDone()
            })
          }}
        >
          <Field label="ИНН организации">
            <input autoFocus inputMode="numeric" autoComplete="username" value={inn} onChange={(e) => setInn(e.target.value)} />
          </Field>
          <Field label="Пароль" hint="Первый вход — пароль, который вам выдали на точке">
            <input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
          </Field>
          <ErrorBox error={error} />
          <Button type="submit" disabled={busy || !inn.trim() || !password}>
            Войти
          </Button>
        </form>
        <p className="border-t border-slate-100 pt-3 text-xs text-slate-500">Забыли пароль? Позвоните на точку — пароль сбросит владелец.</p>
      </Card>
    </div>
  )
}

function ChangePassword({ me, onDone }: { me: Me; onDone: () => void }) {
  const [form, setForm] = useState({ old: '', next: '', again: '' })
  const { busy, error, run } = useAction()
  const mismatch = form.again !== '' && form.next !== form.again
  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-100 p-4">
      <Card className="flex w-full max-w-sm flex-col gap-3">
        <h1 className="text-lg font-semibold">{me.name}</h1>
        <p className="text-sm text-slate-600">Это первый вход: придумайте свой пароль. Начальный пароль после этого перестанет работать.</p>
        <Field label="Текущий пароль">
          <input type="password" autoComplete="current-password" value={form.old} onChange={(e) => setForm({ ...form, old: e.target.value })} />
        </Field>
        <Field label="Новый пароль" hint="Не короче 8 знаков">
          <input type="password" autoComplete="new-password" value={form.next} onChange={(e) => setForm({ ...form, next: e.target.value })} />
        </Field>
        <Field label="Новый пароль ещё раз" error={mismatch ? 'Пароли не совпадают' : null}>
          <input type="password" autoComplete="new-password" value={form.again} onChange={(e) => setForm({ ...form, again: e.target.value })} />
        </Field>
        <ErrorBox error={error} />
        <Button
          disabled={busy || !form.old || form.next.length < 8 || form.next !== form.again}
          onClick={() =>
            void run(async () => {
              await post('/client/password', { old_password: form.old, new_password: form.next })
              toast('Пароль сменён')
              onDone()
            })
          }
        >
          Сменить пароль
        </Button>
      </Card>
    </div>
  )
}

function Purchases({ branch }: { branch: string }) {
  const [from, setFrom] = useState(() => {
    const [y, m, d] = todayBishkek().split('-').map(Number)
    const t = new Date(Date.UTC(y, m - 1, d - 90))
    return t.toISOString().slice(0, 10)
  })
  const [to, setTo] = useState(todayBishkek())
  const list = useLoad(() => get<CabinetSale[]>(`/client/sales${qs({ from, to, branch_id: branch })}`), [from, to, branch])
  const rows = list.data ?? []
  // Итог — по всему периоду; чеки — по страницам, другой период — первая страница.
  const pager = usePager(rows, 'cabinet.sales', `${from}:${to}:${branch}`)
  return (
    <div className="flex flex-col gap-3">
      <Card className="flex flex-wrap items-end gap-3">
        <Field label="С">
          <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
        </Field>
        <div className="text-sm text-slate-600">
          Чеков: {rows.filter((r) => r.kind === 'sale').length} на {formatSom(rows.reduce((a, r) => a + r.total_tyiyn, 0))}
        </div>
        {/* Все покупки периода строками чеков (задача 003 Н13). */}
        <div className="ml-auto">
          <ExportButton
            file="Покупки"
            period={periodText(from, to)}
            disabled={rows.length === 0}
            sheets={() => [listSheet('Покупки', PURCHASE_COLS, purchaseRows(rows), { filter: [`период ${periodText(from, to)}`], total: purchaseTotal(rows) })]}
          />
        </div>
      </Card>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>За этот период покупок нет</Empty>
      ) : (
        pager.shown.map((s) => (
          <Card key={s.id} className="flex flex-col gap-2">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div className="font-medium">
                {s.kind === 'return' ? 'Возврат' : 'Чек'} № {s.number} · {formatDateTime(s.created_at)}
              </div>
              <div className="font-semibold">{formatSom(s.total_tyiyn)}</div>
            </div>
            <div className="text-sm text-slate-600">
              {s.contact_name ? `Брал: ${s.contact_name}` : 'Работник не указан'}
              {s.vehicle_plate && ` · машина ${s.vehicle_plate}`}
              {s.personal_of && ` (личная машина ${s.personal_of})`}
              {s.branch_name && ` · ${s.branch_name}`}
              {s.debt_tyiyn !== 0 && (
                <span className="ml-2">
                  <Badge tone="amber">в долг {formatSom(Math.abs(s.debt_tyiyn))}</Badge>
                </span>
              )}
              {(s.invoice_tyiyn ?? 0) !== 0 && (
                <span className="ml-2">
                  <Badge tone="sky">по счёту {formatSom(Math.abs(s.invoice_tyiyn ?? 0))}</Badge>
                </span>
              )}
            </div>
            <ul className="text-sm">
              {s.lines.map((l, i) => (
                <li key={i} className="flex justify-between gap-3 border-t border-slate-100 py-1">
                  <span>
                    {l.name} <span className="text-slate-500">· {cabinetQtyText(l)}</span>
                  </span>
                  <span className="whitespace-nowrap">{formatSom(l.amount_tyiyn)}</span>
                </li>
              ))}
            </ul>
          </Card>
        ))
      )}
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы покупок" />
    </div>
  )
}

function Contacts() {
  const list = useLoad(() => get<Contact[]>('/client/contacts'), [])
  const rows = list.data ?? []
  return (
    <Card className="p-0">
      {rows.length > 0 && (
        <div className="flex justify-end px-4 pt-3">
          <ExportButton file="Работники" sheets={() => [listSheet('Работники', CONTACT_COLS, rows)]} />
        </div>
      )}
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>Работники не записаны — их добавляют на точке при покупке</Empty>
      ) : (
        <Table head={['Работник', 'Приезжал', 'Последний раз', 'Взял на', 'Из них в долг']}>
          {rows.map((c, i) => (
            <tr key={i}>
              <td className="px-2 py-2 font-medium">
                {c.full_name}
                {(c.position || c.phone) && <div className="text-xs font-normal text-slate-500">{[c.position, c.phone].filter(Boolean).join(' · ')}</div>}
              </td>
              <td className="px-2 py-2">{c.visits}</td>
              <td className="whitespace-nowrap px-2 py-2">{c.last_at ? formatDateTime(c.last_at) : '—'}</td>
              <td className="whitespace-nowrap px-2 py-2">{formatSom(c.total_tyiyn)}</td>
              <td className="whitespace-nowrap px-2 py-2">{c.debt_tyiyn ? formatSom(c.debt_tyiyn) : '—'}</td>
            </tr>
          ))}
        </Table>
      )}
    </Card>
  )
}

interface CabinetMessage {
  id: string
  created_at: string
  kind: string
  text: string
}

/** «Сообщения» (задача 002, П7, П7б): что точка писала фирме об оплатах и долге. */
function Messages() {
  const list = useLoad(() => get<CabinetMessage[]>('/client/messages'), [])
  const rows = list.data ?? []
  return (
    <Card className="flex flex-col gap-2">
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>Сообщений пока нет — здесь появятся «Получили оплату» и долг на 1-е число</Empty>
      ) : (
        <ul className="flex flex-col divide-y divide-slate-100" data-testid="cabinet-messages">
          {rows.map((m) => (
            <li key={m.id} className="py-2 text-sm">
              <div className="text-xs text-slate-500">{formatDateTime(m.created_at)}</div>
              <div className="whitespace-pre-wrap">{m.text}</div>
            </li>
          ))}
        </ul>
      )}
    </Card>
  )
}

/** Запрос «Подтвердите обслуживание» (задача 039 C, ADR-202). */
export interface Approval {
  id: string | null
  status: 'not_needed' | 'pending' | 'approved' | 'rejected' | 'expired' | 'skipped'
  mode: string
  sale_id: string | null
  sale_number: number | null
  summary: { plate?: string | null; contact?: string | null; cashier?: string; lines?: string[]; total_tyiyn?: number }
  debt_tyiyn: number
  created_at: string | null
  expires_at: string | null
  decided_by: string
  reason: string
  forced: boolean
  /** Чек проведён без сети, запрос — после синхронизации. */
  offline?: boolean
  can_force: boolean
  can_post: boolean
}

const APPROVAL_STATUS: Record<string, [string, 'slate' | 'green' | 'amber' | 'rose']> = {
  pending: ['Ждёт ответа', 'amber'],
  expired: ['Ждёт ответа', 'amber'],
  approved: ['Подтверждено', 'green'],
  rejected: ['Отклонено', 'rose'],
}

/** «Запросы»: подтвердить или отклонить обслуживание по чеку в долг фирмы. */
function Approvals() {
  const list = useLoad(() => get<Approval[]>('/client/approvals'), [])
  const { run, busy, error } = useAction()
  const [rejecting, setRejecting] = useState<string | null>(null)
  const [reason, setReason] = useState('')
  const rows = list.data ?? []
  const decide = (id: string, ok: boolean) =>
    run(async () => {
      if (ok) await post(`/client/approvals/${id}/approve`, {})
      else await post(`/client/approvals/${id}/reject`, { reason })
      setRejecting(null)
      setReason('')
      toast(ok ? 'Подтверждено' : 'Отклонено')
      list.reload()
    })
  return (
    <Card className="flex flex-col gap-2">
      <ErrorBox error={list.error ?? error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>Запросов нет — здесь появятся просьбы подтвердить обслуживание в долг</Empty>
      ) : (
        <ul className="flex flex-col divide-y divide-slate-100" data-testid="cabinet-approvals">
          {rows.map((a) => {
            const open = (a.status === 'pending' || a.status === 'expired') && !a.forced && !(a.status === 'expired' && a.sale_id)
            const [label, tone] = a.forced ? ['Проведено без подтверждения', 'slate' as const] : (APPROVAL_STATUS[a.status] ?? [a.status, 'slate' as const])
            return (
              <li key={a.id ?? ''} className="flex flex-col gap-1 py-2 text-sm">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="text-xs text-slate-500">{a.created_at ? formatDateTime(a.created_at) : ''}</span>
                  <Badge tone={tone}>{label}</Badge>
                  {a.sale_number != null && <span className="text-xs text-slate-500">чек № {a.sale_number}</span>}
                  {a.offline && <Badge tone="slate">Проведено без сети</Badge>}
                </div>
                <div>
                  {[a.summary.plate, a.summary.contact].filter(Boolean).join(' · ')}
                  {(a.summary.lines?.length ?? 0) > 0 && <div className="text-slate-600">{a.summary.lines?.filter(Boolean).join(', ')}</div>}
                </div>
                <div className="font-medium">В долг: {formatSom(a.debt_tyiyn)}{a.summary.cashier ? <span className="font-normal text-slate-500"> · кассир {a.summary.cashier}</span> : null}</div>
                {a.reason && <div className="text-rose-700">Причина: {a.reason}</div>}
                {open && a.id && (
                  rejecting === a.id ? (
                    <div className="flex flex-wrap items-end gap-2">
                      <Field label="Причина отказа" required>
                        <input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={500} />
                      </Field>
                      <Button variant="danger" disabled={busy || !reason.trim()} onClick={() => a.id && void decide(a.id, false)}>Отклонить</Button>
                      <Button variant="secondary" onClick={() => setRejecting(null)}>Отмена</Button>
                    </div>
                  ) : (
                    <div className="flex gap-2">
                      <Button disabled={busy} onClick={() => a.id && void decide(a.id, true)}>Подтвердить</Button>
                      <Button variant="secondary" disabled={busy} onClick={() => setRejecting(a.id)}>Отклонить</Button>
                    </div>
                  )
                )}
              </li>
            )
          })}
        </ul>
      )}
    </Card>
  )
}

/** Запрос на подтверждение машины (задача 037, ADR-199). */
export interface VehicleRequest {
  id: string
  plate: string
  brand: string
  model: string
  personal_of: string | null
  source: 'cabinet' | 'bot'
  author: string
  status: 'pending' | 'approved' | 'rejected'
  reason: string
  decided_by: string
  created_at: string
}

const REQUEST_STATUS: Record<string, [string, 'green' | 'amber' | 'rose']> = {
  pending: ['Ждёт подтверждения', 'amber'],
  approved: ['Подтверждена', 'green'],
  rejected: ['Отклонена', 'rose'],
}

/** Гараж фирмы: добавить машину и подтвердить машины работников «Оплачивает фирма». */
function Garage() {
  const list = useLoad(() => get<VehicleRequest[]>('/client/requests'), [])
  const { run, busy, error } = useAction()
  const [plate, setPlate] = useState('')
  const [brand, setBrand] = useState('')
  const [model, setModel] = useState('')
  const [rejecting, setRejecting] = useState<string | null>(null)
  const [reason, setReason] = useState('')
  const add = () =>
    run(async () => {
      await post('/client/vehicles', { op_id: crypto.randomUUID(), plate, brand, model })
      setPlate('')
      setBrand('')
      setModel('')
      toast('Машина добавлена — ждёт подтверждения')
      list.reload()
    })
  const decide = (id: string, ok: boolean) =>
    run(async () => {
      if (ok) await post(`/client/requests/${id}/approve`, {})
      else await post(`/client/requests/${id}/reject`, { reason })
      setRejecting(null)
      setReason('')
      toast(ok ? 'Подтверждено' : 'Отклонено')
      list.reload()
    })
  const rows = list.data ?? []
  return (
    <Card className="flex flex-col gap-2" data-testid="cabinet-garage">
      <ErrorBox error={list.error ?? error} />
      <div className="flex flex-wrap items-end gap-2">
        <Field label="Госномер" required><input value={plate} onChange={(e) => setPlate(e.target.value)} placeholder="01KG123ABC" /></Field>
        <Field label="Марка"><input value={brand} onChange={(e) => setBrand(e.target.value)} maxLength={60} /></Field>
        <Field label="Модель"><input value={model} onChange={(e) => setModel(e.target.value)} maxLength={60} /></Field>
        <Button disabled={busy || !plate.trim()} onClick={() => void add()}>Добавить машину</Button>
      </div>
      {rows.length > 0 && (
        <ul className="flex flex-col divide-y divide-slate-100" data-testid="cabinet-requests">
          {rows.map((r) => {
            const [label, tone] = REQUEST_STATUS[r.status] ?? [r.status, 'amber' as const]
            return (
              <li key={r.id} className="flex flex-col gap-1 py-2 text-sm">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{r.plate}</span>
                  <span>{[r.brand, r.model].filter(Boolean).join(' ')}</span>
                  <Badge tone={tone}>{label}</Badge>
                  {r.personal_of && <span className="text-slate-500">личная машина {r.personal_of}</span>}
                </div>
                {r.reason && <div className="text-rose-700">Причина: {r.reason}</div>}
                {r.status === 'pending' && (rejecting === r.id ? (
                  <div className="flex flex-wrap items-end gap-2">
                    <Field label="Причина отказа" required><input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={300} /></Field>
                    <Button variant="danger" disabled={busy || !reason.trim()} onClick={() => void decide(r.id, false)}>Отклонить</Button>
                    <Button variant="secondary" onClick={() => setRejecting(null)}>Отмена</Button>
                  </div>
                ) : (
                  <div className="flex gap-2">
                    <Button disabled={busy} onClick={() => void decide(r.id, true)}>Подтвердить</Button>
                    <Button variant="secondary" disabled={busy} onClick={() => setRejecting(r.id)}>Отклонить</Button>
                  </div>
                ))}
              </li>
            )
          })}
        </ul>
      )}
    </Card>
  )
}

/** Акт сверки — тот же экран, что в программе (задача 003): периоды, сальдо, строки, обороты,
 *  вывод, «Печать» и «Скачать»; без ссылок на документы точки. */
function Act({ me, branch = '' }: { me: Me; branch?: string }) {
  const seller: Seller = { name: '', inn: '', address: '', phone: '', director: '', bank: '', city: '', ...me.seller }
  return (
    <Card className="flex flex-col gap-3">
      <ReconciliationView source="/client/reconciliation" defaultPeriod={me.act_period ?? 'all'} seller={seller} branchId={branch || undefined} />
      {!seller.name && <div className="text-xs text-slate-400">Реквизиты точки не заполнены — в печати будет {BLANK}.</div>}
    </Card>
  )
}

export default function CabinetApp() {
  const me = useLoad(
    () =>
      get<Me>('/client/me').catch((e: unknown) => {
        if (e instanceof ApiError && e.status === 401) return null
        throw e
      }),
    [],
  )
  const [tab, setTab] = useState<'sales' | 'contacts' | 'cars' | 'act' | 'messages' | 'approvals'>('sales')
  const [stab, setStab] = useState<SupplierTab>('invoices')
  /** Отбор кабинета юрлица по филиалу (задача 036): '' — все филиалы. */
  const [branch, setBranch] = useState('')
  const supplier = me.data?.role === 'supplier'
  useEffect(() => {
    document.title = supplier ? 'Кабинет поставщика — Avtodom' : 'Кабинет клиента — Avtodom'
  }, [supplier])

  if (me.loading && me.data === null && !me.error) return <Loading />
  if (me.error) return <ErrorBox error={me.error} />
  if (!me.data) return <LoginForm onDone={me.reload} />
  const m = me.data
  if (m.must_change)
    return (
      <>
        <ChangePassword me={m} onDone={me.reload} />
        <Toaster />
      </>
    )

  return (
    <div className="min-h-screen bg-slate-100">
      <header className="bg-slate-900 px-4 py-3 text-white">
        <div className="mx-auto flex max-w-4xl flex-wrap items-center justify-between gap-2">
          <div>
            <div className="font-semibold">{m.name}</div>
            <div className="text-xs text-slate-300">ИНН {m.inn}</div>
          </div>
          <button
            type="button"
            className="text-sm text-sky-300 hover:underline"
            onClick={() => void post('/client/logout').finally(me.reload)}
          >
            Выйти
          </button>
        </div>
      </header>
      {supplier ? (
        <main className="mx-auto flex max-w-4xl flex-col gap-4 p-4">
          <SupplierHeader s={m.supplier} />
          <div className="inline-grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm sm:grid-cols-4">
            {SUPPLIER_TABS.map(([k, label]) => (
              <button
                key={k}
                type="button"
                className={`min-h-[42px] px-3 py-2 ${stab === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setStab(k)}
              >
                {label}
              </button>
            ))}
          </div>
          {stab === 'act' ? <Act me={m} /> : <SupplierTabBody tab={stab} />}
        </main>
      ) : (
      <main className="mx-auto flex max-w-4xl flex-col gap-4 p-4">
        <Card className="flex flex-wrap gap-x-8 gap-y-2">
          <div>
            <div className="text-xs text-slate-500">{m.balance_tyiyn >= 0 ? 'Ваш долг' : 'Ваш аванс'}</div>
            <div className={`text-2xl font-bold ${m.balance_tyiyn > 0 ? 'text-amber-700' : ''}`}>{formatSom(Math.abs(m.balance_tyiyn))}</div>
            <DebtKindsLine balance={m.balance_tyiyn} personal={m.personal_debt_tyiyn} className="mt-1 text-xs text-slate-600" />
            {(m.branch_balances?.length ?? 0) > 1 && (
              <div className="text-xs text-slate-600">
                {m.branch_balances?.map((b) => (
                  <div key={b.branch_id}>
                    {b.name}: {formatSom(b.balance_tyiyn)}
                  </div>
                ))}
              </div>
            )}
          </div>
          {m.overdue_tyiyn > 0 && (
            <div>
              <div className="text-xs text-slate-500">Просрочено</div>
              <div className="text-2xl font-bold text-rose-700">{formatSom(m.overdue_tyiyn)}</div>
            </div>
          )}
          {m.due_days !== null && (
            <div>
              <div className="text-xs text-slate-500">Срок оплаты</div>
              <div className="font-medium">{m.due_days} дней</div>
            </div>
          )}
          {m.credit_limit_tyiyn !== null && (
            <div>
              <div className="text-xs text-slate-500">Лимит долга</div>
              <div className="font-medium">{formatSom(m.credit_limit_tyiyn)}</div>
            </div>
          )}
        </Card>
        <div className="inline-grid grid-cols-3 overflow-hidden rounded-md border border-slate-300 text-sm sm:grid-cols-6">
          {(
            [
              ['sales', 'Покупки'],
              ['contacts', 'Работники'],
              ['cars', 'Машины'],
              ['act', 'Акт сверки'],
              ['messages', 'Сообщения'],
              ['approvals', 'Запросы'],
            ] as const
          ).map(([k, label]) => (
            <button
              key={k}
              type="button"
              className={`min-h-[42px] px-3 py-2 ${tab === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
              onClick={() => setTab(k)}
            >
              {label}
            </button>
          ))}
        </div>
        {(m.branches?.length ?? 0) > 1 && (tab === 'sales' || tab === 'act') && (
          <div className="inline-flex flex-wrap overflow-hidden rounded-md border border-slate-300 text-sm" data-testid="cabinet-branch">
            {[{ id: '', name: 'Все филиалы' }, ...(m.branches ?? [])].map((b) => (
              <button
                key={b.id || 'all'}
                type="button"
                className={`min-h-[38px] px-3 py-1.5 ${branch === b.id ? 'bg-slate-800 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setBranch(b.id)}
              >
                {b.name}
              </button>
            ))}
          </div>
        )}
        {tab === 'sales' && <Purchases branch={branch} />}
        {tab === 'contacts' && <Contacts />}
        {tab === 'cars' && <Garage />}
        {tab === 'cars' && <OilBookList path="/client/oil-book" editable={false} />}
        {tab === 'act' && <Act me={m} branch={branch} />}
        {tab === 'messages' && <Messages />}
        {tab === 'approvals' && <Approvals />}
      </main>
      )}
      <Toaster />
    </div>
  )
}

const CONTACT_COLS: ExportCol<Contact>[] = [
  { label: 'Работник', value: (c) => c.full_name },
  { label: 'Должность', value: (c) => c.position ?? '' },
  { label: 'Телефон', value: (c) => c.phone ?? '' },
  { label: 'Приезжал', value: (c) => xl.int(c.visits) },
  { label: 'Последний раз', value: (c) => xl.dateTime(c.last_at) },
  { label: 'Взял на', value: (c) => xl.money(c.total_tyiyn) },
  { label: 'Из них в долг', value: (c) => xl.money(c.debt_tyiyn) },
]
