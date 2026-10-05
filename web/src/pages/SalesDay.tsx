import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDateTime, formatSom, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { divRound } from '../lib/money'
import type { SalesDay as Day } from '../lib/types'

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <Card className="p-3">
      <div className="text-xs text-slate-500">{label}</div>
      <div className="text-lg font-semibold">{value}</div>
    </Card>
  )
}

/** Сдвиг даты YYYY-MM-DD на `days` дней. */
function shiftDate(date: string, days: number): string {
  const d = new Date(`${date}T12:00:00Z`)
  d.setUTCDate(d.getUTCDate() + days)
  return d.toISOString().slice(0, 10)
}

export default function SalesDay() {
  const [date, setDate] = useState(todayBishkek)
  const navigate = useNavigate()
  const { data, error, loading } = useLoad(() => get<Day>(`/sales${qs({ date })}`), [date])
  const sold = data?.sales.filter((s) => s.kind === 'sale').reduce((acc, s) => acc + s.total_tyiyn, 0) ?? 0
  const returns = data?.sales.filter((s) => s.kind === 'return') ?? []
  const returnsSum = returns.reduce((acc, s) => acc + s.total_tyiyn, 0)

  return (
    <div>
      <PageHeader
        title="Чеки за день"
        actions={
          <div className="flex items-center gap-1">
            <Button variant="secondary" aria-label="Предыдущий день" onClick={() => setDate((d) => shiftDate(d, -1))}>
              ←
            </Button>
            <input type="date" value={date} max={todayBishkek()} onChange={(e) => e.target.value && setDate(e.target.value)} />
            <Button
              variant="secondary"
              aria-label="Следующий день"
              disabled={date >= todayBishkek()}
              onClick={() => setDate((d) => shiftDate(d, 1))}
            >
              →
            </Button>
            {date !== todayBishkek() && (
              <Button variant="ghost" onClick={() => setDate(todayBishkek())}>
                Сегодня
              </Button>
            )}
          </div>
        }
      />
      <ErrorBox error={error} />
      {data && (
        <div className="mb-4 grid grid-cols-2 gap-3 md:grid-cols-4 xl:grid-cols-7">
          <Stat label="Выручка" value={formatSom(data.totals.total_tyiyn)} />
          <Stat label="Чеков" value={String(data.totals.count)} />
          <Stat label="Средний чек" value={data.totals.count ? formatSom(divRound(sold, data.totals.count)) : '—'} />
          <Stat label="Возвраты" value={returns.length ? `${returns.length} · ${formatSom(returnsSum)}` : '—'} />
          <Stat label="Наличные" value={formatSom(data.totals.cash_tyiyn)} />
          <Stat label="Карта" value={formatSom(data.totals.card_tyiyn)} />
          <Stat label="Перевод" value={formatSom(data.totals.transfer_tyiyn)} />
        </div>
      )}
      <Card>
        {loading && !data ? (
          <Loading />
        ) : !data || data.sales.length === 0 ? (
          <Empty>За этот день чеков нет</Empty>
        ) : (
          <Table head={['№', 'Время', 'Тип', 'Кассир', 'Мастер', 'Сумма']}>
            {data.sales.map((s) => (
              <tr key={s.id} className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(`/sales/${s.id}`)}>
                <td className="px-2 py-2 font-medium">{s.number}</td>
                <td className="px-2 py-2">{formatDateTime(s.created_at)}</td>
                <td className="px-2 py-2">
                  {s.kind === 'return' ? <Badge tone="rose">возврат</Badge> : s.sale_type === 'service' ? 'В сервис' : 'На вынос'}
                </td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="px-2 py-2">{s.master_name ?? '—'}</td>
                <td className={`px-2 py-2 text-right font-medium ${s.total_tyiyn < 0 ? 'text-rose-700' : ''}`}>{formatSom(s.total_tyiyn)}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>
    </div>
  )
}
