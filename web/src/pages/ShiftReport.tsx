// Отчёт смены, как в Умаге (SPEC-30 §7, §8): обзор (движение денег кассы, выручка по способам,
// валовая — только владельцу, сравнение с прошлой сменой, продажи по часам), продажи, возвраты,
// закупки, денежные операции, долги, выплаты и отменённые товары. Все суммы считает сервер.
import { useState, type ReactNode } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Loading, Money, PageHeader, Table } from '../components/ui'
import { get } from '../lib/api'
import { formatDateTime, formatKg, formatLiters, formatSom, formatSomShort, formatTime, plural } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { ShiftBank } from '../components/ShiftBank'
import { printShift } from '../components/shiftPrint'
import { PURPOSE_LABELS } from '../lib/payouts'
import {
  CANCEL_REASONS,
  TABS,
  accountsText,
  barHeights,
  changeText,
  debtKindText,
  debtTotals,
  filterSales,
  methodName,
  methodsIn,
  paymentsText,
  tabOf,
  type CancelledRow,
  type HourRow,
  type ReportLine,
  type SaleRow,
  type ShiftReport as Report,
  type TabKey,
} from '../lib/shiftReport'

/** Плитка итога: подпись мелко, сумма крупно. */
function Tile({ label, value, tone, hint, testId }: { label: string; value: ReactNode; tone?: 'green' | 'rose' | 'sky'; hint?: ReactNode; testId?: string }) {
  const color = tone === 'green' ? 'text-emerald-700' : tone === 'rose' ? 'text-rose-700' : tone === 'sky' ? 'text-sky-700' : 'text-slate-900'
  return (
    <div className="min-w-0 rounded-lg border border-slate-200 bg-white px-4 py-3 shadow-sm" data-testid={testId}>
      <div className="truncate text-xs text-slate-500">{label}</div>
      <div className={`mt-0.5 text-xl font-semibold tabular-nums sm:text-2xl ${color}`}>{value}</div>
      {hint && <div className="mt-0.5 text-xs text-slate-500">{hint}</div>}
    </div>
  )
}

/** Строка отчёта: подпись и сумма; `sub` — расшифровка (с отступом), `strong` — итог. */
function Row({ label, value, sign, sub, strong, tone, testId }: { label: ReactNode; value: number; sign?: '+' | '−'; sub?: boolean; strong?: boolean; tone?: 'green' | 'rose'; testId?: string }) {
  const color = tone === 'green' ? 'text-emerald-700' : tone === 'rose' ? 'text-rose-700' : ''
  return (
    <div
      data-testid={testId}
      className={`flex items-baseline justify-between gap-3 py-1 ${sub ? 'pl-4 text-sm text-slate-600' : ''} ${strong ? 'mt-1 border-t border-slate-200 pt-2 font-semibold' : ''}`}
    >
      <span className="min-w-0">{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${color} ${sub && value === 0 ? 'text-slate-400' : ''}`}>
        {sign && value !== 0 ? `${sign} ` : ''}
        {formatSom(value)}
      </span>
    </div>
  )
}

/** Количество строки словами: штуки, канистры, литры розлива, килограммы. */
function qtyText(kind: string, qty: number): string {
  if (kind === 'pour') return formatLiters(qty)
  if (kind === 'weight') return formatKg(qty)
  if (kind === 'container') return `${qty} кан.`
  return `${qty} шт`
}

/** Сумма со знаком и цветом: поступление зелёным, расход красным. */
function Signed({ value }: { value: number }) {
  return <Money value={value} tone="sign" className="font-medium" />
}

export default function ShiftReport() {
  const { id = '' } = useParams()
  const [params, setParams] = useSearchParams()
  const tab = tabOf(params.get('tab'))
  const setTab = (t: TabKey) => setParams(t === 'overview' ? {} : { tab: t }, { replace: true })
  const r = useLoad(() => get<Report>(`/shifts/${id}/report`), [id])
  const printing = useAction()
  const data = r.data

  if (r.loading && !data) return <Loading />
  if (!data) return <ErrorBox error={r.error ?? 'Отчёт не загрузился'} />
  const s = data.shift
  const open = s.closed_at === null
  const counts: Partial<Record<TabKey, number>> = {
    sales: data.sales_totals.count,
    returns: data.returns_totals.count,
    receipts: data.receipts.length,
    money: data.money.length,
    debts: data.debts.length,
    payouts: data.payouts.length,
    cancelled: data.cancelled.length,
  }
  return (
    <div className="flex flex-col gap-4" data-testid="shift-report">
      <PageHeader
        title={`Смена № ${s.number}`}
        back={
          <Link to="/shift" className="text-sky-700 hover:underline">
            ← Смена и кассы
          </Link>
        }
        subtitle={
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span>
              Кассир: <b className="font-medium text-slate-700">{s.cashier_name}</b>
            </span>
            <span>
              Касса: <b className="font-medium text-slate-700">{s.account_name}</b>
            </span>
            <span className="tabular-nums">
              с {formatDateTime(s.opened_at)}
              {s.closed_at && <> по {formatDateTime(s.closed_at)}</>}
            </span>
            {open ? <Badge tone="green">открыта</Badge> : <Badge>закрыта</Badge>}
          </span>
        }
        actions={
          <span className="no-print flex flex-wrap gap-2">
            <Button variant="secondary" onClick={() => window.print()}>
              Печать
            </Button>
            {/* Чек смены 80 мм (SPEC-36 §10) — рядом с печатью отчёта на A4. */}
            <Button variant="secondary" disabled={printing.busy} onClick={() => void printing.run(() => printShift(s.id))}>
              Печать чека смены
            </Button>
          </span>
        }
      />

      <div className="no-print -mx-1 flex gap-2 overflow-x-auto px-1 pb-1 md:flex-wrap md:overflow-visible" role="tablist" aria-label="Разделы отчёта смены">
        {TABS.map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`inline-flex min-h-[38px] shrink-0 items-center gap-2 rounded-md border px-4 py-2 text-sm font-medium whitespace-nowrap ${
              tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'
            }`}
            onClick={() => setTab(key)}
          >
            {label}
            {counts[key] ? (
              <span className={`rounded-full px-1.5 text-xs tabular-nums ${tab === key ? 'bg-white/25 text-white' : 'bg-slate-100 text-slate-600'}`}>{counts[key]}</span>
            ) : null}
          </button>
        ))}
      </div>
      <ErrorBox error={printing.error} />

      {tab === 'overview' && <Overview data={data} />}
      {tab === 'sales' && <SalesTab data={data} />}
      {tab === 'returns' && <ReturnsTab data={data} />}
      {tab === 'receipts' && <ReceiptsTab data={data} />}
      {tab === 'money' && <MoneyTab data={data} />}
      {tab === 'debts' && <DebtsTab data={data} />}
      {tab === 'payouts' && <PayoutsTab data={data} />}
      {tab === 'cancelled' && <CancelledTab rows={data.cancelled} total={data.cancelled_tyiyn} />}
    </div>
  )
}

function Overview({ data }: { data: Report }) {
  const c = data.cash
  const v = data.revenue
  const diff = c.diff_tyiyn
  const owner = data.owner_view && v.gross_tyiyn !== undefined
  return (
    <div className="flex flex-col gap-4">
      <div className={`grid grid-cols-2 gap-3 ${owner ? 'lg:grid-cols-5' : 'lg:grid-cols-3'}`}>
        <Tile label="Выручка за смену" value={formatSom(v.net_tyiyn)} hint={`${v.sales_count} ${plural(v.sales_count, ['чек', 'чека', 'чеков'])}`} testId="tile-revenue" />
        {owner && <Tile label="Валовая прибыль" value={formatSom(v.gross_tyiyn ?? 0)} tone="green" testId="tile-gross" />}
        {owner && v.net_profit_tyiyn !== undefined && (
          <Tile
            label="Чистая прибыль"
            value={formatSom(v.net_profit_tyiyn)}
            tone={v.net_profit_tyiyn < 0 ? 'rose' : 'green'}
            hint="без оклада"
            testId="tile-net-profit"
          />
        )}
        <Tile label={c.counted_tyiyn === null ? 'Должно быть в кассе' : 'Пересчитали при закрытии'} value={formatSom(c.counted_tyiyn ?? c.expected_tyiyn)} hint={c.counted_tyiyn === null ? 'смена идёт' : `должно было быть ${formatSom(c.expected_tyiyn)}`} />
        <Tile
          label="Разница"
          value={diff === null ? '—' : diff === 0 ? 'сошлось' : formatSom(diff)}
          tone={diff === null || diff === 0 ? undefined : diff < 0 ? 'rose' : 'green'}
          hint={diff === null ? 'после закрытия' : diff < 0 ? 'недостача' : diff > 0 ? 'излишек' : 'без расхождений'}
        />
      </div>

      {(data.qr_unclaimed ?? []).length > 0 && (
        // Оплачено по QR банка, а чека нет (SPEC-31): деньги у банка, в учёте их нет.
        <div className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900" data-testid="qr-unclaimed">
          QR оплачен, чек не проведён:{' '}
          {(data.qr_unclaimed ?? []).map((q, i) => (
            <span key={q.id}>
              {i > 0 && ', '}
              <b>{formatSom(q.amount_tyiyn)}</b>
              {q.paid_at && ` в ${formatTime(q.paid_at)}`} ({q.user_name})
            </span>
          ))}
          . Проведите чек с этой оплатой картой или спросите у кассира.
        </div>
      )}

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardTitle>Движение денег</CardTitle>
          <div className="text-xs text-slate-500">Наличные в кассе «{data.shift.account_name}» за время смены</div>
          <div className="mt-2" data-testid="cash-flow">
            <Row label="Остаток на начало смены" value={c.opening_tyiyn} testId="flow-opening" />
            {c.opening_tyiyn !== c.opening_expected_tyiyn && (
              <div className="pb-1 pl-4 text-xs text-slate-500">
                Пересчитали при открытии; ожидали {formatSom(c.opening_expected_tyiyn)}
              </div>
            )}
            <div className="mt-2">
              <Row label={<span className="font-medium">Приход</span>} value={c.income_tyiyn} sign="+" tone="green" testId="flow-income" />
              <div className="-mt-1 pb-1 text-xs text-slate-500">поступление наличных в кассу</div>
              {c.income.map((l) => (
                <Row key={l.key} label={l.label} value={l.amount_tyiyn} sub testId={`flow-${l.key}`} />
              ))}
            </div>
            <div className="mt-2">
              <Row label={<span className="font-medium">Расход</span>} value={c.outgo_tyiyn} sign="−" tone="rose" testId="flow-outgo" />
              <div className="-mt-1 pb-1 text-xs text-slate-500">выдано наличными из кассы</div>
              {c.outgo.map((l) => (
                <Row key={l.key} label={l.label} value={l.amount_tyiyn} sub testId={`flow-${l.key}`} />
              ))}
            </div>
            <Row label="Должно быть в кассе" value={c.expected_tyiyn} strong testId="flow-expected" />
            {c.counted_tyiyn !== null && <Row label="Остаток на конец смены (пересчитали)" value={c.counted_tyiyn} testId="flow-counted" />}
            {diff !== null && (
              <div className="flex items-baseline justify-between gap-3 py-1">
                <span>Разница (излишек / недостача)</span>
                {diff === 0 ? <Badge tone="green">сошлось</Badge> : <Badge tone={diff < 0 ? 'rose' : 'amber'}>{`${diff < 0 ? 'недостача' : 'излишек'} ${formatSom(Math.abs(diff))}`}</Badge>}
              </div>
            )}
            {c.handover_tyiyn !== null && (
              <div className="mt-2 rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-600">
                {c.handover_tyiyn > 0 ? (
                  <>
                    После закрытия сдали {c.handover_to ? `в «${c.handover_to}» ` : ''}
                    <b className="tabular-nums">{formatSom(c.handover_tyiyn)}</b>, в кассе осталось <b className="tabular-nums">{formatSom(c.left_tyiyn ?? 0)}</b>
                  </>
                ) : (
                  <>После закрытия деньги остались в кассе: {formatSom(c.left_tyiyn ?? 0)}</>
                )}
              </div>
            )}
          </div>
        </Card>

        <Card>
          <CardTitle>Выручка за смену</CardTitle>
          <div data-testid="revenue">
            <Row label={`Продажи · ${v.sales_count} ${plural(v.sales_count, ['чек', 'чека', 'чеков'])}`} value={v.sales_tyiyn} />
            <Row label={`Возвраты${v.returns_count ? ` · ${v.returns_count}` : ''}`} value={v.returns_tyiyn} sign="−" />
            <Row label="Выручка" value={v.net_tyiyn} strong testId="revenue-net" />
            <div className="mt-3 text-xs font-medium tracking-wide text-slate-500 uppercase">По способам оплаты</div>
            {v.methods.length === 0 && <div className="py-1 text-sm text-slate-500">Продаж не было</div>}
            {v.methods.map((m) => (
              <Row key={m.method} label={methodName(m.method)} value={m.amount_tyiyn} sub testId={`method-${m.method}`} />
            ))}
            {v.rest_tyiyn !== 0 && <Row label="Исправления и прочее" value={v.rest_tyiyn} sub />}
            {/* Безнал по счетам — всегда, даже при одном счёте; по движениям счетов (SPEC-36 §9). */}
            <div className="mt-3">
              <ShiftBank list={data.shift.bank_split ?? []} />
            </div>
            {owner && (
              <div className="mt-3 rounded-md bg-emerald-50 px-3 py-2" data-testid="gross">
                <Row label="Себестоимость проданного" value={v.cost_sold_tyiyn ?? 0} sub />
                {(v.cost_returned_tyiyn ?? 0) !== 0 && <Row label="Себестоимость возвращённого" value={v.cost_returned_tyiyn ?? 0} sign="−" sub />}
                <Row label="Себестоимость" value={v.cost_tyiyn ?? 0} />
                <Row label="Валовая прибыль за смену" value={v.gross_tyiyn ?? 0} strong tone="green" />
                {(v.cost_unknown_lines ?? 0) > 0 && (
                  <div className="pt-1 text-xs text-amber-800">
                    Строк без цены закупки: {v.cost_unknown_lines} — их себестоимость пока ноль, укажите цену товара.
                  </div>
                )}
              </div>
            )}
            {owner && v.net_profit_tyiyn !== undefined && (
              // Чистая прибыль за смену (ADR-120): формула прибыли дня по времени смены.
              <div className="mt-3 rounded-md bg-sky-50 px-3 py-2" data-testid="net-profit">
                {/* Валовая — строкой выше; здесь — что из неё вычли или прибавили. */}
                {(v.profit ?? [])
                  .filter((l) => l.key !== 'gross')
                  .map((l) => (
                    <Row key={l.key} label={l.label} value={Math.abs(l.amount_tyiyn)} sign={l.amount_tyiyn < 0 ? '−' : '+'} sub testId={`profit-${l.key}`} />
                  ))}
                <Row label="Чистая прибыль за смену" value={v.net_profit_tyiyn} strong tone={v.net_profit_tyiyn < 0 ? 'rose' : 'green'} testId="net-profit-total" />
                <div className="pt-1 text-xs text-slate-500">
                  Без оклада — оклад начисляется в конце месяца. Чеки вне смен в смены не входят, поэтому сумма по сменам может не
                  совпасть с прибылью дня.
                </div>
              </div>
            )}
          </div>
        </Card>
      </div>

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
        <Card>
          <CardTitle>Сравнение</CardTitle>
          {data.compare ? (
            <div className="flex flex-col gap-2" data-testid="compare">
              <div className="text-sm text-slate-600">
                Выручка за смену <b className="tabular-nums text-slate-900">{formatSom(data.revenue.net_tyiyn)}</b>
              </div>
              <div className="text-sm text-slate-600">
                По сравнению со{' '}
                <Link to={`/shifts/${data.compare.shift_id}`} className="text-sky-700 hover:underline">
                  сменой № {data.compare.number}
                </Link>{' '}
                <b className="tabular-nums text-slate-900">{formatSom(data.compare.net_tyiyn)}</b>
                {data.compare.same_time && <> за то же время от открытия</>}
              </div>
              {data.compare.same_time && (
                <div className="text-xs text-slate-500" data-testid="compare-same-time">
                  Смена ещё идёт — прошлую смену берём за столько же часов от её открытия.
                </div>
              )}
              <div className={`text-2xl font-semibold tabular-nums ${data.compare.diff_tyiyn < 0 ? 'text-rose-700' : data.compare.diff_tyiyn > 0 ? 'text-emerald-700' : ''}`}>
                {data.compare.diff_tyiyn > 0 ? '▲ +' : data.compare.diff_tyiyn < 0 ? '▼ ' : ''}
                {formatSom(data.compare.diff_tyiyn)}
                {data.compare.change_bp !== null && <span className="ml-2 text-base font-medium">{changeText(data.compare.change_bp)}</span>}
              </div>
            </div>
          ) : (
            <div className="text-sm text-slate-500">Это первая смена кассы — сравнивать не с чем.</div>
          )}
        </Card>
        <Card>
          <CardTitle>Продажи по часам</CardTitle>
          <HourBars hours={data.hours} />
        </Card>
      </div>
    </div>
  )
}

/** Столбики продаж по часам — без библиотек, высота в процентах от лучшего часа. */
function HourBars({ hours }: { hours: HourRow[] }) {
  const [hover, setHover] = useState<number | null>(null)
  const heights = barHeights(hours.map((h) => h.sales_tyiyn))
  if (hours.every((h) => h.sales_tyiyn === 0)) return <div className="rounded-md bg-slate-50 py-10 text-center text-sm text-slate-500">Продаж ещё не было</div>
  const shown = hover === null ? null : hours[hover]
  // Подписи часов: при многих часах — через один, чтобы не налезали.
  const every = hours.length > 16 ? 3 : hours.length > 10 ? 2 : 1
  return (
    <div data-testid="hour-bars">
      <div className="mb-2 h-5 text-sm text-slate-600">
        {shown ? (
          <>
            {formatTime(shown.at)}–{formatTime(new Date(new Date(shown.at).getTime() + 3_600_000).toISOString())}:{' '}
            <b className="tabular-nums text-slate-900">{formatSom(shown.sales_tyiyn)}</b> · {shown.count} {plural(shown.count, ['чек', 'чека', 'чеков'])}
            {shown.returns_tyiyn > 0 && <> · возвраты {formatSom(shown.returns_tyiyn)}</>}
          </>
        ) : (
          <span className="text-slate-400">Над столбиком — продажи за час; нажмите — подробнее</span>
        )}
      </div>
      <div className="flex h-44 items-end gap-1 border-b border-slate-200" onMouseLeave={() => setHover(null)}>
        {hours.map((h, i) => (
          <button
            key={h.at}
            type="button"
            aria-label={`${formatTime(h.at)}: ${formatSom(h.sales_tyiyn)}`}
            className="group flex h-full min-w-0 flex-1 items-end justify-center"
            onMouseEnter={() => setHover(i)}
            onFocus={() => setHover(i)}
            onClick={() => setHover(i)}
          >
            {/* Сумма над столбиком видна сразу, без нажатия: коротко, «2,5 т». */}
            <span className="flex h-full w-full max-w-10 flex-col items-center justify-end">
              {h.sales_tyiyn > 0 && (
                <span className="mb-0.5 text-[10px] leading-none whitespace-nowrap text-slate-600 tabular-nums" data-testid="hour-label">
                  {formatSomShort(h.sales_tyiyn)}
                </span>
              )}
              <span
                className={`block w-full rounded-t transition ${hover === i ? 'bg-sky-700' : 'bg-sky-500 group-hover:bg-sky-600'}`}
                style={{ height: `calc((100% - 14px) * ${heights[i]} / 100)` }}
              />
            </span>
          </button>
        ))}
      </div>
      <div className="mt-1 flex gap-1">
        {hours.map((h, i) => (
          <div key={h.at} className="min-w-0 flex-1 text-center text-[11px] text-slate-500 tabular-nums">
            {i % every === 0 ? formatTime(h.at).slice(0, 2) : ''}
          </div>
        ))}
      </div>
    </div>
  )
}

/** Номер чека ссылкой. */
function SaleLink({ id, number }: { id: string; number: number }) {
  return (
    <Link to={`/sales/${id}`} className="font-medium text-sky-700 hover:underline">
      № {number}
    </Link>
  )
}

function SalesTable({ rows, pagerKey, returns }: { rows: SaleRow[]; pagerKey: string; returns?: boolean }) {
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const [method, setMethod] = useState('')
  const filtered = filterSales(rows, query, method)
  const pager = usePager(filtered, pagerKey, `${query}|${method}`)
  const methods = methodsIn(rows)
  return (
    <Card>
      <div className="mb-3 flex flex-wrap items-end gap-3">
        <label className="flex flex-col gap-1 text-sm">
          <span className="font-medium text-slate-700">Номер чека</span>
          <input className="w-40" inputMode="numeric" placeholder="Поиск" value={query} onChange={(e) => setQuery(e.target.value)} data-testid="sales-search" />
        </label>
        {methods.length > 1 && (
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium text-slate-700">Способ оплаты</span>
            <select value={method} onChange={(e) => setMethod(e.target.value)}>
              <option value="">Все</option>
              {methods.map((m) => (
                <option key={m} value={m}>
                  {methodName(m)}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
      {filtered.length === 0 ? (
        <Empty icon="receipt">{rows.length === 0 ? (returns ? 'Возвратов за смену не было' : 'Продаж за смену не было') : 'Ничего не нашлось'}</Empty>
      ) : (
        <>
          <Table head={returns ? ['№', 'Время', 'По чеку', 'Способ', 'Счёт', 'Кассир', 'Покупатель', 'Сумма'] : ['№', 'Время', 'Способ', 'Счёт', 'Кассир', 'Покупатель', 'Сумма']}>
            {pager.shown.map((s) => (
              <tr key={s.id} className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(`/sales/${s.id}`)}>
                <td className="px-2 py-2 whitespace-nowrap">
                  <SaleLink id={s.id} number={s.number} />
                </td>
                <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(s.created_at)}</td>
                {returns && (
                  <td className="px-2 py-2 whitespace-nowrap">
                    {s.reversal_of && s.reversal_of_number !== null ? <SaleLink id={s.reversal_of} number={s.reversal_of_number} /> : 'Без чека'}
                  </td>
                )}
                <td className="px-2 py-2">{paymentsText(s.payments)}</td>
                <td className="px-2 py-2 text-slate-600">{accountsText(s.payments)}</td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="px-2 py-2">{s.party_name ?? <span className="text-slate-400">Розничный</span>}</td>
                <td className="px-2 py-2 text-right font-medium">
                  <Money value={returns ? -s.total_tyiyn : s.total_tyiyn} />
                </td>
              </tr>
            ))}
          </Table>
          <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label={returns ? 'Страницы возвратов' : 'Страницы продаж'} />
        </>
      )}
    </Card>
  )
}

function SalesTab({ data }: { data: Report }) {
  const t = data.sales_totals
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Сумма продаж" value={formatSom(t.sales_tyiyn)} hint={`${t.count} ${plural(t.count, ['чек', 'чека', 'чеков'])}`} />
        <Tile label="Получено оплатой" value={formatSom(t.paid_tyiyn)} tone="green" />
        <Tile label="Оплачено баллами" value={formatSom(t.bonus_tyiyn)} />
        <Tile label="В долг" value={formatSom(t.debt_tyiyn)} tone={t.debt_tyiyn > 0 ? 'rose' : undefined} />
      </div>
      <SalesTable rows={data.sales.filter((s) => s.kind === 'sale')} pagerKey="shift_report.sales" />
    </div>
  )
}

function ReturnsTab({ data }: { data: Report }) {
  const t = data.returns_totals
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Сумма возвратов" value={formatSom(t.total_tyiyn)} tone={t.total_tyiyn > 0 ? 'rose' : undefined} hint={`${t.count} ${plural(t.count, ['возврат', 'возврата', 'возвратов'])}`} />
        {t.methods.map((m) => (
          <Tile key={m.method} label={methodName(m.method)} value={formatSom(m.amount_tyiyn)} />
        ))}
      </div>
      <SalesTable rows={data.sales.filter((s) => s.kind === 'return')} pagerKey="shift_report.returns" returns />
    </div>
  )
}

function ReceiptsTab({ data }: { data: Report }) {
  const rows = data.receipts
  const pager = usePager(rows, 'shift_report.receipts')
  const total = rows.reduce((a, r) => a + r.total_tyiyn, 0)
  const fromTill = rows.reduce((a, r) => a + r.paid_till_tyiyn, 0)
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Приходов на сумму" value={formatSom(total)} hint={`${rows.length} ${plural(rows.length, ['накладная', 'накладные', 'накладных'])}`} />
        <Tile label="Оплачено из кассы смены" value={formatSom(fromTill)} tone={fromTill > 0 ? 'rose' : undefined} />
      </div>
      <Card>
        {rows.length === 0 ? (
          <Empty icon="receipt">Приходов за смену не было</Empty>
        ) : (
          <>
            <Table head={['№', 'Время', 'Поставщик', 'Сумма', 'Из кассы смены', 'С других счетов', 'В долг', 'Кто']}>
              {pager.shown.map((x) => (
                <tr key={x.id}>
                  <td className="px-2 py-2 whitespace-nowrap">
                    <Link to={`/receipts/${x.id}`} className="font-medium text-sky-700 hover:underline">
                      № {x.number}
                    </Link>
                    {x.reversal_of && (
                      <span className="ml-2">
                        <Badge tone="rose">сторно</Badge>
                      </span>
                    )}
                  </td>
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(x.created_at)}</td>
                  <td className="px-2 py-2">
                    {x.supplier_name ?? '—'}
                    {x.supplier_doc && <div className="text-xs text-slate-500">накладная {x.supplier_doc}</div>}
                  </td>
                  <td className="px-2 py-2 text-right font-medium">
                    <Money value={x.total_tyiyn} />
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={x.paid_till_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2 text-right">
                    {x.paid_other_tyiyn ? (
                      <>
                        <Money value={x.paid_other_tyiyn} />
                        {x.paid_account_name && <div className="text-xs text-slate-500">{x.paid_account_name}</div>}
                      </>
                    ) : (
                      '—'
                    )}
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Money value={x.on_debt_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2">{x.user_name}</td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы приходов смены" />
          </>
        )}
      </Card>
    </div>
  )
}

function TotalsGrid({ lines }: { lines: ReportLine[] }) {
  return (
    <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
      {lines.map((l) => (
        <Tile key={l.key} label={totalsLabel(l.key, l.label)} value={formatSom(l.amount_tyiyn)} testId={`money-total-${l.key}`} />
      ))}
    </div>
  )
}

/** Подпись итога денежных операций — во множественном числе. */
function totalsLabel(key: string, label: string): string {
  const plural: Record<string, string> = {
    cash_in: 'Внесения',
    cash_out: 'Изъятия',
    transfers_in: 'Переводы в кассу',
    transfers_out: 'Переводы из кассы',
    expenses: 'Расходы',
    payouts: 'Выплаты',
    advances: 'Авансы',
    debt_repayments: 'Оплаты долга',
    supplier_payments: 'Оплаты поставщикам',
    supplier_refunds: 'Деньги от поставщиков',
    transfers: 'Переводы между счетами',
  }
  return plural[key] ?? label
}

function MoneyTab({ data }: { data: Report }) {
  // Сначала поздние, как в остальных списках; кнопка показывает текущий порядок и меняет его.
  const [newFirst, setNewFirst] = useState(true)
  const rows = newFirst ? [...data.money].reverse() : data.money
  const pager = usePager(rows, 'shift_report.money', String(newFirst))
  return (
    <div className="flex flex-col gap-4">
      <TotalsGrid lines={data.money_totals} />
      <Card>
        <CardTitle
          actions={
            data.money.length > 1 && (
              <Button variant="secondary" className="min-h-9 px-3 py-1" title="Поменять порядок" onClick={() => setNewFirst(!newFirst)}>
                {newFirst ? 'Сначала поздние ⇅' : 'Сначала ранние ⇅'}
              </Button>
            )
          }
        >
          Операции по всем счетам
        </CardTitle>
        {rows.length === 0 ? (
          <Empty icon="wallet">Денежных операций за смену не было</Empty>
        ) : (
          <>
            <Table head={['Время', 'Вид', 'Сумма', 'Счёт', 'Пользователь', 'Кому / от кого', 'Комментарий']}>
              {pager.shown.map((m) => (
                <tr key={m.id} data-testid="money-row">
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(m.created_at)}</td>
                  <td className="px-2 py-2">
                    {m.label}
                    {m.kind === 'expenses' && m.counterparty && <span className="text-slate-500"> ({m.counterparty})</span>}
                    {m.reversal && (
                      <span className="ml-2">
                        <Badge tone="rose">сторно</Badge>
                      </span>
                    )}
                  </td>
                  <td className="px-2 py-2 text-right">
                    <Signed value={m.amount_tyiyn} />
                  </td>
                  <td className="px-2 py-2">
                    {m.account_name}
                    {m.to_account_name && <span className="text-slate-500"> {m.amount_tyiyn < 0 || m.kind === 'transfers' ? '→' : '←'} {m.to_account_name}</span>}
                  </td>
                  <td className="px-2 py-2">{m.user_name}</td>
                  <td className="px-2 py-2">{m.kind === 'expenses' ? '—' : (m.counterparty ?? '—')}</td>
                  <td className="px-2 py-2 text-slate-600">{m.comment || '—'}</td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы денежных операций" />
          </>
        )}
        <div className="mt-2 text-xs text-slate-500">
          Чеки — во вкладках «Продажи» и «Возвраты». Наличные кассы смены из этих операций сведены в «Обзоре», в движении денег.
        </div>
      </Card>
    </div>
  )
}

function DebtsTab({ data }: { data: Report }) {
  const rows = data.debts
  const t = debtTotals(rows)
  const pager = usePager(rows, 'shift_report.debts')
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Продано в долг" value={formatSom(t.sold)} tone={t.sold > 0 ? 'rose' : undefined} hint="за вычетом возвратов" />
        <Tile label="Оплачено долгов" value={formatSom(t.repaid)} tone={t.repaid > 0 ? 'green' : undefined} />
        {t.adjusted !== 0 && <Tile label="Правки долга" value={formatSom(t.adjusted)} />}
      </div>
      <Card>
        {rows.length === 0 ? (
          <Empty icon="debts">Продаж в долг и оплат долга за смену не было</Empty>
        ) : (
          <>
            <Table head={['Время', 'Что', 'Клиент', 'Чек', 'Сумма', 'Счёт', 'Кто']}>
              {pager.shown.map((d) => (
                <tr key={d.id}>
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(d.created_at)}</td>
                  <td className="px-2 py-2">
                    {debtKindText(d)}
                    {d.comment && <div className="text-xs text-slate-500">{d.comment}</div>}
                  </td>
                  <td className="px-2 py-2">
                    <Link to={`/clients/${d.party_id}`} className="text-sky-700 hover:underline">
                      {d.party_name}
                    </Link>
                  </td>
                  <td className="px-2 py-2 whitespace-nowrap">{d.sale_id && d.sale_number !== null ? <SaleLink id={d.sale_id} number={d.sale_number} /> : '—'}</td>
                  <td className="px-2 py-2 text-right">
                    {/* Долг вырос — красным, уменьшился — зелёным. */}
                    <Money value={d.amount_tyiyn} className={`font-medium ${d.amount_tyiyn > 0 ? 'text-rose-700' : 'text-emerald-700'}`} />
                  </td>
                  <td className="px-2 py-2">{d.account_name ?? '—'}</td>
                  <td className="px-2 py-2">{d.user_name}</td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы долгов смены" />
          </>
        )}
      </Card>
    </div>
  )
}

function PayoutsTab({ data }: { data: Report }) {
  const rows = data.payouts
  const pager = usePager(rows, 'shift_report.payouts')
  // Авансы — отдельно от выплат заработанного (ADR-122).
  const advances = rows.filter((p) => p.purpose === 'advance')
  const advanceSum = advances.reduce((a, p) => a + p.amount_tyiyn, 0)
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Выдано сотрудникам" value={formatSom(data.payouts_tyiyn)} hint={`${rows.length} ${plural(rows.length, ['выплата', 'выплаты', 'выплат'])}`} />
        {advances.length > 0 && (
          <Tile label="Из них авансы" value={formatSom(advanceSum)} hint={`${advances.length} ${plural(advances.length, ['аванс', 'аванса', 'авансов'])}`} testId="payouts-advances" />
        )}
      </div>
      <Card>
        {rows.length === 0 ? (
          <Empty icon="payroll">Выплат за смену не было</Empty>
        ) : (
          <>
            <Table head={['Время', 'Сотрудник', 'Сумма', 'За что', 'Откуда', 'Кто выдал']}>
              {pager.shown.map((p) => (
                <tr key={p.id}>
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(p.created_at)}</td>
                  <td className="px-2 py-2 font-medium">
                    {p.employee_name}
                    {p.reversal && (
                      <span className="ml-2">
                        <Badge tone="rose">сторно</Badge>
                      </span>
                    )}
                  </td>
                  <td className="px-2 py-2 text-right font-medium">
                    <Money value={p.amount_tyiyn} tone="negative" />
                  </td>
                  <td className="px-2 py-2" data-testid="payout-purpose">
                    {PURPOSE_LABELS[p.purpose ?? 'general'] ?? p.purpose}
                    {p.comment && <div className="text-xs text-slate-500">{p.comment}</div>}
                  </td>
                  <td className="px-2 py-2">{p.account_name ?? 'не из кассы'}</td>
                  <td className="px-2 py-2">{p.user_name}</td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы выплат смены" />
          </>
        )}
      </Card>
    </div>
  )
}

function CancelledTab({ rows, total }: { rows: CancelledRow[]; total: number }) {
  const pager = usePager(rows, 'shift_report.cancelled')
  return (
    <div className="flex flex-col gap-4">
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Tile label="Отменено строк" value={rows.length} testId="cancelled-count" />
        <Tile label="На сумму" value={formatSom(total)} tone={total > 0 ? 'rose' : undefined} testId="cancelled-total" />
      </div>
      <Card>
        {rows.length === 0 ? (
          <Empty icon="receipt">Из чеков ничего не удаляли</Empty>
        ) : (
          <>
            <Table head={['Время', 'Товар или услуга', 'Количество', 'Цена', 'Сумма', 'Как', 'Чек', 'Кассир', 'Пользователь']}>
              {pager.shown.map((c) => (
                <tr key={c.id} data-testid="cancelled-row">
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{formatTime(c.removed_at)}</td>
                  <td className="px-2 py-2 font-medium">{c.name}</td>
                  <td className="px-2 py-2 whitespace-nowrap tabular-nums">{qtyText(c.kind, c.qty)}</td>
                  <td className="px-2 py-2 text-right">
                    <Money value={c.unit_price_tyiyn} />
                  </td>
                  <td className="px-2 py-2 text-right font-medium">
                    <Money value={c.amount_tyiyn} />
                  </td>
                  <td className="px-2 py-2">{CANCEL_REASONS[c.reason]}</td>
                  <td className="px-2 py-2 whitespace-nowrap">
                    {c.sale_id && c.sale_number !== null ? (
                      <>
                        проведён <SaleLink id={c.sale_id} number={c.sale_number} />
                      </>
                    ) : (
                      <span className="text-slate-400">не проведён</span>
                    )}
                  </td>
                  <td className="px-2 py-2">{c.cashier_name ?? '—'}</td>
                  <td className="px-2 py-2">{c.user_name}</td>
                </tr>
              ))}
            </Table>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы отменённых товаров" />
          </>
        )}
        <div className="mt-2 text-xs text-slate-500">
          Строки, удалённые из чека до проведения: крестиком, кнопкой «Очистить» или вместе с отложенным чеком. Уменьшение количества
          кнопкой «−» сюда не попадает.
        </div>
      </Card>
    </div>
  )
}
