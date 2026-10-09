// Клиенты с живыми итогами «должны нам» и «авансы» (SPEC-10, ADR-028).
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { balanceText } from '../components/ClientPicker'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, PageHeader, Table } from '../components/ui'
import { get, post, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Party } from '../lib/types'

/** Таблица долгов обновляется сама, пока экран открыт (ADR-028). */
const REFRESH_MS = 10_000

export default function Clients() {
  const navigate = useNavigate()
  const [q, setQ] = useState('')
  const [kind, setKind] = useState<'' | 'person' | 'company'>('')
  const [onlyDebtors, setOnlyDebtors] = useState(false)
  const [form, setForm] = useState({ kind: 'person' as Party['kind'], name: '', phone: '', inn: '' })
  const query = useDebounced(q.trim(), 250)
  const create = useAction()
  const list = useLoad(
    () => get<Party[]>(`/parties${qs({ role: 'customer', q: query, kind, only_debtors: onlyDebtors })}`),
    [query, kind, onlyDebtors],
  )

  const { reload } = list
  useEffect(() => {
    const t = setInterval(reload, REFRESH_MS)
    return () => clearInterval(t)
  }, [reload])

  const rows = list.data ?? []
  const owe = rows.filter((p) => p.balance_tyiyn > 0).reduce((a, p) => a + p.balance_tyiyn, 0)
  const advance = rows.filter((p) => p.balance_tyiyn < 0).reduce((a, p) => a - p.balance_tyiyn, 0)
  const company = form.kind === 'company'
  const notFilled = missingWithFocus(
    [Boolean(form.name.trim()), company ? 'название фирмы' : 'ФИО', '#new-client-name'],
    [!company || Boolean(form.inn.trim()), 'ИНН фирмы', '#new-client-inn'],
  )

  const add = () =>
    void create.run(async () => {
      const p = await post<Party>('/parties', {
        role: 'customer',
        kind: form.kind,
        name: form.name.trim(),
        phone: form.phone.trim(),
        inn: form.inn.trim(),
      })
      setForm({ kind: form.kind, name: '', phone: '', inn: '' })
      navigate(`/clients/${p.id}`)
    })

  return (
    <div>
      <PageHeader title="Клиенты" />

      <Card className="mb-4">
        <div className="grid gap-3 sm:grid-cols-[1fr_1fr_1fr_auto] sm:items-end">
          <Field label={company ? 'Название фирмы' : 'ФИО'} required>
            <input id="new-client-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} />
          </Field>
          <Field label="Телефон">
            <input type="tel" value={form.phone} onChange={(e) => setForm({ ...form, phone: e.target.value })} />
          </Field>
          <Field label="ИНН" required={company}>
            <input id="new-client-inn" inputMode="numeric" value={form.inn} onChange={(e) => setForm({ ...form, inn: e.target.value })} />
          </Field>
          <Button disabled={create.busy || notFilled.length > 0} onClick={add}>
            Добавить
          </Button>
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <div className="flex overflow-hidden rounded-md border border-slate-300 text-sm">
            {(
              [
                ['person', 'Физлицо'],
                ['company', 'Юрлицо'],
              ] as const
            ).map(([k, label]) => (
              <button
                key={k}
                type="button"
                className={`px-3 py-2 ${form.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setForm({ ...form, kind: k })}
              >
                {label}
              </button>
            ))}
          </div>
          <Missing items={notFilled} />
          <ErrorBox error={create.error} />
        </div>
      </Card>

      <Card>
        <div className="mb-4 grid gap-3 sm:grid-cols-[2fr_1fr_auto] sm:items-end">
          <Field label="Поиск">
            <input placeholder="Название, телефон, ИНН" value={q} onChange={(e) => setQ(e.target.value)} />
          </Field>
          <Field label="Вид">
            <select value={kind} onChange={(e) => setKind(e.target.value as typeof kind)}>
              <option value="">Все</option>
              <option value="company">Юрлица</option>
              <option value="person">Физлица</option>
            </select>
          </Field>
          <div className="pb-2">
            <Checkbox label="Только с долгом" checked={onlyDebtors} onChange={setOnlyDebtors} />
          </div>
        </div>

        <div className="mb-3 flex flex-wrap gap-6 text-sm">
          <div>
            <div className="text-xs text-slate-500">Должны нам</div>
            <div className="text-xl font-bold text-amber-700">{formatSom(owe)}</div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Авансы клиентов</div>
            <div className="text-xl font-bold text-sky-700">{formatSom(advance)}</div>
          </div>
          <div className="self-end text-xs text-slate-400">обновляется каждые 10 секунд</div>
        </div>

        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty>Клиентов не найдено. Заведите первого — его можно выбрать прямо в кассе.</Empty>
        ) : (
          <Table head={['Клиент', 'Вид', 'Телефон', 'ИНН', 'Баланс']}>
            {rows.map((p) => (
              <tr
                key={p.id}
                className={`cursor-pointer hover:bg-slate-50 ${p.active ? '' : 'text-slate-400'}`}
                onClick={() => navigate(`/clients/${p.id}`)}
              >
                <td className="px-2 py-2 font-medium">{p.name}</td>
                <td className="px-2 py-2">{p.kind === 'company' ? 'Юрлицо' : 'Физлицо'}</td>
                <td className="whitespace-nowrap px-2 py-2">{p.phone || '—'}</td>
                <td className="whitespace-nowrap px-2 py-2">{p.inn || '—'}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {p.balance_tyiyn === 0 ? (
                    <span className="text-slate-500">—</span>
                  ) : (
                    <Badge tone={p.balance_tyiyn > 0 ? 'amber' : 'sky'}>{balanceText(p.balance_tyiyn)}</Badge>
                  )}
                  {p.overdue_tyiyn > 0 && (
                    <span className="ml-2">
                      <Badge tone="rose">просрочено {formatSom(p.overdue_tyiyn)}</Badge>
                    </span>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
