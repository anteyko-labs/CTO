import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { balanceText, qtyText as unitQty } from '../lib/defects'
import { formatDateTime, formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { DeliveryReceipt, DeliveryRow, Party, ReceiptListItem, Supplier, SupplierDeliveries } from '../lib/types'
import { ReconciliationButton } from '../components/ReconciliationButton'
import { RepaymentModal } from '../components/RepaymentModal'
import { SupplierEditModal } from '../components/SupplierFormModal'

/** За что последняя цена: канистра масла, килограмм или штука. */
const perText = (r: { unit: string; container_ml: number | null }): string =>
  r.unit === 'ml' && r.container_ml ? ` / ${unitQty(r.container_ml, 'ml')}` : r.unit === 'g' ? ' / кг' : ' / шт'

/** Карточка поставщика: поставки по товарам с остатком (SPEC-26) и накладные (SPEC-10). */
export default function SupplierCard() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const [editing, setEditing] = useState(false)
  const [paying, setPaying] = useState(false)
  const owner = useUser().role === 'owner'
  const [tab, setTab] = useState<'deliveries' | 'receipts'>('deliveries')
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const [opened, setOpened] = useState<DeliveryRow | null>(null)
  const party = useLoad(() => get<Party>(`/parties/${id}`).catch(() => null), [id])
  const supplier = useLoad(() => get<Supplier>(`/suppliers/${id}`), [id])
  const deliveries = useLoad(() => get<SupplierDeliveries>(`/suppliers/${id}/deliveries${qs({ from, to })}`), [id, from, to])
  const receipts = useLoad(() => get<ReceiptListItem[]>(`/receipts${qs({ supplier_id: id })}`), [id])

  const owed = party.data ? -Math.min(party.data.balance_tyiyn, 0) : 0

  const rows = deliveries.data?.rows ?? []
  const docs = receipts.data ?? []
  const last = rows.reduce<string | null>((acc, r) => (r.last_at !== null && (acc === null || r.last_at > acc) ? r.last_at : acc), null)

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
          <div className="text-xs text-slate-500">Привезено на сумму{from || to ? ' за период' : ''}</div>
          <div className="font-medium">{formatSom(deliveries.data?.amount_tyiyn ?? 0)}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Последняя поставка</div>
          <div className="font-medium">{last ? formatDateTime(last) : '—'}</div>
        </div>
        <div>
          <div className="text-xs text-slate-500">Расчёты</div>
          <div className={`font-medium ${owed > 0 ? 'text-rose-700' : ''}`}>{balanceText(party.data?.balance_tyiyn ?? 0)}</div>
        </div>
        {(deliveries.data?.pending_defects ?? 0) > 0 && (
          <div>
            <div className="text-xs text-slate-500">Брак ждёт решения</div>
            <div className="font-medium">
              {deliveries.data?.pending_defects}
              {deliveries.data?.pending_defects_tyiyn != null && ` на ${formatSom(deliveries.data.pending_defects_tyiyn)}`}
            </div>
          </div>
        )}
        {owed > 0 && (
          <Button onClick={() => setPaying(true)}>Оплатить</Button>
        )}
        {!s.active && <Badge tone="rose">не работаем</Badge>}
        {s.comment && <div className="w-full text-slate-600">{s.comment}</div>}
      </Card>

      <div className="flex gap-2" role="tablist">
        {(
          [
            ['deliveries', 'Поставки'],
            ['receipts', 'Накладные'],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`min-h-[38px] rounded-md border px-4 py-2 text-sm font-medium ${tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setTab(key)}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === 'deliveries' && (
        <Card>
          <div className="mb-3 flex flex-wrap items-end gap-3">
            <Field label="С">
              <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
            </Field>
            <Field label="По">
              <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
            </Field>
            {(from || to) && (
              <Button
                variant="secondary"
                onClick={() => {
                  setFrom('')
                  setTo('')
                }}
              >
                За всё время
              </Button>
            )}
          </div>
          <ErrorBox error={deliveries.error} />
          {deliveries.loading && !deliveries.data ? (
            <Loading />
          ) : rows.length === 0 ? (
            <Empty>Поставок за период не было</Empty>
          ) : (
            <>
              <Table
                head={[
                  'Товар',
                  'Привёз',
                  'Вернули (брак)',
                  'Сумма поставок',
                  'Последняя цена',
                  'На складе сейчас',
                  'Из его поставок (оценка)',
                  ...(owner ? ['Себестоимость оценки'] : []),
                ]}
              >
                {rows.map((r) => (
                  <tr key={r.product_id} className="cursor-pointer hover:bg-slate-50" onClick={() => setOpened(r)}>
                    <td className="px-2 py-2 font-medium text-sky-700 underline">{r.name}</td>
                    <td className="whitespace-nowrap px-2 py-2">{unitQty(r.delivered_qty, r.unit)}</td>
                    <td className="whitespace-nowrap px-2 py-2">{r.returned_qty ? unitQty(r.returned_qty, r.unit) : '—'}</td>
                    <td className="whitespace-nowrap px-2 py-2">{formatSom(r.amount_tyiyn)}</td>
                    <td className="whitespace-nowrap px-2 py-2">
                      {r.last_price_tyiyn !== null ? (
                        <>
                          {formatSom(r.last_price_tyiyn)}
                          <span className="text-xs text-slate-500">{perText(r)}</span>
                          {r.last_at && <span className="block text-xs text-slate-500">{formatDateTime(r.last_at)}</span>}
                        </>
                      ) : (
                        '—'
                      )}
                    </td>
                    <td className="whitespace-nowrap px-2 py-2">{unitQty(r.stock_qty, r.unit)}</td>
                    <td className="whitespace-nowrap px-2 py-2">{unitQty(r.estimate_qty, r.unit)}</td>
                    {owner && <td className="whitespace-nowrap px-2 py-2">{formatSom(r.estimate_value_tyiyn ?? 0)}</td>}
                  </tr>
                ))}
              </Table>
              <p className="mt-2 text-xs text-slate-500">
                Склад ведётся по средней цене без партий, поэтому «из его поставок» — оценка: первым продаётся пришедшее раньше.
              </p>
            </>
          )}
        </Card>
      )}

      {tab === 'receipts' && (
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
      )}

      {opened && <ProductReceipts supplierId={id} row={opened} from={from} to={to} onClose={() => setOpened(null)} />}

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

/** Накладные поставщика с одним товаром: дата, номер, количество, цена (SPEC-26). */
function ProductReceipts({ supplierId, row, from, to, onClose }: { supplierId: string; row: DeliveryRow; from: string; to: string; onClose: () => void }) {
  const list = useLoad(() => get<DeliveryReceipt[]>(`/suppliers/${supplierId}/deliveries/${row.product_id}${qs({ from, to })}`), [supplierId, row.product_id, from, to])
  const items = list.data ?? []
  return (
    <Modal title={row.name} wide onClose={onClose}>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : items.length === 0 ? (
        <Empty>Накладных нет</Empty>
      ) : (
        <Table head={['Дата', 'Номер', 'Количество', 'Цена', 'Сумма', '']}>
          {items.map((r) => (
            <tr key={`${r.receipt_id}-${r.qty}`} className={r.reversed ? 'text-slate-400' : ''}>
              <td className="whitespace-nowrap px-2 py-2">{formatDateTime(r.created_at)}</td>
              <td className="px-2 py-2">
                <Link className="text-sky-700 underline" to={`/receipts/${r.receipt_id}`}>
                  № {r.number}
                </Link>
                {r.supplier_doc && <span className="block text-xs text-slate-500">{r.supplier_doc}</span>}
                {r.reversed && <Badge tone="amber">отменена</Badge>}
              </td>
              <td className="whitespace-nowrap px-2 py-2">{unitQty(r.qty, row.unit)}</td>
              <td className="whitespace-nowrap px-2 py-2">
                {r.price_tyiyn !== null ? formatSom(r.price_tyiyn) : '—'}
                <span className="text-xs text-slate-500">{perText(row)}</span>
              </td>
              <td className="whitespace-nowrap px-2 py-2">{formatSom(r.cost_tyiyn)}</td>
              <td />
            </tr>
          ))}
        </Table>
      )}
    </Modal>
  )
}
