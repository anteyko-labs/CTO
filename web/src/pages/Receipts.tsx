// Список приходов за период (SPEC-03).
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDateTime, formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { ReceiptListItem } from '../lib/types'

export default function Receipts() {
  const navigate = useNavigate()
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -30))
  const list = useLoad(() => get<ReceiptListItem[]>(`/receipts${qs({ from, to })}`), [from, to])

  return (
    <div>
      <PageHeader title="Приход" actions={<Button onClick={() => navigate('/receipts/new')}>Новый приход</Button>} />
      <Card>
        <div className="mb-4 grid grid-cols-2 gap-3 sm:max-w-md">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
        </div>
        <ErrorBox error={list.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : list.data && list.data.length === 0 ? (
          <Empty>Приходов за период нет</Empty>
        ) : (
          list.data && (
            <Table head={['№', 'Дата', 'Поставщик', 'Сумма', 'Принял', '']}>
              {list.data.map((r) => (
                <tr key={r.id} className="cursor-pointer hover:bg-slate-50" onClick={() => navigate(`/receipts/${r.id}`)}>
                  <td className="px-2 py-2 font-medium">{r.number}</td>
                  <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.created_at)}</td>
                  <td className="px-2 py-2">
                    {r.supplier_name ?? '—'}
                    {r.supplier_doc && <span className="ml-2 text-slate-500">док. {r.supplier_doc}</span>}
                  </td>
                  <td className="whitespace-nowrap px-2 py-2 text-right">{formatSom(r.total_tyiyn)}</td>
                  <td className="px-2 py-2">{r.user_name}</td>
                  <td className="px-2 py-2">
                    {r.reversal_of && <Badge tone="amber">сторно</Badge>}
                    {r.reversed && <Badge tone="rose">сторнирован</Badge>}
                  </td>
                </tr>
              ))}
            </Table>
          )
        )}
      </Card>
    </div>
  )
}
