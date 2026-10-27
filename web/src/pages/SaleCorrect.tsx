// «Исправить» чек (SPEC-29 §24): экран кассы, заполненный исходным чеком. Сохранение — сторно
// исходного и верный чек одной операцией; после — переход к новому чеку.
import { Link, useParams } from 'react-router-dom'
import { ErrorBox, Loading } from '../components/ui'
import { get, randomId } from '../lib/api'
import { loadCorrectionDraft } from '../lib/corrections'
import { useLoad } from '../lib/hooks'
import { saleFixState, type SaleDraftDoc } from '../lib/saleFix'
import type { Party, Product, Service } from '../lib/types'
import Cashier from './Cashier'

async function loadFix(id: string) {
  const draft = await loadCorrectionDraft<SaleDraftDoc>('sale', id)
  const doc = draft.doc
  const productIds = [...new Set(doc.lines.map((l) => l.product_id).filter((x): x is string => Boolean(x)))]
  const [products, services, party] = await Promise.all([
    Promise.all(productIds.map((p) => get<Product>(`/products/${p}`))),
    doc.lines.some((l) => l.service_id) ? get<Service[]>('/services') : Promise.resolve([] as Service[]),
    doc.party_id ? get<Party>(`/parties/${doc.party_id}`) : Promise.resolve(null),
  ])
  const byId = new Map(products.filter((p): p is Product => p !== null).map((p) => [p.id, p]))
  const state = saleFixState(doc, byId, new Map(services.map((s) => [s.id, s])), party, () => randomId())
  return { originalId: id, number: draft.number, state }
}

export default function SaleCorrect() {
  const { id = '' } = useParams()
  const fix = useLoad(() => loadFix(id), [id])
  if (fix.loading && !fix.data) return <Loading />
  if (!fix.data)
    return (
      <div className="flex flex-col gap-3">
        <ErrorBox error={fix.error} />
        <Link to={`/sales/${id}`} className="text-sm text-sky-700 hover:underline">
          ← К чеку
        </Link>
      </div>
    )
  return <Cashier key={id} fix={fix.data} />
}
