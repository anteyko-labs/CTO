import { useMemo, useState } from 'react'
import { printLabels } from '../components/labels'
import { ProductFormModal } from '../components/ProductFormModal'
import { stockText } from '../components/ProductPicker'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, PageHeader, Table } from '../components/ui'
import { get, patch, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { Category, Product } from '../lib/types'

interface Selected {
  product: Product
  copies: string
}

export default function Products() {
  const owner = useUser().role === 'owner'
  const [q, setQ] = useState('')
  const [categoryId, setCategoryId] = useState('')
  const [attrFilters, setAttrFilters] = useState<Record<string, string>>({})
  const [archived, setArchived] = useState(false)
  const [editing, setEditing] = useState<Product | 'new' | null>(null)
  const [selected, setSelected] = useState<Record<string, Selected>>({})
  const [message, setMessage] = useState<string | null>(null)
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
      limit: 200,
    }
    for (const [k, v] of Object.entries(attrs)) p[`attr.${k}`] = v.trim()
    return qs(p)
  }, [query, categoryId, archived, attrs])
  const products = useLoad(() => get<Product[]>(`/products${params}`), [params])

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
    product: products.data?.find((p) => p.id === s.product.id) ?? s.product,
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
      products.reload()
    })

  const onSaved = (p: Product) => {
    setEditing(null)
    setSelected((s) => (s[p.id] ? { ...s, [p.id]: { ...s[p.id], product: p } } : s))
    products.reload()
  }

  return (
    <div>
      <PageHeader title="Товары" actions={<Button onClick={() => setEditing('new')}>Новый товар</Button>} />

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
                      {o}
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
                <span className="min-w-0 flex-1">{s.product.name}</span>
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
        <ErrorBox error={products.error ?? categories.error} />
        {products.loading && !products.data ? (
          <Loading />
        ) : !products.data?.length ? (
          <Empty action={<Button onClick={() => setEditing('new')}>Новый товар</Button>}>Товары не найдены</Empty>
        ) : (
          <Table
            head={['', 'Название', 'Артикул', 'Категория', 'Цена', 'Остаток', ...(owner ? ['Себестоимость'] : []), '']}
          >
            {products.data.map((p) => {
              const oil = p.unit === 'ml'
              return (
                <tr
                  key={p.id}
                  className={`cursor-pointer hover:bg-slate-50 ${p.archived ? 'text-slate-400' : ''}`}
                  onClick={() => setEditing(p)}
                >
                  <td className="px-2 py-2" onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      className="h-4 w-4"
                      aria-label="Выбрать для этикеток"
                      checked={Boolean(selected[p.id])}
                      onChange={(e) => toggle(p, e.target.checked)}
                    />
                  </td>
                  <td className="px-2 py-2">
                    <div className="font-medium">{p.name}</div>
                    <div className="flex flex-wrap items-center gap-1 text-xs text-slate-500">
                      {p.brand}
                      {p.needs_review && <Badge tone="amber">проверить</Badge>}
                      {p.archived && <Badge>архив</Badge>}
                    </div>
                  </td>
                  <td className="px-2 py-2 whitespace-nowrap">{p.article || '—'}</td>
                  <td className="px-2 py-2">{categoryName(p.category_id)}</td>
                  <td className="px-2 py-2 whitespace-nowrap">
                    {formatSom(p.sale_price_tyiyn)}
                    {oil && <span className="text-xs text-slate-500"> за канистру</span>}
                    {oil && p.pour_price_per_l_tyiyn != null && (
                      <div className="text-xs text-slate-500">розлив {formatSom(p.pour_price_per_l_tyiyn)} за л</div>
                    )}
                  </td>
                  <td className="px-2 py-2">{stockText(p)}</td>
                  {owner && (
                    <td className="px-2 py-2 whitespace-nowrap">
                      {p.avg_cost_tyiyn != null ? formatSom(p.avg_cost_tyiyn) : '—'}
                      {oil && p.avg_cost_tyiyn != null && <span className="text-xs text-slate-500"> за канистру</span>}
                    </td>
                  )}
                  <td className="px-2 py-2 text-right" onClick={(e) => e.stopPropagation()}>
                    <Button
                      variant="ghost"
                      className="px-2 py-1 text-xs"
                      disabled={busy}
                      onClick={() => {
                        if (p.archived || window.confirm(`Убрать «${p.name}» в архив? Он пропадёт из поиска в кассе; вернуть можно через «Показать архив».`)) {
                          void setArchivedFlag(p, !p.archived)
                        }
                      }}
                    >
                      {p.archived ? 'Вернуть' : 'В архив'}
                    </Button>
                  </td>
                </tr>
              )
            })}
          </Table>
        )}
      </Card>

      {editing && (
        <ProductFormModal
          product={editing === 'new' ? undefined : editing}
          onClose={() => {
            setEditing(null)
            products.reload()
          }}
          onSaved={onSaved}
        />
      )}
    </div>
  )
}
