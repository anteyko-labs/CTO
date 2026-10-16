// Самые продаваемые товары за период: количество и выручка с фильтром по категории (SPEC-27 §4).
// Себестоимости и маржи здесь нет — экран открыт и администратору.
import { useState } from 'react'
import { stockText } from '../components/ProductPicker'
import { Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { Category, TopProducts as TopProductsOut } from '../lib/types'

export default function TopProducts() {
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -29))
  const [categoryId, setCategoryId] = useState('')
  const [sort, setSort] = useState<'amount' | 'qty'>('amount')
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const top = useLoad(
    () => get<TopProductsOut>(`/reports/top-products${qs({ from, to, category_id: categoryId, sort, limit: 100 })}`),
    [from, to, categoryId, sort],
  )
  const items = top.data?.items ?? []
  const total = items.reduce((acc, r) => acc + r.amount_tyiyn, 0)

  return (
    <div>
      <PageHeader title="Продаваемые товары" />
      <Card>
        <div className="mb-3 grid grid-cols-2 gap-3 sm:grid-cols-4 sm:items-end">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          <Field label="Категория">
            <select value={categoryId} onChange={(e) => setCategoryId(e.target.value)}>
              <option value="">Все</option>
              {(categories.data ?? []).map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
          <Field label="Порядок">
            <select value={sort} onChange={(e) => setSort(e.target.value as 'amount' | 'qty')}>
              <option value="amount">по выручке</option>
              <option value="qty">по количеству</option>
            </select>
          </Field>
        </div>
        <ErrorBox error={top.error ?? categories.error} />
        {top.loading && !top.data ? (
          <Loading />
        ) : items.length === 0 ? (
          <Empty>Продаж товаров за период не было</Empty>
        ) : (
          <>
            <Table head={['№', 'Товар', 'Продано', 'Выручка', 'Чеков']}>
              {items.map((r, i) => (
                <tr key={r.product_id}>
                  <td className="px-2 py-2 tabular-nums text-slate-500">{i + 1}</td>
                  <td className="px-2 py-2 font-medium">{r.name}</td>
                  <td className="whitespace-nowrap px-2 py-2 tabular-nums">{stockText({ unit: r.unit, container_ml: r.container_ml, stock_qty: r.qty })}</td>
                  <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(r.amount_tyiyn)}</td>
                  <td className="px-2 py-2 text-right tabular-nums">{r.checks}</td>
                </tr>
              ))}
            </Table>
            <div className="mt-3 border-t border-slate-200 pt-3 text-right text-sm">
              Выручка по списку: <span className="font-semibold tabular-nums">{formatSom(total)}</span>
              <div className="text-xs text-slate-500">Продажи за вычетом возвратов, без подарков. Масло — в литрах: канистры и розлив вместе.</div>
            </div>
          </>
        )}
      </Card>
    </div>
  )
}
