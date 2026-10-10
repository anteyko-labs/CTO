import { useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Money, Notice, PageHeader } from '../components/ui'
import { Icon } from '../components/icons'
import { get, qs } from '../lib/api'
import { formatDate, formatSom, hourBishkek, shiftDate, todayBishkek } from '../lib/format'
import { useLoad, usePolling } from '../lib/hooks'
import { Heatmap } from '../components/Heatmap'
import { StaffPayList } from '../components/StaffPayList'
import { ProfitChart, type Point, type Series } from '../components/ProfitChart'
import type { Dashboard, HeatCell, ProfitReport } from '../lib/types'

const percent = (bp: number | null): string => (bp === null ? '—' : `${(bp / 100).toFixed(1).replace('.', ',')} %`)

function Tile({ label, value, tone = '', children }: { label: string; value: string; tone?: string; children?: ReactNode }) {
  return (
    <div>
      <div className="text-xs text-slate-500">{label}</div>
      <div className={`text-2xl font-bold tabular-nums ${tone}`}>{value}</div>
      {children}
    </div>
  )
}

/** Сравнение с прошлым днём к этому же часу: «▲ +12 % к вчера на этот час». */
function Delta({ now, base, label, then, money }: { now: number; base: number; label: string; then: string; money: boolean }) {
  // Не с чем сравнить (в тот день к этому часу продаж не было) — строку не показываем.
  if (base === 0) return null
  const pct = Math.round(((now - base) * 100) / base)
  const tone = pct > 0 ? 'text-emerald-700' : pct < 0 ? 'text-rose-700' : 'text-slate-500'
  // «+8113 %» ничего не говорит: при большой разнице показываем, сколько было тогда.
  if (Math.abs(pct) > 200) {
    return (
      <div className="text-xs text-slate-500">
        <span className={`font-medium ${tone}`}>{pct > 0 ? '▲' : '▼'}</span> {then}: {money ? formatSom(base) : base}
      </div>
    )
  }
  return (
    <div className="text-xs">
      <span className={`font-medium tabular-nums ${tone}`}>
        {pct > 0 ? '▲ +' : pct < 0 ? '▼ −' : '= '}
        {Math.abs(pct)} %
      </span>{' '}
      <span className="text-slate-500">{label}</span>
    </div>
  )
}

/** Обе строки сравнения под плиткой: со вчера и с тем же днём неделю назад. */
function Compare({
  pick,
  data,
  money = true,
}: {
  pick: (x: { revenue_tyiyn: number; gross_tyiyn: number; sales_count: number }) => number
  data: Dashboard['compare']
  money?: boolean
}) {
  if (!data) return null
  return (
    <div className="mt-1 flex flex-col gap-0.5">
      <Delta now={pick(data.today)} base={pick(data.yesterday)} label="к вчера на этот час" then="вчера к этому часу" money={money} />
      <Delta now={pick(data.today)} base={pick(data.week_ago)} label="к неделе назад" then="неделю назад" money={money} />
    </div>
  )
}

/** Строка «лесенки» прибыли: подпись слева, сумма справа. */
function Step({ label, value, strong = false, minus = false }: { label: string; value: number; strong?: boolean; minus?: boolean }) {
  return (
    <div className={`flex items-baseline justify-between gap-4 py-1 ${strong ? 'border-t border-slate-200 font-semibold' : 'text-slate-700'}`}>
      <span>{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${strong && value < 0 ? 'text-rose-700' : ''}`}>
        {minus && value !== 0 ? `− ${formatSom(value)}` : formatSom(value)}
      </span>
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
  const heat = useLoad(() => get<HeatCell[]>('/reports/heatmap?days=28'), [])
  // Сводка живая: чеки с кассы появляются без перезагрузки страницы.
  usePolling(d.reload)
  usePolling(r.reload)
  if (d.loading && !d.data) return <Loading />
  if (!d.data) return <ErrorBox error={d.error} />
  const t = r.data?.totals ?? d.data.totals
  const revenue = t.goods_tyiyn + t.services_tyiyn
  // Сравнение «к этому часу» имеет смысл только для сегодняшнего дня.
  const compare = period === 'today' ? d.data.compare : undefined
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
          <Tile label="Продано" value={formatSom(revenue)}>
            <Compare data={compare} pick={(x) => x.revenue_tyiyn} />
          </Tile>
          <Tile label="Валовая прибыль" value={formatSom(t.gross_tyiyn)}>
            <Compare data={compare} pick={(x) => x.gross_tyiyn} />
          </Tile>
          <Tile label="Чистая прибыль" value={formatSom(t.net_tyiyn)} tone={t.net_tyiyn < 0 ? 'text-rose-700' : 'text-emerald-700'} />
          <Tile label="Чеков" value={String(t.sales_count)}>
            <Compare data={compare} pick={(x) => x.sales_count} money={false} />
          </Tile>
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

        {/* Карта — подсказка, а не главное: если не загрузилась, сводка работает без неё. */}
        {!heat.error && (
          <Card>
            <h2 className="mb-1 font-semibold">Когда покупают</h2>
            <div className="mb-3 text-xs text-slate-500">Выручка по дням недели и часам за последние 4 недели</div>
            {heat.data ? <Heatmap cells={heat.data} /> : <Loading />}
          </Card>
        )}

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
          <div className="text-xs text-slate-500">Всего денег</div>
          <div className="text-3xl font-bold tabular-nums">{formatSom(d.data.money_total_tyiyn)}</div>
          <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-sm text-slate-600">
            {d.data.accounts.map((a) => (
              <span key={a.name}>
                {a.name} <Money value={a.balance_tyiyn} tone="negative" className="font-medium text-slate-800" />
              </span>
            ))}
          </div>
          <div className="mt-3 grid grid-cols-2 gap-4 border-t border-slate-200 pt-3 sm:grid-cols-4">
            <Link to="/debts" className="rounded-md hover:bg-slate-50">
              <Tile label="Нам должны" value={formatSom(d.data.debts_in_tyiyn)} tone="text-amber-700" />
            </Link>
            <Link to="/debts" className="rounded-md hover:bg-slate-50">
              <Tile label="Мы должны поставщикам" value={formatSom(d.data.debts_out_tyiyn)} />
            </Link>
          </div>
        </Card>

        {(d.data.low_stock > 0 || d.data.needs_review > 0 || d.data.stale_stock > 0) && (
          <Card>
            <h2 className="mb-1 font-semibold">Склад</h2>
            <div className="flex flex-col divide-y divide-slate-100 text-sm">
              {(
                [
                  [d.data.needs_review, 'Проверить остаток', 'bg-rose-500'],
                  [d.data.low_stock, 'Заканчивается', 'bg-amber-500'],
                  [d.data.stale_stock, 'Залежался больше 60 дней', 'bg-slate-400'],
                ] as const
              )
                .filter(([n]) => n > 0)
                .map(([n, label, dot]) => (
                  <Link key={label} to="/stock" className="flex items-center gap-3 py-2 hover:text-sky-700">
                    <span className={`h-2.5 w-2.5 shrink-0 rounded-full ${dot}`} />
                    <span className="flex-1">{label}</span>
                    <span className="font-semibold tabular-nums">{n}</span>
                    <Icon name="chevron" className="h-4 w-4 text-slate-400" />
                  </Link>
                ))}
            </div>
          </Card>
        )}
      </div>
    </div>
  )
}

interface Col {
  label: string
  right?: boolean
}

/**
 * Таблица отчёта: суммы по правому краю ровными цифрами. Своя разметка, а не общий Table,
 * чтобы выровнять и заголовки; на телефоне — те же карточки (класс responsive, data-label).
 */
function ReportTable({ cols, rows, foot }: { cols: Col[]; rows: { key: string; cells: ReactNode[] }[]; foot?: ReactNode[] }) {
  const align = (c: Col) => (c.right ? 'text-right tabular-nums whitespace-nowrap' : '')
  return (
    <div className="overflow-x-auto">
      <table className="responsive w-full text-sm">
        <thead>
          <tr className="border-b border-slate-200 text-xs uppercase text-slate-500">
            {cols.map((c) => (
              <th key={c.label} className={`px-2 py-2 font-medium ${c.right ? 'text-right' : 'text-left'}`}>
                {c.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-100">
          {rows.map((r) => (
            <tr key={r.key} className="break-inside-avoid">
              {r.cells.map((cell, i) => (
                <td key={cols[i].label} data-label={cols[i].label} className={`px-2 py-2 ${align(cols[i])} ${i === 0 ? 'font-medium' : ''}`}>
                  {cell}
                </td>
              ))}
            </tr>
          ))}
          {foot && (
            <tr className="border-t-2 border-slate-200 font-semibold">
              {foot.map((cell, i) => (
                <td key={cols[i].label} data-label={cols[i].label} className={`px-2 py-2 ${align(cols[i])}`}>
                  {cell}
                </td>
              ))}
            </tr>
          )}
        </tbody>
      </table>
    </div>
  )
}

/** Строка расчёта прибыли: вычеты со знаком «−», итог красным при убытке. */
function CalcRow({ label, value, minus = false, total = false, big = false }: { label: string; value: number; minus?: boolean; total?: boolean; big?: boolean }) {
  return (
    <div
      className={`flex items-baseline justify-between gap-4 py-1 ${total ? 'border-t border-slate-200 font-semibold' : 'text-slate-700'} ${big ? 'text-lg font-bold' : ''}`}
    >
      <span>{label}</span>
      {minus ? (
        <span className="whitespace-nowrap tabular-nums">{value === 0 ? formatSom(0) : `− ${formatSom(value)}`}</span>
      ) : (
        <Money value={value} tone="negative" />
      )}
    </div>
  )
}

/** Прибыль за период с разрезами (SPEC-08): отчёт, который можно распечатать. */
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
  const days = r.data?.days ?? []
  const categories = r.data?.categories ?? []
  const articles = r.data?.articles ?? []
  const sum = (pick: (x: (typeof days)[number]) => number) => days.reduce((a, x) => a + pick(x), 0)
  const money = (v: number) => <Money value={v} tone="negative" />

  return (
    <div>
      <PageHeader
        title="Прибыль"
        subtitle={from === to ? `Отчёт за ${formatDate(from)}` : `Отчёт за период ${formatDate(from)} — ${formatDate(to)}`}
        actions={
          <Button variant="secondary" className="no-print" disabled={!t} onClick={() => window.print()}>
            Печать
          </Button>
        }
      />
      <Card className="no-print mb-4 flex flex-wrap items-end gap-3">
        <Field label="С">
          <input type="date" className="w-40" value={from} max={to} onChange={(e) => e.target.value && setFrom(e.target.value)} />
        </Field>
        <Field label="По">
          <input type="date" className="w-40" value={to} min={from} onChange={(e) => e.target.value && setTo(e.target.value)} />
        </Field>
        <div className="flex flex-wrap gap-2">
          {(
            [
              [0, 'Сегодня'],
              [6, '7 дней'],
              [29, 'Месяц'],
            ] as const
          ).map(([n, label]) => {
            const on = to === today && from === shiftDate(today, -n)
            return (
              <button
                key={label}
                type="button"
                aria-pressed={on}
                className={`min-h-[38px] rounded-md border px-3 py-2 text-sm ${on ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 hover:bg-slate-50'}`}
                onClick={() => quick(n)}
              >
                {label}
              </button>
            )
          })}
        </div>
      </Card>

      <ErrorBox error={r.error} />
      {r.loading && !r.data ? (
        <Loading />
      ) : t ? (
        <div className="flex flex-col gap-4">
          <Card className="break-inside-avoid">
            <CardTitle>Как получилась прибыль</CardTitle>
            <div className="text-sm">
              <CalcRow label="Выручка за товары" value={t.goods_tyiyn} />
              <CalcRow label="Выручка за работы" value={t.services_tyiyn} />
              <CalcRow label="Себестоимость проданного" value={t.cost_tyiyn} minus />
              <CalcRow label="Валовая прибыль" value={t.gross_tyiyn} total />
              <CalcRow label="Оплата труда" value={t.payroll_tyiyn} minus />
              <CalcRow label="Комиссия банка" value={t.bank_fee_tyiyn} minus />
              <CalcRow label="Расходы" value={t.expenses_tyiyn} minus />
              <CalcRow label="Скидки баллами" value={t.bonus_tyiyn} minus />
              <CalcRow label="Чистая прибыль" value={t.net_tyiyn} total big />
              <div className="mt-2 text-xs text-slate-500">
                Чеков {t.sales_count}, маржа {percent(t.margin_bp)}
              </div>
              {r.data && r.data.warnings.length > 0 && (
                <div className="mt-3">
                  <Notice tone="amber">
                    {r.data.warnings.map((w) => (
                      <div key={w}>{w}</div>
                    ))}
                  </Notice>
                </div>
              )}
            </div>
          </Card>

          <Card>
            <CardTitle>По категориям</CardTitle>
            {categories.length === 0 ? (
              <Empty>Продаж за период нет</Empty>
            ) : (
              <ReportTable
                cols={[{ label: 'Категория' }, { label: 'Выручка', right: true }, { label: 'Себестоимость', right: true }, { label: 'Валовая', right: true }]}
                rows={categories.map((c) => ({
                  key: c.name,
                  cells: [c.name, money(c.revenue_tyiyn), money(c.cost_tyiyn), money(c.gross_tyiyn)],
                }))}
                foot={
                  categories.length > 1
                    ? [
                        'Итого',
                        money(categories.reduce((a, c) => a + c.revenue_tyiyn, 0)),
                        money(categories.reduce((a, c) => a + c.cost_tyiyn, 0)),
                        money(categories.reduce((a, c) => a + c.gross_tyiyn, 0)),
                      ]
                    : undefined
                }
              />
            )}
          </Card>

          <Card>
            <CardTitle>По дням</CardTitle>
            {days.length === 0 ? (
              <Empty>За период нет ни продаж, ни расходов</Empty>
            ) : (
              <ReportTable
                cols={[
                  { label: 'День' },
                  { label: 'Выручка', right: true },
                  { label: 'Валовая', right: true },
                  { label: 'Оплата труда', right: true },
                  { label: 'Комиссия', right: true },
                  { label: 'Расходы', right: true },
                  { label: 'Баллы', right: true },
                  { label: 'Чистая', right: true },
                ]}
                rows={days.map((x) => ({
                  key: x.date,
                  cells: [
                    formatDate(x.date),
                    money(x.revenue_tyiyn),
                    money(x.gross_tyiyn),
                    money(x.payroll_tyiyn),
                    money(x.fee_tyiyn ?? 0),
                    money(x.expenses_tyiyn),
                    money(x.bonus_tyiyn ?? 0),
                    <Money key="net" value={x.net_tyiyn} tone="negative" className="font-semibold" />,
                  ],
                }))}
                foot={
                  days.length > 1
                    ? [
                        'Итого',
                        money(sum((x) => x.revenue_tyiyn)),
                        money(sum((x) => x.gross_tyiyn)),
                        money(sum((x) => x.payroll_tyiyn)),
                        money(sum((x) => x.fee_tyiyn ?? 0)),
                        money(sum((x) => x.expenses_tyiyn)),
                        money(sum((x) => x.bonus_tyiyn ?? 0)),
                        money(sum((x) => x.net_tyiyn)),
                      ]
                    : undefined
                }
              />
            )}
          </Card>

          {articles.length > 0 && (
            <Card>
              <CardTitle>Расходы по статьям</CardTitle>
              <ReportTable
                cols={[{ label: 'Статья' }, { label: 'Сумма', right: true }]}
                rows={articles.map((a) => ({ key: a.name, cells: [a.name, money(a.amount_tyiyn)] }))}
                foot={articles.length > 1 ? ['Итого', money(articles.reduce((a, x) => a + x.amount_tyiyn, 0))] : undefined}
              />
            </Card>
          )}
        </div>
      ) : null}
    </div>
  )
}
