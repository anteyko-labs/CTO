// Справочник товаров: поиск, фильтры по характеристикам, карточка, архив, печать этикеток (SPEC-02).
// Список «как в Умаге» (SPEC-27): остаток, цены и суммы остатка с итогами по отбору — итоги считает
// сервер; закупку и стоимость остатка видит только владелец (ADR-008).
// Бывший экран «Остатки» (SPEC-03) — здесь же (SPEC-30 §11): отборы «ниже минимума», «нет»,
// залежалые и «проверить», цена закупки и «Проверено», «В брак» и вкладка «Брак» (SPEC-25),
// сверка остатков с движениями — владельцу.
// Старая база Умаг здесь видна (SPEC-35 §2): при поиске под списком — её строки, ещё не заведённые;
// скан кода (Enter) — товар, окно строки Умага или «Код не найден»; «Связать с Умагом» — /products/umag.
import { useMemo, useState, type ReactNode } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { DefectMarkModal, DefectsTab } from '../components/Defects'
import { printLabels } from '../components/labels'
import { Pager, usePageState, usePager } from '../components/Pager'
import { ProductFormModal } from '../components/ProductFormModal'
import { stockText } from '../components/ProductPicker'
import { UmagHitModal, UmagSearchBlock } from '../components/Umag'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Modal, PageHeader, RowMenu, Table, toast } from '../components/ui'
import { ApiError, get, newOpId, patch, post, put, qs } from '../lib/api'
import { POLARITY_LABELS, productTitle } from '../lib/battery'
import { useUser } from '../lib/auth'
import { formatLiters, formatSom, parseSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { pourPricePerL } from '../lib/money'
import { qtyText } from '../lib/units'
import type { Category, OldCatalogHit, Product, ProductList, ProductRow, StockChip, StockMismatch } from '../lib/types'
import { findByBarcode, readSnapshot } from '../lib/offline'
import { offlineCodeText, resolveCode, type CodeOutcome } from '../lib/umagLink'
import { BranchBreakdown, BranchScope } from '../components/BranchScope'
import { useAllBranches, type AllBranches } from '../lib/branchReport'
import { useUmagOn } from '../lib/umagSwitch'

interface Selected {
  product: Product
  copies: string
}

type SortKey = 'name' | 'code' | 'stock' | 'purchase' | 'sale' | 'stock_purchase' | 'stock_sale'

type Mismatch = StockMismatch & { product?: Product }

/** Отборы по складу: подпись кнопки и что сказать, если пусто. */
const CHIPS: { key: StockChip; label: string; empty: string }[] = [
  { key: 'all', label: 'Все', empty: 'Товары не найдены' },
  { key: 'low', label: 'Ниже минимума', empty: 'Ничего не заканчивается: остатки не ниже минимума.' },
  { key: 'out', label: 'Нет на складе', empty: 'Все товары в наличии.' },
  { key: 'stale', label: 'Залежалые', empty: 'Залежалых товаров нет.' },
  { key: 'review', label: 'Проверить', empty: 'Проверять нечего.' },
]

const chipOf = (v: string | null): StockChip => CHIPS.find((c) => c.key === v)?.key ?? 'all'

/** Сортировка на телефоне: заголовков таблицы там нет. */
const MOBILE_SORTS: { value: string; label: string; owner?: boolean }[] = [
  { value: 'name:asc', label: 'по названию' },
  { value: 'stock:desc', label: 'остаток: больше сверху' },
  { value: 'stock:asc', label: 'остаток: меньше сверху' },
  { value: 'stock_purchase:desc', label: 'по закупке: больше сверху', owner: true },
  { value: 'stock_sale:desc', label: 'по продаже: больше сверху' },
]

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
  const user = useUser()
  const owner = user.role === 'owner'
  // «Все филиалы» (SPEC-33 §3): остаток и суммы — сложенные по филиалам, страницы режет экран.
  const [all, setAll] = useAllBranches(owner && Boolean(user.several_branches))
  // Заводить товары — право администратора «Товары и штрихкоды» (ADR-097).
  const mayAdd = useCan()('products')
  // Выключенный Умаг (SPEC-36 §15) — без «Связать с Умагом».
  const umagOn = useUmagOn()
  // Отбор по складу и вкладка — в адресе: ссылки сводки и старый адрес «Остатков» ведут сюда.
  const [search, setSearch] = useSearchParams()
  const chip = chipOf(search.get('stock'))
  const tab = search.get('tab') === 'defects' ? 'defects' : 'products'
  const setUrl = (changes: Record<string, string | null>) =>
    setSearch(
      (cur) => {
        const next = new URLSearchParams(cur)
        for (const [k, v] of Object.entries(changes)) {
          if (v === null) next.delete(k)
          else next.set(k, v)
        }
        return next
      },
      { replace: true },
    )
  const [q, setQ] = useState(() => search.get('q') ?? '')
  const [categoryId, setCategoryId] = useState(() => search.get('category_id') ?? '')
  const [attrFilters, setAttrFilters] = useState<Record<string, string>>({})
  const [archived, setArchived] = useState(false)
  const [editing, setEditing] = useState<Product | 'new' | null>(null)
  /** Новый товар с отсканированным кодом («Код не найден» → «Новый товар»). */
  const [newCode, setNewCode] = useState<string | undefined>(undefined)
  /** Строка Умага: скан незаведённого кода или «Завести» / «Это уже есть» под списком (SPEC-35 §2). */
  const [umagHit, setUmagHit] = useState<{ hit: OldCatalogHit; code: string; start: 'info' | 'create' | 'pick' } | null>(null)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  /** Блок Умага перечитывается после заведения и связывания. */
  const [umagVersion, setUmagVersion] = useState(0)
  const scan = useAction()
  /** «+ Оприходовать» у товара (SPEC-29 §8). */
  // «+ Оприходовать» — экран «Новый приход» в виде оприходования с этим товаром (ADR-086).
  const navigate = useNavigate()
  const [selected, setSelected] = useState<Record<string, Selected>>({})
  const [message, setMessage] = useState<string | null>(null)
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: 'name', desc: false })
  const { busy, error, run } = useAction()
  const [marking, setMarking] = useState<Product | null>(null)
  // Цена закупки товара, проданного без неё: 2 % кассира начислятся после неё (ADR-062).
  const [pricing, setPricing] = useState<{ product: Product; text: string; opId: string } | null>(null)
  const priceAct = useAction()
  const review = useAction()
  const [mismatches, setMismatches] = useState<Mismatch[] | null>(null)
  const verify = useAction()
  const [staleDays, setStaleDays] = useState('')
  const staleSave = useAction()

  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const category = categories.data?.find((c) => c.id === categoryId)
  const filterDefs = category?.attributes.filter((a) => a.filterable) ?? []
  // Полярность аккумуляторов — и без выбранной категории, как было в «Остатках» (SPEC-27), если в
  // списке есть аккумуляторы; в категории — только у аккумуляторов (её собственный признак).
  const polarityDef = categoryId
    ? undefined
    : categories.data?.flatMap((c) => c.attributes).find((a) => a.key === 'polarity' && a.filterable && a.type === 'select')

  const query = useDebounced(q.trim(), 300)
  const attrs = useDebounced(attrFilters, 300)
  const params = useMemo(() => {
    const p: Record<string, string | boolean | number | undefined> = {
      q: query,
      category_id: categoryId,
      archived: archived || undefined,
      sort: sort.key,
      dir: sort.desc ? 'desc' : undefined,
      stock: chip === 'all' ? undefined : chip,
    }
    for (const [k, v] of Object.entries(attrs)) p[`attr.${k}`] = v.trim()
    return qs(p)
  }, [query, categoryId, archived, attrs, sort, chip])
  // Страницу режет сервер, итоги — по всему отбору; смена отбора или сортировки — снова первая страница.
  const pager = usePageState('products', params)
  const list = useLoad(
    () =>
      all
        ? get<AllBranches<ProductList>>(`/products/list${params}${params ? '&' : '?'}branch=all`)
        : get<AllBranches<ProductList>>(`/products/list${params}${params ? '&' : '?'}limit=${pager.size}&offset=${pager.page * pager.size}`),
    [params, pager.page, pager.size, all],
  )
  const items = all ? list.data?.items.slice(pager.page * pager.size, (pager.page + 1) * pager.size) : list.data?.items
  const totals = list.data?.totals
  const counts = totals?.counts

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
    setNewCode(undefined)
    setSelected((s) => (s[p.id] ? { ...s, [p.id]: { ...s[p.id], product: p } } : s))
    list.reload()
    setUmagVersion((v) => v + 1)
  }

  /**
   * Enter в поиске — скан или набранный код: товар (по коду или артикулу) — его карточка; код из
   * Умага — связанная строка даёт товар, несвязанная — окно Умага; нигде нет — «Код не найден»
   * (SPEC-35 §2). Без сети — снимок каталога; несвязанную строку Умага без сети не узнать.
   */
  const onSearchEnter = () => {
    const code = q.trim()
    if (!/^\S{3,}$/.test(code)) return
    void scan.run(async () => {
      let out: CodeOutcome
      try {
        out = await resolveCode(code)
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 0)) throw e
        const snap = await readSnapshot()
        const found = snap ? findByBarcode(snap, code) : undefined
        if (found) {
          setEditing(found)
          return
        }
        throw new Error(umagOn ? offlineCodeText(code) : `Без сети: код ${code} в снимке каталога не найден`)
      }
      if (out.kind === 'product') setEditing(out.product)
      else if (out.kind === 'umag') setUmagHit({ hit: out.hit, code, start: 'info' })
      else if (out.kind === 'unknown' && /^\d{4,}$/.test(code)) {
        if (mayAdd) setUnknownCode(code)
        else throw new Error(`Код ${code} не найден ни у товаров, ни в базе Умаг`)
      }
    })
  }

  const runVerify = () => {
    void verify.run(async () => {
      const found = await get<StockMismatch[]>('/stock/verify')
      // Названия товаров для расхождений; список обычно пуст.
      const out: Mismatch[] = []
      for (const m of found) {
        const product = await get<Product>(`/products/${m.product_id}`).catch(() => undefined)
        out.push({ ...m, product })
      }
      setMismatches(out)
    })
  }

  const markReviewed = (p: Product) =>
    void review.run(async () => {
      await post(`/stock/${p.id}/review-done`)
      list.reload()
    })

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
      list.reload()
      toast(`Цена закупки указана: строк чеков ${r.lines}, кассирам начислено ${formatSom(r.accrued_tyiyn)}`)
    })
  }
  const priceUnit = (p: Product) => (p.unit === 'g' ? 'за кг' : p.container_ml ? 'за канистру' : 'за штуку')
  const emptyText = CHIPS.find((c) => c.key === chip)?.empty ?? 'Товары не найдены'
  const sortValue = `${sort.key}:${sort.desc ? 'desc' : 'asc'}`

  return (
    // Снизу место под «+» на телефоне: последние карточки не прячутся под кнопкой.
    <div className="pb-20 md:pb-0">
      <PageHeader
        title="Товары"
        actions={
          <>
            {owner && (
              <Button variant="secondary" disabled={verify.busy} onClick={runVerify} title="Остатки в учёте против суммы движений">
                Сверить остатки
              </Button>
            )}
            {mayAdd && umagOn && (
              <Button variant="secondary" onClick={() => navigate('/products/umag')} title="Связать товары со строками старой базы Умаг: их коды станут кодами товаров">
                Связать с Умагом
              </Button>
            )}
            {mayAdd && (
              <Button onClick={() => setEditing('new')} aria-label="Добавить товар" title="Добавить товар: полная карточка">
                + Товар
              </Button>
            )}
          </>
        }
      />
      <div className="mb-3 flex gap-2" role="tablist">
        {(
          [
            ['products', 'Товары и остатки'],
            ['defects', 'Брак'],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            className={`min-h-[38px] rounded-md border px-4 py-2 text-sm font-medium ${tab === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white hover:bg-slate-50'}`}
            onClick={() => setUrl({ tab: key === 'defects' ? 'defects' : null })}
          >
            {label}
          </button>
        ))}
      </div>
      <div className="mb-3 flex flex-col gap-2 empty:hidden">
        <ErrorBox error={verify.error ?? review.error ?? scan.error} />
      </div>
      {tab === 'products' && <BranchScope user={user} all={all} onChange={setAll} />}
      {tab === 'products' && all && (
        <BranchBreakdown
          parts={list.data?.branches}
          columns={[
            ...(owner ? [{ label: 'По закупке', value: (x: ProductList) => x.totals.purchase_tyiyn ?? 0 }] : []),
            { label: 'По продаже', value: (x: ProductList) => x.totals.sale_tyiyn },
          ]}
        />
      )}
      {tab === 'defects' ? (
        <DefectsTab />
      ) : (
        <>
      <Card className="mb-4">
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <Field label="Поиск">
            <input
              placeholder="Название, бренд, артикул, штрихкод"
              value={q}
              onChange={(e) => {
                setQ(e.target.value)
                scan.setError(null)
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  onSearchEnter()
                }
              }}
            />
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
          {/* Без категории — только если в списке есть аккумуляторы (или отбор уже выбран). */}
          {polarityDef && ((list.data?.items ?? []).some((it) => it.category_kind === 'battery') || Boolean(attrFilters.polarity)) && (
            <Field label="Полярность">
              <select value={attrFilters.polarity ?? ''} onChange={(e) => setAttrFilters((f) => ({ ...f, polarity: e.target.value }))}>
                <option value="">Все</option>
                {polarityDef.options?.map((o) => (
                  <option key={o} value={o}>
                    {POLARITY_LABELS[o] ?? o}
                  </option>
                ))}
              </select>
            </Field>
          )}
        </div>
        <div className="-mx-1 mt-3 flex gap-2 overflow-x-auto px-1 pb-1 text-sm" role="group" aria-label="Отбор по складу">
          {CHIPS.map(({ key, label }) => (
            <button
              key={key}
              type="button"
              aria-pressed={chip === key}
              className={`inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-3 py-1.5 ${
                chip === key ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white text-slate-700 hover:bg-slate-50'
              }`}
              onClick={() => setUrl({ stock: key === 'all' ? null : key })}
            >
              {label}
              {counts && <span className={`tabular-nums ${chip === key ? 'text-sky-100' : 'text-slate-400'}`}>{counts[key]}</span>}
            </button>
          ))}
        </div>
        {chip === 'stale' && totals?.stale_days !== undefined && (
          <div className="mt-2 flex flex-wrap items-center gap-2 text-sm text-slate-600">
            Залежалым считается товар без продаж {totals.stale_days} дней и больше.
            {owner && (
              <>
                <input
                  className="w-20"
                  aria-label="Срок залежалости, дней"
                  inputMode="numeric"
                  placeholder={String(totals.stale_days)}
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
                      list.reload()
                    })
                  }
                >
                  Изменить срок
                </Button>
              </>
            )}
            <ErrorBox error={staleSave.error} />
          </div>
        )}
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
          <Empty icon="stock" action={chip === 'all' && mayAdd ? <Button onClick={() => setEditing('new')}>Новый товар</Button> : undefined}>
            {emptyText}
          </Empty>
        ) : (
          <>
            <label className="mb-2 flex items-center justify-end gap-2 text-sm text-slate-600 md:hidden">
              Сортировка
              <select
                className="w-auto"
                value={MOBILE_SORTS.some((o) => o.value === sortValue) ? sortValue : ''}
                onChange={(e) => {
                  const [key, dir] = e.target.value.split(':')
                  if (key) setSort({ key: key as SortKey, desc: dir === 'desc' })
                }}
              >
                {!MOBILE_SORTS.some((o) => o.value === sortValue) && <option value="">как выбрано</option>}
                {MOBILE_SORTS.filter((o) => owner || !o.owner).map((o) => (
                  <option key={o.value} value={o.value}>
                    {o.label}
                  </option>
                ))}
              </select>
            </label>
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
                      onStockIn={() => navigate(`/receipts/new?kind=stock_in&product=${p.id}`)}
                      onDefect={() => setMarking(p)}
                      ownerAction={
                        !owner ? null : p.cost_pending ? (
                          <Button
                            variant="secondary"
                            className="min-h-[32px] px-2 py-1 text-xs"
                            onClick={() => {
                              priceAct.setError(null)
                              setPricing({ product: p, text: '', opId: newOpId() })
                            }}
                          >
                            Цена закупки
                          </Button>
                        ) : p.needs_review ? (
                          <Button variant="secondary" className="min-h-[32px] px-2 py-1 text-xs" disabled={review.busy} onClick={() => markReviewed(p)}>
                            Проверено
                          </Button>
                        ) : null
                      }
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
      {query && <UmagSearchBlock query={query} version={umagVersion} onOpen={(hit, start) => setUmagHit({ hit, code: '', start })} />}
        </>
      )}

      {/* «+» внизу экрана на телефоне: добавить товар (SPEC-29 §7). */}
      {mayAdd && (
        <button
          type="button"
          className="fixed bottom-20 right-3 z-20 flex h-12 w-12 items-center justify-center rounded-full bg-sky-600 text-2xl text-white opacity-90 shadow-lg md:hidden"
          aria-label="Добавить товар"
          onClick={() => setEditing('new')}
        >
          +
        </button>
      )}
      {editing && (
        <ProductFormModal
          product={editing === 'new' ? undefined : editing}
          presetBarcode={editing === 'new' ? newCode : undefined}
          // Новый товар при выбранной категории — сразу в ней (проверка 20.10.2026).
          presetCategoryId={editing === 'new' ? categoryId || undefined : undefined}
          onClose={() => {
            setEditing(null)
            setNewCode(undefined)
            list.reload()
            setUmagVersion((v) => v + 1)
          }}
          onSaved={onSaved}
        />
      )}
      {umagHit && (
        <UmagHitModal
          hit={umagHit.hit}
          code={umagHit.code}
          start={umagHit.start}
          onClose={() => setUmagHit(null)}
          onDone={() => {
            setUmagHit(null)
            list.reload()
            setUmagVersion((v) => v + 1)
          }}
        />
      )}
      {unknownCode !== null && (
        <UnknownCodeModal
          code={unknownCode}
          onClose={() => setUnknownCode(null)}
          onLinked={(p) => {
            setUnknownCode(null)
            toast(`Код привязан к «${p.name}»`)
            list.reload()
          }}
          onCreateNew={() => {
            setNewCode(unknownCode)
            setUnknownCode(null)
            setEditing('new')
          }}
        />
      )}
      {marking && (
        <DefectMarkModal
          product={marking}
          onClose={() => setMarking(null)}
          onDone={() => {
            setMarking(null)
            list.reload()
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
          {mismatches.length === 0 ? <div className="py-4 text-center text-emerald-700">Расхождений нет</div> : <MismatchTable rows={mismatches} />}
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
  onDefect,
  ownerAction,
}: {
  p: ProductRow
  owner: boolean
  category: string
  selected: boolean
  onOpen: () => void
  onToggle: (on: boolean) => void
  onArchive: () => void
  onStockIn: () => void
  onDefect: () => void
  /** «Цена закупки» или «Проверено» — владельцу, когда нужно. */
  ownerAction: ReactNode
}) {
  // Розлив за литр — от цены канистры (ADR-074).
  const pour = pourPricePerL(p)
  const minus = p.stock_qty < 0
  // Пункты меню строки — по правам администратора (ADR-097).
  const can = useCan()
  // Как было в «Остатках»: нет — красным, ниже минимума — жёлтым.
  const out = p.stock_qty <= 0
  const low = !out && p.stock_qty < p.min_stock
  const tone = p.archived
    ? 'text-slate-400 hover:bg-slate-50'
    : out && p.min_stock > 0
      ? 'bg-rose-50 hover:bg-rose-100'
      : low
        ? 'bg-amber-50 hover:bg-amber-100'
        : 'hover:bg-slate-50'
  return (
    <tr className={`cursor-pointer ${tone}`} onClick={onOpen}>
      <td className="px-2 py-2" data-label="" onClick={(e) => e.stopPropagation()}>
        <input type="checkbox" className="h-4 w-4" aria-label="Выбрать для этикеток" checked={selected} onChange={(e) => onToggle(e.target.checked)} />
      </td>
      <td className="px-2 py-2 md:min-w-[18rem]" data-label="Название">
        <div className="font-medium">{productTitle(p)}</div>
        <div className="flex flex-wrap items-center gap-1 text-xs text-slate-500">
          {[p.brand, category].filter(Boolean).join(' · ')}
          {out && !p.archived && <Badge tone="rose">нет</Badge>}
          {low && <Badge tone="amber">ниже минимума</Badge>}
          {p.needs_review && <Badge tone="rose">проверить</Badge>}
          {p.cost_pending && <Badge tone="amber">без цены закупки</Badge>}
          {p.stale && (
            <Badge tone="slate">
              {p.stale.sold_ever ? 'не продавался' : 'ни разу не продан'} {p.stale.days} дн.
            </Badge>
          )}
          {p.archived && <Badge>архив</Badge>}
        </div>
      </td>
      <td className="px-2 py-2 whitespace-nowrap" data-label="Код">
        {p.code || '—'}
      </td>
      <td className={`px-2 py-2 text-right tabular-nums ${minus || (out && p.min_stock > 0) ? 'font-semibold text-rose-700' : low ? 'font-semibold text-amber-800' : ''}`} data-label="Остаток">
        {stockText(p)}
        {p.min_stock > 0 && <div className="whitespace-nowrap text-xs font-normal text-slate-500">мин. {qtyText(p.unit, p.min_stock)}</div>}
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
        <div className="flex items-center justify-end gap-2">
          {ownerAction}
          <RowMenu
            items={[
              { label: 'Открыть карточку', onClick: onOpen },
              ...(p.archived || !can('stock_in') ? [] : [{ label: '+ Оприходовать', onClick: onStockIn }]),
              // «В брак» — только из того, что есть на складе (SPEC-25).
              ...(p.stock_qty > 0 && can('defects') ? [{ label: 'В брак', onClick: onDefect }] : []),
              { label: selected ? 'Убрать из этикеток' : 'Печать этикетки', onClick: () => onToggle(!selected) },
              ...(can('products') ? [{ label: p.archived ? 'Вернуть из архива' : 'В архив', danger: !p.archived, onClick: onArchive }] : []),
            ]}
          />
        </div>
      </td>
    </tr>
  )
}
