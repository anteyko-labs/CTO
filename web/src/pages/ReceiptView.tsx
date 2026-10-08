// Просмотр прихода, сторно и печать этикеток (SPEC-03).
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { printLabels } from '../components/labels'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatDateTime, formatLiters, formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
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
  const receipt = useLoad(() => get<Receipt>(`/receipts/${id}`), [id])
  const [reverseOpen, setReverseOpen] = useState(false)
  const [reverseComment, setReverseComment] = useState('')
  const [opId] = useState(newOpId)
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
              <Button variant="danger" onClick={() => setReverseOpen(true)}>
                Сторно
              </Button>
            )}
          </>
        }
      />
      <ErrorBox error={receipt.error} />
      <ErrorBox error={labels.error} />
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
      <Card>
        <Table head={['№', 'Товар', 'Количество', 'Сумма']}>
          {r.lines.map((l) => (
            <tr key={l.line_no}>
              <td className="px-2 py-2">{l.line_no}</td>
              <td className="px-2 py-2">{l.product_name}</td>
              <td className="whitespace-nowrap px-2 py-2">{qtyText(l)}</td>
              <td className="whitespace-nowrap px-2 py-2 text-right">{formatSom(l.cost_tyiyn)}</td>
            </tr>
          ))}
        </Table>
        <div className="mt-3 border-t border-slate-200 pt-3 text-right text-lg font-semibold">
          Итого: {formatSom(r.total_tyiyn)}
        </div>
      </Card>

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
