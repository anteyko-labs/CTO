// Самые продаваемые товары за период: количество и выручка с фильтром по категории (SPEC-27 §4).
// Две вкладки (SPEC-28 §4): «Чаще берут» — по количеству, чекам или выручке, открыта и
// администратору; «Больше приносят» — по валовой прибыли, только владельцу: администратору сервер
// прибыль не отдаёт вовсе (инвариант 13).
import { useState } from 'react'
import { stockText } from '../components/ProductPicker'
import { Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useUser } from '../lib/auth'
import type { Category, TopProducts as TopProductsOut } from '../lib/types'

export default function TopProducts() {
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -29))
  const [categoryId, setCategoryId] = useState('')
  const owner = useUser().role === 'owner'
  const [tab, setTab] = useState<'often' | 'profit'>('often')
  const [sort, setSort] = useState<'amount' | 'qty' | 'checks'>('qty')
  const byProfit = owner && tab === 'profit'
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const top = useLoad(
    () => get<TopProductsOut>(`/reports/top-products${qs({ from, to, category_id: categoryId, sort: byProfit ? 'profit' : sort, limit: 100 })}`),
    [from, to, categoryId, sort, byProfit],
  )
  const items = top.data?.items ?? []
  const total = items.reduce((acc, r) => acc + r.amount_tyiyn, 0)
  const profit = items.reduce((acc, r) => acc + (r.profit_tyiyn ?? 0), 0)
  const showProfit = owner && items.some((r) => r.profit_tyiyn !== undefined)

  return (
    <div>
      <PageHeader title="Продаваемые товары" />
      <Card>
        <div role="tablist" aria-label="Рейтинг товаров" className="mb-3 flex flex-wrap gap-2">
          <button
            type="button"
            role="tab"
            aria-selected={!byProfit}
            className={`rounded-md border px-4 py-2 text-sm font-medium ${!byProfit ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'}`}
            onClick={() => setTab('often')}
          >
            Чаще берут
          </button>
          {owner && (
            <button
              type="button"
              role="tab"
              aria-selected={byProfit}
              className={`rounded-md border px-4 py-2 text-sm font-medium ${byProfit ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'}`}
              onClick={() => setTab('profit')}
            >
              Больше приносят
            </button>
          )}
        </div>
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
          {byProfit ? (
            <div className="text-sm text-slate-600 sm:pb-2">По валовой прибыли: выручка минус себестоимость, за вычетом возвратов</div>
          ) : (
            <Field label="Порядок">
              <select value={sort} onChange={(e) => setSort(e.target.value as 'amount' | 'qty' | 'checks')}>
                <option value="qty">по количеству</option>
                <option value="checks">по числу чеков</option>
                <option value="amount">по выручке</option>
              </select>
            </Field>
          )}
        </div>
        <ErrorBox error={top.error ?? categories.error} />
        {top.loading && !top.data ? (
          <Loading />
        ) : items.length === 0 ? (
          <Empty>Продаж товаров за период не было</Empty>
        ) : (
          <>
            <Table head={showProfit ? ['№', 'Товар', 'Продано', 'Выручка', 'Прибыль', 'Чеков'] : ['№', 'Товар', 'Продано', 'Выручка', 'Чеков']}>
              {items.map((r, i) => (
                <tr key={r.product_id}>
                  <td className="px-2 py-2 tabular-nums text-slate-500">{i + 1}</td>
                  <td className="px-2 py-2 font-medium">{r.name}</td>
                  <td className="whitespace-nowrap px-2 py-2 tabular-nums">{stockText({ unit: r.unit, container_ml: r.container_ml, stock_qty: r.qty })}</td>
                  <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(r.amount_tyiyn)}</td>
                  {showProfit && (
                    <td className={`whitespace-nowrap px-2 py-2 text-right tabular-nums ${byProfit ? 'font-semibold' : ''}`}>{formatSom(r.profit_tyiyn ?? 0)}</td>
                  )}
                  <td className="px-2 py-2 text-right tabular-nums">{r.checks}</td>
                </tr>
              ))}
            </Table>
            <div className="mt-3 border-t border-slate-200 pt-3 text-right text-sm">
              Выручка по списку: <span className="font-semibold tabular-nums">{formatSom(total)}</span>
              {showProfit && (
                <>
                  {' · '}прибыль: <span className="font-semibold tabular-nums">{formatSom(profit)}</span>
                </>
              )}
              <div className="text-xs text-slate-500">Продажи за вычетом возвратов, без подарков. Масло — в литрах: канистры и розлив вместе.</div>
            </div>
          </>
        )}
      </Card>
    </div>
  )
}
