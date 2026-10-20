import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { Pager, usePagedLoad, usePager } from '../components/Pager'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { balanceText, qtyText as unitQty } from '../lib/defects'
import { formatDateTime, formatSom } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { DeliveryReceipt, DeliveryRow, InvoiceStatus, Party, ReceiptListItem, Supplier, SupplierDeliveries, SupplierInvoices } from '../lib/types'
import { ReconciliationButton } from '../components/ReconciliationButton'
import { RepaymentModal, type InvoiceToPay } from '../components/RepaymentModal'
import { InvoiceStatusBadge, SupplierPaymentHistory } from '../components/SupplierPayments'
import { correctionMarks, loadCorrections } from '../lib/corrections'
import { SupplierEditModal } from '../components/SupplierFormModal'
import { SupplierReturnList, SupplierReturnModal } from '../components/SupplierReturns'

/** За что последняя цена: канистра масла, килограмм или штука. */
const perText = (r: { unit: string; container_ml: number | null }): string =>
  r.unit === 'ml' && r.container_ml ? ` / ${unitQty(r.container_ml, 'ml')}` : r.unit === 'g' ? ' / кг' : ' / шт'

/** Статус накладной в долг по списку приходов; null — оплачена сразу или отменена. */
function invoiceStatus(d: ReceiptListItem): InvoiceStatus | null {
  const debt = d.on_debt_tyiyn ?? 0
  if (debt <= 0 || d.reversed || d.reversal_of) return null
  const left = d.debt_left_tyiyn ?? 0
  return left <= 0 ? 'paid' : left >= debt ? 'unpaid' : 'partial'
}

/** Карточка поставщика: поставки по товарам с остатком (SPEC-26), накладные с остатком долга и платежи (SPEC-10, SPEC-27), возвраты товара (SPEC-29 §9). */
export default function SupplierCard() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const [editing, setEditing] = useState(false)
  const [paying, setPaying] = useState(false)
  const [payingInvoice, setPayingInvoice] = useState<InvoiceToPay | null>(null)
  const owner = useUser().role === 'owner'
  const [tab, setTab] = useState<'deliveries' | 'receipts' | 'payments' | 'returns'>('deliveries')
  const [returning, setReturning] = useState(false)
  const [returnsKey, setReturnsKey] = useState(0)
  const [from, setFrom] = useState('')
  const [to, setTo] = useState('')
  const [opened, setOpened] = useState<DeliveryRow | null>(null)
  const party = useLoad(() => get<Party>(`/parties/${id}`).catch(() => null), [id])
  const supplier = useLoad(() => get<Supplier>(`/suppliers/${id}`), [id])
  const deliveries = useLoad(() => get<SupplierDeliveries>(`/suppliers/${id}/deliveries${qs({ from, to })}`), [id, from, to])
  // Накладные режет на страницы сервер; поставки — здесь (пришли целиком за период).
  const receipts = usePagedLoad<ReceiptListItem>(`/receipts${qs({ supplier_id: id })}`, 'supplier.receipts')
  const invoices = useLoad(() => get<SupplierInvoices>(`/parties/${id}/invoices`).catch(() => null), [id])
  // «Исправить» платёж поставщику (SPEC-29 §24): то же окно оплаты, заполненное платежом.
  const [fixing, setFixing] = useState<string | null>(null)
  const corrections = useLoad(() => loadCorrections('ledger'), [])
  const marks = correctionMarks(corrections.data ?? [])
  const reloadMoney = () => {
    party.reload()
    receipts.reload()
    invoices.reload()
    deliveries.reload()
  }

  const owed = party.data ? -Math.min(party.data.balance_tyiyn, 0) : 0

  const rows = deliveries.data?.rows ?? []
  const docs = receipts.items
  const delivPager = usePager(rows, 'supplier.deliveries', `${id}:${from}:${to}`)
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
            <Button variant="secondary" onClick={() => setReturning(true)}>
              Возврат поставщику
            </Button>
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
          <div className="font-medium">{receipts.total}</div>
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
        {(invoices.data?.debt_tyiyn ?? 0) > 0 && (
          <div>
            <div className="text-xs text-slate-500">Неоплаченных накладных</div>
            <div className="font-medium">
              {invoices.data?.invoices.filter((i) => i.left_tyiyn > 0).length} на {formatSom(invoices.data?.debt_tyiyn ?? 0)}
            </div>
          </div>
        )}
        {(invoices.data?.advance_tyiyn ?? 0) > 0 && (
          <div>
            <div className="text-xs text-slate-500">Аванс поставщику</div>
            <div className="font-medium">{formatSom(invoices.data?.advance_tyiyn ?? 0)}</div>
          </div>
        )}
        {owed > 0 && (
          <Button onClick={() => setPaying(true)}>Оплатить</Button>
        )}
        {!s.active && <Badge tone="rose">не работаем</Badge>}
        {s.comment && <div className="w-full text-slate-600">{s.comment}</div>}
      </Card>

      <div className="-mx-1 flex gap-2 overflow-x-auto px-1" role="tablist">
        {(
          [
            ['deliveries', 'Поставки'],
            ['receipts', 'Накладные'],
            ['payments', 'Платежи'],
            ['returns', 'Возвраты'],
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
                  'Вернули',
                  'Сумма поставок',
                  'Последняя цена',
                  'На складе сейчас',
                  'Из его поставок (оценка)',
                  ...(owner ? ['Себестоимость оценки'] : []),
                ]}
              >
                {delivPager.shown.map((r) => (
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
              <Pager page={delivPager.page} size={delivPager.size} total={delivPager.total} onPage={delivPager.setPage} onSize={delivPager.setSize} label="Страницы поставок" />
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
        ) : receipts.total === 0 ? (
          <Empty
            action={
              <Button onClick={() => navigate('/receipts/new')}>Новый приход</Button>
            }
          >
            Накладных от этого поставщика нет
          </Empty>
        ) : (
          <Table head={['Номер', 'Дата', 'Документ', 'Сумма', 'Оплачено', 'Осталось', 'Статус', 'Принял', '']}>
            {docs.map((d) => {
              const st = invoiceStatus(d)
              const left = d.debt_left_tyiyn ?? 0
              return (
              <tr key={d.id} className="hover:bg-slate-50">
                <td className="px-2 py-2 font-medium">
                  № {d.number}
                  {d.reversal_of && <Badge tone="rose">сторно</Badge>}
                  {d.reversed && <Badge tone="amber">отменён</Badge>}
                </td>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(d.created_at)}</td>
                <td className="px-2 py-2">{d.supplier_doc || '—'}</td>
                <td className="whitespace-nowrap px-2 py-2">{formatSom(d.total_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2 tabular-nums">
                  {st ? formatSom((d.on_debt_tyiyn ?? 0) - left) : d.reversal_of || d.reversed ? '—' : 'сразу'}
                </td>
                <td className={`whitespace-nowrap px-2 py-2 tabular-nums ${left > 0 && st ? 'font-semibold text-rose-700' : ''}`}>
                  {st ? formatSom(left) : '—'}
                </td>
                <td className="px-2 py-2">{st ? <InvoiceStatusBadge status={st} /> : '—'}</td>
                <td className="px-2 py-2">{d.user_name}</td>
                <td className="px-2 py-2 text-right">
                  <div className="flex items-center justify-end gap-2">
                    {st && left > 0 && (
                      <Button
                        variant="secondary"
                        className="px-2 py-1 text-xs"
                        onClick={() => setPayingInvoice({ receipt_id: d.id, number: d.number, left_tyiyn: left })}
                      >
                        Оплатить
                      </Button>
                    )}
                    <Link className="text-sky-700 underline" to={`/receipts/${d.id}`}>
                      Открыть
                    </Link>
                  </div>
                </td>
              </tr>
              )
            })}
          </Table>
        )}
        <Pager page={receipts.page} size={receipts.size} total={receipts.total} onPage={receipts.setPage} onSize={receipts.setSize} label="Страницы накладных" />
      </Card>
      )}

      {tab === 'payments' && (
        <Card>
          <h2 className="mb-1 font-semibold">История платежей</h2>
          <p className="mb-3 text-xs text-slate-500">
            Оплата без выбора накладной и зачёт брака закрывают самые старые неоплаченные накладные. Сторно платежа возвращает долг на те же накладные.
          </p>
          <ErrorBox error={invoices.error} />
          {invoices.loading && !invoices.data ? <Loading /> : <SupplierPaymentHistory payments={invoices.data?.payments ?? []} marks={marks} onFix={(p) => setFixing(p.ledger_id)} />}
        </Card>
      )}

      {tab === 'returns' && (
        <Card>
          <h2 className="mb-1 font-semibold">Возвраты товара</h2>
          <p className="mb-3 text-xs text-slate-500">Возврат не брака: товар уходит со склада, наш долг меньше или поставщик отдаёт деньги. Брак — в «Остатки» → «Брак».</p>
          <SupplierReturnList key={returnsKey} supplierId={id} onChanged={reloadMoney} />
        </Card>
      )}

      {returning && (
        <SupplierReturnModal
          supplierId={id}
          supplierName={s.name}
          onClose={() => setReturning(false)}
          onDone={() => {
            setReturning(false)
            setReturnsKey((k) => k + 1)
            setTab('returns')
            reloadMoney()
          }}
        />
      )}

      {payingInvoice && party.data && (
        <RepaymentModal
          party={party.data}
          invoice={payingInvoice}
          onClose={() => setPayingInvoice(null)}
          onDone={() => {
            setPayingInvoice(null)
            reloadMoney()
          }}
        />
      )}

      {opened && <ProductReceipts supplierId={id} row={opened} from={from} to={to} onClose={() => setOpened(null)} />}

      {fixing && party.data && (
        <RepaymentModal
          key={fixing}
          party={party.data}
          fixId={fixing}
          onClose={() => setFixing(null)}
          onDone={() => {
            setFixing(null)
            reloadMoney()
            corrections.reload()
          }}
        />
      )}

      {paying && party.data && (
        <RepaymentModal
          party={party.data}
          onClose={() => setPaying(false)}
          onDone={() => {
            setPaying(false)
            reloadMoney()
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
  const pager = usePager(list.data ?? [], 'supplier.product_receipts')
  const items = pager.shown
  return (
    <Modal title={row.name} wide onClose={onClose}>
      <ErrorBox error={list.error} />
      {list.loading && !list.data ? (
        <Loading />
      ) : pager.total === 0 ? (
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
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы накладных товара" />
    </Modal>
  )
}
