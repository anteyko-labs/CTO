// Кабинет юрлица: вход по ИНН, смена начального пароля, покупки, работники, долг, акт сверки
// (SPEC-12, ADR-049). Только просмотр — ничего не проводится.
import { useEffect, useState } from 'react'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Table, Toaster, toast } from '../../components/ui'
import { ApiError, get, post, qs } from '../../lib/api'
import { OilBookList } from '../../components/OilBook'
import { BLANK, printReconciliation, type Reconciliation, type Seller } from '../../lib/debtDocs'
import { formatDateTime, formatKg, formatLiters, formatSom, todayBishkek } from '../../lib/format'
import { useAction, useLoad } from '../../lib/hooks'

interface Me {
  name: string
  inn: string
  phone: string
  must_change: boolean
  balance_tyiyn: number
  overdue_tyiyn: number
  due_days: number | null
  credit_limit_tyiyn: number | null
  seller: Partial<Seller>
}

interface SaleLine {
  name: string
  kind: string
  qty: number
  unit: string
  container_ml: number | null
  amount_tyiyn: number
}

interface ClientSale {
  id: string
  number: number
  kind: 'sale' | 'return'
  business_date: string
  created_at: string
  contact_name: string | null
  vehicle_plate: string | null
  total_tyiyn: number
  debt_tyiyn: number
  lines: SaleLine[]
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

const dateRu = (iso: string) => iso.slice(0, 10).split('-').reverse().join('.')

function qtyText(l: SaleLine): string {
  if (l.kind === 'pour') return formatLiters(l.qty)
  if (l.kind === 'weight') return formatKg(l.qty)
  if (l.kind === 'container') return `${l.qty} кан.${l.container_ml ? ` × ${formatLiters(l.container_ml)}` : ''}`
  if (l.kind === 'service') return `${l.qty} усл.`
  return `${l.qty} шт`
}

function LoginForm({ onDone }: { onDone: () => void }) {
  const [inn, setInn] = useState('')
  const [password, setPassword] = useState('')
  const { busy, error, run } = useAction()
  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-100 p-4">
      <Card className="flex w-full max-w-sm flex-col gap-3">
        <h1 className="text-lg font-semibold">Кабинет клиента Avtodom</h1>
        <p className="text-sm text-slate-600">Для организаций: покупки ваших работников, долг и акт сверки.</p>
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

function Purchases() {
  const [from, setFrom] = useState(() => {
    const [y, m, d] = todayBishkek().split('-').map(Number)
    const t = new Date(Date.UTC(y, m - 1, d - 90))
    return t.toISOString().slice(0, 10)
  })
  const [to, setTo] = useState(todayBishkek())
  const list = useLoad(() => get<ClientSale[]>(`/client/sales${qs({ from, to })}`), [from, to])
  const rows = list.data ?? []
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
      </Card>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>За этот период покупок нет</Empty>
      ) : (
        rows.map((s) => (
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
              {s.debt_tyiyn !== 0 && (
                <span className="ml-2">
                  <Badge tone="amber">в долг {formatSom(Math.abs(s.debt_tyiyn))}</Badge>
                </span>
              )}
            </div>
            <ul className="text-sm">
              {s.lines.map((l, i) => (
                <li key={i} className="flex justify-between gap-3 border-t border-slate-100 py-1">
                  <span>
                    {l.name} <span className="text-slate-500">· {qtyText(l)}</span>
                  </span>
                  <span className="whitespace-nowrap">{formatSom(l.amount_tyiyn)}</span>
                </li>
              ))}
            </ul>
          </Card>
        ))
      )}
    </div>
  )
}

function Contacts() {
  const list = useLoad(() => get<Contact[]>('/client/contacts'), [])
  const rows = list.data ?? []
  return (
    <Card className="p-0">
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

function Act({ me }: { me: Me }) {
  const [from, setFrom] = useState('')
  const [to, setTo] = useState(todayBishkek())
  const act = useLoad(() => get<Reconciliation>(`/client/reconciliation${qs({ from, to })}`), [from, to])
  const seller: Seller = { name: '', inn: '', address: '', phone: '', director: '', bank: '', city: '', ...me.seller }
  const a = act.data
  return (
    <div className="flex flex-col gap-3">
      <Card className="flex flex-wrap items-end gap-3">
        <Field label="С" hint="Пусто — с первой операции">
          <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
        </Field>
        <Button variant="secondary" disabled={!a} onClick={() => a && printReconciliation(a, seller)}>
          Печать
        </Button>
      </Card>
      <ErrorBox error={act.error} />
      {act.loading && !a ? (
        <Loading />
      ) : a ? (
        <Card className="p-0">
          <Table head={['Дата', 'Документ', 'Отгружено вам', 'Оплачено вами']}>
            <tr className="bg-slate-50 font-medium">
              <td className="px-2 py-2" colSpan={2}>
                Сальдо на {dateRu(a.from)}
              </td>
              <td className="px-2 py-2" colSpan={2}>
                {a.opening_tyiyn > 0 ? `ваш долг ${formatSom(a.opening_tyiyn)}` : a.opening_tyiyn < 0 ? `ваш аванс ${formatSom(-a.opening_tyiyn)}` : 'долга нет'}
              </td>
            </tr>
            {a.rows.map((r, i) => (
              <tr key={i}>
                <td className="whitespace-nowrap px-2 py-2">{dateRu(r.date)}</td>
                <td className="px-2 py-2">{r.document}</td>
                <td className="whitespace-nowrap px-2 py-2">{r.debit_tyiyn ? formatSom(r.debit_tyiyn) : ''}</td>
                <td className="whitespace-nowrap px-2 py-2">{r.credit_tyiyn ? formatSom(r.credit_tyiyn) : ''}</td>
              </tr>
            ))}
            <tr className="bg-slate-50 font-semibold">
              <td className="px-2 py-2" colSpan={2}>
                Сальдо на {dateRu(a.to)}
              </td>
              <td className="px-2 py-2" colSpan={2}>
                {a.closing_tyiyn > 0 ? `ваш долг ${formatSom(a.closing_tyiyn)}` : a.closing_tyiyn < 0 ? `ваш аванс ${formatSom(-a.closing_tyiyn)}` : 'долга нет'}
              </td>
            </tr>
          </Table>
        </Card>
      ) : null}
      {!seller.name && <div className="text-xs text-slate-400">Реквизиты точки не заполнены — в печати будет {BLANK}.</div>}
    </div>
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
  const [tab, setTab] = useState<'sales' | 'contacts' | 'cars' | 'act'>('sales')
  useEffect(() => {
    document.title = 'Кабинет клиента — Avtodom'
  }, [])

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
      <main className="mx-auto flex max-w-4xl flex-col gap-4 p-4">
        <Card className="flex flex-wrap gap-x-8 gap-y-2">
          <div>
            <div className="text-xs text-slate-500">{m.balance_tyiyn >= 0 ? 'Ваш долг' : 'Ваш аванс'}</div>
            <div className={`text-2xl font-bold ${m.balance_tyiyn > 0 ? 'text-amber-700' : ''}`}>{formatSom(Math.abs(m.balance_tyiyn))}</div>
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
        <div className="inline-grid grid-cols-4 overflow-hidden rounded-md border border-slate-300 text-sm">
          {(
            [
              ['sales', 'Покупки'],
              ['contacts', 'Работники'],
              ['cars', 'Машины'],
              ['act', 'Акт сверки'],
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
        {tab === 'sales' && <Purchases />}
        {tab === 'contacts' && <Contacts />}
        {tab === 'cars' && <OilBookList path="/client/oil-book" editable={false} />}
        {tab === 'act' && <Act me={m} />}
      </main>
      <Toaster />
    </div>
  )
}
