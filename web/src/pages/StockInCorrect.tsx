// «Исправить» оприходование (SPEC-29 §24, ADR-086): экран «Новый приход» в виде оприходования,
// заполненный исходным документом. Сохранение — сторно исходного и верный документ одной операцией.
import { Link, useParams } from 'react-router-dom'
import { ErrorBox, Loading } from '../components/ui'
import { get, randomId } from '../lib/api'
import { loadCorrectionDraft } from '../lib/corrections'
import { useLoad } from '../lib/hooks'
import { stockInFixLines, type StockInDraftDoc } from '../lib/stockIn'
import type { Product } from '../lib/types'
import ReceiptNew, { type ReceiptFix } from './ReceiptNew'

async function loadFix(id: string): Promise<ReceiptFix> {
  const draft = await loadCorrectionDraft<StockInDraftDoc>('stock_in', id)
  const ids = [...new Set(draft.doc.lines.map((l) => l.product_id))]
  const products = await Promise.all(ids.map((p) => get<Product>(`/products/${p}`)))
  const byId = new Map(products.filter((p): p is Product => p !== null).map((p) => [p.id, p]))
  return {
    kind: 'stock_in',
    originalId: id,
    number: draft.number,
    state: {
      supplierId: '',
      supplierDoc: '',
      comment: draft.doc.comment,
      payMode: 'paid',
      partText: '',
      paidFrom: '',
      lines: stockInFixLines(draft.doc, byId, () => randomId()),
      rounded: [],
    },
  }
}

export default function StockInCorrect() {
  const { id = '' } = useParams()
  const fix = useLoad(() => loadFix(id), [id])
  if (fix.loading && !fix.data) return <Loading />
  if (!fix.data)
    return (
      <div className="flex flex-col gap-3">
        <ErrorBox error={fix.error} />
        <Link to={`/stock-ins/${id}`} className="text-sm text-sky-700 hover:underline">
          ← К оприходованию
        </Link>
      </div>
    )
  return <ReceiptNew key={id} fix={fix.data} />
}
