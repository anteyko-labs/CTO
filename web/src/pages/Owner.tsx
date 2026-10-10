import { useState } from 'react'
import { Link } from 'react-router-dom'
import { Badge, Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDate, formatSom, hourBishkek, shiftDate, todayBishkek } from '../lib/format'
import { useLoad, usePolling } from '../lib/hooks'
import { StaffPayList } from '../components/StaffPayList'
import { ProfitChart, type Point, type Series } from '../components/ProfitChart'
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

/** Строка «лесенки» прибыли: подпись слева, сумма справа. */
function Step({ label, value, strong = false, minus = false }: { label: string; value: number; strong?: boolean; minus?: boolean }) {
  return (
    <div className={`flex items-baseline justify-between gap-4 py-1 ${strong ? 'border-t border-slate-200 font-semibold' : 'text-slate-700'}`}>
      <span>{label}</span>
      <span className={strong && value < 0 ? 'text-rose-700' : ''}>{minus && value !== 0 ? `− ${formatSom(value)}` : formatSom(value)}</span>
    </div>
  )
}

/** Сводка владельца: что с точкой прямо сейчас (SPEC-08). */
const PERIODS = [
  ['today', 'Сегодня', 0],
  ['week', '7 дней', 6],
  ['month', '30 дней', 29],
] as const

export function OwnerDashboard() {
  const today = todayBishkek()
  const [period, setPeriod] = useState<(typeof PERIODS)[number][0]>('today')
  const back = PERIODS.find((x) => x[0] === period)?.[2] ?? 0
  const from = shiftDate(today, -back)
  const d = useLoad(() => get<Dashboard>('/owner/dashboard'), [])
  const r = useLoad(() => get<ProfitReport>(`/reports/profit${qs({ from, to: today })}`), [from, today])
  // Сводка живая: чеки с кассы появляются без перезагрузки страницы.
  usePolling(d.reload)
  usePolling(r.reload)
  if (d.loading && !d.data) return <Loading />
  if (!d.data) return <ErrorBox error={d.error} />
  const t = r.data?.totals ?? d.data.totals
  const revenue = t.goods_tyiyn + t.services_tyiyn
  const word = period === 'today' ? 'сегодня' : period === 'week' ? 'за 7 дней' : 'за 30 дней'
  // Сегодня — каждый час с открытия (8:00) до текущего, без пропусков: линия идёт вверх и вниз честно.
  const hours = r.data?.hours ?? []
  const nowHour = hourBishkek()
  const firstHour = Math.min(8, ...hours.map((h) => h.hour))
  const lastHour = Math.max(Number.isFinite(nowHour) ? nowHour : 0, ...hours.map((h) => h.hour))
  const hourRange = Array.from({ length: Math.max(lastHour - firstHour + 1, 1) }, (_, k) => firstHour + k)
  const byHour = new Map(hours.map((h) => [h.hour, h]))
  const days = [...(r.data?.days ?? [])].reverse()
  const weekday = (d: string) => ['Вс', 'Пн', 'Вт', 'Ср', 'Чт', 'Пт', 'Сб'][new Date(`${d}T12:00:00`).getDay()]
  const chart: { points: Point[]; series: Series[] } =
    period === 'today'
      ? {
          points: hourRange.map((h) => ({
            label: `${h}:00`,
            title: `${h}:00–${h + 1}:00 · чеков ${byHour.get(h)?.sales_count ?? 0}`,
          })),
          series: [
            { name: 'Выручка', color: 'slate', values: hourRange.map((h) => byHour.get(h)?.revenue_tyiyn ?? 0) },
            { name: 'Валовая прибыль', color: 'sky', values: hourRange.map((h) => byHour.get(h)?.gross_tyiyn ?? 0) },
          ],
        }
      : {
          points: days.map((x) => ({
            label: x.date.slice(8, 10) + '.' + x.date.slice(5, 7),
            title: `${weekday(x.date)}, ${formatDate(x.date)}`,
            extra: [
              ['Выручка', x.revenue_tyiyn],
              ['Зарплата', x.payroll_tyiyn],
              ['Расходы', x.expenses_tyiyn],
            ],
          })),
          series: [
            { name: 'Валовая прибыль', color: 'sky', values: days.map((x) => x.gross_tyiyn) },
            { name: 'Чистая прибыль', color: 'emerald', values: days.map((x) => x.net_tyiyn) },
          ],
        }
  // Лучший и худший день периода — словами под графиком.
  const best = days.length > 1 ? days.reduce((a, b) => (b.net_tyiyn > a.net_tyiyn ? b : a)) : null
  const worst = days.length > 1 ? days.reduce((a, b) => (b.net_tyiyn < a.net_tyiyn ? b : a)) : null

  return (
    <div>
      <PageHeader
        title={`Сводка ${word}`}
        actions={
          <div className="grid grid-cols-3 overflow-hidden rounded-md border border-slate-300 text-sm">
            {PERIODS.map(([key, label]) => (
              <button
                key={key}
                type="button"
                aria-pressed={period === key}
                className={`min-h-[38px] px-3 ${period === key ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setPeriod(key)}
              >
                {label}
              </button>
            ))}
          </div>
        }
      />
      <div className="flex flex-col gap-4">
        <ErrorBox error={r.error} />
        <Card className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <Tile label="Продано" value={formatSom(revenue)} />
          <Tile label="Валовая прибыль" value={formatSom(t.gross_tyiyn)} />
          <Tile label="Чистая прибыль" value={formatSom(t.net_tyiyn)} tone={t.net_tyiyn < 0 ? 'text-rose-700' : 'text-emerald-700'} />
          <Tile label="Чеков" value={String(t.sales_count)} />
        </Card>

        <div className="grid gap-4 lg:grid-cols-2">
          <Card>
            <h2 className="mb-2 font-semibold">Откуда прибыль</h2>
            <div className="text-sm">
              <Step label="Товары" value={t.goods_tyiyn} />
              <Step label="Работы (услуги)" value={t.services_tyiyn} />
              <Step label="Выручка" value={revenue} strong />
              <Step label="Закупка проданного товара" value={t.cost_tyiyn} minus />
              <Step label="Валовая прибыль" value={t.gross_tyiyn} strong />
              <Step label="Зарплата сотрудникам" value={t.payroll_tyiyn} minus />
              {t.bank_fee_tyiyn !== 0 && <Step label="Комиссия банка (карта, QR)" value={t.bank_fee_tyiyn} minus />}
              {t.expenses_tyiyn !== 0 && <Step label="Расходы" value={t.expenses_tyiyn} minus />}
              {t.bonus_tyiyn !== 0 && <Step label="Скидки баллами" value={t.bonus_tyiyn} minus />}
              <Step label="Чистая прибыль" value={t.net_tyiyn} strong />
            </div>
            <div className="mt-2 text-xs text-slate-500">
              Средний чек {formatSom(d.data.average_check_tyiyn)} · маржа {percent(t.margin_bp)}
              {d.data.returns_tyiyn !== 0 && ` · возвраты ${formatSom(d.data.returns_tyiyn)}`}
            </div>
          </Card>

          <Card>
            <h2 className="mb-2 font-semibold">Кто сколько заработал {word}</h2>
            <StaffPayList staff={r.data?.staff ?? d.data.staff ?? []} />
            <div className="mt-2 flex justify-between border-t border-slate-200 pt-2 text-sm">
              <span className="text-slate-600">Всего к выплате (за все дни)</span>
              <Link to="/payroll" className="font-semibold text-sky-700 underline">
                {formatSom(d.data.to_pay_tyiyn)}
              </Link>
            </div>
          </Card>
        </div>

        <Card>
          <h2 className="mb-2 font-semibold">
            {period === 'today' ? 'Продажи сегодня по часам' : `Прибыль по дням ${word}`}
          </h2>
          <ProfitChart points={chart.points} series={chart.series} />
          {period !== 'today' && best && worst && best.date !== worst.date && (
            <div className="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-xs text-slate-600">
              <span>
                Лучший день: <b className="text-emerald-700">{formatDate(best.date)}</b> — чистая {formatSom(best.net_tyiyn)}
              </span>
              <span>
                Слабый день: <b className={worst.net_tyiyn < 0 ? 'text-rose-700' : 'text-slate-800'}>{formatDate(worst.date)}</b> — чистая {formatSom(worst.net_tyiyn)}
              </span>
            </div>
          )}
          {period === 'today' && (
            <div className="mt-1 text-xs text-slate-500">Чистая прибыль считается за день целиком (зарплата и расходы — за день), поэтому по часам — выручка и валовая.</div>
          )}
        </Card>

        <Card>
          <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
            <h2 className="font-semibold">Деньги сейчас</h2>
            <span className="text-sm text-slate-500">
              Смена: {d.data.shift_open ? `открыта, кассир ${d.data.shift_cashier}` : 'не открыта'} ·{' '}
              <Link to="/shift" className="text-sky-700 underline">
                подробно
              </Link>
            </span>
          </div>
          <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
            {d.data.accounts.map((a) => (
              <Tile key={a.name} label={a.name} value={formatSom(a.balance_tyiyn)} />
            ))}
            <Tile label="Всего денег" value={formatSom(d.data.money_total_tyiyn)} />
          </div>
          <div className="mt-3 grid grid-cols-2 gap-4 border-t border-slate-200 pt-3 sm:grid-cols-4">
            <Tile label="Нам должны" value={formatSom(d.data.debts_in_tyiyn)} tone="text-amber-700" />
            <Tile label="Мы должны поставщикам" value={formatSom(d.data.debts_out_tyiyn)} />
          </div>
        </Card>

        {(d.data.low_stock > 0 || d.data.needs_review > 0 || d.data.stale_stock > 0) && (
          <Card className="flex flex-wrap items-center gap-3 text-sm">
            <span className="font-medium">Склад:</span>
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
                ['− Скидки баллами', -t.bonus_tyiyn, ''],
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
            <Table head={['День', 'Выручка', 'Валовая', 'Оплата труда', 'Комиссия', 'Расходы', 'Баллы', 'Чистая']}>
              {(r.data?.days ?? []).map((d) => (
                <tr key={d.date}>
                  <td className="whitespace-nowrap px-2 py-2">{d.date}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.revenue_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.gross_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.payroll_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.fee_tyiyn ?? 0)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.expenses_tyiyn)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatSom(d.bonus_tyiyn ?? 0)}</td>
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
