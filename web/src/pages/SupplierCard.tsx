import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatDateTime, formatLiters, formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { Party, ReceiptListItem, Supplier, SupplierSupply } from '../lib/types'
import { ReconciliationButton } from '../components/ReconciliationButton'
import { RepaymentModal } from '../components/RepaymentModal'
import { SupplierEditModal } from '../components/SupplierFormModal'

const qtyText = (s: SupplierSupply): string =>
  s.unit === 'ml' && s.container_ml ? `${s.qty} кан. · ${formatLiters(s.qty * s.container_ml)}` : `${s.qty} шт`

/** Карточка поставщика: что привозил, по каким ценам и какими накладными (SPEC-10). */
export default function SupplierCard() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const [editing, setEditing] = useState(false)
  const [paying, setPaying] = useState(false)
  const party = useLoad(() => get<Party>(`/parties/${id}`).catch(() => null), [id])
  const supplier = useLoad(() => get<Supplier>(`/suppliers/${id}`), [id])
  const supplies = useLoad(() => get<SupplierSupply[]>(`/suppliers/${id}/supplies`), [id])
  const receipts = useLoad(() => get<ReceiptListItem[]>(`/receipts${qs({ supplier_id: id })}`), [id])

  const owed = party.data ? -Math.min(party.data.balance_tyiyn, 0) : 0

  const rows = supplies.data ?? []
  const docs = receipts.data ?? []
  const total = rows.reduce((acc, r) => acc + r.amount_tyiyn, 0)
  const last = rows.reduce<string | null>((acc, r) => (acc === null || r.last_at > acc ? r.last_at : acc), null)

  if (supplier.loading && !supplier.data) return <Loading />
  if (!supplier.data) return <ErrorBox error={supplier.error ?? 'Поставщик не найден'} />
  const s = supplier.data

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={s.name}
        actions={
          <>
            <Button variant="secondary" onClick={() => navigate('/suppliers')}>
              К списку
            </Button>
            <ReconciliationButton partyId={id} />
            <Button onClick={() => setEditing(true)}>Изменить</Button>
          </>
        }
      />

      <Card className="flex flex-wrap items-center gap-x-8 gap-y-2 text-sm">
        <div>
          <div className="text-xs text-slate-500">Телефон</div>
          <div className="font-medium">{s.phone || '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Поставок</div>
          <div className="font-medium">{docs.length}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Привезено на сумму</div>
          <div className="font-medium">{formatSom(total)}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Последняя поставка</div>
          <div className="font-medium">{last ? formatDateTime(last) : '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Должны ему</div>
          <div className={`font-medium ${owed > 0 ? 'text-rose-700' : ''}`}>{formatSom(owed)}</div>
        </div>
        {owed > 0 && (
          <Button onClick={() => setPaying(true)}>Оплатить</Button>
        )}
        {!s.active && <Badge tone="rose">не работаем</Badge>}
        {s.comment && <div className="w-full text-slate-600">{s.comment}</div>}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Что привозил</h2>
        <ErrorBox error={supplies.error} />
        {supplies.loading && !supplies.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty>Поставок пока не было</Empty>
        ) : (
          <Table head={['Товар', 'Поставок', 'Количество', 'Сумма', 'Последняя цена', 'Когда']}>
            {rows.map((r) => (
              <tr key={r.product_id} className="hover:bg-slate-50">
                <td className="px-2 py-2 font-medium">{r.name}</td>
                <td className="px-2 py-2">{r.receipts}</td>
                <td className="whitespace-nowrap px-2 py-2">{qtyText(r)}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(r.amount_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2">
                  {formatSom(r.last_price_tyiyn)}
                  <span className="text-xs text-slate-500">{r.unit === 'ml' ? ' / кан.' : ' / шт'}</span>
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.last_at)}</td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Накладные</h2>
        <ErrorBox error={receipts.error} />
        {receipts.loading && !receipts.data ? (
          <Loading />
        ) : docs.length === 0 ? (
          <Empty
            action={
              <Button onClick={() => navigate('/receipts/new')}>Новый приход</Button>
            }
          >
            Накладных от этого поставщика нет
          </Empty>
        ) : (
          <Table head={['Номер', 'Дата', 'Документ', 'Сумма', 'Принял', '']}>
            {docs.map((d) => (
              <tr key={d.id} className="hover:bg-slate-50">
                <td className="px-2 py-2 font-medium">
                  № {d.number}
                  {d.reversal_of && <Badge tone="rose">сторно</Badge>}
                  {d.reversed && <Badge tone="amber">отменён</Badge>}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(d.created_at)}</td>
                <td className="px-2 py-2">{d.supplier_doc || '—'}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(d.total_tyiyn)}</td>
                <td className="px-2 py-2">{d.user_name}</td>
                <td className="px-2 py-2 text-right">
                  <Link className="text-sky-700 underline" to={`/receipts/${d.id}`}>
                    Открыть
                  </Link>
                </td>
              </tr>
            ))}
          </Table>
        )}
      </Card>

      {paying && party.data && (
        <RepaymentModal
          party={party.data}
          onClose={() => setPaying(false)}
          onDone={() => {
            setPaying(false)
            party.reload()
          }}
        />
      )}
      {editing && (
        <SupplierEditModal
          supplier={s}
          onClose={() => setEditing(false)}
          onSaved={() => {
            setEditing(false)
            supplier.reload()
          }}
        />
      )}
    </div>
  )
}
