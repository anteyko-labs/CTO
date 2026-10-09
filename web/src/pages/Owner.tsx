import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Badge, Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useLoad, usePolling } from '../lib/hooks'
import type { Dashboard, ProfitReport } from '../lib/types'

const percent = (bp: number | null): string => (bp === null ? '—' : `${(bp / 100).toFixed(1).replace('.', ',')} %`)

function Tile({ label, value, tone = '' }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`text-2xl font-bold ${tone}`}>{value}</div>
    </div>
  )
}

/** Сводка владельца: что с точкой прямо сейчас (SPEC-08). */
export function OwnerDashboard() {
  const d = useLoad(() => get<Dashboard>('/owner/dashboard'), [])
  // Сводка живая: чеки с кассы появляются без перезагрузки страницы.
  usePolling(d.reload)
  if (d.loading && !d.data) return <Loading />
  if (!d.data) return <ErrorBox error={d.error} />
  const t = d.data.totals

  return (
    <div>
      <PageHeader title="Сводка" />
      <div className="flex flex-col gap-4">
        <Card className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <Tile label="Продано сегодня" value={formatSom(t.goods_tyiyn + t.services_tyiyn)} />
          <Tile label="Чистая прибыль" value={formatSom(t.net_tyiyn)} tone={t.net_tyiyn < 0 ? 'text-rose-700' : 'text-emerald-700'} />
          <Tile label="Денег всего" value={formatSom(d.data.money_total_tyiyn)} />
          <Tile label="К выплате" value={formatSom(d.data.to_pay_tyiyn)} />
        </Card>

        <Card className="flex flex-wrap gap-x-8 gap-y-3 text-sm">
          <div>
            <div className="text-xs text-slate-500">Чеков</div>
            <div className="font-medium">
              {t.sales_count}, средний {formatSom(d.data.average_check_tyiyn)}
            </div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Возвраты</div>
            <div className="font-medium">{formatSom(d.data.returns_tyiyn)}</div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Валовая</div>
            <div className="font-medium">
              {formatSom(t.gross_tyiyn)} · маржа {percent(t.margin_bp)}
            </div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Оплата труда</div>
            <div className="font-medium">{formatSom(t.payroll_tyiyn)}</div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Расходы</div>
            <div className="font-medium">{formatSom(t.expenses_tyiyn)}</div>
          </div>
          <div>
            <div className="text-xs text-slate-500">Смена</div>
            <div className="font-medium">
              {d.data.shift_open ? `открыта, ${d.data.shift_cashier}` : 'не открыта'}
            </div>
          </div>
        </Card>

        <Card>
          <h2 className="mb-3 font-semibold">Деньги</h2>
          <div className="flex flex-wrap gap-6">
            {d.data.accounts.map((a) => (
              <Tile key={a.name} label={a.name} value={formatSom(a.balance_tyiyn)} />
            ))}
            <Tile label="Должны нам" value={formatSom(d.data.debts_in_tyiyn)} />
            <Tile label="Должны мы" value={formatSom(d.data.debts_out_tyiyn)} />
          </div>
        </Card>

        {(d.data.low_stock > 0 || d.data.needs_review > 0 || d.data.stale_stock > 0) && (
          <Card className="flex flex-wrap items-center gap-3 text-sm">
            {d.data.low_stock > 0 && (
              <Link to="/stock" className="underline">
                <Badge tone="amber">заканчивается: {d.data.low_stock}</Badge>
              </Link>
            )}
            {d.data.needs_review > 0 && (
              <Link to="/stock" className="underline">
                <Badge tone="rose">проверить остаток: {d.data.needs_review}</Badge>
              </Link>
            )}
            {d.data.stale_stock > 0 && (
              <Link to="/stock" className="underline">
                <Badge tone="slate">залежался: {d.data.stale_stock}</Badge>
              </Link>
            )}
          </Card>
        )}
      </div>
    </div>
  )
}

/** Прибыль за период с разрезами (SPEC-08). */
export default function Profit() {
  const today = todayBishkek()
  const [from, setFrom] = useState(today)
  const [to, setTo] = useState(today)
  const r = useLoad(() => get<ProfitReport>(`/reports/profit${qs({ from, to })}`), [from, to])
  const quick = (days: number) => {
    setFrom(shiftDate(today, -days))
    setTo(today)
  }

  const t = r.data?.totals

  return (
    <div>
      <PageHeader title="Прибыль" />
      <Card className="mb-4 flex flex-wrap items-end gap-3">
        <Field label="С">
          <input type="date" className="w-40" value={from} onChange={(e) => setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" className="w-40" value={to} onChange={(e) => setTo(e.target.value)} />
        </Field>
        <div className="flex gap-2">
          {[
            [0, 'Сегодня'],
            [6, '7 дней'],
            [29, 'Месяц'],
          ].map(([d, label]) => (
            <button
              key={label as string}
              type="button"
              className="rounded-md border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50"
              onClick={() => quick(d as number)}
            >
              {label as string}
            </button>
          ))}
        </div>
      </Card>

      <ErrorBox error={r.error} />
      {r.loading && !r.data ? (
        <Loading />
      ) : t ? (
        <div className="flex flex-col gap-4">
          <Card>
            <h2 className="mb-3 font-semibold">Как получилась прибыль</h2>
            <div className="flex flex-col gap-1 text-sm">
              {[
                ['Выручка за товары', t.goods_tyiyn, ''],
                ['Выручка за работы', t.services_tyiyn, ''],
                ['− Себестоимость', -t.cost_tyiyn, ''],
                ['= Валовая прибыль', t.gross_tyiyn, 'font-semibold'],
                ['− Оплата труда', -t.payroll_tyiyn, ''],
                ['− Комиссия банка', -t.bank_fee_tyiyn, ''],
                ['− Расходы', -t.expenses_tyiyn, ''],
                ['= Чистая прибыль', t.net_tyiyn, 'text-lg font-bold'],
              ].map(([label, value, cls]) => (
                <div key={label as string} className={`flex justify-between gap-4 ${cls as string}`}>
                  <span>{label as string}</span>
                  <span>{formatSom(value as number)}</span>
                </div>
              ))}
              <div className="mt-2 text-xs text-slate-500">
                Чеков {t.sales_count}, маржа {percent(t.margin_bp)}
              </div>
            </div>
          </Card>

          {r.data && r.data.warnings.length > 0 && (
            <Card className="flex flex-col gap-1 text-sm text-amber-800">
              {r.data.warnings.map((w) => (
                <div key={w}>{w}</div>
              ))}
            </Card>
          )}

          <Card>
            <h2 className="mb-3 font-semibold">По категориям</h2>
            {(r.data?.categories ?? []).length === 0 ? (
              <Empty>Продаж за период нет</Empty>
            ) : (
              <Table head={['Категория', 'Выручка', 'Себестоимость', 'Валовая']}>
                {(r.data?.categories ?? []).map((c) => (
                  <tr key={c.name}>
                    <td className="px-2 py-2 font-medium">{c.name}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatSom(c.revenue_tyiyn)}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatSom(c.cost_tyiyn)}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatSom(c.gross_tyiyn)}</td>
                  </tr>
                ))}
              </Table>
            )}
          </Card>

          <Card>
            <h2 className="mb-3 font-semibold">По дням</h2>
            <Table head={['День', 'Выручка', 'Валовая', 'Оплата труда', 'Расходы', 'Чистая']}>
              {(r.data?.days ?? []).map((d) => (
                <tr key={d.date}>
                  <td className="whitespace-nowrap px-2 py-2">{d.date}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.revenue_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.gross_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.payroll_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.expenses_tyiyn)}</td>
                  <td className={`whitespace-nowrap px-2 py-2 font-medium ${d.net_tyiyn < 0 ? 'text-rose-700' : ''}`}>
                    {formatSom(d.net_tyiyn)}
                  </td>
                </tr>
              ))}
            </Table>
          </Card>

          {(r.data?.articles ?? []).length > 0 && (
            <Card>
              <h2 className="mb-3 font-semibold">Расходы по статьям</h2>
              <Table head={['Статья', 'Сумма']}>
                {(r.data?.articles ?? []).map((a) => (
                  <tr key={a.name}>
                    <td className="px-2 py-2">{a.name}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatSom(a.amount_tyiyn)}</td>
                  </tr>
                ))}
              </Table>
            </Card>
          )}
        </div>
      ) : null}
    </div>
  )
}
