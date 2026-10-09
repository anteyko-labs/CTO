// Долги: кто должен нам и кому должны мы, с оплатой и погашением (SPEC-10).
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { RepaymentModal } from '../components/RepaymentModal'
import { Badge, Button, Card, Empty, ErrorBox, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { Party } from '../lib/types'

/** Долги обновляются сами, пока экран открыт (ADR-028). */
const REFRESH_MS = 10_000

function Side({
  title,
  rows,
  total,
  tone,
  empty,
  actionLabel,
  onOpen,
  onPay,
}: {
  title: string
  rows: Party[]
  total: number
  tone: 'amber' | 'rose'
  empty: string
  actionLabel: string
  onOpen: (p: Party) => void
  onPay: (p: Party) => void
}) {
  return (
    <Card>
      <div className="mb-3 flex items-baseline justify-between gap-3">
        <h2 className="font-semibold">{title}</h2>
        <span className={`text-2xl font-bold ${tone === 'amber' ? 'text-amber-700' : 'text-rose-700'}`}>
          {formatSom(total)}
        </span>
      </div>
      {rows.length === 0 ? (
        <Empty>{empty}</Empty>
      ) : (
        <Table head={['Контрагент', 'Телефон', 'Сумма', '']}>
          {rows.map((p) => (
            <tr key={p.id} className="cursor-pointer hover:bg-slate-50" onClick={() => onOpen(p)}>
              <td className="px-2 py-2">
                <div className="font-medium">{p.name}</div>
                <div className="text-xs text-slate-500">
                  {p.kind === 'company' ? 'Юрлицо' : 'Физлицо'}
                  {p.inn && ` · ИНН ${p.inn}`}
                </div>
              </td>
              <td className="whitespace-nowrap px-2 py-2">{p.phone || '—'}</td>
              <td className="whitespace-nowrap px-2 py-2">
                <Badge tone={tone}>{formatSom(Math.abs(p.balance_tyiyn))}</Badge>
                {p.overdue_tyiyn > 0 && (
                  <span className="ml-2">
                    <Badge tone="rose">просрочено {formatSom(p.overdue_tyiyn)}</Badge>
                  </span>
                )}
              </td>
              <td className="px-2 py-2 text-right">
                <Button
                  variant="secondary"
                  className="px-2 py-1 text-xs"
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
            tone="amber"
            empty="Никто не должен. Долг появляется, когда в кассе выбирают оплату «В долг»."
            actionLabel="Погасить"
            onOpen={open}
            onPay={setPaying}
          />
          <Side
            title="Должны мы"
            rows={fromUs}
            total={sum(fromUs)}
            tone="rose"
            empty="Мы никому не должны. Долг появляется, когда приход проводят с расчётом «В долг»."
            actionLabel="Оплатить"
            onOpen={open}
            onPay={setPaying}
          />
          <div className="text-xs text-slate-400">Обновляется каждые 10 секунд</div>
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
