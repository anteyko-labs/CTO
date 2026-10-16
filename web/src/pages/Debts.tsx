// Долги: кто должен нам и кому должны мы, с оплатой и погашением (SPEC-10). Экран на всю
// ширину: таблица с сортировкой и страницами, срок и последняя оплата — с сервера (SPEC-27 §6).
import { useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Pager, usePager } from '../components/Pager'
import { RepaymentModal } from '../components/RepaymentModal'
import { Button, Card, Empty, ErrorBox, Loading, Money, PageHeader } from '../components/ui'
import { get } from '../lib/api'
import { formatDate, formatDateTime, formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { sortDebts, type DebtSortKey } from '../lib/income'
import type { DebtRow, Party } from '../lib/types'

/** Долги обновляются сами, пока экран открыт (ADR-028). */
const REFRESH_MS = 10_000
/** Клиента без имени касса записывает по телефону или ИНН — такое имя показываем как «Без имени». */
const named = (p: Party) => {
  const name = p.name.trim()
  return name !== '' && name !== p.inn && name !== p.phone && !/^\+?[\d\s()-]+$/.test(name)
}

const COLS: { key: DebtSortKey; label: string; right?: boolean }[] = [
  { key: 'name', label: 'Контрагент' },
  { key: 'phone', label: 'Телефон' },
  { key: 'debt', label: 'Долг', right: true },
  { key: 'overdue', label: 'Просрочка', right: true },
  { key: 'last_payment', label: 'Последняя оплата' },
  { key: 'limit', label: 'Лимит', right: true },
]

function Side({
  title,
  pageKey,
  rows,
  owedToUs,
  empty,
  actionLabel,
  onOpen,
  onPay,
}: {
  title: string
  /** Ключ размера страницы этого списка. */
  pageKey: string
  rows: DebtRow[]
  /** Нам должны — янтарный; мы должны — нейтральный; просрочка — красный. */
  owedToUs: boolean
  empty: string
  actionLabel: string
  onOpen: (p: DebtRow) => void
  onPay: (p: DebtRow) => void
}) {
  // Крупные долги сверху; щелчок по заголовку — своя сортировка.
  const [sort, setSort] = useState<{ key: DebtSortKey; desc: boolean }>({ key: 'debt', desc: true })
  const sorted = useMemo(() => sortDebts(rows, sort.key, sort.desc), [rows, sort])
  // Смена сортировки — снова первая страница.
  const pager = usePager(sorted, pageKey, `${sort.key}:${sort.desc}`)
  const shown = pager.shown
  const total = rows.reduce((a, p) => a + Math.abs(p.balance_tyiyn), 0)
  const overdue = rows.reduce((a, p) => a + p.overdue_tyiyn, 0)
  const sortBy = (key: DebtSortKey) => {
    setSort((s) => (s.key === key ? { key, desc: !s.desc } : { key, desc: key !== 'name' && key !== 'phone' }))
  }
  return (
    <Card>
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
        <h2 className="font-semibold">
          {title} <span className="text-sm font-normal text-slate-500">· {rows.length}</span>
        </h2>
        <div className="text-right">
          <div className={`text-2xl font-bold tabular-nums ${owedToUs ? 'text-amber-700' : 'text-slate-900'}`}>{formatSom(total)}</div>
          {overdue > 0 && <div className="text-xs font-medium text-rose-700">просрочено {formatSom(overdue)}</div>}
        </div>
      </div>
      {rows.length === 0 ? (
        <Empty>{empty}</Empty>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="responsive w-full text-sm" aria-label={title}>
              <thead>
                <tr className="border-b border-slate-200 text-left text-xs uppercase text-slate-500">
                  {COLS.map((c) => (
                    <th
                      key={c.key}
                      className={`px-2 py-2 font-medium ${c.right ? 'text-right' : ''}`}
                      aria-sort={sort.key === c.key ? (sort.desc ? 'descending' : 'ascending') : 'none'}
                    >
                      <button type="button" className="inline-flex items-center gap-1 uppercase hover:text-slate-800" onClick={() => sortBy(c.key)}>
                        {c.label}
                        <span aria-hidden className="text-[10px]">
                          {sort.key === c.key ? (sort.desc ? '▼' : '▲') : ''}
                        </span>
                      </button>
                    </th>
                  ))}
                  <th className="px-2 py-2" />
                </tr>
              </thead>
              <tbody className="divide-y divide-slate-100">
                {shown.map((p) => (
                  <tr key={p.id} className="cursor-pointer hover:bg-slate-50" onClick={() => onOpen(p)}>
                    <td className="px-2 py-2" data-label="Контрагент">
                      {named(p) ? (
                        <div className="font-medium">{p.name}</div>
                      ) : (
                        <div className="font-medium text-slate-500">
                          Без имени{p.inn ? ` · ИНН ${p.inn}` : p.phone ? ` · ${p.phone}` : ''}
                        </div>
                      )}
                      <div className="text-xs text-slate-500">
                        {p.kind === 'company' ? 'Юрлицо' : 'Физлицо'}
                        {p.inn && named(p) && ` · ИНН ${p.inn}`}
                      </div>
                    </td>
                    <td className="whitespace-nowrap px-2 py-2" data-label="Телефон">
                      {p.phone || '—'}
                    </td>
                    <td className="px-2 py-2 text-right" data-label="Долг">
                      <Money value={Math.abs(p.balance_tyiyn)} className={`font-semibold ${owedToUs ? 'text-amber-700' : ''}`} />
                    </td>
                    <td className="px-2 py-2 text-right" data-label="Просрочка">
                      {p.overdue_tyiyn > 0 ? (
                        <div className="text-rose-700">
                          <div className="font-medium tabular-nums">{p.overdue_days} дн.</div>
                          <Money value={p.overdue_tyiyn} className="text-xs" />
                        </div>
                      ) : p.due_date ? (
                        <span className="text-xs text-slate-500">до {formatDate(p.due_date)}</span>
                      ) : (
                        <span className="text-slate-400">—</span>
                      )}
                    </td>
                    <td className="whitespace-nowrap px-2 py-2 text-slate-600" data-label="Последняя оплата">
                      {p.last_payment_at ? formatDateTime(p.last_payment_at) : '—'}
                    </td>
                    <td className="px-2 py-2 text-right" data-label="Лимит">
                      {p.credit_limit_tyiyn === null ? <span className="text-slate-400">—</span> : <Money value={p.credit_limit_tyiyn} />}
                    </td>
                    <td className="px-2 py-2 text-right">
                      <Button
                        variant="secondary"
                        className="min-h-9 px-3 py-1 text-xs"
                        onClick={(e) => {
                          e.stopPropagation()
                          onPay(p)
                        }}
                      >
                        {actionLabel}
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label={`Страницы: ${title.toLowerCase()}`} />
        </>
      )}
    </Card>
  )
}

/** Кто кому должен: клиенты нам, мы поставщикам (SPEC-10). */
export default function Debts() {
  const navigate = useNavigate()
  const [paying, setPaying] = useState<Party | null>(null)
  const list = useLoad(() => get<DebtRow[]>('/debts/list'), [])
  const { reload } = list

  useEffect(() => {
    const t = setInterval(reload, REFRESH_MS)
    return () => clearInterval(t)
  }, [reload])

  const rows = useMemo(() => list.data ?? [], [list.data])
  const toUs = useMemo(() => rows.filter((p) => p.balance_tyiyn > 0), [rows])
  const fromUs = useMemo(() => rows.filter((p) => p.balance_tyiyn < 0), [rows])
  const open = (p: Party) => navigate(p.role === 'supplier' ? `/suppliers/${p.id}` : `/clients/${p.id}`)

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Долги" />
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : (
        <>
          <Side
            title="Должны нам"
            pageKey="debts.to_us"
            rows={toUs}
            owedToUs
            empty="Никто не должен. Долг появляется, когда в кассе выбирают оплату «В долг»."
            actionLabel="Погасить"
            onOpen={open}
            onPay={setPaying}
          />
          <Side
            title="Должны мы"
            pageKey="debts.from_us"
            rows={fromUs}
            owedToUs={false}
            empty="Мы никому не должны. Долг появляется, когда приход проводят с расчётом «В долг»."
            actionLabel="Оплатить"
            onOpen={open}
            onPay={setPaying}
          />
        </>
      )}
      {paying && (
        <RepaymentModal
          party={paying}
          onClose={() => setPaying(null)}
          onDone={() => {
            setPaying(null)
            reload()
          }}
        />
      )}
    </div>
  )
}
