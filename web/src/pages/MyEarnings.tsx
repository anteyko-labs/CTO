// «Мой заработок» (задача 005 Н1, ADR-{n1}): вход, связанный с сотрудником (мастер, кассир,
// администратор), видит только своё — сколько заработал, сколько выплачено, сколько ему должны.
// Права «Расчёт» не нужно: сервер (`GET /me/earnings`) отдаёт только сотрудника самого входа.
// Расчёт — тот же, что в кабинете работника в боте и на экране «Расчёт».
import { useState } from 'react'
import { Card, CardTitle, Empty, ErrorBox, Loading, PageHeader, Table } from '../components/ui'
import { get } from '../lib/api'
import { formatDate, formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { monthLabel, type MyEarnings } from '../lib/myEarnings'
import { PURPOSE_LABELS } from '../lib/payouts'

function Stat({ label, value, hint, testid }: { label: string; value: number; hint?: string; testid: string }) {
  return (
    <div className="rounded-lg border border-slate-200 p-3" data-testid={testid}>
      <div className="text-xs text-slate-500">{label}</div>
      <div className="text-xl font-semibold tabular-nums">{formatSom(value)}</div>
      {hint && <div className="text-xs text-slate-500">{hint}</div>}
    </div>
  )
}

export default function MyEarningsPage() {
  const [month, setMonth] = useState<string | null>(null)
  const data = useLoad(() => get<MyEarnings>(`/me/earnings${month ? `?month=${month}` : ''}`), [month])
  const d = data.data

  return (
    <div>
      <PageHeader
        title="Мой заработок"
        subtitle={d ? d.full_name : undefined}
        actions={
          d && (
            <select aria-label="Месяц" data-testid="my-earnings-month" value={d.month_from} onChange={(e) => setMonth(e.target.value)}>
              {d.months.map((m) => (
                <option key={m} value={m}>
                  {monthLabel(m)}
                </option>
              ))}
            </select>
          )
        }
      />
      <ErrorBox error={data.error} />
      {!d ? (
        data.loading && <Loading />
      ) : (
        <div className="flex flex-col gap-4">
          <Card data-testid="my-earnings-summary">
            <CardTitle>
              {monthLabel(d.month_from)} · {formatDate(d.month_from)}–{formatDate(d.month_to)}
            </CardTitle>
            <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
              <Stat testid="my-earnings-accrued" label="Заработано за месяц" value={d.accrued_tyiyn} />
              <Stat
                testid="my-earnings-paid"
                label="Выплачено за месяц"
                value={d.paid_tyiyn}
                hint={d.advance_tyiyn > 0 ? `в том числе авансом ${formatSom(d.advance_tyiyn)}` : undefined}
              />
              <Stat testid="my-earnings-closing" label="Должны на конец месяца" value={d.closing_tyiyn} hint={`на начало — ${formatSom(d.opening_tyiyn)}`} />
              <Stat testid="my-earnings-owed" label="Должны сейчас" value={d.owed_now_tyiyn} hint="всё начисленное минус всё выданное" />
            </div>
            {d.salary_line && <p className="mt-3 text-sm text-slate-600">{d.salary_line}</p>}
          </Card>

          <Card data-testid="my-earnings-today">
            <CardTitle>Сегодня, {formatDate(d.today)}</CardTitle>
            <div className="text-lg font-semibold tabular-nums">{formatSom(d.today_tyiyn)}</div>
            {d.today_lines.length > 0 && (
              <ul className="mt-1 text-sm text-slate-600">
                {d.today_lines.map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
              </ul>
            )}
          </Card>

          <Card>
            <CardTitle>За что начислено</CardTitle>
            {d.lines.length === 0 ? (
              <Empty>За этот месяц начислений нет</Empty>
            ) : (
              <ul className="text-sm text-slate-700">
                {d.lines.map((l, i) => (
                  <li key={i}>{l}</li>
                ))}
              </ul>
            )}
          </Card>

          <div className="grid gap-4 lg:grid-cols-2">
            <Card>
              <CardTitle>По дням</CardTitle>
              {d.days.length === 0 ? (
                <Empty>Пока ничего</Empty>
              ) : (
                <Table head={['День', 'Заработано']}>
                  {d.days.map((r) => (
                    <tr key={r.date}>
                      <td className="px-2 py-1.5">{formatDate(r.date)}</td>
                      <td className="px-2 py-1.5 text-right tabular-nums">{formatSom(r.amount_tyiyn)}</td>
                    </tr>
                  ))}
                </Table>
              )}
            </Card>
            <Card data-testid="my-earnings-payouts">
              <CardTitle>Выплаты</CardTitle>
              {d.payouts.length === 0 ? (
                <Empty>Выплат за месяц не было</Empty>
              ) : (
                <Table head={['Дата', 'Вид', 'Сумма']}>
                  {d.payouts.map((p, i) => (
                    <tr key={i} className={p.reversal ? 'text-slate-400' : ''}>
                      <td className="px-2 py-1.5">{formatDate(p.date)}</td>
                      <td className="px-2 py-1.5">
                        {PURPOSE_LABELS[p.purpose] ?? p.purpose}
                        {p.reversal && ' (отмена)'}
                      </td>
                      <td className="px-2 py-1.5 text-right tabular-nums">{formatSom(p.amount_tyiyn)}</td>
                    </tr>
                  ))}
                </Table>
              )}
            </Card>
          </div>
        </div>
      )}
    </div>
  )
}
