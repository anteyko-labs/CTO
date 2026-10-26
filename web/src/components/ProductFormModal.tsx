import { useEffect, useMemo, useState } from 'react'
import { del, get, newOpId, patch, post, qs, put } from '../lib/api'
import { useUser } from '../lib/auth'
import { can } from '../lib/permissions'
import { POLARITY_LABELS } from '../lib/battery'
import { formatSom, litersInput, parseLiters, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { canisterPourPrice } from '../lib/money'
import type { CardPreset } from '../lib/oldCatalog'
import { KIND_LABELS, type Category, type CategoryKind, type OldCatalogHit, type Product } from '../lib/types'
import { hitCodes } from '../lib/umagLink'
import { Badge, Button, Checkbox, Empty, ErrorBox, Field, Missing, Modal, toast } from './ui'
import { UmagCard } from './UmagCard'

interface Form {
  category_id: string
  name: string
  brand: string
  article: string
  container_l: string
  attrs: Record<string, string>
  barcode: string
  sale_price: string
  min_stock: string
}

function initialForm(
  p: Product | undefined,
  presetBarcode: string | undefined,
  presetName: string | undefined,
  presetCategoryId?: string,
  preset?: CardPreset,
): Form {
  const oil = p?.unit === 'ml'
  const toLiters = litersInput
  return {
    category_id: p?.category_id ?? (preset?.categoryId || presetCategoryId) ?? '',
    name: p?.name ?? preset?.name ?? presetName ?? '',
    brand: p?.brand ?? '',
    article: p?.article ?? preset?.article ?? '',
    container_l: p?.container_ml ? toLiters(p.container_ml) : (preset?.liters ?? ''),
    attrs: { ...(p?.attrs ?? {}) },
    barcode: presetBarcode ?? preset?.barcode ?? '',
    sale_price: p ? somInput(p.sale_price_tyiyn) : (preset?.price ?? ''),
    min_stock: p ? (oil ? toLiters(p.min_stock) : String(p.min_stock)) : '0',
  }
}

/** Создание или правка товара. Для нового товара показывает похожие, чтобы не завести дубль. */
export function ProductFormModal({
  product,
  presetBarcode,
  presetName,
  presetCategoryId,
  preset,
  umagRow,
  onClose,
  onSaved,
}: {
  product?: Product
  presetBarcode?: string
  /** Название, набранное в поиске: товар заводится без повторного ввода. */
  presetName?: string
  /** «+ Товар» на экране категорий: категория уже выбрана (SPEC-27). */
  presetCategoryId?: string
  /** Данные из списка старой программы (SPEC-20, SPEC-29 §1): карточка заполнена, человек проверяет. */
  preset?: CardPreset
  /** Строка старой базы Умаг, из которой заводят товар (SPEC-35): сервер в той же операции свяжет
   *  её с товаром и перенесёт её коды. */
  umagRow?: OldCatalogHit
  onClose: () => void
  onSaved: (p: Product) => void
}) {
  const user = useUser()
  const owner = user.role === 'owner'
  // Заводить и править товары и штрихкоды — право администратора (ADR-097); без него — просмотр.
  const mayEdit = can(user, 'products')
  const editing = Boolean(product)
  const [form, setForm] = useState<Form>(() => initialForm(product, presetBarcode, presetName, presetCategoryId, preset))
  // Цена филиала (SPEC-33 §2): своя — только здесь; без неё — общая для всех филиалов.
  const branchPrices = owner && editing && Boolean(user.several_branches)
  const [ownPrice, setOwnPrice] = useState(Boolean(product?.own_price))
  const [opId] = useState(newOpId)
  const [codes, setCodes] = useState<string[]>(product?.barcodes ?? [])
  /** Новый товар: заводские коды, отсканированные один за другим (SPEC-29 §2). */
  const [newCodes, setNewCodes] = useState<string[]>([])
  const [newCategory, setNewCategory] = useState<{ name: string; kind: CategoryKind } | null>(null)
  const [merging, setMerging] = useState(false)
  const [mergeQuery, setMergeQuery] = useState('')
  const merge = useAction()
  const categoryAction = useAction()
  const { busy, error, setError, run } = useAction()
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const category = useMemo(
    () => categories.data?.find((c) => c.id === form.category_id),
    [categories.data, form.category_id],
  )
  const isOil = category?.kind === 'oil'
  const isFilter = category?.kind === 'filter'
  // Кросс-номера фильтра: по ним касса находит товар и предлагает аналоги (SPEC-17).
  const [cross, setCross] = useState<string | null>(null)
  useEffect(() => {
    if (!product) return
    void get<{ codes: string[] }>(`/products/${product.id}/cross`)
      .then((r) => setCross((c) => c ?? r.codes.join(', ')))
      .catch(() => undefined)
  }, [product])

  const nameQuery = useDebounced(form.name.trim(), 300)
  const [similar, setSimilar] = useState<Product[]>([])
  useEffect(() => {
    if (editing || nameQuery.length < 3) {
      setSimilar([])
      return
    }
    let alive = true
    get<Product[]>(`/products/similar${qs({ name: nameQuery })}`)
      .then((r) => alive && setSimilar(r))
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [nameQuery, editing])

  // Розлив за литр — от цены и объёма канистры, как на сервере (ADR-074).
  const shownSale = parseSom(form.sale_price)
  const shownMl = parseLiters(form.container_l)
  const pourShown = isOil && shownSale !== null && shownMl ? canisterPourPrice(shownSale, shownMl) : null

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => ({ ...f, [k]: v }))

  const parsed = () => {
    const sale = parseSom(form.sale_price || '0')
    const container = isOil ? parseLiters(form.container_l) : null
    const min = isOil ? parseLiters(form.min_stock || '0') : /^\d+$/.test(form.min_stock.trim() || '0') ? Number(form.min_stock || 0) : null
    if (sale === null) throw new Error('Неверная цена продажи')
    if (isOil && (!container || container <= 0)) throw new Error('Укажите объём канистры в литрах')
    if (min === null) throw new Error('Неверный минимальный остаток')
    return { sale, container, min }
  }

  /** Новая категория создаётся здесь же и сразу выбирается в товаре. */
  const createCategory = () =>
    categoryAction.run(async () => {
      if (!newCategory) return
      const name = newCategory.name.trim()
      if (!name) throw new Error('Введите название категории')
      const c = await post<Category>('/categories', { name, kind: newCategory.kind })
      categories.setData((list) => [...(list ?? []), c])
      set('category_id', c.id)
      setNewCategory(null)
      toast('Категория создана')
    })

  // Чего не хватает для сохранения: кнопка заблокирована, пока список не пуст.
  const missing = missingWithFocus(
    [Boolean(form.category_id), 'категорию', '#product-category'],
    [Boolean(form.name.trim()), 'название', '#product-name'],
    [!isOil || Boolean(form.container_l.trim()), 'объём канистры', '#product-container'],
    [Boolean(form.sale_price.trim()), 'цену продажи', '#product-price'],
  )

  const save = () =>
    run(async () => {
      if (!form.category_id) throw new Error('Выберите категорию')
      const v = parsed()
      let saved: Product
      if (!product) {
        saved = await post<Product>('/products', {
          op_id: opId,
          category_id: form.category_id,
          name: form.name,
          brand: form.brand,
          article: form.article,
          container_ml: v.container,
          attrs: form.attrs,
          barcodes: [...new Set([...newCodes, form.barcode.trim()].filter(Boolean))],
          sale_price_tyiyn: v.sale,
          min_stock: v.min,
          ...(umagRow ? { old_catalog_id: umagRow.id } : {}),
        })
      } else {
        await patch<Product>(`/products/${product.id}`, {
          category_id: form.category_id,
          name: form.name,
          brand: form.brand,
          article: form.article,
          attrs: form.attrs,
        })
        const prices: Record<string, number | boolean> = {}
        const wasOwn = Boolean(product.own_price)
        if (branchPrices && wasOwn && !ownPrice) {
          // «Вернуть общую»: цена филиала снова общая, поле цены не трогаем.
          prices.own_price = false
        } else {
          if (v.sale !== product.sale_price_tyiyn) prices.sale_price_tyiyn = v.sale
          if (branchPrices && ownPrice && !wasOwn) {
            prices.own_price = true
            prices.sale_price_tyiyn = v.sale
          }
        }
        if (v.min !== product.min_stock) prices.min_stock = v.min
        saved = Object.keys(prices).length
          ? await patch<Product>(`/products/${product.id}/prices`, prices)
          : await get<Product>(`/products/${product.id}`)
      }
      if (isFilter && cross !== null) {
        const list = cross.split(/[,;\n]/).map((c) => c.trim()).filter(Boolean)
        await put(`/products/${saved.id}/cross`, { codes: list })
      }
      toast(product ? 'Товар сохранён' : 'Товар создан')
      onSaved(saved)
    })

  const removeCode = (code: string) =>
    run(async () => {
      if (!product) return
      await del(`/products/${product.id}/barcodes/${encodeURIComponent(code)}`)
      setCodes((c) => c.filter((x) => x !== code))
    })

  const addCode = (generate: boolean) =>
    run(async () => {
      if (!product) return
      const code = generate ? undefined : form.barcode.trim()
      if (!generate && !code) throw new Error('Введите штрихкод')
      const res = await post<{ code: string }>(`/products/${product.id}/barcodes`, { code })
      setCodes((c) => [...c, res.code])
      set('barcode', '')
    })

  /** Слить дубль в основной товар: коды и остаток уходят туда, дубль в архив (SPEC-02). */
  const doMerge = (into: Product) =>
    merge.run(async () => {
      if (!product) return
      if (!window.confirm(`Перенести коды и остаток «${product.name}» в «${into.name}»? Дубль уйдёт в архив.`)) return
      const saved = await post<Product>(`/products/${product.id}/merge`, {
        op_id: newOpId(),
        into_product_id: into.id,
      })
      toast('Товары объединены')
      setMerging(false)
      onSaved(saved)
    })

  if (merging && product) {
    return (
      <Modal title="Объединить дубли" onClose={() => setMerging(false)}>
        <div className="flex flex-col gap-3">
          <div className="text-sm text-slate-600">
            Куда перенести коды и остаток товара «{product.name}»? Сам он уйдёт в архив, его чеки останутся за ним.
          </div>
          <Field label="Основной товар">
            <input autoFocus placeholder="Название, бренд, артикул" value={mergeQuery} onChange={(e) => setMergeQuery(e.target.value)} />
          </Field>
          <MergeTargets query={mergeQuery} exclude={product.id} busy={merge.busy} onPick={(p) => void doMerge(p)} />
          <ErrorBox error={merge.error} />
        </div>
      </Modal>
    )
  }

  if (newCategory) {
    return (
      <Modal title="Новая категория" onClose={() => setNewCategory(null)}>
        <div className="flex flex-col gap-3">
          <Field label="Название" required>
            <input
              autoFocus
              value={newCategory.name}
              onChange={(e) => setNewCategory({ ...newCategory, name: e.target.value })}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void createCategory()
                }
              }}
            />
          </Field>
          <Field label="Вид" hint="Масло учитывается в литрах, остальное — в штуках">
            <select
              value={newCategory.kind}
              onChange={(e) => setNewCategory({ ...newCategory, kind: e.target.value as CategoryKind })}
            >
              {(Object.keys(KIND_LABELS) as CategoryKind[]).map((k) => (
                <option key={k} value={k}>
                  {KIND_LABELS[k]}
                </option>
              ))}
            </select>
          </Field>
          <ErrorBox error={categoryAction.error} />
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setNewCategory(null)}>
              Отмена
            </Button>
            <Button disabled={categoryAction.busy || !newCategory.name.trim()} onClick={() => void createCategory()}>
              Создать
            </Button>
          </div>
        </div>
      </Modal>
    )
  }

  return (
    <Modal title={editing ? 'Товар' : 'Новый товар'} onClose={onClose} wide>
      {preset && !editing && (
        <div className="mb-3 rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-sm text-sky-900">
          Данные взяты из старой программы{umagRow ? ' (база Умаг)' : ''}: {preset.name}. Проверьте категорию, цену и характеристики.
          {umagRow && hitCodes(umagRow).length > 0 && (
            <div className="mt-1" data-testid="umag-codes">
              Коды Умага перейдут к товару (кроме занятых другими товарами): {hitCodes(umagRow).join(', ')}
            </div>
          )}
        </div>
      )}
      {/* Без права «Товары и штрихкоды» карточка только для чтения (ADR-097). */}
      <fieldset disabled={!mayEdit} className="m-0 min-w-0 border-0 p-0">
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="sm:col-span-2">
          <Field
            label="Штрихкод"
            hint={
              editing
                ? 'Кодов может быть несколько: привязывайте сюда заводские коды разных поставок'
                : 'Сканируйте заводские коды один за другим (Enter — следующий); без кода система создаст свой'
            }
          >
            <div className="flex gap-2">
              <input
                id="product-barcode"
                className="flex-1"
                autoFocus={!editing && !presetBarcode}
                placeholder="Сканируйте или введите"
                value={form.barcode}
                onChange={(e) => set('barcode', e.target.value)}
                onKeyDown={(e) => {
                  // Сканер завершает код Enter: у карточки — привязываем код; у нового товара код
                  // ложится в список (можно отсканировать ещё), пустой Enter — к категории.
                  if (e.key !== 'Enter') return
                  e.preventDefault()
                  const code = form.barcode.trim()
                  if (editing) {
                    if (code) void addCode(false)
                  } else if (code) {
                    setNewCodes((c) => (c.includes(code) ? c : [...c, code]))
                    set('barcode', '')
                  } else document.getElementById('product-category')?.focus()
                }}
              />
              {editing && mayEdit && (
                <>
                  <Button variant="secondary" disabled={busy} onClick={() => void addCode(false)}>
                    Добавить
                  </Button>
                  <Button variant="secondary" disabled={busy} onClick={() => void addCode(true)}>
                    Создать свой
                  </Button>
                </>
              )}
            </div>
          </Field>
          {!editing && newCodes.length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-2" data-testid="new-codes">
              {newCodes.map((c) => (
                <span key={c} className="inline-flex items-center gap-1">
                  <Badge>{c}</Badge>
                  <button
                    type="button"
                    className="text-xs text-slate-400 hover:text-rose-600"
                    aria-label={`Убрать код ${c}`}
                    onClick={() => setNewCodes((list) => list.filter((x) => x !== c))}
                  >
                    ✕
                  </button>
                </span>
              ))}
            </div>
          )}
          {codes.length > 0 && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              {codes.map((c) => (
                <span key={c} className="inline-flex items-center gap-1">
                  <Badge>{c}</Badge>
                  {editing && mayEdit && codes.length > 1 && (
                    <button
                      type="button"
                      className="text-xs text-slate-400 hover:text-rose-600"
                      aria-label={`Отвязать код ${c}`}
                      disabled={busy}
                      onClick={() => void removeCode(c)}
                    >
                      ✕
                    </button>
                  )}
                </span>
              ))}
            </div>
          )}
        </div>
        <Field
          label="Категория"
          required
          hint={
            editing ? undefined : (
              <button type="button" className="text-sky-700 underline" onClick={() => setNewCategory({ name: '', kind: 'oil' })}>
                + новая категория
              </button>
            )
          }
        >
          <select
            id="product-category"
            autoFocus={!editing && Boolean(presetBarcode)}
            value={form.category_id}
            disabled={editing && !category}
            onChange={(e) => {
              set('category_id', e.target.value)
              setError(null)
            }}
          >
            <option value="">— выберите —</option>
            {categories.data
              ?.filter((c) => !editing || (c.kind === 'oil') === (product?.unit === 'ml'))
              .map((c) => (
                <option key={c.id} value={c.id}>
                  {c.name}
                </option>
              ))}
          </select>
        </Field>
        <Field label="Название" required>
          <input id="product-name" autoFocus={editing} value={form.name} onChange={(e) => set('name', e.target.value)} />
        </Field>
        {similar.length > 0 && (
          <div className="rounded-md border border-amber-200 bg-amber-50 p-2 text-sm sm:col-span-2">
            <div className="mb-1 font-medium text-amber-800">Возможно, товар уже заведён:</div>
            <ul className="list-inside list-disc text-amber-900">
              {similar.map((s) => (
                <li key={s.id}>
                  {s.name} {s.article && <span className="text-amber-700">({s.article})</span>}
                </li>
              ))}
            </ul>
          </div>
        )}
        <Field label="Бренд">
          <input value={form.brand} onChange={(e) => set('brand', e.target.value)} />
        </Field>
        <Field label="Артикул">
          <input value={form.article} onChange={(e) => set('article', e.target.value)} />
        </Field>
        {isFilter && (
          <Field label="Кросс-номера" hint="Номера других производителей через запятую: по ним найдётся товар и подскажутся аналоги">
            <textarea rows={2} value={cross ?? ''} onChange={(e) => setCross(e.target.value)} placeholder="90915-YZZE1, W 712/75, OC 217" />
          </Field>
        )}
        {isOil && (
          <Field label="Объём канистры, л" required>
            <input
              id="product-container"
              inputMode="decimal"
              disabled={editing}
              value={form.container_l}
              onChange={(e) => set('container_l', e.target.value)}
            />
          </Field>
        )}
        {/* Кросс-номера фильтра — отдельным полем выше; прежний признак «cross» категории не дублируем. */}
        {category?.attributes.filter((a) => !(isFilter && a.key === 'cross')).map((a) => (
          <Field key={a.key} label={a.label}>
            {a.type === 'select' ? (
              <select value={form.attrs[a.key] ?? ''} onChange={(e) => set('attrs', { ...form.attrs, [a.key]: e.target.value })}>
                <option value="">—</option>
                {a.options?.map((o) => (
                  <option key={o} value={o}>
                    {a.key === 'polarity' ? (POLARITY_LABELS[o] ?? o) : o}
                  </option>
                ))}
              </select>
            ) : (
              <input
                inputMode={a.type === 'number' ? 'numeric' : undefined}
                value={form.attrs[a.key] ?? ''}
                onChange={(e) => set('attrs', { ...form.attrs, [a.key]: e.target.value })}
              />
            )}
          </Field>
        ))}
        <Field
          label={isOil ? 'Цена канистры, с' : product?.unit === 'g' ? 'Цена продажи за кг, с' : 'Цена продажи, с'}
          required
          hint={
            branchPrices ? (
              <span className="flex flex-col gap-1" data-testid="branch-price">
                <Checkbox label={`Своя цена в филиале «${user.branch_name ?? ''}»`} checked={ownPrice} onChange={setOwnPrice} />
                <span>
                  {ownPrice
                    ? `общая цена ${formatSom(product?.shared_sale_price_tyiyn ?? 0)} этот филиал не меняет; снять флажок — вернуть общую`
                    : 'общая для всех филиалов без своей цены'}
                </span>
              </span>
            ) : undefined
          }
        >
          <input
            id="product-price"
            inputMode="decimal"
            value={form.sale_price}
            disabled={branchPrices && Boolean(product?.own_price) && !ownPrice}
            onChange={(e) => set('sale_price', e.target.value)}
          />
        </Field>
        {isOil && (
          // Одна цена масла — цена канистры; розлив за литр считается от неё (ADR-074).
          <div className="self-end pb-2 text-sm text-slate-600" data-testid="product-pour-derived">
            {pourShown !== null ? `на розлив: ${formatSom(pourShown)} за л` : 'на розлив: от цены канистры'}
          </div>
        )}
        <Field label={isOil ? 'Минимальный остаток, л' : 'Минимальный остаток, шт'}>
          <input inputMode="decimal" value={form.min_stock} onChange={(e) => set('min_stock', e.target.value)} />
        </Field>
      </div>
      </fieldset>
      {/* Связанная строка старой базы Умаг (SPEC-35 §3): после связывания у товара новые коды. */}
      {product && <UmagCard productId={product.id} mayEdit={mayEdit} onProduct={(p) => setCodes(p.barcodes)} />}
      <div className="mt-4 flex flex-col gap-3">
        <ErrorBox error={error} />
        <Missing items={missing} className="text-right" />
        <div className="flex flex-wrap justify-end gap-2">
          {editing && owner && (
            <Button variant="secondary" className="mr-auto" onClick={() => setMerging(true)}>
              Объединить дубли
            </Button>
          )}
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          {mayEdit ? (
            <Button disabled={busy || missing.length > 0} onClick={() => void save()}>
              Сохранить
            </Button>
          ) : (
            <span className="self-center text-xs text-slate-500">Править товары — право «Товары и штрихкоды»</span>
          )}
        </div>
      </div>
    </Modal>
  )
}

/** Поиск основного товара при объединении дублей. */
function MergeTargets({
  query,
  exclude,
  busy,
  onPick,
}: {
  query: string
  exclude: string
  busy: boolean
  onPick: (p: Product) => void
}) {
  const text = useDebounced(query.trim(), 250)
  const found = useLoad(
    () => (text.length < 2 ? Promise.resolve<Product[]>([]) : get<Product[]>(`/products${qs({ q: text, limit: 8 })}`)),
    [text],
  )
  const rows = (found.data ?? []).filter((p) => p.id !== exclude)
  if (text.length < 2) return <Empty>Введите хотя бы две буквы</Empty>
  if (rows.length === 0 && !found.loading) return <Empty>Ничего не нашлось</Empty>
  return (
    <ul className="flex flex-col gap-1">
      {rows.map((p) => (
        <li key={p.id}>
          <Button variant="secondary" className="w-full justify-between" disabled={busy} onClick={() => onPick(p)}>
            <span>{p.name}</span>
            <span className="text-xs text-slate-500">{p.article || '—'}</span>
          </Button>
        </li>
      ))}
    </ul>
  )
}
