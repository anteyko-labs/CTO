import { useCallback, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Money, Notice, PageHeader } from '../components/ui'
import { Icon } from '../components/icons'
import { get, qs } from '../lib/api'
import { formatDate, formatPoints, formatSom, hourBishkek, pointsClients, shiftDate, todayBishkek } from '../lib/format'
import { productsLink } from '../lib/stockLink'
import { useLoad, usePolling } from '../lib/hooks'
import { Heatmap } from '../components/Heatmap'
import { ProfitChart, type Point, type Series } from '../components/ProfitChart'
import type { Dashboard, HeatCell, ProfitReport, StaffPay } from '../lib/types'
import { VS_PREV_DAY, VS_PREV_PERIOD, earnedList, minusSom, payReason, signedSom, vsYesterday, weekAgo } from '../lib/dashboard'
import { QUICK, normalizeRange, quickOf, quickRange, rangeTitle, type Range } from '../lib/income'

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

const TONE = { up: 'text-emerald-700', down: 'text-rose-700', flat: 'text-slate-500' } as const

type Part = { revenue_tyiyn: number; gross_tyiyn: number; sales_count: number }

/**
 * Две короткие строки под плиткой. Один день: «▲ 40 % к вчера» (сегодня — к этому же часу,
 * прошедший день — к предыдущему целиком) и «нед. назад: 9 200 с». Период: «▲ 12 % к пред.
 * периоду». Короткие, чтобы на телефоне не переносились; полная подпись — в подсказке.
 */
function Compare({
  pick,
  data,
  now,
  money = true,
}: {
  pick: (x: Part) => number
  data: Dashboard
  /** Значение выбранного периода — для сравнения с предыдущим периодом. */
  now: number
  money?: boolean
}) {
  const fmt = money ? (v: number) => formatSom(v) : String
  const c = data.compare
  const p = data.previous
  const y = c
    ? vsYesterday(pick(c.today), pick(c.yesterday), fmt, c.until_now ? undefined : VS_PREV_DAY)
    : p
      ? vsYesterday(now, pick(p), fmt, VS_PREV_PERIOD)
      : null
  const w = c ? weekAgo(pick(c.week_ago), fmt) : null
  if (!y && !w) return null
  return (
    <div className="mt-1 flex flex-col gap-0.5 whitespace-nowrap text-xs">
      {y && (
        <span title={y.title} className={`font-medium tabular-nums ${TONE[y.tone]}`}>
          {y.text}
        </span>
      )}
      {w && (
        <span title={c?.until_now ? 'тот же день неделю назад к этому часу' : 'тот же день неделю назад'} className="tabular-nums text-slate-500">
          {w}
        </span>
      )}
    </div>
  )
}

/** Строка «лесенки» прибыли: подпись слева, сумма справа; вычеты — «−85 400 с». */
function Step({
  label,
  value,
  strong = false,
  minus = false,
  signed = false,
  sub = false,
}: {
  label: string
  value: number
  strong?: boolean
  minus?: boolean
  signed?: boolean
  /** Расшифровка строки выше: с отступом и мельче. */
  sub?: boolean
}) {
  const text = signed ? signedSom(value) : minus ? minusSom(value) : formatSom(value)
  const red = (strong || signed) && value < 0
  return (
    <div
      className={`flex items-baseline justify-between gap-4 ${sub ? 'py-0.5 pl-4 text-xs text-slate-500' : 'py-1'} ${strong ? 'border-t border-slate-200 font-semibold' : sub ? '' : 'text-slate-700'}`}
    >
      <span>{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${red ? 'text-rose-700' : ''}`}>{text}</span>
    </div>
  )
}

/** Кто сколько заработал: строка на человека, первые пять, остальные по кнопке. */
function EarnedList({ staff, empty }: { staff: StaffPay[]; empty: string }) {
  const [all, setAll] = useState(false)
  const list = earnedList(staff)
  if (list.length === 0) return <div className="py-2 text-sm text-slate-500">{empty}</div>
  const shown = all ? list : list.slice(0, 5)
  return (
    <>
      <ul className="divide-y divide-slate-100">
        {shown.map((p) => (
          <li key={p.employee_id} className="grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-3 py-1.5">
            <div className="min-w-0">
              <div className="truncate text-sm font-medium">{p.name}</div>
              <div className="truncate text-xs text-slate-500" title={payReason(p.items)}>
                {payReason(p.items)}
              </div>
            </div>
            <div className={`whitespace-nowrap text-right font-semibold tabular-nums ${p.total_tyiyn < 0 ? 'text-rose-700' : ''}`}>
              {p.total_tyiyn < 0 && <span className="mr-1 text-xs font-normal text-slate-500">удержание</span>}
              {formatSom(p.total_tyiyn)}
            </div>
          </li>
        ))}
      </ul>
      {list.length > 5 && (
        <button type="button" className="mt-1 min-h-[42px] text-sm text-sky-700 underline" onClick={() => setAll(!all)}>
          {all ? 'Свернуть' : `Все (${list.length})`}
        </button>
      )}
    </>
  )
}

/**
 * Сводка владельца (SPEC-08): сегодня, 7 или 30 дней, любой день или период (SPEC-27 §6).
 * Все блоки — за выбранный период; остатки касс, долги и склад — на сейчас.
 */
export function OwnerDashboard() {
  const today = todayBishkek()
  const [range, setRange] = useState<Range>(() => quickRange('today', today))
  const [custom, setCustom] = useState(false)
  const { from, to } = range
  const single = from === to
  const isToday = single && to === today
  const quick = quickOf(range, today)
  const d = useLoad(() => get<Dashboard>(`/owner/dashboard${qs({ from, to })}`), [from, to])
  const r = useLoad(() => get<ProfitReport>(`/reports/profit${qs({ from, to })}`), [from, to])
  // Один день — карта за последние 4 недели (по одному дню она пуста), период — за период.
  const heat = useLoad(() => get<HeatCell[]>(`/reports/heatmap${single ? '?days=28' : qs({ from, to })}`), [from, to, single])
  // Сводка живая, пока в период входит сегодня: чеки с кассы появляются без перезагрузки.
  const live = to === today
  const { reload: reloadDash } = d
  const { reload: reloadReport } = r
  const reloadLive = useCallback(() => {
    if (!live) return
    reloadDash()
    reloadReport()
  }, [live, reloadDash, reloadReport])
  usePolling(reloadLive)
  const pick = (next: Range) => setRange(normalizeRange(next.from, next.to))
  const word = rangeTitle(range, today)

  const picker = (
    <div className="flex flex-wrap items-center gap-2">
      <div className="grid grid-cols-3 overflow-hidden rounded-md border border-slate-300 text-sm">
        {QUICK.map(([key, label]) => (
          <button
            key={key}
            type="button"
            aria-pressed={quick === key}
            className={`min-h-[38px] px-3 ${quick === key ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
            onClick={() => {
              setCustom(false)
              setRange(quickRange(key, today))
            }}
          >
            {label}
          </button>
        ))}
      </div>
      <input
        type="date"
        aria-label="День сводки"
        className="w-40"
        max={today}
        value={single ? to : ''}
        onChange={(e) => {
          if (!e.target.value) return
          setCustom(false)
          pick({ from: e.target.value, to: e.target.value })
        }}
      />
      <button
        type="button"
        aria-pressed={custom || (!single && quick === null)}
        className={`min-h-[38px] rounded-md border px-3 text-sm ${custom || (!single && quick === null) ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
        onClick={() => setCustom(!custom)}
      >
        Период
      </button>
    </div>
  )

  if (d.loading && !d.data) return <Loading />
  if (!d.data) return <ErrorBox error={d.error} />
  const data = d.data
  const t = data.totals
  const revenue = t.goods_tyiyn + t.services_tyiyn
  const hours = r.data?.hours ?? []
  // Один день по часам: с открытия (8:00) до текущего часа (сегодня) или до закрытия (прошедший
  // день), без пропусков — линия идёт вверх и вниз честно.
  const nowHour = hourBishkek()
  const firstHour = Math.min(8, ...hours.map((h) => h.hour))
  const lastHour = Math.max(isToday ? (Number.isFinite(nowHour) ? nowHour : 0) : 20, ...hours.map((h) => h.hour))
  const hourRange = Array.from({ length: Math.max(lastHour - firstHour + 1, 1) }, (_, k) => firstHour + k)
  const byHour = new Map(hours.map((h) => [h.hour, h]))
  const days = [...(r.data?.days ?? [])].reverse()
  const weekday = (x: string) => ['Вс', 'Пн', 'Вт', 'Ср', 'Чт', 'Пт', 'Сб'][new Date(`${x}T12:00:00`).getDay()]
  const chart: { points: Point[]; series: Series[] } = single
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
  const prevDay = data.compare && !data.compare.until_now ? 'накануне' : 'вчера'

  return (
    <div>
      <PageHeader title={`Сводка ${word}`} actions={picker} />
      <div className="flex flex-col gap-4">
        {(custom || (!single && quick === null)) && (
          <Card className="flex flex-wrap items-end gap-3">
            <Field label="С">
              <input
                type="date"
                aria-label="Период с"
                className="w-40"
                value={from}
                max={today}
                onChange={(e) => e.target.value && pick({ from: e.target.value, to })}
              />
            </Field>
            <Field label="По">
              <input
                type="date"
                aria-label="Период по"
                className="w-40"
                value={to}
                max={today}
                onChange={(e) => e.target.value && pick({ from, to: e.target.value })}
              />
            </Field>
            <div className="pb-2 text-xs text-slate-500">Не больше года; сравнение — с таким же периодом перед ним.</div>
          </Card>
        )}
        <ErrorBox error={r.error ?? d.error} />
        <Card className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <Tile label="Продано" value={formatSom(revenue)}>
            <Compare data={data} now={revenue} pick={(x) => x.revenue_tyiyn} />
          </Tile>
          <Tile label="Валовая прибыль" value={formatSom(t.gross_tyiyn)}>
            <Compare data={data} now={t.gross_tyiyn} pick={(x) => x.gross_tyiyn} />
          </Tile>
          <Tile label="Чистая прибыль" value={formatSom(t.net_tyiyn)} tone={t.net_tyiyn < 0 ? 'text-rose-700' : 'text-emerald-700'}>
            {/* Зарплата и расходы считаются за день целиком: «к этому часу» для чистой не честно — только предыдущий день целиком. */}
            {data.compare && (
              <div className="mt-1 whitespace-nowrap text-xs tabular-nums text-slate-500" title="чистая прибыль предыдущего дня за весь день">
                {prevDay} за день:{' '}
                <span className={data.compare.yesterday_net_tyiyn < 0 ? 'text-rose-700' : ''}>{formatSom(data.compare.yesterday_net_tyiyn)}</span>
              </div>
            )}
            {data.previous && (
              <div className="mt-1 whitespace-nowrap text-xs tabular-nums text-slate-500" title="чистая прибыль предыдущего периода той же длины">
                пред. период: <span className={data.previous.net_tyiyn < 0 ? 'text-rose-700' : ''}>{formatSom(data.previous.net_tyiyn)}</span>
              </div>
            )}
          </Tile>
          <Tile label="Чеков" value={String(t.sales_count)}>
            <Compare data={data} now={t.sales_count} pick={(x) => x.sales_count} money={false} />
          </Tile>
        </Card>

        <Card>
          <div className="mb-3 flex flex-wrap items-baseline justify-between gap-2">
            <h2 className="font-semibold">Деньги сейчас</h2>
            <span className="text-sm text-slate-500">
              Смена: {data.shift_open ? `открыта, кассир ${data.shift_cashier}` : 'не открыта'} ·{' '}
              <Link to="/shift" className="text-sky-700 underline">
                подробно
              </Link>
            </span>
          </div>
          <div className="text-xs text-slate-500">Всего денег</div>
          <div className="text-3xl font-bold tabular-nums">{formatSom(data.money_total_tyiyn)}</div>
          <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-sm text-slate-600">
            {data.accounts.map((a) => (
              <span key={a.name}>
                {a.name} <Money value={a.balance_tyiyn} tone="negative" className="font-medium text-slate-800" />
              </span>
            ))}
          </div>
          <div className="mt-3 grid grid-cols-2 gap-4 border-t border-slate-200 pt-3 sm:grid-cols-3 xl:grid-cols-5">
            <Link to="/bank" className="rounded-md hover:bg-slate-50" data-testid="noncash">
              <Tile label={`Безнал ${word}`} value={formatSom(data.noncash.total_tyiyn)}>
                <div className="mt-1 whitespace-nowrap text-xs tabular-nums text-slate-500">
                  карта / QR банка {formatSom(data.noncash.card_tyiyn)} · QR владельцу {formatSom(data.noncash.transfer_tyiyn)}
                  {(data.noncash.invoice_tyiyn ?? 0) !== 0 && ` · по счёту ${formatSom(data.noncash.invoice_tyiyn ?? 0)}`}
                </div>
                {(data.noncash.by_account ?? []).length > 1 && (
                  <div className="text-xs tabular-nums text-slate-500" data-testid="noncash-banks">
                    {(data.noncash.by_account ?? []).map((b) => `${b.name} ${formatSom(b.total_tyiyn)}`).join(' · ')}
                  </div>
                )}
              </Tile>
            </Link>
            <Link to="/debts" className="rounded-md hover:bg-slate-50">
              <Tile label="Нам должны" value={formatSom(data.debts_in_tyiyn)} tone="text-amber-700" />
            </Link>
            <Link to="/debts" className="rounded-md hover:bg-slate-50">
              <Tile label="Мы должны поставщикам" value={formatSom(data.debts_out_tyiyn)} />
            </Link>
            {data.points && (
              // Баллы — обязательство точки: клиенты спишут их покупками (SPEC-30 §12).
              <Link to="/clients" className="rounded-md hover:bg-slate-50" data-testid="points-total">
                <Tile label="Баллы у клиентов" value={formatPoints(data.points.points_tyiyn)} tone="text-sky-700">
                  <div className="mt-1 whitespace-nowrap text-xs tabular-nums text-slate-500">
                    на {formatSom(data.points.points_tyiyn)} · {pointsClients(data.points.clients)}
                  </div>
                </Tile>
              </Link>
            )}
            <Link to={`/income${qs({ from, to })}`} className="flex items-center rounded-md text-sm text-sky-700 hover:bg-slate-50">
              Куда сколько поступило →
            </Link>
          </div>
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
              {(t.salary_tyiyn ?? 0) !== 0 && <Step label="в т.ч. оклады за месяц" value={t.salary_tyiyn ?? 0} minus sub />}
              {t.bank_fee_tyiyn !== 0 && <Step label="Комиссия банка (карта, QR)" value={t.bank_fee_tyiyn} minus />}
              {t.expenses_tyiyn !== 0 && <Step label="Расходы" value={t.expenses_tyiyn} minus />}
              {t.bonus_tyiyn !== 0 && <Step label="Скидки баллами" value={t.bonus_tyiyn} minus />}
              {(t.cash_diff_tyiyn ?? 0) !== 0 && <Step label="Недостача / излишек кассы" value={t.cash_diff_tyiyn ?? 0} signed />}
              {(t.defect_writeoff_tyiyn ?? 0) !== 0 && <Step label="Списание брака" value={t.defect_writeoff_tyiyn ?? 0} minus />}
              {(t.defect_diff_tyiyn ?? 0) !== 0 && <Step label="Брак: разница возмещения" value={t.defect_diff_tyiyn ?? 0} signed />}
              {(t.return_diff_tyiyn ?? 0) !== 0 && <Step label="Возврат поставщику: разница" value={t.return_diff_tyiyn ?? 0} signed />}
              <Step label="Чистая прибыль" value={t.net_tyiyn} strong />
            </div>
            <div className="mt-2 text-xs text-slate-500">
              Средний чек {formatSom(data.average_check_tyiyn)} · маржа {percent(t.margin_bp)}
              {data.returns_tyiyn !== 0 && ` · возвраты ${formatSom(data.returns_tyiyn)}`}
            </div>
          </Card>

          <Card>
            <h2 className="mb-2 font-semibold">Кто сколько заработал {word}</h2>
            <EarnedList key={`${from}:${to}`} staff={data.staff} empty={isToday ? 'Сегодня начислений нет' : 'За период начислений нет'} />
            <Link to="/payroll" className="mt-2 flex min-h-[42px] items-center justify-between gap-3 border-t border-slate-200 pt-2 text-sm hover:text-sky-700">
              <span className="text-slate-600">К выплате за все дни:</span>
              <span className="whitespace-nowrap font-semibold tabular-nums text-sky-700">{formatSom(data.to_pay_tyiyn)} →</span>
            </Link>
          </Card>
        </div>

        <Card>
          <h2 className="mb-2 font-semibold">{single ? `Продажи ${word} по часам` : `Прибыль по дням ${word}`}</h2>
          <ProfitChart points={chart.points} series={chart.series} />
          {!single && best && worst && best.date !== worst.date && (
            <div className="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-xs text-slate-600">
              <span>
                Лучший день: <b className="text-emerald-700">{formatDate(best.date)}</b> — чистая {formatSom(best.net_tyiyn)}
              </span>
              <span>
                Слабый день: <b className={worst.net_tyiyn < 0 ? 'text-rose-700' : 'text-slate-800'}>{formatDate(worst.date)}</b> — чистая {formatSom(worst.net_tyiyn)}
              </span>
            </div>
          )}
          {single && (
            <div className="mt-1 text-xs text-slate-500">Чистая прибыль считается за день целиком (зарплата и расходы — за день), поэтому по часам — выручка и валовая.</div>
          )}
        </Card>

        {/* Карта — подсказка, а не главное: если не загрузилась, сводка работает без неё. */}
        {!heat.error && (
          <Card>
            <h2 className="mb-1 font-semibold">Когда покупают</h2>
            <div className="mb-3 text-xs text-slate-500">
              {single ? 'Выручка по дням недели и часам за последние 4 недели' : `Выручка по дням недели и часам ${word}`}
            </div>
            {heat.data ? <Heatmap cells={heat.data} /> : <Loading />}
          </Card>
        )}

        {(data.low_stock > 0 || data.needs_review > 0 || data.stale_stock > 0) && (
          <Card>
            <h2 className="mb-1 font-semibold">Склад сейчас</h2>
            <div className="flex flex-col divide-y divide-slate-100 text-sm">
              {(
                [
                  [data.needs_review, 'Проверить остаток', 'bg-rose-500', 'review'],
                  [data.low_stock, 'Заканчивается', 'bg-amber-500', 'low'],
                  [data.stale_stock, 'Залежался больше 60 дней', 'bg-slate-400', 'stale'],
                ] as const
              )
                .filter(([n]) => n > 0)
                .map(([n, label, dot, chip]) => (
                  // Сразу на нужный отбор «Товаров» (SPEC-30 §11).
                  <Link key={label} to={productsLink(chip)} className="flex items-center gap-3 py-2 hover:text-sky-700">
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

/** Сумма со знаком действия для отчёта «Прибыль»: «− 1 000 с» уменьшает прибыль, «+ 300 с» добавляет. */
function reportSigned(value: number): string {
  if (value === 0) return formatSom(0)
  return `${value < 0 ? '−' : '+'} ${formatSom(Math.abs(value))}`
}

/** Строка расчёта прибыли: вычеты со знаком «−», итог красным при убытке. */
/** Брак и возвраты за день одним числом: разница возмещения минус списание (SPEC-25) и разница возвратов поставщику (SPEC-29 §9). */
const defectNet = (x: { defect_writeoff_tyiyn?: number; defect_diff_tyiyn?: number; return_diff_tyiyn?: number }): number =>
  (x.defect_diff_tyiyn ?? 0) - (x.defect_writeoff_tyiyn ?? 0) + (x.return_diff_tyiyn ?? 0)

function CalcRow({
  label,
  value,
  minus = false,
  signed = false,
  total = false,
  big = false,
  hint,
}: {
  label: string
  value: number
  minus?: boolean
  signed?: boolean
  total?: boolean
  big?: boolean
  hint?: string
}) {
  return (
    <div
      className={`flex items-baseline justify-between gap-4 py-1 ${total ? 'border-t border-slate-200 font-semibold' : 'text-slate-700'} ${big ? 'text-lg font-bold' : ''}`}
    >
      <span>
        {label}
        {hint && <span className="block text-xs font-normal text-slate-500">{hint}</span>}
      </span>
      {signed ? (
        <span className={`whitespace-nowrap tabular-nums ${value < 0 ? 'text-rose-700' : ''}`}>{reportSigned(value)}</span>
      ) : minus ? (
        // Вычитаемое меньше нуля (удержанная недостача в оплате труда, переоценка вверх) — это
        // прибавка: «+ 10 605,20 с», а не «− −10 605,20 с» (проверка 20.10.2026).
        <span className="whitespace-nowrap tabular-nums">{value === 0 ? formatSom(0) : value < 0 ? `+ ${formatSom(-value)}` : `− ${formatSom(value)}`}</span>
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
              {/* Себестоимость проданного — как в категориях и на графике; переоценка остатков
                  (ревизия, сторно закупок) — отдельной строкой (проверка 20.10.2026). */}
              <CalcRow label="Себестоимость проданного" value={t.cost_tyiyn - (t.revaluation_tyiyn ?? 0)} minus />
              {(t.revaluation_tyiyn ?? 0) !== 0 && <CalcRow label="Переоценка остатков" value={t.revaluation_tyiyn ?? 0} minus />}
              <CalcRow label="Валовая прибыль" value={t.gross_tyiyn} total />
              <CalcRow label="Оплата труда" value={t.payroll_tyiyn} minus />
              <CalcRow label="Комиссия банка" value={t.bank_fee_tyiyn} minus />
              <CalcRow label="Расходы" value={t.expenses_tyiyn} minus />
              <CalcRow label="Скидки баллами" value={t.bonus_tyiyn} minus />
              <CalcRow
                label="Недостача / излишек кассы"
                value={t.cash_diff_tyiyn ?? 0}
                signed
                hint="Пересчёт смен; удержание недостачи с кассира уже уменьшило оплату труда"
              />
              <CalcRow label="Списание брака" value={t.defect_writeoff_tyiyn ?? 0} minus />
              <CalcRow
                label="Брак: разница возмещения"
                value={t.defect_diff_tyiyn ?? 0}
                signed
                hint="Возмещение поставщика минус стоимость брака: минус — убыток, плюс — доход"
              />
              <CalcRow
                label="Возврат поставщику: разница"
                value={t.return_diff_tyiyn ?? 0}
                signed
                hint="Сумма возврата минус себестоимость возвращённого товара: минус — убыток, плюс — доход"
              />
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
                  { label: 'Касса ±', right: true },
                  { label: 'Брак, возвраты', right: true },
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
                    <Money key="cash" value={x.cash_diff_tyiyn ?? 0} tone="negative" zero="—" />,
                    <Money key="defect" value={defectNet(x)} tone="negative" zero="—" />,
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
                        <Money key="cash" value={sum((x) => x.cash_diff_tyiyn ?? 0)} tone="negative" zero="—" />,
                        <Money key="defect" value={sum(defectNet)} tone="negative" zero="—" />,
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
