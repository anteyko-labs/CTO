import { useState } from 'react'
import { stockText } from '../components/ProductPicker'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatLiters, formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Category, Product, StockMismatch } from '../lib/types'

type Mismatch = StockMismatch & { product?: Product }

const minText = (p: Product): string => (p.unit === 'ml' ? formatLiters(p.min_stock) : `${p.min_stock} шт`)

const qtyText = (unit: Product['unit'] | undefined, qty: number): string => (unit === 'ml' ? formatLiters(qty) : `${qty} шт`)

export default function Stock() {
  const owner = useUser().role === 'owner'
  const [q, setQ] = useState('')
  const [categoryId, setCategoryId] = useState('')
  const [low, setLow] = useState(false)
  const query = useDebounced(q.trim())
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const stock = useLoad(
    () => get<Product[]>(`/stock${qs({ q: query, category_id: categoryId, low: low || undefined })}`),
    [query, categoryId, low],
  )
  const [mismatches, setMismatches] = useState<Mismatch[] | null>(null)
  const verify = useAction()
  const review = useAction()

  const rows = stock.data ?? []
  const categoryName = new Map((categories.data ?? []).map((c) => [c.id, c.name]))
  const totalValue = rows.reduce((acc, p) => acc + (p.stock_value_tyiyn ?? 0), 0)

  const runVerify = () => {
    void verify.run(async () => {
      const list = await get<StockMismatch[]>('/stock/verify')
      // Названия товаров для расхождений; список обычно пуст.
      const out: Mismatch[] = []
      for (const m of list) {
        const product = await get<Product>(`/products/${m.product_id}`).catch(() => undefined)
        out.push({ ...m, product })
      }
      setMismatches(out)
    })
  }

  const markReviewed = (p: Product) => {
    void review.run(async () => {
      await post(`/stock/${p.id}/review-done`)
      stock.reload()
    })
  }

  const head = ['Товар', 'Артикул', 'Категория', 'Остаток', 'Минимум', '']
  if (owner) head.splice(5, 0, 'Средняя цена', 'Стоимость')

  return (
    <div>
      <PageHeader
        title="Остатки"
        actions={
          owner && (
            <Button variant="secondary" disabled={verify.busy} onClick={runVerify}>
              Сверить остатки
            </Button>
          )
        }
      />
      <Card>
        <div className="mb-4 grid grid-cols-1 gap-3 sm:grid-cols-[2fr_1fr_auto] sm:items-end">
          <Field label="Поиск">
            <input placeholder="Название, бренд, артикул, штрихкод" value={q} onChange={(e) => setQ(e.target.value)} />
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
          <div className="pb-2">
            <Checkbox label="Только заканчивающиеся" checked={low} onChange={setLow} />
          </div>
        </div>
        <ErrorBox error={stock.error ?? categories.error} />
        <ErrorBox error={verify.error ?? review.error} />
        {stock.loading && !stock.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty>Товаров не найдено</Empty>
        ) : (
          <>
            <Table head={head}>
              {rows.map((p) => (
                <tr key={p.id}>
                  <td className="px-2 py-2">
                    <div className="font-medium">{p.name}</div>
                    <div className="mt-1 flex flex-wrap gap-1">
                      {p.stock_qty < p.min_stock && <Badge tone="amber">мало</Badge>}
                      {p.needs_review && <Badge tone="rose">проверить</Badge>}
                    </div>
                  </td>
                  <td className="px-2 py-2">{p.article || '—'}</td>
                  <td className="px-2 py-2">{categoryName.get(p.category_id) ?? '—'}</td>
                  <td className="whitespace-nowrap px-2 py-2">{stockText(p)}</td>
                  <td className="whitespace-nowrap px-2 py-2">{minText(p)}</td>
                  {owner && (
                    <td className="whitespace-nowrap px-2 py-2 text-right">
                      {p.avg_cost_tyiyn != null ? (
                        <>
                          {formatSom(p.avg_cost_tyiyn)}
                          <span className="text-xs text-slate-500">{p.unit === 'ml' && p.container_ml ? ' / кан.' : ' / шт'}</span>
                        </>
                      ) : (
                        '—'
                      )}
                    </td>
                  )}
                  {owner && (
                    <td className="whitespace-nowrap px-2 py-2 text-right">
                      {p.stock_value_tyiyn != null ? formatSom(p.stock_value_tyiyn) : '—'}
                    </td>
                  )}
                  <td className="px-2 py-2 text-right">
                    {owner && p.needs_review && (
                      <Button variant="secondary" disabled={review.busy} onClick={() => markReviewed(p)}>
                        Проверено
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </Table>
            {owner && (
              <div className="mt-3 border-t border-slate-200 pt-3 text-right font-semibold">
                Стоимость остатка по списку: {formatSom(totalValue)}
              </div>
            )}
            {rows.length >= 500 && (
              <div className="mt-2 text-xs text-slate-500">Показаны первые 500 товаров — уточните фильтр.</div>
            )}
          </>
        )}
      </Card>

      {mismatches && (
        <Modal title="Сверка остатков" wide onClose={() => setMismatches(null)}>
          {mismatches.length === 0 ? (
            <div className="py-4 text-center text-emerald-700">Расхождений нет</div>
          ) : (
            <MismatchTable rows={mismatches} />
          )}
        </Modal>
      )}
    </div>
  )
}

function MismatchTable({ rows }: { rows: Mismatch[] }) {
  return (
    <Table head={['Товар', 'Остаток в учёте', 'По движениям', 'Стоимость в учёте', 'По движениям']}>
      {rows.map((m) => {
        const p = m.product
        return (
          <tr key={m.product_id}>
            <td className="px-2 py-2">{p?.name ?? m.product_id}</td>
            <td className="whitespace-nowrap px-2 py-2">{qtyText(p?.unit, m.cached_qty)}</td>
            <td className="whitespace-nowrap px-2 py-2">{qtyText(p?.unit, m.moved_qty)}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right">{formatSom(m.cached_value_tyiyn)}</td>
            <td className="whitespace-nowrap px-2 py-2 text-right">{formatSom(m.moved_value_tyiyn)}</td>
          </tr>
        )
      })}
    </Table>
  )
}
