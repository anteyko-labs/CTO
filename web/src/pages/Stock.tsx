// Остатки филиала: фильтры «мало» и «проверить», себестоимость и сверка с движениями — только владельцу (SPEC-03).
import { useState } from 'react'
import { ProductFormModal } from '../components/ProductFormModal'
import { stockText } from '../components/ProductPicker'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, post, put, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatLiters, formatSom, formatKg } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Category, Product, StockMismatch } from '../lib/types'

type Mismatch = StockMismatch & { product?: Product }

const minText = (p: Product): string => (p.unit === 'ml' ? formatLiters(p.min_stock) : `${p.min_stock} шт`)

/** Цена за единицу учёта: «за шт», для масла — «за 4 л» / «за 200 л» по объёму тары. */
const perUnit = (p: Product): string => (p.unit === 'ml' && p.container_ml ? `за ${formatLiters(p.container_ml)}` : 'за шт')

/** Остаток коротко для телефона: без расшифровки по канистрам. */
const stockShort = (p: Product): string => (p.unit === 'ml' ? formatLiters(p.stock_qty) : p.unit === 'g' ? formatKg(p.stock_qty) : `${p.stock_qty} шт`)

const qtyText = (unit: Product['unit'] | undefined, qty: number): string =>
  unit === 'ml' ? formatLiters(qty) : unit === 'g' ? formatKg(qty) : `${qty} шт`

export default function Stock() {
  const owner = useUser().role === 'owner'
  const [q, setQ] = useState('')
  const [categoryId, setCategoryId] = useState('')
  const [filter, setFilter] = useState<'all' | 'low' | 'out' | 'review' | 'stale'>('all')
  // Залежалый товар: остаток есть, продаж нет дольше срока (ADR-039).
  const staleList = useLoad(
    () => get<{ stale_days: number; items: { product_id: string; days: number; sold_ever: boolean }[] }>('/stock/stale'),
    [],
  )
  const [staleDays, setStaleDays] = useState('')
  const staleSave = useAction()
  const [editing, setEditing] = useState<Product | null>(null)
  const query = useDebounced(q.trim())
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const stock = useLoad(() => get<Product[]>(`/stock${qs({ q: query, category_id: categoryId })}`), [query, categoryId])
  const [mismatches, setMismatches] = useState<Mismatch[] | null>(null)
  const verify = useAction()
  const review = useAction()

  const all = stock.data ?? []
  // «Нет» — остаток ноль или минус; «мало» — есть, но меньше минимума.
  const isOut = (p: Product) => p.stock_qty <= 0
  const isLow = (p: Product) => p.stock_qty > 0 && p.stock_qty < p.min_stock
  const lowCount = all.filter(isLow).length
  const outCount = all.filter(isOut).length
  const reviewCount = all.filter((p) => p.needs_review).length
  const stale = new Map((staleList.data?.items ?? []).map((i) => [i.product_id, i]))
  const rows =
    filter === 'low'
      ? all.filter(isLow)
      : filter === 'out'
        ? all.filter(isOut)
        : filter === 'review'
        ? all.filter((p) => p.needs_review)
          : filter === 'stale'
            ? all.filter((p) => stale.has(p.id))
            : all
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

  const rowTone = (p: Product) => (isOut(p) ? 'bg-rose-50 hover:bg-rose-100' : isLow(p) ? 'bg-amber-50 hover:bg-amber-100' : 'hover:bg-slate-50')
  const qtyTone = (p: Product) => (isOut(p) ? 'text-rose-700' : isLow(p) ? 'text-amber-800' : '')
  const badges = (p: Product) => (
    <>
      {isOut(p) && <Badge tone="rose">нет</Badge>}
      {isLow(p) && <Badge tone="amber">мало</Badge>}
      {p.needs_review && <Badge tone="rose">проверить</Badge>}
      {stale.has(p.id) && (
        <Badge tone="slate">
          {stale.get(p.id)?.sold_ever ? 'не продавался' : 'ни разу не продан'} {stale.get(p.id)?.days} дн.
        </Badge>
      )}
    </>
  )

  const head = ['Товар', 'Артикул', 'Категория', <span key="qty" className="block text-right">Остаток</span>, <span key="min" className="block text-right">Минимум</span>, '']
  if (owner) head.splice(5, 0, <span key="avg" className="block text-right">Средняя цена</span>, <span key="value" className="block text-right">Стоимость</span>)

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
        <div className="mb-3 grid grid-cols-1 gap-3 sm:grid-cols-[2fr_1fr] sm:items-end">
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
        </div>
        <div className="-mx-1 mb-4 flex gap-2 overflow-x-auto px-1 pb-1 text-sm">
          {(
            [
              ['all', 'Все', all.length],
              ['low', 'Мало', lowCount],
              ['out', 'Нет на складе', outCount],
              ['stale', 'Залежалые', stale.size],
              ['review', 'Проверить', reviewCount],
            ] as const
          ).map(([key, label, count]) => (
            <button
              key={key}
              type="button"
              aria-pressed={filter === key}
              className={`inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-3 py-1.5 ${
                filter === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'
              }`}
              onClick={() => setFilter(key)}
            >
              {label}
              <span className={`tabular-nums ${filter === key ? 'text-sky-100' : 'text-slate-400'}`}>{count}</span>
            </button>
          ))}
        </div>
        {filter === 'stale' && staleList.data && (
          <div className="mb-3 flex flex-wrap items-center gap-2 text-sm text-slate-600">
            Залежалым считается товар без продаж {staleList.data.stale_days} дней и больше.
            {owner && (
              <>
                <input
                  className="w-20"
                  inputMode="numeric"
                  placeholder={String(staleList.data.stale_days)}
                  value={staleDays}
                  onChange={(e) => setStaleDays(e.target.value)}
                />
                <Button
                  variant="secondary"
                  disabled={staleSave.busy || !/^\d+$/.test(staleDays.trim())}
                  onClick={() =>
                    void staleSave.run(async () => {
                      await put('/settings/stock', { stale_days: Number(staleDays) })
                      setStaleDays('')
                      staleList.reload()
                    })
                  }
                >
                  Изменить срок
                </Button>
              </>
            )}
            <ErrorBox error={staleSave.error ?? staleList.error} />
          </div>
        )}
        <ErrorBox error={stock.error ?? categories.error} />
        <ErrorBox error={verify.error ?? review.error} />
        {stock.loading && !stock.data ? (
          <Loading />
        ) : rows.length === 0 ? (
          <Empty icon="stock">
            {filter === 'out'
              ? 'Все товары в наличии.'
              : filter === 'low'
                ? 'Ничего не заканчивается: остатки выше минимума.'
                : 'Товаров не найдено. Проверьте фильтры или заведите товар на экране «Товары».'}
          </Empty>
        ) : (
          <>
            <ul className="-mx-4 divide-y divide-slate-100 border-y border-slate-100 md:hidden">
              {rows.map((p) => (
                <li key={p.id} className={rowTone(p)}>
                  <button type="button" className="flex w-full items-start gap-3 px-4 py-2.5 text-left" onClick={() => setEditing(p)}>
                    <div className="min-w-0 flex-1">
                      <div className="font-semibold leading-snug">{p.name}</div>
                      <div className="mt-0.5 truncate text-xs text-slate-500">
                        {[p.article, categoryName.get(p.category_id)].filter(Boolean).join(' · ') || '—'}
                      </div>
                      {(isOut(p) || isLow(p) || p.needs_review || stale.has(p.id)) && <div className="mt-1 flex flex-wrap gap-1">{badges(p)}</div>}
                    </div>
                    <div className={`shrink-0 text-right text-lg font-bold tabular-nums ${qtyTone(p)}`}>{stockShort(p)}</div>
                  </button>
                  {owner && p.needs_review && (
                    <div className="px-4 pb-2.5">
                      <Button variant="secondary" className="min-h-[36px] py-1 text-xs" disabled={review.busy} onClick={() => markReviewed(p)}>
                        Проверено
                      </Button>
                    </div>
                  )}
                </li>
              ))}
            </ul>
            <div className="hidden md:block">
              <Table head={head}>
                {rows.map((p) => (
                  <tr key={p.id} className={`cursor-pointer ${rowTone(p)}`} onClick={() => setEditing(p)}>
                    <td className="px-2 py-2">
                      <div className="font-medium">{p.name}</div>
                      <div className="mt-1 flex flex-wrap gap-1">{badges(p)}</div>
                    </td>
                    <td className="px-2 py-2">{p.article || '—'}</td>
                    <td className="px-2 py-2">{categoryName.get(p.category_id) ?? '—'}</td>
                    <td className={`whitespace-nowrap px-2 py-2 text-right font-semibold tabular-nums ${qtyTone(p)}`}>{stockText(p)}</td>
                    <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums text-slate-500">{minText(p)}</td>
                    {owner && (
                      <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">
                        {p.avg_cost_tyiyn != null ? (
                          <>
                            {formatSom(p.avg_cost_tyiyn)}
                            <span className="text-xs text-slate-500"> {perUnit(p)}</span>
                          </>
                        ) : (
                          '—'
                        )}
                      </td>
                    )}
                    {owner && (
                      <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">
                        {p.stock_value_tyiyn != null ? formatSom(p.stock_value_tyiyn) : '—'}
                      </td>
                    )}
                    <td className="px-2 py-2 text-right">
                      {owner && p.needs_review && (
                        <Button
                          variant="secondary"
                          disabled={review.busy}
                          onClick={(e) => {
                            e.stopPropagation()
                            markReviewed(p)
                          }}
                        >
                          Проверено
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </Table>
            </div>
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

      {editing && (
        <ProductFormModal
          product={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null)
            stock.reload()
          }}
        />
      )}

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
