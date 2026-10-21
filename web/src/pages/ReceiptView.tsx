// Просмотр прихода, сторно и печать этикеток (SPEC-03); оплата накладной в долг по частям и история
// платежей (SPEC-27); фото накладной (SPEC-29 §3).
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CorrectionLinks, FixButton } from '../components/Correction'
import { printLabels } from '../components/labels'
import { ReceiptPhotos } from '../components/ReceiptPhotos'
import { RepaymentModal } from '../components/RepaymentModal'
import { InvoicePayments, InvoiceStatusBadge } from '../components/SupplierPayments'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatDateTime, formatLiters, formatPermille, formatSom } from '../lib/format'
import { markupPermille } from '../lib/money'
import { unitCost } from '../lib/receiptLines'
import { useAction, useLoad } from '../lib/hooks'
import { paidFromText } from '../lib/receiptPay'
import { useCan } from '../lib/permissions'
import type { Product, Receipt, ReceiptLine } from '../lib/types'

/** Количество строки: канистры для масла, штуки для штучного товара. */
function qtyText(l: ReceiptLine): string {
  if (l.unit === 'ml') {
    if (l.container_ml && l.qty % l.container_ml === 0) {
      return `${l.qty / l.container_ml} кан. по ${formatLiters(l.container_ml)}`
    }
    return formatLiters(l.qty)
  }
  return `${l.qty} шт`
}

/** Число этикеток по строке: штуки или целые канистры. */
function labelCopies(l: ReceiptLine): number {
  if (l.unit === 'ml') return l.container_ml ? Math.trunc(l.qty / l.container_ml) : 0
  return l.qty
}

export default function ReceiptView() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const can = useCan()
  const receipt = useLoad(() => get<Receipt>(`/receipts/${id}`), [id])
  const [reverseOpen, setReverseOpen] = useState(false)
  const [reverseComment, setReverseComment] = useState('')
  const [opId] = useState(newOpId)
  const [paying, setPaying] = useState(false)
  const reverse = useAction()
  const labels = useAction()

  const r = receipt.data

  const doReverse = () => {
    const text = reverseComment.trim()
    if (!text) {
      reverse.setError('Укажите причину сторно')
      return
    }
    void reverse.run(async () => {
      const out = await post<Receipt>(`/receipts/${id}/reverse`, { op_id: opId, comment: text })
      setReverseOpen(false)
      navigate(`/receipts/${out.id}`)
    })
  }

  const doPrint = () => {
    if (!r) return
    void labels.run(async () => {
      const items: { product: Product; copies: number }[] = []
      for (const l of r.lines) {
        const copies = labelCopies(l)
        if (copies <= 0) continue
        items.push({ product: await get<Product>(`/products/${l.product_id}`), copies })
      }
      const n = await printLabels(items)
      if (n === 0) throw new Error('Нет этикеток для печати: у товаров нет штрихкодов')
    })
  }

  if (receipt.loading && !r) return <Loading />
  if (!r) return <ErrorBox error={receipt.error ?? 'Документ не найден'} />

  const isReversal = r.reversal_of !== null
  const canReverse = !isReversal && r.reversed_by === null
  // Сторно, оплата и фото накладной — право администратора «Приход» (ADR-097).
  const mayReceipts = can('receipts')

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={`${isReversal ? 'Сторно прихода' : 'Приход'} № ${r.number}`}
        actions={
          <>
            <Button variant="secondary" onClick={() => navigate('/receipts')}>
              К списку
            </Button>
            {!isReversal && (
              <Button variant="secondary" disabled={labels.busy} onClick={doPrint}>
                Печать этикеток
              </Button>
            )}
            {canReverse && (
              // Исправить накладную: сторно и верная одним действием (SPEC-29 §24).
              <FixButton onClick={() => navigate(`/receipts/${r.id}/correct`)} />
            )}
            {canReverse && mayReceipts && (
              <Button variant="danger" onClick={() => setReverseOpen(true)}>
                Сторно
              </Button>
            )}
          </>
        }
      />
      <ErrorBox error={receipt.error} />
      <ErrorBox error={labels.error} />
      <CorrectionLinks id={r.id} hrefOf={(doc) => `/receipts/${doc}`} />
      <Card>
        <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-sm sm:grid-cols-2">
          <div>
            <dt className="text-slate-500">Дата</dt>
            <dd>{formatDateTime(r.created_at)}</dd>
          </div>
          <div>
            <dt className="text-slate-500">Принял</dt>
            <dd>{r.user_name}</dd>
          </div>
          <div>
            <dt className="text-slate-500">Поставщик</dt>
            <dd>{r.supplier_name ?? '—'}</dd>
          </div>
          <div>
            <dt className="text-slate-500">Документ поставщика</dt>
            <dd>{r.supplier_doc || '—'}</dd>
          </div>
          {r.paid_from && (
            <div>
              <dt className="text-slate-500">Оплачено</dt>
              <dd>{paidFromText(r.paid_from)}</dd>
            </div>
          )}
          {r.comment && (
            <div className="sm:col-span-2">
              <dt className="text-slate-500">Комментарий</dt>
              <dd>{r.comment}</dd>
            </div>
          )}
          {r.reversal_of && (
            <div className="sm:col-span-2">
              <Badge tone="amber">сторно</Badge>{' '}
              <Link className="text-sky-700 underline" to={`/receipts/${r.reversal_of}`}>
                Исходный приход
              </Link>
            </div>
          )}
          {r.reversed_by && (
            <div className="sm:col-span-2">
              <Badge tone="rose">сторнирован</Badge>{' '}
              <Link className="text-sky-700 underline" to={`/receipts/${r.reversed_by}`}>
                Документ сторно
              </Link>
            </div>
          )}
        </dl>
      </Card>
      {r.debt && (
        <Card>
          <div className="mb-3 flex flex-wrap items-center gap-x-8 gap-y-2 text-sm">
            <h2 className="font-semibold">Оплата поставщику</h2>
            <InvoiceStatusBadge status={r.debt.status} />
            <div>
              <span className="text-slate-500">В долг: </span>
              <span className="font-medium tabular-nums">{formatSom(r.debt.total_tyiyn)}</span>
            </div>
            <div>
              <span className="text-slate-500">Оплачено: </span>
              <span className="font-medium tabular-nums" data-testid="invoice-paid">{formatSom(r.debt.paid_tyiyn)}</span>
            </div>
            <div>
              <span className="text-slate-500">Осталось: </span>
              <span className={`font-medium tabular-nums ${r.debt.left_tyiyn > 0 ? 'text-rose-700' : ''}`} data-testid="invoice-left">
                {formatSom(r.debt.left_tyiyn)}
              </span>
            </div>
            {r.debt.left_tyiyn > 0 && r.supplier_id && mayReceipts && <Button onClick={() => setPaying(true)}>Оплатить</Button>}
          </div>
          <InvoicePayments payments={r.debt.payments} />
        </Card>
      )}
      <Card>
        {/* Как в старой программе: цена по накладной, наценка, цена продажи, сумма (SPEC-28 §1). */}
        <Table head={['№', 'Товар', 'Количество', 'Цена по накладной', 'Наценка', 'Цена продажи', 'Сумма']}>
          {r.lines.map((l) => {
            const cost = unitCost(l)
            const sale = l.sale_price_tyiyn ?? null
            const markup = cost !== null && sale !== null ? markupPermille(cost, sale) : null
            return (
              <tr key={l.line_no}>
                <td className="px-2 py-2">{l.line_no}</td>
                <td className="px-2 py-2">{l.product_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{qtyText(l)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{cost === null ? '—' : formatSom(cost)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{markup === null ? '—' : formatPermille(markup)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{sale === null ? '—' : formatSom(sale)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(l.cost_tyiyn)}</td>
              </tr>
            )
          })}
        </Table>
        <div className="mt-3 border-t border-slate-200 pt-3 text-right text-lg font-semibold">
          Итого: {formatSom(r.total_tyiyn)}
        </div>
      </Card>

      {!isReversal && (
        <Card>
          <ReceiptPhotos receiptId={r.id} canAdd={mayReceipts} />
        </Card>
      )}

      {paying && r.debt && r.supplier_id && (
        <RepaymentModal
          party={{ id: r.supplier_id, name: r.supplier_name ?? '', balance_tyiyn: -r.debt.left_tyiyn, role: 'supplier' }}
          invoice={{ receipt_id: r.id, number: r.number, left_tyiyn: r.debt.left_tyiyn }}
          onClose={() => setPaying(false)}
          onDone={() => {
            setPaying(false)
            receipt.reload()
          }}
        />
      )}

      {reverseOpen && (
        <Modal title={`Сторно прихода № ${r.number}`} onClose={() => setReverseOpen(false)}>
          <div className="flex flex-col gap-3">
            <p className="text-sm text-slate-600">
              Остатки по всем строкам уменьшатся на пришедшее количество. Отменить сторно нельзя.
            </p>
            <Field label="Причина">
              <textarea autoFocus rows={3} value={reverseComment} onChange={(e) => setReverseComment(e.target.value)} />
            </Field>
            <ErrorBox error={reverse.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setReverseOpen(false)}>
                Отмена
              </Button>
              <Button variant="danger" disabled={reverse.busy || !reverseComment.trim()} onClick={doReverse}>
                Сторнировать
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
