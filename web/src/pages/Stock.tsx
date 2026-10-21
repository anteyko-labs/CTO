// Остатки филиала: фильтры «мало» и «проверить», себестоимость и сверка с движениями — только владельцу (SPEC-03).
// Вкладка «Брак» и кнопка «В брак» в строке товара — SPEC-25.
import { useState } from 'react'
import { DefectMarkModal, DefectsTab } from '../components/Defects'
import { Pager, usePager } from '../components/Pager'
import { ProductFormModal } from '../components/ProductFormModal'
import { stockText } from '../components/ProductPicker'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, PageHeader, Table, toast } from '../components/ui'
import { get, newOpId, post, put, qs } from '../lib/api'
import { polarityMark, productTitle } from '../lib/battery'
import { useUser } from '../lib/auth'
import { formatSom, parseSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { avgPrice, qtyText } from '../lib/units'
import { nextStockSort, sortStock, type StockSort, type StockSortKey } from '../lib/stockSort'
import type { Category, Product, StockMismatch } from '../lib/types'

type Mismatch = StockMismatch & { product?: Product }

const minText = (p: Product): string => qtyText(p.unit, p.min_stock)

/** Остаток коротко для телефона: без расшифровки по канистрам. */
const stockShort = (p: Product): string => qtyText(p.unit, p.stock_qty)

/** Средняя цена: весовой товар — за кг, масло — за канистру, остальное — за шт. */
function avgCell(p: Product) {
  const a = avgPrice(p)
  if (!a) return '—'
  return (
    <>
      {formatSom(a.tyiyn)}
      <span className="text-xs text-slate-500"> {a.per}</span>
    </>
  )
}

/** Строка под остатком на телефоне: «минимум 5 кг · средняя цена 80 с за кг». */
function mobileDetails(p: Product, owner: boolean): string {
  const a = owner ? avgPrice(p) : null
  return [p.min_stock > 0 ? `минимум ${minText(p)}` : '', a ? `средняя цена ${formatSom(a.tyiyn)} ${a.per}` : ''].filter(Boolean).join(' · ')
}

export default function Stock() {
  const owner = useUser().role === 'owner'
  const mayDefect = useCan()('defects')
  const [q, setQ] = useState('')
  const [categoryId, setCategoryId] = useState('')
  const [filter, setFilter] = useState<'all' | 'low' | 'out' | 'review' | 'stale'>('all')
  // Полярность аккумуляторов: «Л» прямая, «П» обратная (SPEC-27).
  const [polarity, setPolarity] = useState<'' | 'Л' | 'П'>('')
  // Залежалый товар: остаток есть, продаж нет дольше срока (ADR-039).
  const staleList = useLoad(
    () => get<{ stale_days: number; items: { product_id: string; days: number; sold_ever: boolean }[] }>('/stock/stale'),
    [],
  )
  const [staleDays, setStaleDays] = useState('')
  const staleSave = useAction()
  const [editing, setEditing] = useState<Product | null>(null)
  const [tab, setTab] = useState<'stock' | 'defects'>('stock')
  // Сортировка по заголовкам: название, остаток, стоимость (стоимость — только владельцу).
  const [sort, setSort] = useState<StockSort>({ key: null, desc: false })
  const [marking, setMarking] = useState<Product | null>(null)
  const query = useDebounced(q.trim())
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const stock = useLoad(() => get<Product[]>(`/stock${qs({ q: query, category_id: categoryId })}`), [query, categoryId])
  const [mismatches, setMismatches] = useState<Mismatch[] | null>(null)
  const verify = useAction()
  const review = useAction()
  // Цена закупки товара, проданного без неё: 2 % кассира начислятся после неё (ADR-062).
  const [pricing, setPricing] = useState<{ product: Product; text: string; opId: string } | null>(null)
  const priceAct = useAction()

  const loaded = stock.data ?? []
  const hasPolarity = loaded.some((p) => polarityMark(p.attrs) !== null)
  const all = polarity ? loaded.filter((p) => polarityMark(p.attrs) === polarity) : loaded
  // «Нет» — остаток ноль или минус; «мало» — есть, но меньше минимума.
  const isOut = (p: Product) => p.stock_qty <= 0
  const isLow = (p: Product) => p.stock_qty > 0 && p.stock_qty < p.min_stock
  const lowCount = all.filter(isLow).length
  const outCount = all.filter(isOut).length
  const reviewCount = all.filter((p) => p.needs_review).length
  const stale = new Map((staleList.data?.items ?? []).map((i) => [i.product_id, i]))
  const filtered =
    filter === 'low'
      ? all.filter(isLow)
      : filter === 'out'
        ? all.filter(isOut)
        : filter === 'review'
        ? all.filter((p) => p.needs_review)
          : filter === 'stale'
            ? all.filter((p) => stale.has(p.id))
            : all
  const rows = sortStock(filtered, owner || sort.key !== 'value' ? sort : { key: null, desc: false })
  const categoryName = new Map((categories.data ?? []).map((c) => [c.id, c.name]))
  // Стоимость — по всему отбору; на экране — страница. Смена отбора — первая страница.
  const totalValue = rows.reduce((acc, p) => acc + (p.stock_value_tyiyn ?? 0), 0)
  const pager = usePager(rows, 'stock', `${query}:${categoryId}:${filter}:${polarity}:${sort.key ?? ''}:${sort.desc}`)

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

  const savePrice = () => {
    if (!pricing) return
    const price = parseSom(pricing.text)
    if (price === null || price <= 0) {
      priceAct.setError('Укажите цену закупки')
      return
    }
    void priceAct.run(async () => {
      const r = await post<{ lines: number; accrued_tyiyn: number }>(`/stock/${pricing.product.id}/purchase-price`, {
        op_id: pricing.opId,
        price_tyiyn: price,
      })
      setPricing(null)
      stock.reload()
      toast(`Цена закупки указана: строк чеков ${r.lines}, кассирам начислено ${formatSom(r.accrued_tyiyn)}`)
    })
  }
  const priceUnit = (p: Product) => (p.unit === 'g' ? 'за кг' : p.container_ml ? 'за канистру' : 'за штуку')
  /** Владелец: без цены закупки отметку «проверить» не снять — сначала цена. */
  const ownerAction = (p: Product, cls: string) =>
    !owner ? null : p.cost_pending ? (
      <Button
        variant="secondary"
        className={cls}
        onClick={(e) => {
          e.stopPropagation()
          priceAct.setError(null)
          setPricing({ product: p, text: '', opId: newOpId() })
        }}
      >
        Цена закупки
      </Button>
    ) : p.needs_review ? (
      <Button
        variant="secondary"
        className={cls}
        disabled={review.busy}
        onClick={(e) => {
          e.stopPropagation()
          markReviewed(p)
        }}
      >
        Проверено
      </Button>
    ) : null

  /** «В брак» — владелец и администратор с правом (ADR-097), только из того, что есть на складе. */
  const defectButton = (p: Product, cls: string) =>
    p.stock_qty > 0 && mayDefect ? (
      <Button
        variant="secondary"
        className={cls}
        onClick={(e) => {
          e.stopPropagation()
          setMarking(p)
        }}
      >
        В брак
      </Button>
    ) : null

  const rowTone = (p: Product) => (isOut(p) ? 'bg-rose-50 hover:bg-rose-100' : isLow(p) ? 'bg-amber-50 hover:bg-amber-100' : 'hover:bg-slate-50')
  const qtyTone = (p: Product) => (isOut(p) ? 'text-rose-700' : isLow(p) ? 'text-amber-800' : '')
  const badges = (p: Product) => (
    <>
      {isOut(p) && <Badge tone="rose">нет</Badge>}
      {isLow(p) && <Badge tone="amber">мало</Badge>}
      {p.needs_review && <Badge tone="rose">проверить</Badge>}
      {p.cost_pending && <Badge tone="amber">без цены закупки</Badge>}
      {stale.has(p.id) && (
        <Badge tone="slate">
          {stale.get(p.id)?.sold_ever ? 'не продавался' : 'ни разу не продан'} {stale.get(p.id)?.days} дн.
        </Badge>
      )}
    </>
  )

  /** Заголовок-кнопка: щелчок сортирует, повторный — в обратную сторону. */
  const sortHead = (key: StockSortKey, label: string, right = false) => (
    <button
      key={key}
      type="button"
      aria-label={`${label}: сортировать${sort.key === key ? (sort.desc ? ', сейчас по убыванию' : ', сейчас по возрастанию') : ''}`}
      className={`inline-flex items-center gap-1 uppercase hover:text-slate-800 ${right ? 'w-full justify-end' : ''} ${sort.key === key ? 'text-slate-800' : ''}`}
      onClick={() => setSort((s) => nextStockSort(s, key))}
    >
      {label}
      <span aria-hidden className="text-[10px]">
        {sort.key === key ? (sort.desc ? '▼' : '▲') : '↕'}
      </span>
    </button>
  )
  const head = [sortHead('name', 'Товар'), 'Артикул', 'Категория', sortHead('qty', 'Остаток', true), <span key="min" className="block text-right">Минимум</span>, '']
  if (owner) head.splice(5, 0, <span key="avg" className="block text-right">Средняя цена</span>, sortHead('value', 'Стоимость', true))
  const sortOptions: [string, string][] = [
    ['', 'как есть'],
    ['name:asc', 'по названию'],
    ['qty:desc', 'остаток: больше сверху'],
    ['qty:asc', 'остаток: меньше сверху'],
    ...(owner ? ([['value:desc', 'стоимость: больше сверху']] as [string, string][]) : []),
  ]

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
      <div className="mb-3 flex gap-2" role="tablist">
        {(
          [
            ['stock', 'Остатки'],
            ['defects', 'Брак'],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`min-h-[38px] rounded-md border px-4 py-2 text-sm font-medium ${tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setTab(key)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'defects' ? (
        <DefectsTab />
      ) : (
      <Card>
        <div className={`mb-3 grid grid-cols-1 gap-3 sm:items-end ${hasPolarity || polarity ? 'sm:grid-cols-[2fr_1fr_1fr]' : 'sm:grid-cols-[2fr_1fr]'}`}>
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
          {(hasPolarity || polarity) && (
            <Field label="Полярность">
              <select value={polarity} onChange={(e) => setPolarity(e.target.value as '' | 'Л' | 'П')}>
                <option value="">Все</option>
                <option value="Л">прямая (+ слева, Л)</option>
                <option value="П">обратная (+ справа, П)</option>
              </select>
            </Field>
          )}
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
            <label className="mb-2 flex items-center justify-end gap-2 text-sm text-slate-600 md:hidden">
              Сортировка
              <select
                className="w-auto"
                value={sort.key ? `${sort.key}:${sort.desc ? 'desc' : 'asc'}` : ''}
                onChange={(e) => {
                  const [key, dir] = e.target.value.split(':')
                  setSort(key ? { key: key as StockSortKey, desc: dir === 'desc' } : { key: null, desc: false })
                }}
              >
                {sortOptions.map(([v, label]) => (
                  <option key={v} value={v}>
                    {label}
                  </option>
                ))}
              </select>
            </label>
            <ul className="-mx-4 divide-y divide-slate-100 border-y border-slate-100 md:hidden">
              {pager.shown.map((p) => (
                <li key={p.id} className={rowTone(p)}>
                  <button type="button" className="flex w-full items-start gap-3 px-4 py-2.5 text-left" onClick={() => setEditing(p)}>
                    <div className="min-w-0 flex-1">
                      <div className="font-semibold leading-snug">{productTitle(p)}</div>
                      <div className="mt-0.5 truncate text-xs text-slate-500">
                        {[p.article, categoryName.get(p.category_id)].filter(Boolean).join(' · ') || '—'}
                      </div>
                      {(isOut(p) || isLow(p) || p.needs_review || stale.has(p.id)) && <div className="mt-1 flex flex-wrap gap-1">{badges(p)}</div>}
                      {mobileDetails(p, owner) && <div className="mt-0.5 text-xs text-slate-500">{mobileDetails(p, owner)}</div>}
                    </div>
                    <div className={`shrink-0 text-right text-lg font-bold tabular-nums ${qtyTone(p)}`}>{stockShort(p)}</div>
                  </button>
                  {((owner && (p.needs_review || p.cost_pending)) || p.stock_qty > 0) && (
                    <div className="flex gap-2 px-4 pb-2.5">
                      {owner && ownerAction(p, 'min-h-[36px] py-1 text-xs')}
                      {defectButton(p, 'min-h-[36px] py-1 text-xs')}
                    </div>
                  )}
                </li>
              ))}
            </ul>
            <div className="hidden md:block">
              <Table head={head}>
                {pager.shown.map((p) => (
                  <tr key={p.id} className={`cursor-pointer ${rowTone(p)}`} onClick={() => setEditing(p)}>
                    <td className="px-2 py-2">
                      <div className="font-medium">{productTitle(p)}</div>
                      <div className="mt-1 flex flex-wrap gap-1">{badges(p)}</div>
                    </td>
                    <td className="px-2 py-2">{p.article || '—'}</td>
                    <td className="px-2 py-2">{categoryName.get(p.category_id) ?? '—'}</td>
                    <td className={`whitespace-nowrap px-2 py-2 text-right font-semibold tabular-nums ${qtyTone(p)}`}>{stockText(p)}</td>
                    <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums text-slate-500">{minText(p)}</td>
                    {owner && (
                      <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">
                        {avgCell(p)}
                      </td>
                    )}
                    {owner && (
                      <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">
                        {p.stock_value_tyiyn != null ? formatSom(p.stock_value_tyiyn) : '—'}
                      </td>
                    )}
                    <td className="whitespace-nowrap px-2 py-2 text-right">
                      <div className="flex justify-end gap-2">
                        {ownerAction(p, '')}
                        {defectButton(p, '')}
                      </div>
                    </td>
                  </tr>
                ))}
              </Table>
            </div>
            <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы остатков" />
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
      )}

      {marking && (
        <DefectMarkModal
          product={marking}
          onClose={() => setMarking(null)}
          onDone={() => {
            setMarking(null)
            stock.reload()
          }}
        />
      )}

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

      {pricing && (
        <Modal title={`Цена закупки · ${pricing.product.name}`} onClose={() => setPricing(null)}>
          <div className="flex flex-col gap-3">
            <p className="text-sm text-slate-600">
              Товар продали без цены закупки: себестоимость этих строк неизвестна, и 2 % кассиру с них ещё не начислены. Укажите, почём товар
              закупается, — процент начислится сегодня, прошлые дни не изменятся.
            </p>
            <Field label={`Цена закупки ${priceUnit(pricing.product)}, с`} required>
              <input
                autoFocus
                inputMode="decimal"
                value={pricing.text}
                onChange={(e) => setPricing({ ...pricing, text: e.target.value })}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') savePrice()
                }}
              />
            </Field>
            <ErrorBox error={priceAct.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setPricing(null)}>
                Отмена
              </Button>
              <Button disabled={priceAct.busy || !pricing.text.trim()} onClick={savePrice}>
                Сохранить
              </Button>
            </div>
          </div>
        </Modal>
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
  const pager = usePager(rows, 'stock.mismatches')
  return (
    <>
      <Table head={['Товар', 'Остаток в учёте', 'По движениям', 'Стоимость в учёте', 'По движениям']}>
        {pager.shown.map((m) => {
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
      <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы расхождений" />
    </>
  )
}
