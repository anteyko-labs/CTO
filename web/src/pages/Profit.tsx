// «Прибыль и убытки» владельца (задача 003 Н12, SPEC-08): дерево по образцу прежней программы с
// раскрытием до документов, «Не входит в прибыль», «Скачать» — .xlsx с листами. Итоги — серверные
// (`gross_tyiyn`, `net_tyiyn`); администратору сервер отчёт не отдаёт (инвариант 13).
import { useMemo, useState, type ReactNode } from 'react'
import { Link } from 'react-router-dom'
import { BranchBreakdown, BranchScope } from '../components/BranchScope'
import { ExportButton } from '../components/ExportButton'
import { PnlTree } from '../components/PnlTree'
import { Button, Card, CardTitle, Checkbox, Empty, ErrorBox, Field, Loading, Money, Notice, PageHeader } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { branchParam, useAllBranches, type AllBranches } from '../lib/branchReport'
import { periodText } from '../lib/exportTable'
import { formatDate, formatDateTime, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { notInProfit, periodTitle, pnlSheets, pnlTree, QUICK_PERIODS, quickPeriod, type PnlNode, type QuickPeriod } from '../lib/pnl'
import type { ProfitReport, ProfitSettings } from '../lib/types'

const percent = (bp: number | null): string => {
  if (bp === null) return '—'
  const abs = Math.abs(bp)
  return `${bp < 0 ? '−' : ''}${Math.trunc(abs / 100)},${String(Math.trunc((abs % 100) / 10))} %`
}

/** Все ключи строк с раскрытием — для «Развернуть всё». */
function allIds(nodes: PnlNode[]): string[] {
  return nodes.flatMap((n) => (n.children?.length || n.hidden ? [n.id, ...allIds(n.children ?? [])] : []))
}

export default function Profit() {
  const today = todayBishkek()
  const user = useUser()
  const settings = useLoad(() => get<ProfitSettings>('/settings/profit-report'), [])
  const [picked, setRange] = useState<{ from: string; to: string } | null>(null)
  const [reversalsPick, setShowReversals] = useState<boolean | null>(null)
  // Период по умолчанию и показ сторно — из настроек «Отчёт о прибыли» («Этот месяц», свёрнуто).
  const ready = !settings.loading || Boolean(settings.error)
  const range = picked ?? (ready ? quickPeriod(settings.data?.default_period ?? 'month', today) : null)
  const showReversals = reversalsPick ?? settings.data?.show_reversals ?? false
  const [all, setAll] = useAllBranches(Boolean(user.several_branches))
  const r = useLoad(
    () => (range ? get<AllBranches<ProfitReport>>(`/reports/profit${qs({ ...range, detail: 1, ...branchParam(all) })}`) : Promise.resolve(null)),
    [range?.from, range?.to, all],
  )
  const [open, toggle, setOpen] = useOpenSet()
  const [moneyOpen, toggleMoney] = useOpenSet()
  const tree = useMemo(() => (r.data ? pnlTree(r.data, showReversals) : []), [r.data, showReversals])
  const money = useMemo(() => (r.data ? notInProfit(r.data) : []), [r.data])
  const t = r.data?.totals
  const branchName = all ? 'Все филиалы' : (user.branches?.find((b) => b.id === user.branch_id)?.name ?? user.branch_name ?? '')

  const from = range?.from ?? today
  const to = range?.to ?? today
  const quickOn = (k: QuickPeriod) => {
    const q = quickPeriod(k, today)
    return q.from === from && q.to === to
  }

  return (
    <div>
      <PageHeader
        title="Прибыль и убытки"
        subtitle={range ? periodTitle(from, to) : undefined}
        actions={
          <div className="no-print flex flex-wrap gap-2">
            <ExportButton
              label="Скачать"
              file={`Прибыль и убытки${all ? ' все филиалы' : ''}`}
              period={periodText(from, to)}
              disabled={!r.data}
              sheets={() => (r.data ? pnlSheets(r.data, { branch: branchName, now: formatDateTime(new Date().toISOString()) }) : [])}
            />
            <Button variant="secondary" disabled={!t} onClick={() => window.print()}>
              Печать
            </Button>
          </div>
        }
      />
      <Card className="no-print mb-4 flex flex-col gap-3">
        <div className="flex flex-wrap gap-2" role="group" aria-label="Период">
          {QUICK_PERIODS.map(([k, label]) => (
            <button
              key={k}
              type="button"
              aria-pressed={quickOn(k)}
              className={`min-h-[38px] rounded-md border px-3 py-2 text-sm ${quickOn(k) ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 hover:bg-slate-50'}`}
              onClick={() => setRange(quickPeriod(k, today))}
            >
              {label}
            </button>
          ))}
        </div>
        <div className="flex flex-wrap items-end gap-3">
          <Field label="С">
            <input type="date" className="w-40" value={from} max={to} onChange={(e) => e.target.value && setRange({ from: e.target.value, to })} />
          </Field>
          <Field label="По">
            <input type="date" className="w-40" value={to} min={from} onChange={(e) => e.target.value && setRange({ from, to: e.target.value })} />
          </Field>
          <div className="pb-2">
            <Checkbox label="Показать сторно и исправления" checked={showReversals} onChange={setShowReversals} />
          </div>
          <Link to="/settings/profit-report" className="ml-auto pb-2 text-sm text-sky-700 underline">
            Настройки отчёта
          </Link>
        </div>
      </Card>
      <div className="no-print">
        <BranchScope user={user} all={all} onChange={setAll} />
      </div>

      <ErrorBox error={r.error} />
      {(r.loading && !r.data) || !range ? (
        <Loading />
      ) : t && r.data ? (
        <div className="flex flex-col gap-4">
          {all && <div className="text-sm font-medium">Все филиалы</div>}
          <BranchBreakdown
            parts={r.data.branches}
            columns={[
              { label: 'Выручка', value: (x) => x.totals.goods_tyiyn + x.totals.services_tyiyn },
              { label: 'Валовая', value: (x) => x.totals.gross_tyiyn },
              { label: 'Расходы', value: (x) => x.totals.expenses_tyiyn },
              { label: 'Чистая', value: (x) => x.totals.net_tyiyn },
            ]}
          />
          {r.data.warnings.length > 0 && (
            <Notice tone="amber">
              {r.data.warnings.map((w) => (
                <div key={w}>{w}</div>
              ))}
            </Notice>
          )}
          <Card className="break-inside-avoid" data-testid="pnl-tree">
            <CardTitle
              actions={
                <div className="no-print flex gap-2">
                  <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setOpen(allIds(tree))}>
                    Развернуть всё
                  </Button>
                  <Button variant="ghost" className="px-2 py-1 text-xs" onClick={() => setOpen([])}>
                    Свернуть
                  </Button>
                </div>
              }
            >
              Отчёт о прибыли и убытках
            </CardTitle>
            <PnlTree nodes={tree} open={open} onToggle={toggle} />
            <div className="mt-2 text-xs text-slate-500">
              Чеков {t.sales_count}, маржа {percent(t.margin_bp)}. Зарплата — по начислению; выдача денег прибыль не меняет.
            </div>
          </Card>

          {money.length > 0 && (
            <Card className="break-inside-avoid" data-testid="pnl-money">
              <CardTitle>Не входит в прибыль — движение денег</CardTitle>
              <div className="mb-2 text-xs text-slate-500">Деньги переместились (поставщикам, в сейф, работникам), прибыль от этого не меняется.</div>
              <PnlTree nodes={money} open={moneyOpen} onToggle={toggleMoney} />
            </Card>
          )}

          <Card>
            <CardTitle>По дням</CardTitle>
            {r.data.days.length === 0 ? (
              <Empty>За период нет ни продаж, ни расходов</Empty>
            ) : (
              <ReportTable
                cols={['День', 'Выручка', 'Валовая', 'Оплата труда', 'Комиссия', 'Расходы', 'Баллы', 'Касса ±', 'Брак, возвраты', 'Чистая']}
                rows={r.data.days.map((x) => ({
                  key: x.date,
                  cells: [
                    formatDate(x.date),
                    <Money key="rev" value={x.revenue_tyiyn} tone="negative" />,
                    <Money key="gross" value={x.gross_tyiyn} tone="negative" />,
                    <Money key="pay" value={x.payroll_tyiyn} tone="negative" />,
                    <Money key="fee" value={x.fee_tyiyn ?? 0} tone="negative" />,
                    <Money key="exp" value={x.expenses_tyiyn} tone="negative" />,
                    <Money key="bonus" value={x.bonus_tyiyn ?? 0} tone="negative" />,
                    <Money key="cash" value={x.cash_diff_tyiyn ?? 0} tone="negative" zero="—" />,
                    <Money key="defect" value={(x.defect_diff_tyiyn ?? 0) - (x.defect_writeoff_tyiyn ?? 0) + (x.return_diff_tyiyn ?? 0)} tone="negative" zero="—" />,
                    <Money key="net" value={x.net_tyiyn} tone="negative" className="font-semibold" />,
                  ],
                }))}
              />
            )}
          </Card>
        </div>
      ) : null}
    </div>
  )
}

/** Таблица отчёта: суммы по правому краю; на телефоне — карточки (класс responsive, data-label). */
function ReportTable({ cols, rows }: { cols: string[]; rows: { key: string; cells: ReactNode[] }[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="responsive w-full text-sm">
        <thead>
          <tr className="border-b border-slate-200 text-xs uppercase text-slate-500">
            {cols.map((c, i) => (
              <th key={c} className={`px-2 py-2 font-medium ${i ? 'text-right' : 'text-left'}`}>
                {c}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-slate-100">
          {rows.map((r) => (
            <tr key={r.key} className="break-inside-avoid">
              {r.cells.map((cell, i) => (
                <td key={cols[i]} data-label={cols[i]} className={`px-2 py-2 ${i ? 'whitespace-nowrap text-right tabular-nums' : 'font-medium'}`}>
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** Какие строки раскрыты: набор ключей и переключатель. */
function useOpenSet(initial: string[] = []): [Set<string>, (id: string) => void, (ids: string[]) => void] {
  const [open, setOpen] = useState(() => new Set(initial))
  const toggle = (id: string) =>
    setOpen((s) => {
      const next = new Set(s)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  return [open, toggle, (ids) => setOpen(new Set(ids))]
}
