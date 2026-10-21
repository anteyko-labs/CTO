// Самые продаваемые товары за период: количество и выручка с фильтром по категории (SPEC-27 §4).
// Две вкладки (SPEC-28 §4): «Чаще берут» — по числу чеков (как часто покупают) или по выручке, открыта и
// администратору; «Больше приносят» — по валовой прибыли, только владельцу: администратору сервер
// прибыль не отдаёт вовсе (инвариант 13). Сортировки «по количеству» нет: литры, килограммы и штуки
// не сравнить между собой — масло всегда было бы сверху (проверка 20.10.2026). «Замены» (SPEC-30 §2):
// по каждому виду замены — сколько раз, сколько заплатили клиенты, сколько начислено мастерам и
// какие масла залиты в этих чеках; без себестоимости — видит и администратор.
import { useState } from 'react'
import { Card, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { formatSom, shiftDate, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useUser } from '../lib/auth'
import { oilAmountText } from '../lib/serviceSale'
import { qtyText } from '../lib/units'
import type { Category, ServiceKinds, TopProducts as TopProductsOut } from '../lib/types'

/** Вкладка «Замены»: виды замены за период и масла в их чеках. */
function ServiceKindsTab({ from, to }: { from: string; to: string }) {
  const r = useLoad(() => get<ServiceKinds>(`/reports/service-kinds${qs({ from, to })}`), [from, to])
  const kinds = r.data?.kinds ?? []
  return (
    <>
      <ErrorBox error={r.error} />
      {r.loading && !r.data ? (
        <Loading />
      ) : kinds.length === 0 ? (
        <Empty>Замен за период не было</Empty>
      ) : (
        <>
          <Table head={['Вид замены', 'Раз', 'Заплатили клиенты', 'Мастерам', 'Масла в этих чеках']}>
            {kinds.map((k) => (
              <tr key={k.service_id ?? 'none'} data-testid="service-kind-row">
                <td className="px-2 py-2 font-medium">{k.name}</td>
                <td className="px-2 py-2 text-right tabular-nums">{k.count}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(k.clients_tyiyn)}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(k.masters_tyiyn)}</td>
                <td className="px-2 py-2 text-sm">
                  {k.oils.length === 0 ? (
                    <span className="text-slate-500">масло клиента</span>
                  ) : (
                    <ul className="flex flex-col gap-0.5">
                      {k.oils.map((o) => (
                        <li key={o.product_id}>
                          {o.name}: <span className="tabular-nums">{oilAmountText(o)}</span>
                        </li>
                      ))}
                    </ul>
                  )}
                </td>
              </tr>
            ))}
          </Table>
          <div className="mt-3 border-t border-slate-200 pt-3 text-right text-sm">
            Заплатили клиенты: <span className="font-semibold tabular-nums">{formatSom(kinds.reduce((a, k) => a + k.clients_tyiyn, 0))}</span>
            {' · '}мастерам: <span className="font-semibold tabular-nums">{formatSom(kinds.reduce((a, k) => a + k.masters_tyiyn, 0))}</span>
            <div className="text-xs text-slate-500">
              Чеки «В сервис» за период вместе с их возвратами. Масло чека с двумя видами замены видно у обоих. «Без вида замены» — прежние чеки
              со ставкой за чек.
            </div>
          </div>
        </>
      )}
    </>
  )
}

export default function TopProducts() {
  const [to, setTo] = useState(todayBishkek)
  const [from, setFrom] = useState(() => shiftDate(todayBishkek(), -29))
  const [categoryId, setCategoryId] = useState('')
  const owner = useUser().role === 'owner'
  const [tab, setTab] = useState<'often' | 'profit' | 'services'>('often')
  const [sort, setSort] = useState<'amount' | 'checks'>('checks')
  const byProfit = owner && tab === 'profit'
  const services = tab === 'services'
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const top = useLoad(
    () =>
      services
        ? Promise.resolve(null)
        : get<TopProductsOut>(`/reports/top-products${qs({ from, to, category_id: categoryId, sort: byProfit ? 'profit' : sort, limit: 100 })}`),
    [from, to, categoryId, sort, byProfit, services],
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
            aria-selected={tab === 'often'}
            className={`rounded-md border px-4 py-2 text-sm font-medium ${tab === 'often' ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'}`}
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
          <button
            type="button"
            role="tab"
            aria-selected={services}
            className={`rounded-md border px-4 py-2 text-sm font-medium ${services ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'}`}
            onClick={() => setTab('services')}
          >
            Замены
          </button>
        </div>
        <div className="mb-3 grid grid-cols-2 gap-3 sm:grid-cols-4 sm:items-end">
          <Field label="С">
            <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
          </Field>
          <Field label="По">
            <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
          </Field>
          {!services && (
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
          )}
          {services ? null : byProfit ? (
            <div className="text-sm text-slate-600 sm:pb-2">По валовой прибыли: выручка минус себестоимость, за вычетом возвратов</div>
          ) : (
            <Field label="Порядок">
              <select value={sort} onChange={(e) => setSort(e.target.value as 'amount' | 'checks')}>
                <option value="checks">как часто берут (по числу чеков)</option>
                <option value="amount">по выручке</option>
              </select>
            </Field>
          )}
        </div>
        {services && <ServiceKindsTab from={from} to={to} />}
        <ErrorBox error={top.error ?? categories.error} />
        {services ? null : top.loading && !top.data ? (
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
                  <td className="whitespace-nowrap px-2 py-2 tabular-nums">{qtyText(r.unit, r.qty)}</td>
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
              <div className="text-xs text-slate-500">Продажи за вычетом возвратов, без подарков. Масло — в литрах (канистры и розлив вместе), весовой товар — в кг.</div>
            </div>
          </>
        )}
      </Card>
    </div>
  )
}
