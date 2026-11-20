// «Похожие в базе» в карточке товара при правке (задача 041 §5–7): товары общей базы обоих
// филиалов с похожим названием или тем же кодом. Похожий правится прямо в строке — название и
// категория; «Взять категорию» ставит его категорию этому товару; «Открыть» — его карточка.
import { useState } from 'react'
import { get, patch } from '../lib/api'
import { formatSom, litersInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { Category, Product } from '../lib/types'
import { isMilli, type StockUnit } from '../lib/units'
import { Button, ErrorBox } from './ui'

export interface SimilarProduct {
  id: string
  name: string
  article: string
  unit: StockUnit
  category_id: string
  category_name: string
  sale_price_tyiyn: number
  barcodes: string[]
  stocks: { branch_id: string; branch_name: string; stock_qty: number }[]
  reason: 'code' | 'name'
}

function SimilarRow({
  s,
  categories,
  onChanged,
  onTake,
  onOpen,
}: {
  s: SimilarProduct
  categories: Category[]
  onChanged: (s: SimilarProduct) => void
  onTake: (s: SimilarProduct) => void
  onOpen: (id: string) => void
}) {
  const [name, setName] = useState(s.name)
  const [cat, setCat] = useState(s.category_id)
  const { busy, error, run } = useAction()
  const dirty = name.trim() !== s.name || cat !== s.category_id
  const save = () =>
    void run(async () => {
      if (busy || !name.trim()) return
      const p = await patch<Product>(`/products/${s.id}`, { name: name.trim(), category_id: cat })
      const c = categories.find((x) => x.id === p.category_id)
      onChanged({ ...s, name: p.name, category_id: p.category_id, category_name: c?.name ?? s.category_name })
    })
  const qty = (q: number) => (isMilli(s.unit) ? `${litersInput(q)} л` : String(q))
  return (
    <li className="flex flex-col gap-1 border-t border-slate-200 py-2 first:border-t-0" data-testid="similar-row">
      <div className="flex flex-wrap items-center gap-2">
        <input className="min-w-0 flex-1" aria-label="Название похожего" value={name} onChange={(e) => setName(e.target.value)} />
        <select aria-label="Категория похожего" value={cat} onChange={(e) => setCat(e.target.value)}>
          {!categories.some((c) => c.id === cat) && <option value={cat}>{s.category_name}</option>}
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        {dirty && (
          <Button className="px-2 py-1 text-xs" disabled={busy} onClick={save}>
            Сохранить
          </Button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-slate-600">
        {s.reason === 'code' && <span className="font-medium text-amber-700">тот же код</span>}
        {s.article && <span>арт. {s.article}</span>}
        <span>{formatSom(s.sale_price_tyiyn)}</span>
        <span>{s.stocks.length ? s.stocks.map((b) => `${b.branch_name}: ${qty(b.stock_qty)}`).join(' · ') : 'остатка нет'}</span>
        <Button variant="secondary" className="px-2 py-0.5 text-xs" onClick={() => onTake(s)}>
          Взять категорию
        </Button>
        <Button variant="secondary" className="px-2 py-0.5 text-xs" onClick={() => onOpen(s.id)}>
          Открыть
        </Button>
      </div>
      <ErrorBox error={error} />
    </li>
  )
}

export function ProductSimilarBlock({
  productId,
  categories,
  onTakeCategory,
  onOpen,
}: {
  productId: string
  categories: Category[]
  onTakeCategory: (categoryId: string) => Promise<void>
  onOpen: (id: string) => void
}) {
  const rows = useLoad(() => get<SimilarProduct[]>(`/products/${productId}/similar`), [productId])
  const take = useAction()
  if (!rows.data?.length) return null
  return (
    <div className="mt-4 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm" data-testid="product-similar">
      <div className="mb-1 font-medium text-amber-800">Похожие в базе</div>
      <ul>
        {rows.data.map((s) => (
          <SimilarRow
            key={s.id}
            s={s}
            categories={categories}
            onChanged={(n) => rows.setData((list) => (list ?? []).map((x) => (x.id === n.id ? n : x)))}
            onTake={(x) => void take.run(() => onTakeCategory(x.category_id))}
            onOpen={onOpen}
          />
        ))}
      </ul>
      <ErrorBox error={take.error} />
    </div>
  )
}
