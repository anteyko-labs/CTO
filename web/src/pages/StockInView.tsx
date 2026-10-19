// Просмотр оприходования без поставщика (SPEC-29 §8, ADR-086): строки, комментарий, кто и когда;
// сторно всего документа с причиной, «Исправить» у владельца, печать этикеток.
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CorrectionLinks, FixButton } from '../components/Correction'
import { printLabels } from '../components/labels'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatDateTime, formatKg, formatLiters, formatPermille, formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { markupPermille } from '../lib/money'
import type { Product, StockIn, StockInLine } from '../lib/types'

/** Количество строки: штуки, литры масла (и канистры, если целые), килограммы. */
function qtyText(l: StockInLine): string {
  if (l.unit === 'ml') {
    const cans = l.container_ml && l.qty % l.container_ml === 0 ? ` (${Math.abs(l.qty / l.container_ml)} кан.)` : ''
    return `${formatLiters(l.qty)}${cans}`
  }
  return l.unit === 'g' ? formatKg(l.qty) : `${l.qty} шт`
}

/** Единица цены строки словами. */
const perText = (l: StockInLine): string => (l.unit === 'ml' && l.container_ml ? `за канистру ${formatLiters(l.container_ml)}` : l.unit === 'g' ? 'за кг' : 'за шт')

export default function StockInView() {
  const { id = '' } = useParams()
  const navigate = useNavigate()
  const doc = useLoad(() => get<StockIn>(`/stock-ins/${id}`), [id])
  const [reverseOpen, setReverseOpen] = useState(false)
  const [reason, setReason] = useState('')
  const [opId] = useState(newOpId)
  const reverse = useAction()
  const labels = useAction()
  const d = doc.data

  if (doc.loading && !d) return <Loading />
  if (!d) return <ErrorBox error={doc.error ?? 'Документ не найден'} />

  const isReversal = d.reversal_of !== null
  const canReverse = !isReversal && d.reversed_by === null

  const doReverse = () =>
    void reverse.run(async () => {
      const out = await post<StockIn>(`/stock-ins/${id}/reverse`, { op_id: opId, comment: reason.trim() })
      setReverseOpen(false)
      navigate(`/stock-ins/${out.id}`)
    })

  const doPrint = () =>
    void labels.run(async () => {
      const items: { product: Product; copies: number }[] = []
      for (const l of d.lines) {
        const copies = l.unit === 'piece' ? l.qty : l.unit === 'ml' && l.container_ml ? Math.trunc(l.qty / l.container_ml) : 0
        if (copies <= 0) continue
        items.push({ product: await get<Product>(`/products/${l.product_id}`), copies })
      }
      const n = await printLabels(items)
      if (n === 0) throw new Error('Нет этикеток для печати: у товаров нет штрихкодов')
    })

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={`${isReversal ? 'Сторно оприходования' : 'Оприходование'} № ${d.number}`}
        subtitle="Без поставщика: без накладной и без денег"
        actions={
          <>
            <Button variant="secondary" onClick={() => navigate('/receipts?kind=stock_in')}>
              К списку
            </Button>
            {!isReversal && (
              <Button variant="secondary" disabled={labels.busy} onClick={doPrint}>
                Печать этикеток
              </Button>
            )}
            {canReverse && <FixButton onClick={() => navigate(`/stock-ins/${d.id}/correct`)} />}
            {canReverse && (
              <Button variant="danger" onClick={() => setReverseOpen(true)}>
                Сторно
              </Button>
            )}
          </>
        }
      />
      <ErrorBox error={labels.error} />
      <CorrectionLinks id={d.id} hrefOf={(x) => `/stock-ins/${x}`} />
      <Card>
        <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-sm sm:grid-cols-2">
          <div>
            <dt className="text-slate-500">Дата</dt>
            <dd>{formatDateTime(d.created_at)}</dd>
          </div>
          <div>
            <dt className="text-slate-500">Провёл</dt>
            <dd>{d.user_name}</dd>
          </div>
          <div className="sm:col-span-2">
            <dt className="text-slate-500">{isReversal ? 'Причина сторно' : 'Откуда товар'}</dt>
            <dd data-testid="stock-in-comment">{d.comment}</dd>
          </div>
          {d.reversal_of && (
            <div className="sm:col-span-2">
              <Badge tone="amber">сторно</Badge>{' '}
              <Link className="text-sky-700 underline" to={`/stock-ins/${d.reversal_of}`}>
                Исходное оприходование
              </Link>
            </div>
          )}
          {d.reversed_by && (
            <div className="sm:col-span-2">
              <Badge tone="rose">сторнировано</Badge>{' '}
              <Link className="text-sky-700 underline" to={`/stock-ins/${d.reversed_by}`}>
                Документ сторно
              </Link>
            </div>
          )}
        </dl>
      </Card>
      <Card>
        <Table head={['№', 'Товар', 'Количество', 'Цена закупки', 'Наценка', 'Цена продажи', 'Сумма']}>
          {d.lines.map((l) => {
            const sale = l.sale_price_tyiyn
            const markup = sale !== null ? markupPermille(l.price_tyiyn, sale) : null
            return (
              <tr key={l.line_no} data-testid="stock-in-line">
                <td className="px-2 py-2">{l.line_no}</td>
                <td className="px-2 py-2">{l.product_name}</td>
                <td className="whitespace-nowrap px-2 py-2">{qtyText(l)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">
                  {formatSom(l.price_tyiyn)} <span className="text-xs text-slate-500">{perText(l)}</span>
                </td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{markup === null ? '—' : formatPermille(markup)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{sale === null ? '—' : formatSom(sale)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(l.cost_tyiyn)}</td>
              </tr>
            )
          })}
        </Table>
        <div className="mt-3 border-t border-slate-200 pt-3 text-right text-lg font-semibold" data-testid="stock-in-total">
          Итого: {formatSom(d.cost_tyiyn)}
        </div>
      </Card>

      {reverseOpen && (
        <Modal title={`Сторно оприходования № ${d.number}`} onClose={() => setReverseOpen(false)}>
          <div className="flex flex-col gap-3">
            <p className="text-sm text-slate-600">
              Товар всех строк уйдёт со склада по той же стоимости. Если его уже продали и остаток уйдёт в минус — сторнирует только владелец.
            </p>
            <Field label="Причина" required>
              <textarea autoFocus rows={3} value={reason} onChange={(e) => setReason(e.target.value)} />
            </Field>
            <ErrorBox error={reverse.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setReverseOpen(false)}>
                Отмена
              </Button>
              <Button variant="danger" disabled={reverse.busy || !reason.trim()} onClick={doReverse}>
                Сторнировать
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
