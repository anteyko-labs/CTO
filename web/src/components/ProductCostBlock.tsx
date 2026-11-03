// Блок «Себестоимость» в карточке товара (задача 003, Н7, ADR-150). Владельцу — средняя за канистру
// и за литр (за шт, за кг), стоимость остатка, наценка и «Указать цену закупки»; администратору сервер
// этих полей не отдаёт (I13) — у него только «Последний приход» (ADR-040). Данные — свежим
// GET /products/{id}, откуда бы карточку ни открыли; только показ, остаток не меняется (I6).
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { get } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { costSummary, priceWithLiter } from '../lib/purchasePrice'
import type { LastReceipt, Product } from '../lib/types'
import { PurchasePriceModal } from './PurchasePriceModal'
import { Button } from './ui'

export function ProductCostBlock({ productId }: { productId: string }) {
  const owner = useUser().role === 'owner'
  const card = useLoad(() => get<Product>(`/products/${productId}`), [productId])
  const last = useLoad(() => get<LastReceipt | null>(`/products/${productId}/last-receipt`), [productId])
  const [pricing, setPricing] = useState(false)
  const p = card.data
  if (!p) return null
  const s = costSummary(p)
  const r = last.data
  const lastLine = r ? (
    <>
      {priceWithLiter(r.price_tyiyn, p)}, {formatDate(r.created_at)}
      {r.supplier_name ? `, ${r.supplier_name}` : ''},{' '}
      <Link className="text-sky-700 underline" to={`/receipts/${r.receipt_id}`}>
        приход № {r.number}
      </Link>
    </>
  ) : p.last_purchase_price_tyiyn != null ? (
    <>начальная цена закупки: {priceWithLiter(p.last_purchase_price_tyiyn, p)}</>
  ) : (
    <>цены закупки нет</>
  )
  return (
    <div className="mt-4 rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-sm" data-testid="product-cost">
      {owner && (
        <>
          <div className="mb-1 font-medium text-slate-700">Себестоимость</div>
          <dl className="grid gap-x-3 gap-y-1 sm:grid-cols-[auto_1fr]" data-testid="product-cost-owner">
            {s.average && (
              <>
                <dt className="text-slate-500">Средняя</dt>
                <dd data-testid="product-cost-avg">{s.average}</dd>
              </>
            )}
            {s.stockValue && (
              <>
                <dt className="text-slate-500">Стоимость остатка</dt>
                <dd data-testid="product-cost-stock">{s.stockValue}</dd>
              </>
            )}
            {s.markup && (
              <>
                <dt className="text-slate-500">Наценка</dt>
                <dd data-testid="product-cost-markup">{s.markup}</dd>
              </>
            )}
          </dl>
          {s.note && (
            <div className="mt-1 text-amber-800" data-testid="product-cost-note">
              {s.note}
            </div>
          )}
        </>
      )}
      <div className={owner ? 'mt-1' : ''} data-testid="product-last-receipt">
        <span className="text-slate-500">Последний приход: </span>
        {lastLine}
      </div>
      {owner && !p.archived && (
        <Button variant="secondary" className="mt-2 px-2 py-1 text-xs" onClick={() => setPricing(true)}>
          {s.askPrice ? 'Указать цену закупки' : 'Исправить цену закупки'}
        </Button>
      )}
      {pricing && (
        <PurchasePriceModal
          product={p}
          fix={!s.askPrice}
          onClose={() => setPricing(false)}
          onDone={() => {
            setPricing(false)
            card.reload()
            last.reload()
          }}
        />
      )}
    </div>
  )
}
