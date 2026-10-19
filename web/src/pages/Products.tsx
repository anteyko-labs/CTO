// Справочник товаров: поиск, фильтры по характеристикам, карточка, архив, печать этикеток (SPEC-02).
// Список «как в Умаге» (SPEC-27): остаток, цены и суммы остатка с итогами по отбору — итоги считает
// сервер; закупку и стоимость остатка видит только владелец (ADR-008).
import { useMemo, useState } from 'react'
import { printLabels } from '../components/labels'
import { Pager, usePageState } from '../components/Pager'
import { ProductFormModal } from '../components/ProductFormModal'
import { stockText } from '../components/ProductPicker'
import { StockInModal } from '../components/StockInModal'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, PageHeader, RowMenu } from '../components/ui'
import { get, patch, qs } from '../lib/api'
import { POLARITY_LABELS, productTitle } from '../lib/battery'
import { useUser } from '../lib/auth'
import { formatLiters, formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { pourPricePerL } from '../lib/money'
import type { Category, Product, ProductList, ProductRow } from '../lib/types'

interface Selected {
  product: Product
  copies: string
}

type SortKey = 'name' | 'code' | 'stock' | 'purchase' | 'sale' | 'stock_purchase' | 'stock_sale'

/** «1 товар», «2 товара», «5 товаров». */
function goodsWord(n: number): string {
  const n10 = n % 10
  const n100 = n % 100
  if (n100 >= 11 && n100 <= 19) return 'товаров'
  if (n10 === 1) return 'товар'
  if (n10 >= 2 && n10 <= 4) return 'товара'
  return 'товаров'
}

/** За что цена: канистра масла, килограмм или штука. */
const perText = (p: Product): string =>
  p.unit === 'ml' && p.container_ml ? `за ${formatLiters(p.container_ml)}` : p.unit === 'g' ? 'за кг' : ''

export default function Products() {
  const owner = useUser().role === 'owner'
  const [q, setQ] = useState('')
  const [categoryId, setCategoryId] = useState('')
  const [attrFilters, setAttrFilters] = useState<Record<string, string>>({})
  const [archived, setArchived] = useState(false)
  const [editing, setEditing] = useState<Product | 'new' | null>(null)
  /** «+ Оприходовать» у товара (SPEC-29 §8). */
  const [stockIn, setStockIn] = useState<Product | null>(null)
  const [selected, setSelected] = useState<Record<string, Selected>>({})
  const [message, setMessage] = useState<string | null>(null)
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: 'name', desc: false })
  const { busy, error, run } = useAction()

  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const category = categories.data?.find((c) => c.id === categoryId)
  const filterDefs = category?.attributes.filter((a) => a.filterable) ?? []

  const query = useDebounced(q.trim(), 300)
  const attrs = useDebounced(attrFilters, 300)
  const params = useMemo(() => {
    const p: Record<string, string | boolean | number | undefined> = {
      q: query,
      category_id: categoryId,
      archived: archived || undefined,
      sort: sort.key,
      dir: sort.desc ? 'desc' : undefined,
    }
    for (const [k, v] of Object.entries(attrs)) p[`attr.${k}`] = v.trim()
    return qs(p)
  }, [query, categoryId, archived, attrs, sort])
  // Страницу режет сервер, итоги — по всему отбору; смена отбора или сортировки — снова первая страница.
  const pager = usePageState('products', params)
  const list = useLoad(
    () => get<ProductList>(`/products/list${params}${params ? '&' : '?'}limit=${pager.size}&offset=${pager.page * pager.size}`),
    [params, pager.page, pager.size],
  )
  const items = list.data?.items
  const totals = list.data?.totals

  /** Щелчок по заголовку: та же колонка — обратный порядок; числа — сначала большие. */
  const sortBy = (key: SortKey) =>
    setSort((s) => (s.key === key ? { key, desc: !s.desc } : { key, desc: key !== 'name' && key !== 'code' }))
  const head = (key: SortKey, label: string, right = false) => (
    <th key={key} className={`px-2 py-2 font-medium ${right ? 'text-right' : ''}`} aria-sort={sort.key === key ? (sort.desc ? 'descending' : 'ascending') : 'none'}>
      <button type="button" className="inline-flex items-center gap-1 uppercase hover:text-slate-800" onClick={() => sortBy(key)}>
        {label}
        <span aria-hidden className="text-[10px]">{sort.key === key ? (sort.desc ? '▼' : '▲') : ''}</span>
      </button>
    </th>
  )

  const categoryName = (id: string) => categories.data?.find((c) => c.id === id)?.name ?? '—'

  const toggle = (p: Product, on: boolean) =>
    setSelected((s) => {
      const next = { ...s }
      if (on) next[p.id] = { product: p, copies: '1' }
      else delete next[p.id]
      return next
    })

  const setCopies = (id: string, copies: string) =>
    setSelected((s) => {
      const cur = s[id]
      return cur ? { ...s, [id]: { ...cur, copies } } : s
    })

  // Свежие данные товара из списка: штрихкод мог появиться после правки
  const selectedList = Object.values(selected).map((s) => ({
    ...s,
    product: items?.find((p) => p.id === s.product.id) ?? s.product,
  }))
  const noBarcode = selectedList.filter((s) => s.product.barcodes.length === 0)

  const print = () =>
    run(async () => {
      setMessage(null)
      const items = selectedList.map((s) => {
        const copies = s.copies.trim()
        if (!/^\d+$/.test(copies) || Number(copies) < 1) throw new Error(`Неверное число копий: ${s.product.name}`)
        return { product: s.product, copies: Number(copies) }
      })
      const count = await printLabels(items)
      if (count === 0) setMessage('Нет этикеток для печати: у выбранных товаров нет штрихкода')
    })

  const setArchivedFlag = (p: Product, value: boolean) =>
    run(async () => {
      await patch<Product>(`/products/${p.id}`, { archived: value })
      list.reload()
    })

  const onSaved = (p: Product) => {
    setEditing(null)
    setSelected((s) => (s[p.id] ? { ...s, [p.id]: { ...s[p.id], product: p } } : s))
    list.reload()
  }

  return (
    <div>
      <PageHeader
        title="Товары"
        actions={
          <Button onClick={() => setEditing('new')} aria-label="Добавить товар" title="Добавить товар: полная карточка">
            + Товар
          </Button>
        }
      />

      <Card className="mb-4">
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <Field label="Поиск">
            <input placeholder="Название, бренд, артикул, штрихкод" value={q} onChange={(e) => setQ(e.target.value)} />
          </Field>
          <Field label="Категория">
            <select
              value={categoryId}
              onChange={(e) => {
                setCategoryId(e.target.value)
                setAttrFilters({})
              }}
            >
              <option value="">Все</option>
              {categories.data?.map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
            </select>
          </Field>
          {filterDefs.map((a) => (
            <Field key={a.key} label={a.label}>
              {a.type === 'select' ? (
                <select
                  value={attrFilters[a.key] ?? ''}
                  onChange={(e) => setAttrFilters((f) => ({ ...f, [a.key]: e.target.value }))}
                >
                  <option value="">Все</option>
                  {a.options?.map((o) => (
                    <option key={o} value={o}>
                      {/* Полярность аккумулятора — с подсказкой «+ слева, Л» (SPEC-27). */}
                      {a.key === 'polarity' ? (POLARITY_LABELS[o] ?? o) : o}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  inputMode={a.type === 'number' ? 'numeric' : undefined}
                  value={attrFilters[a.key] ?? ''}
                  onChange={(e) => setAttrFilters((f) => ({ ...f, [a.key]: e.target.value }))}
                />
              )}
            </Field>
          ))}
        </div>
        <div className="mt-3">
          <Checkbox label="Показать архив" checked={archived} onChange={setArchived} />
        </div>
      </Card>

      {selectedList.length > 0 && (
        <Card className="mb-4">
          <div className="mb-2 text-sm font-medium">Этикетки: выбрано {selectedList.length}</div>
          <ul className="mb-3 flex flex-col gap-2">
            {selectedList.map((s) => (
              <li key={s.product.id} className="flex flex-wrap items-center gap-2 text-sm">
                <span className="min-w-0 flex-1">{productTitle(s.product)}</span>
                <label className="flex items-center gap-1 text-slate-600">
                  копий
                  <input
                    className="w-20"
                    inputMode="numeric"
                    value={s.copies}
                    onChange={(e) => setCopies(s.product.id, e.target.value)}
                  />
                </label>
                <Button variant="ghost" onClick={() => toggle(s.product, false)} aria-label="Убрать">
                  ✕
                </Button>
              </li>
            ))}
          </ul>
          {noBarcode.length > 0 && (
            <div className="mb-3 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">
              Нет штрихкода: {noBarcode.map((s) => s.product.name).join(', ')}. Откройте товар и добавьте заводской код
              или нажмите «Создать свой».
            </div>
          )}
          <div className="flex flex-wrap gap-2">
            <Button disabled={busy} onClick={() => void print()}>
              Печать этикеток
            </Button>
            <Button variant="secondary" onClick={() => setSelected({})}>
              Снять выбор
            </Button>
          </div>
        </Card>
      )}

      <div className="mb-3 flex flex-col gap-2">
        <ErrorBox error={error} />
        {message && <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-800">{message}</div>}
      </div>

      <Card>
        <ErrorBox error={list.error ?? categories.error} />
        {list.loading && !list.data ? (
          <Loading />
        ) : !items || (items.length === 0 && !totals?.count) ? (
          <Empty action={<Button onClick={() => setEditing('new')}>Новый товар</Button>}>Товары не найдены</Empty>
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="responsive w-full text-sm" aria-label="Список товаров">
                <thead>
                  <tr className="border-b border-slate-200 text-left text-xs uppercase text-slate-500">
                    <th className="px-2 py-2" />
                    {head('name', 'Название')}
                    {head('code', 'Код')}
                    {head('stock', 'Остаток', true)}
                    {owner && head('purchase', 'Закупка', true)}
                    {head('sale', 'Продажа', true)}
                    {owner && head('stock_purchase', 'Сумма по закупке', true)}
                    {head('stock_sale', 'Сумма по продаже', true)}
                    <th className="px-2 py-2" />
                  </tr>
                </thead>
                <tbody className="divide-y divide-slate-100">
                  {items.map((p) => (
                    <ProductTableRow
                      key={p.id}
                      p={p}
                      owner={owner}
                      category={categoryName(p.category_id)}
                      selected={Boolean(selected[p.id])}
                      onOpen={() => setEditing(p)}
                      onToggle={(on) => toggle(p, on)}
                      onStockIn={() => setStockIn(p)}
                      onArchive={() => {
                        if (p.archived || window.confirm(`Убрать «${p.name}» в архив? Он пропадёт из поиска в кассе; вернуть можно через «Показать архив».`)) {
                          void setArchivedFlag(p, !p.archived)
                        }
                      }}
                    />
                  ))}
                </tbody>
              </table>
            </div>
            {/* Итоги — по всему отбору (все страницы), считает сервер; на компьютере прижаты к низу экрана. */}
            <div className="-mx-4 -mb-4 mt-3 rounded-b-lg border-t-2 border-slate-300 bg-white px-4 pb-3 pt-2 md:sticky md:bottom-0 md:z-10 md:shadow-[0_-6px_12px_-10px_rgba(15,23,42,0.35)]">
              {totals && (
                <div className="flex flex-wrap items-baseline justify-end gap-x-2 gap-y-1 text-base font-semibold" data-testid="product-totals">
                  <span>
                    Итого по отбору: <span className="tabular-nums">{totals.count}</span> {goodsWord(totals.count)}
                  </span>
                  {owner && totals.purchase_tyiyn !== undefined && (
                    <span>
                      · по закупке <span className="tabular-nums">{formatSom(totals.purchase_tyiyn)}</span>
                    </span>
                  )}
                  <span>
                    · по продаже <span className="tabular-nums">{formatSom(totals.sale_tyiyn)}</span>
                  </span>
                </div>
              )}
              <Pager page={pager.page} size={pager.size} total={totals?.count ?? items.length} onPage={pager.setPage} onSize={pager.setSize} label="Страницы товаров" />
            </div>
          </>
        )}
      </Card>

      {stockIn && <StockInModal product={stockIn} onClose={() => setStockIn(null)} onDone={() => list.reload()} />}
      {/* «+» внизу экрана на телефоне: добавить товар (SPEC-29 §7). */}
      <button
        type="button"
        className="fixed bottom-20 right-4 z-20 flex h-14 w-14 items-center justify-center rounded-full bg-sky-600 text-3xl text-white shadow-lg md:hidden"
        aria-label="Добавить товар"
        onClick={() => setEditing('new')}
      >
        +
      </button>
      {editing && (
        <ProductFormModal
          product={editing === 'new' ? undefined : editing}
          onClose={() => {
            setEditing(null)
            list.reload()
          }}
          onSaved={onSaved}
        />
      )}
    </div>
  )
}

/** Строка списка: на телефоне ячейки подписаны (`data-label`), на компьютере — таблица. */
function ProductTableRow({
  p,
  owner,
  category,
  selected,
  onOpen,
  onToggle,
  onArchive,
  onStockIn,
}: {
  p: ProductRow
  owner: boolean
  category: string
  selected: boolean
  onOpen: () => void
  onToggle: (on: boolean) => void
  onArchive: () => void
  onStockIn: () => void
}) {
  // Розлив за литр — от цены канистры (ADR-074).
  const pour = pourPricePerL(p)
  const minus = p.stock_qty < 0
  return (
    <tr className={`cursor-pointer hover:bg-slate-50 ${p.archived ? 'text-slate-400' : ''}`} onClick={onOpen}>
      <td className="px-2 py-2" data-label="" onClick={(e) => e.stopPropagation()}>
        <input type="checkbox" className="h-4 w-4" aria-label="Выбрать для этикеток" checked={selected} onChange={(e) => onToggle(e.target.checked)} />
      </td>
      <td className="px-2 py-2 md:min-w-[18rem]" data-label="Название">
        <div className="font-medium">{productTitle(p)}</div>
        <div className="flex flex-wrap items-center gap-1 text-xs text-slate-500">
          {[p.brand, category].filter(Boolean).join(' · ')}
          {p.needs_review && <Badge tone="amber">проверить</Badge>}
          {p.archived && <Badge>архив</Badge>}
        </div>
      </td>
      <td className="px-2 py-2 whitespace-nowrap" data-label="Код">
        {p.code || '—'}
      </td>
      <td className={`px-2 py-2 text-right tabular-nums ${minus ? 'font-semibold text-rose-700' : ''}`} data-label="Остаток">
        {stockText(p)}
      </td>
      {owner && (
        <td className="px-2 py-2 whitespace-nowrap text-right tabular-nums" data-label="Закупка">
          {p.purchase_price_tyiyn != null ? formatSom(p.purchase_price_tyiyn) : '—'}
          {p.purchase_price_tyiyn != null && perText(p) && <span className="text-xs text-slate-500"> {perText(p)}</span>}
        </td>
      )}
      <td className="px-2 py-2 whitespace-nowrap text-right tabular-nums" data-label="Продажа">
        {formatSom(p.sale_price_tyiyn)}
        {perText(p) && <span className="text-xs text-slate-500"> {perText(p)}</span>}
        {pour !== null && <div className="text-xs text-slate-500">на розлив {formatSom(pour)} за л</div>}
      </td>
      {owner && (
        <td className={`px-2 py-2 whitespace-nowrap text-right tabular-nums ${minus ? 'text-rose-700' : ''}`} data-label="Сумма по закупке">
          {formatSom(p.stock_purchase_tyiyn ?? 0)}
        </td>
      )}
      <td className={`px-2 py-2 whitespace-nowrap text-right tabular-nums ${minus ? 'text-rose-700' : ''}`} data-label="Сумма по продаже">
        {formatSom(p.stock_sale_tyiyn)}
      </td>
      <td className="px-2 py-2 text-right" data-label="" onClick={(e) => e.stopPropagation()}>
        <RowMenu
          items={[
            { label: 'Открыть карточку', onClick: onOpen },
            ...(p.archived ? [] : [{ label: '+ Оприходовать', onClick: onStockIn }]),
            { label: selected ? 'Убрать из этикеток' : 'Печать этикетки', onClick: () => onToggle(!selected) },
            { label: p.archived ? 'Вернуть из архива' : 'В архив', danger: !p.archived, onClick: onArchive },
          ]}
        />
      </td>
    </tr>
  )
}
