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
          <Empty icon="incoming" action={<Button onClick={() => navigate('/receipts/new')}>Новый приход</Button>}>
            Приходов за период нет
          </Empty>
        ) : (
          list.data && (
            <>
              <ul className="-mx-4 divide-y divide-slate-100 border-y border-slate-100 md:hidden">
                {list.data.map((r) => (
                  <li key={r.id}>
                    <button type="button" className="flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-slate-50" onClick={() => navigate(`/receipts/${r.id}`)}>
                      <div className="min-w-0 flex-1">
                        <div className="truncate font-semibold">
                          № {r.number} · {r.supplier_name ?? 'без поставщика'}
                        </div>
                        <div className="mt-0.5 truncate text-xs text-slate-500">
                          {formatDateTime(r.created_at)}
                          {r.supplier_doc && ` · док. ${r.supplier_doc}`} · {r.user_name}
                        </div>
                        <div className="mt-1 flex gap-1">
                          {r.reversal_of && <Badge tone="amber">сторно</Badge>}
                          {r.reversed && <Badge tone="rose">сторнирован</Badge>}
                          {!r.reversal_of && !r.reversed && ((r.on_debt_tyiyn ?? 0) > 0 ? <Badge tone="amber">в долг поставщику</Badge> : <Badge tone="green">оплачен</Badge>)}
                        </div>
                      </div>
                      <div className="shrink-0 text-right font-bold tabular-nums">{formatSom(r.total_tyiyn)}</div>
                    </button>
                  </li>
                ))}
              </ul>
              <div className="hidden md:block">
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
                        {!r.reversal_of && !r.reversed && ((r.on_debt_tyiyn ?? 0) > 0 ? <Badge tone="amber">в долг поставщику</Badge> : <Badge tone="green">оплачен</Badge>)}
                      </td>
                    </tr>
                  ))}
                </Table>
              </div>
            </>
          )
        )}
      </Card>
    </div>
  )
}
