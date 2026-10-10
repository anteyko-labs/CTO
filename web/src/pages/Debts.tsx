// Долги: кто должен нам и кому должны мы, с оплатой и погашением (SPEC-10).
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { RepaymentModal } from '../components/RepaymentModal'
import { Button, Card, Empty, ErrorBox, Loading, Money, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { Party } from '../lib/types'

/** Долги обновляются сами, пока экран открыт (ADR-028). */
const REFRESH_MS = 10_000

/** Клиента без имени касса записывает по телефону или ИНН — такое имя показываем как «Без имени». */
const named = (p: Party) => {
  const name = p.name.trim()
  return name !== '' && name !== p.inn && name !== p.phone && !/^\+?[\d\s()-]+$/.test(name)
}

function Side({
  title,
  rows,
  total,
  owedToUs,
  empty,
  actionLabel,
  onOpen,
  onPay,
}: {
  title: string
  rows: Party[]
  total: number
  /** Нам должны — янтарный; мы должны — нейтральный; просрочка — красный. */
  owedToUs: boolean
  empty: string
  actionLabel: string
  onOpen: (p: Party) => void
  onPay: (p: Party) => void
}) {
  const overdue = rows.reduce((a, p) => a + p.overdue_tyiyn, 0)
  // Крупные долги сверху.
  const sorted = [...rows].sort((a, b) => Math.abs(b.balance_tyiyn) - Math.abs(a.balance_tyiyn))
  return (
    <Card>
      <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
        <h2 className="font-semibold">{title}</h2>
        <div className="text-right">
          <div className={`text-2xl font-bold tabular-nums ${owedToUs ? 'text-amber-700' : 'text-slate-900'}`}>{formatSom(total)}</div>
          {overdue > 0 && <div className="text-xs font-medium text-rose-700">просрочено {formatSom(overdue)}</div>}
        </div>
      </div>
      {rows.length === 0 ? (
        <Empty>{empty}</Empty>
      ) : (
        <Table head={['Контрагент', 'Телефон', 'Долг', 'Просрочено', '']}>
          {sorted.map((p) => (
            <tr key={p.id} className="cursor-pointer hover:bg-slate-50" onClick={() => onOpen(p)}>
              <td className="px-2 py-2">
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
              <td className="whitespace-nowrap px-2 py-2">{p.phone || '—'}</td>
              <td className="px-2 py-2 text-right">
                <Money value={Math.abs(p.balance_tyiyn)} className={`font-semibold ${owedToUs ? 'text-amber-700' : ''}`} />
              </td>
              <td className="px-2 py-2 text-right">
                {p.overdue_tyiyn > 0 && <Money value={p.overdue_tyiyn} className="font-medium text-rose-700" />}
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
        </Table>
      )}
    </Card>
  )
}

/** Кто кому должен: клиенты нам, мы поставщикам (SPEC-10). */
export default function Debts() {
  const navigate = useNavigate()
  const [paying, setPaying] = useState<Party | null>(null)
  const list = useLoad(() => get<Party[]>(`/parties${qs({ only_debtors: true })}`), [])
  const { reload } = list

  useEffect(() => {
    const t = setInterval(reload, REFRESH_MS)
    return () => clearInterval(t)
  }, [reload])

  const rows = list.data ?? []
  const toUs = rows.filter((p) => p.balance_tyiyn > 0)
  const fromUs = rows.filter((p) => p.balance_tyiyn < 0)
  const sum = (list: Party[]) => list.reduce((a, p) => a + Math.abs(p.balance_tyiyn), 0)
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
            rows={toUs}
            total={sum(toUs)}
            owedToUs
            empty="Никто не должен. Долг появляется, когда в кассе выбирают оплату «В долг»."
            actionLabel="Погасить"
            onOpen={open}
            onPay={setPaying}
          />
          <Side
            title="Должны мы"
            rows={fromUs}
            total={sum(fromUs)}
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
