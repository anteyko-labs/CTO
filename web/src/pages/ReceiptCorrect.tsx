// «Исправить» приход (SPEC-29 §24): экран «Новый приход», заполненный исходной накладной.
// Сохранение — сторно исходной и верная накладная одной операцией; после — переход к новой.
import { Link, useParams } from 'react-router-dom'
import { ErrorBox, Loading } from '../components/ui'
import { get } from '../lib/api'
import { loadCorrectionDraft } from '../lib/corrections'
import { useLoad } from '../lib/hooks'
import { receiptFixState, type ReceiptDraftDoc } from '../lib/receiptFix'
import type { Product } from '../lib/types'
import ReceiptNew from './ReceiptNew'

async function loadFix(id: string) {
  const draft = await loadCorrectionDraft<ReceiptDraftDoc>('receipt', id)
  const ids = [...new Set(draft.doc.lines.map((l) => l.product_id))]
  const products = await Promise.all(ids.map((p) => get<Product>(`/products/${p}`).catch(() => null)))
  const byId = new Map(products.filter((p): p is Product => p !== null).map((p) => [p.id, p]))
  return { originalId: id, number: draft.number, state: receiptFixState(draft.doc, byId, () => crypto.randomUUID()) }
}

export default function ReceiptCorrect() {
  const { id = '' } = useParams()
  const fix = useLoad(() => loadFix(id), [id])
  if (fix.loading && !fix.data) return <Loading />
  if (!fix.data)
    return (
      <div className="flex flex-col gap-3">
        <ErrorBox error={fix.error} />
        <Link to={`/receipts/${id}`} className="text-sm text-sky-700 hover:underline">
          ← К приходу
        </Link>
      </div>
    )
  return <ReceiptNew key={id} fix={fix.data} />
}
