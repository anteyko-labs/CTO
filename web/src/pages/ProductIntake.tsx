// Работа с товарами: сканируем штрихкод — система находит товар у нас или подсказывает позицию
// из списка старой программы; нет нигде — сразу новая карточка. Режим пересчёта кладёт количество
// в ревизию (SPEC-20, ADR-056).
import { useEffect, useRef, useState } from 'react'
import { Icon } from '../components/icons'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, Missing, Money, Notice, PageHeader, toast } from '../components/ui'
import { ApiError, get, newOpId, post, put, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatLiters, formatSom, parseLiters, parseSom, plural } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { canisterPourPrice } from '../lib/money'
import { presetFromHit } from '../lib/oldCatalog'
import type { Category, OldCatalogHit, Product } from '../lib/types'
import { mapUmagRows } from '../lib/umag'
import { linkUmag, umagStep } from '../lib/umagLink'
import { readXlsx } from '../lib/xlsx'

type Hit = OldCatalogHit

interface Form {
  barcode: string
  name: string
  categoryId: string
  liters: string
  price: string
  cost: string
  count: string
  /** Откуда взяли данные — позиция старого списка. */
  from: Hit | null
}

const MODE_KEY = 'avtodom.intake.revision'
const CAT_KEY = 'avtodom.intake.category'

const read = (k: string): string => {
  try {
    return localStorage.getItem(k) ?? ''
  } catch {
    return ''
  }
}
const write = (k: string, v: string) => {
  try {
    localStorage.setItem(k, v)
  } catch {
    // Без хранилища выбор живёт до перезагрузки.
  }
}

const emptyForm = (barcode: string, categoryId: string): Form => ({
  barcode,
  name: '',
  categoryId,
  liters: '',
  price: '',
  cost: '',
  count: '',
  from: null,
})

export default function ProductIntake() {
  const owner = useUser().role === 'owner'
  // Пересчёт — право «Ревизия», завести товар или привязать код — «Товары и штрихкоды» (ADR-097).
  const can = useCan()
  const mayCount = can('revision')
  const mayCreate = can('products')
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const stats = useLoad(() => get<{ rows: number; imported_at: string | null }>('/old-catalog'), [])
  const [revisionOn, setRevision] = useState(read(MODE_KEY) === '1')
  const revision = revisionOn && mayCount
  const [text, setText] = useState('')
  const [hits, setHits] = useState<Hit[]>([])
  const [found, setFound] = useState<Product | null>(null)
  /** Сколько этого товара уже посчитано в черновике (вторая полка): null — ещё не считали. */
  const [already, setAlready] = useState<number | null>(null)
  const [form, setForm] = useState<Form | null>(null)
  const [similar, setSimilar] = useState<Product[]>([])
  const [recent, setRecent] = useState<{ name: string; at: string; count: string; isNew: boolean }[]>([])
  const [opId, setOpId] = useState(newOpId)
  const revisionId = useRef<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const scanRef = useRef<HTMLInputElement>(null)
  const act = useAction()
  const upload = useAction()

  const cats = categories.data ?? []
  const category = cats.find((x) => x.id === form?.categoryId)
  const isOil = category?.kind === 'oil'

  const focusScan = () => setTimeout(() => scanRef.current?.focus(), 30)
  const reset = () => {
    setText('')
    setHits([])
    setFound(null)
    setAlready(null)
    setForm(null)
    setSimilar([])
    setOpId(newOpId())
    focusScan()
  }

  useEffect(() => write(MODE_KEY, revision ? '1' : ''), [revision])

  /** Карточка из позиции старого списка: масло — литры из названия, цена за литр → за канистру. */
  const fromHit = (h: Hit, barcode: string): Form => {
    const p = presetFromHit(h, cats, read(CAT_KEY), barcode)
    return { barcode: p.barcode, name: p.name, categoryId: p.categoryId, liters: p.liters, price: p.price, cost: p.cost, count: '', from: h }
  }

  const loadSimilar = (name: string) => {
    if (name.trim().length < 3) return setSimilar([])
    void get<Product[]>(`/products/similar${qs({ name })}`)
      .then(setSimilar)
      .catch(() => setSimilar([]))
  }

  /** Товар уже заведён: показываем; в пересчёте спросим, прибавить ли к посчитанному. */
  const showFound = async (p: Product) => {
    // Товар мог лежать на другой полке и уже быть посчитан — покажем и спросим, прибавить ли.
    setAlready(revision ? await countedBefore(p.id) : null)
    setFound({ ...p })
    setForm(null)
    setTimeout(() => document.getElementById('intake-count')?.focus(), 50)
  }

  /** Скан: у нас есть — показываем; нет — строка старой базы Умаг (связанная — её товар); нет и там — новая карточка. */
  const lookup = (code: string) =>
    void act.run(async () => {
      setFound(null)
      setForm(null)
      setSimilar([])
      try {
        const p = await get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`)
        setHits([])
        await showFound(p)
        return
      } catch (e) {
        if (!(e instanceof ApiError && e.status === 404)) throw e
      }
      const list = await get<Hit[]>(`/old-catalog/search${qs({ barcode: code })}`)
      setHits(list)
      const step = umagStep(list)
      if (step.kind === 'linked') {
        await showFound(await get<Product>(`/products/${step.productId}`))
        return
      }
      const exact = step.kind === 'umag' ? step.hit : undefined
      if (exact) {
        setForm(fromHit(exact, code))
        loadSimilar(exact.name)
        setTimeout(() => document.getElementById(revision ? 'intake-count' : 'intake-price')?.focus(), 50)
      } else {
        setForm(emptyForm(code, cats.some((x) => x.id === read(CAT_KEY)) ? read(CAT_KEY) : ''))
        setTimeout(() => document.getElementById('intake-name')?.focus(), 50)
      }
    })

  // Сканер «печатает» цифры очень быстро и без пауз: короткая тишина после цифр — код дочитан, Enter не нужен.
  const onScanChange = (v: string) => {
    setText(v)
    if (timer.current) clearTimeout(timer.current)
    const s = v.trim()
    if (/^\d{6,}$/.test(s)) {
      timer.current = setTimeout(() => {
        lookup(s)
        setText('')
      }, 150)
    } else if (s.length >= 2) {
      timer.current = setTimeout(() => {
        void get<Hit[]>(`/old-catalog/search${qs({ q: s })}`)
          .then(setHits)
          .catch(() => setHits([]))
      }, 250)
    } else {
      setHits([])
    }
  }

  // Название в новой карточке — подсказки из старого списка и похожие уже заведённые.
  useEffect(() => {
    if (!form || form.from || form.name.trim().length < 2) return
    const t = setTimeout(() => {
      void get<Hit[]>(`/old-catalog/search${qs({ q: form.name.trim() })}`)
        .then(setHits)
        .catch(() => undefined)
      loadSimilar(form.name)
    }, 300)
    return () => clearTimeout(t)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [form?.name, form?.from])

  /** Открытый черновик ревизии «весь склад», если он есть. */
  const findRevision = async (): Promise<string | null> => {
    if (revisionId.current) return revisionId.current
    const list = await get<{ id: string; status: string; category_id: string | null }[]>('/revisions')
    const draft = list.find((r) => r.status === 'draft' && r.category_id === null)
    if (draft) revisionId.current = draft.id
    return draft?.id ?? null
  }

  /** Черновик ревизии «весь склад»: открытый — продолжаем, нет — заводим. */
  const ensureRevision = async (): Promise<string> => {
    const found = await findRevision()
    if (found) return found
    const id = (await post<{ head: { id: string } }>('/revisions', { category_id: null })).head.id
    revisionId.current = id
    return id
  }

  /** Уже посчитанное по товару в черновике; черновика нет — не считали. */
  const countedBefore = async (productId: string): Promise<number | null> => {
    const id = await findRevision()
    if (!id) return null
    return (await get<{ counted_qty: number | null }>(`/revisions/${id}/lines/${productId}`)).counted_qty
  }

  const countValue = (unit: string, s: string): number | null => {
    if (!s.trim()) return null
    return unit === 'ml' ? parseLiters(s) : /^\d+$/.test(s.trim()) ? Number(s.trim()) : null
  }

  const countAttempt = useRef<{ key: string; opId: string } | null>(null)

  /**
   * Записывает пересчёт в ревизию. По умолчанию прибавляет: товар на двух полках — 5 и 3 — это 8,
   * а не 3 (ADR-059). «Заменить» — только явным выбором. Возвращает итог строки.
   */
  const saveCount = async (productId: string, unit: string, s: string, mode: 'add' | 'set' = 'add'): Promise<number | null> => {
    const qty = countValue(unit, s)
    if (qty === null) return null
    const id = await ensureRevision()
    // Повтор того же ввода после обрыва связи идёт с тем же op_id: сервер не прибавит полку дважды (ADR-062).
    const key = `${id}:${productId}:${qty}:${mode}`
    if (countAttempt.current?.key !== key) countAttempt.current = { key, opId: newOpId() }
    const r = await put<{ counted_qty: number }>(`/revisions/${id}/lines`, {
      product_id: productId,
      counted_qty: qty,
      mode,
      op_id: countAttempt.current.opId,
    })
    countAttempt.current = null
    return r.counted_qty
  }

  const qtyText = (unit: string, q: number) => (unit === 'ml' ? formatLiters(q) : `${q} шт`)

  const ml = isOil && form ? parseLiters(form.liters) : null
  const shownPrice = form ? parseSom(form.price) : null
  const pourShown = ml && shownPrice !== null ? canisterPourPrice(shownPrice, ml) : null
  const formBlockers = form
    ? missingWithFocus(
        [Boolean(form.barcode.trim()), 'штрихкод', '#intake-barcode'],
        [Boolean(form.name.trim()), 'название', '#intake-name'],
        [Boolean(form.categoryId), 'категорию', '#intake-category'],
        [!isOil || (ml !== null && ml > 0), 'объём тары, л', '#intake-liters'],
        [!form.price.trim() || parseSom(form.price) !== null, 'цену числом', '#intake-price'],
        [!form.cost.trim() || parseSom(form.cost) !== null, 'закупку числом', '#intake-cost'],
        [!form.count.trim() || countValue(isOil ? 'ml' : 'piece', form.count) !== null, 'количество числом', '#intake-count'],
      )
    : []

  const saveNew = () =>
    void act.run(async () => {
      if (!form) return
      write(CAT_KEY, form.categoryId)
      const p = await post<Product>('/products', {
        op_id: opId,
        category_id: form.categoryId,
        name: form.name.trim(),
        article: form.from?.article ?? '',
        container_ml: isOil ? ml : null,
        barcodes: [form.barcode.trim()],
        sale_price_tyiyn: parseSom(form.price) ?? 0,
        initial_cost_tyiyn: form.cost.trim() ? parseSom(form.cost) : null,
        // Из старой базы Умаг: товар свяжется со строкой, её коды станут его кодами (SPEC-35).
        ...(form.from && !form.from.product_id ? { old_catalog_id: form.from.id } : {}),
      })
      if (revision) await saveCount(p.id, p.unit, form.count)
      setRecent((r) => [{ name: p.name, at: new Date().toISOString(), count: form.count, isNew: true }, ...r].slice(0, 15))
      toast(`Заведён: ${p.name}`)
      reset()
    })

  /** Похожий товар уже есть — не плодим дубль, а добавляем ему этот штрихкод. */
  const attachTo = (p: Product) =>
    void act.run(async () => {
      if (!form) return
      await post(`/products/${p.id}/barcodes`, { code: form.barcode.trim() })
      // Данные из строки Умага — связываем её с этим товаром: остальные её коды тоже станут его (SPEC-35).
      if (form.from && !form.from.product_id) await linkUmag(form.from.id, p.id, newOpId())
      // Карточка уже была — её могли посчитать под другим кодом: прибавляем и показываем итог.
      const total = revision ? await saveCount(p.id, p.unit, form.count) : null
      if (total !== null) toast(`В ревизии теперь ${qtyText(p.unit, total)}`)
      setRecent((r) => [{ name: `${p.name} (+ код)`, at: new Date().toISOString(), count: form.count, isNew: false }, ...r].slice(0, 15))
      toast(`Код привязан к «${p.name}»`)
      reset()
    })

  const saveFoundCount = (s: string, mode: 'add' | 'set') =>
    void act.run(async () => {
      if (!found) return
      const total = await saveCount(found.id, found.unit, s, mode)
      setRecent((r) => [{ name: found.name, at: new Date().toISOString(), count: s, isNew: false }, ...r].slice(0, 15))
      toast(total === null ? 'Записано в ревизию' : `Записано в ревизию: всего ${qtyText(found.unit, total)}`)
      reset()
    })

  const onUpload = (file: File) =>
    void upload.run(async () => {
      const rows = mapUmagRows(await readXlsx(file))
      // Новая выгрузка сохраняет связи строк с товарами и переносит товарам новые коды (SPEC-35 §1).
      const r = await put<{ rows: number; linked?: number; copied?: number }>('/old-catalog', rows)
      stats.reload()
      const kept = r.linked ? `; связи с товарами сохранены: ${r.linked.toLocaleString('ru-RU')}` : ''
      const copied = r.copied ? `, новых кодов у товаров: ${r.copied.toLocaleString('ru-RU')}` : ''
      toast(`Загружено позиций: ${r.rows.toLocaleString('ru-RU')}${kept}${copied}`)
    })

  return (
    <div>
      <PageHeader
        title="Работа с товарами"
        subtitle="Сканируйте штрихкод — товар найдётся у нас или в списке старой программы. Enter не нужен."
        actions={
          mayCount && (
            <label className="inline-flex cursor-pointer items-center gap-2 rounded-md border border-slate-300 bg-white px-3 py-2 text-sm">
              <input type="checkbox" checked={revision} onChange={(e) => setRevision(e.target.checked)} />
              Пересчёт (ревизия)
            </label>
          )
        }
      />
      <div className="mb-4 flex flex-wrap items-center gap-3 text-sm text-slate-600">
        <span>
          Список старой программы:{' '}
          {stats.data?.rows ? (
            <>
              <b>{stats.data.rows.toLocaleString('ru-RU')}</b> {plural(stats.data.rows, ['позиция', 'позиции', 'позиций'])}
              {stats.data.imported_at && <> · загружен {formatDateTime(stats.data.imported_at)}</>}
            </>
          ) : (
            'не загружен'
          )}
        </span>
        {owner && (
          <label className="cursor-pointer text-sky-700 underline">
            {upload.busy ? 'Загружаю…' : stats.data?.rows ? 'загрузить заново' : 'загрузить Excel'}
            <input
              type="file"
              accept=".xlsx"
              className="hidden"
              disabled={upload.busy}
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) onUpload(f)
                e.target.value = ''
              }}
            />
          </label>
        )}
        {revision && <Badge tone="amber">пересчитанное попадёт в ревизию — проведёт владелец</Badge>}
      </div>
      <ErrorBox error={upload.error ?? act.error} />

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)]">
        <div className="flex min-w-0 flex-col gap-3">
          <Card>
            <input
              ref={scanRef}
              autoFocus
              data-intake-scan
              className="w-full py-3 text-lg"
              placeholder="Сканируйте штрихкод или начните писать название"
              value={text}
              onChange={(e) => onScanChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && /^\d{4,}$/.test(text.trim())) {
                  if (timer.current) clearTimeout(timer.current)
                  lookup(text.trim())
                  setText('')
                }
              }}
            />
          </Card>
          <Card className="p-0">
            <div className="border-b border-slate-200 px-4 py-2 text-sm font-medium">Из старой программы</div>
            {hits.length === 0 ? (
              <div className="px-4 py-6 text-center text-sm text-slate-500">
                {stats.data?.rows ? 'Подсказки появятся при скане или вводе названия' : 'Загрузите Excel из старой программы — будут подсказки'}
              </div>
            ) : (
              <ul className="max-h-[60vh] divide-y divide-slate-100 overflow-y-auto">
                {hits.map((h) => (
                  <li key={h.id}>
                    <button
                      type="button"
                      className={`flex w-full items-start gap-3 px-4 py-2.5 text-left hover:bg-sky-50 ${form?.from?.id === h.id ? 'bg-sky-50' : ''}`}
                      onClick={() => {
                        // Связанная строка — это уже заведённый товар (SPEC-35): показываем его, дубля не заводим.
                        if (h.product_id) {
                          const id = h.product_id
                          void act.run(async () => showFound(await get<Product>(`/products/${id}`)))
                          return
                        }
                        const code = form?.barcode || (found ? '' : text.trim())
                        setFound(null)
                        setForm(fromHit(h, code))
                        loadSimilar(h.name)
                      }}
                    >
                      <span className="min-w-0 flex-1">
                        <span className="block font-medium">{h.name}</span>
                        <span className="block text-xs text-slate-500">
                          {[h.barcode && `код ${h.barcode}`, h.supplier, h.unit].filter(Boolean).join(' · ')}
                        </span>
                        <span className="mt-1 flex flex-wrap gap-1">
                          {h.exact && <Badge tone="green">этот штрихкод</Badge>}
                          {(h.product_id || h.linked_product_id) && (
                            <Badge tone="slate">{h.product_name ? `заведён: ${h.product_name}` : 'уже заведён'}</Badge>
                          )}
                        </span>
                      </span>
                      <span className="shrink-0 text-right text-sm">
                        <Money value={h.price_tyiyn} zero="—" className="font-semibold" />
                        {h.cost_tyiyn !== null && (
                          <span className="block text-xs text-slate-500">
                            закуп <Money value={h.cost_tyiyn} zero="—" />
                          </span>
                        )}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </div>

        <div className="flex min-w-0 flex-col gap-3">
          {found && (
            <Card className="flex flex-col gap-3 border-emerald-300">
              <CardTitle>
                <span className="flex items-center gap-2">
                  <Icon name="products" className="h-5 w-5 text-emerald-600" />
                  Уже заведён
                </span>
              </CardTitle>
              <div>
                <div className="text-lg font-semibold">{found.name}</div>
                <div className="text-sm text-slate-600">
                  Остаток {found.unit === 'ml' ? formatLiters(found.stock_qty) : `${found.stock_qty} шт`} · цена {formatSom(found.sale_price_tyiyn)}
                  {found.barcodes.length > 0 && ` · коды: ${found.barcodes.join(', ')}`}
                </div>
              </div>
              {revision ? (
                <FoundCount product={found} already={already} busy={act.busy} onSave={saveFoundCount} onSkip={reset} />
              ) : (
                <div>
                  <Button variant="secondary" onClick={reset}>
                    Дальше
                  </Button>
                </div>
              )}
            </Card>
          )}

          {form && (
            <Card className="flex flex-col gap-3">
              <CardTitle>{form.from ? 'Завести из старой программы' : 'Новый товар'}</CardTitle>
              {form.from && (
                <Notice tone="sky">
                  Данные взяты из старой программы: {form.from.name}. Проверьте цену и категорию.
                </Notice>
              )}
              {similar.length > 0 && (
                <Notice tone="amber">
                  <div className="mb-1 font-medium">Похоже, такой товар уже заведён — не создавайте дубль:</div>
                  <ul className="flex flex-col gap-1">
                    {similar.map((p) => (
                      <li key={p.id} className="flex flex-wrap items-center justify-between gap-2">
                        <span>
                          {p.name} <span className="text-xs">· {p.barcodes.length ? p.barcodes.join(', ') : 'без кода'}</span>
                        </span>
                        {mayCreate && (
                          <Button variant="secondary" className="min-h-0 px-2 py-1 text-xs" disabled={act.busy || !form.barcode.trim()} onClick={() => attachTo(p)}>
                            Это он — привязать код
                          </Button>
                        )}
                      </li>
                    ))}
                  </ul>
                </Notice>
              )}
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="Штрихкод" required>
                  <input id="intake-barcode" value={form.barcode} onChange={(e) => setForm({ ...form, barcode: e.target.value })} />
                </Field>
                <Field label="Категория" required>
                  <select id="intake-category" value={form.categoryId} onChange={(e) => setForm({ ...form, categoryId: e.target.value })}>
                    <option value="">— выберите —</option>
                    {cats.map((x) => (
                      <option key={x.id} value={x.id}>
                        {x.name}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
              <Field label="Название" required>
                <input id="intake-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder="Как на упаковке" />
              </Field>
              <div className="grid gap-3 sm:grid-cols-3">
                {isOil && (
                  <Field label="Объём тары, л" required>
                    <input id="intake-liters" inputMode="decimal" value={form.liters} onChange={(e) => setForm({ ...form, liters: e.target.value })} />
                  </Field>
                )}
                <Field label={isOil ? 'Цена канистры, с' : 'Цена, с'}>
                  <input id="intake-price" inputMode="decimal" value={form.price} onChange={(e) => setForm({ ...form, price: e.target.value })} />
                </Field>
                <Field label={isOil ? 'Закупка канистры, с' : 'Закупка, с'} hint="По ней ревизия оценит остаток">
                  <input id="intake-cost" inputMode="decimal" value={form.cost} onChange={(e) => setForm({ ...form, cost: e.target.value })} />
                </Field>
                {isOil && (
                  // Одна цена масла — цена канистры (бочки); розлив за литр считается от неё (ADR-074).
                  <div className="self-end pb-2 text-sm text-slate-600" data-testid="intake-pour-derived">
                    {pourShown !== null ? `на розлив: ${formatSom(pourShown)} за л` : 'на розлив: от цены канистры'}
                  </div>
                )}
                {revision && (
                  <Field label={isOil ? 'На полке, л' : 'На полке, шт'}>
                    <input
                      id="intake-count"
                      inputMode={isOil ? 'decimal' : 'numeric'}
                      value={form.count}
                      onChange={(e) => setForm({ ...form, count: e.target.value })}
                      onKeyDown={(e) => e.key === 'Enter' && formBlockers.length === 0 && saveNew()}
                    />
                  </Field>
                )}
              </div>
              <Missing items={formBlockers} />
              <div className="flex flex-wrap justify-end gap-2">
                <Button variant="secondary" onClick={reset}>
                  Отмена
                </Button>
                {mayCreate ? (
                  <Button disabled={act.busy || formBlockers.length > 0} onClick={saveNew}>
                    Завести товар
                  </Button>
                ) : (
                  <span className="self-center text-xs text-slate-500">Заводить товары — право «Товары и штрихкоды»</span>
                )}
              </div>
            </Card>
          )}

          {!found && !form && (
            <Card>
              <Empty icon="products">
                Отсканируйте товар. Найдётся у нас — увидите остаток{revision ? ' и впишете, сколько на полке' : ''}. Нет — карточка заполнится из
                старой программы или откроется новая.
              </Empty>
            </Card>
          )}

          {recent.length > 0 && (
            <Card>
              <CardTitle>Только что</CardTitle>
              <ul className="divide-y divide-slate-100 text-sm">
                {recent.map((r, i) => (
                  <li key={i} className="flex items-center justify-between gap-2 py-1.5">
                    <span className="min-w-0 truncate">
                      {r.isNew && <Badge tone="green">новый</Badge>} {r.name}
                    </span>
                    {r.count && <span className="shrink-0 text-slate-500">на полке {r.count}</span>}
                  </li>
                ))}
              </ul>
            </Card>
          )}
        </div>
      </div>
    </div>
  )
}

/**
 * Пересчёт уже заведённого товара: сколько на полке. Уже считали (товар на второй полке) —
 * показываем, сколько, и спрашиваем: прибавить (по умолчанию, Enter) или заменить (ADR-059).
 */
function FoundCount({
  product,
  already,
  busy,
  onSave,
  onSkip,
}: {
  product: Product
  already: number | null
  busy: boolean
  onSave: (s: string, mode: 'add' | 'set') => void
  onSkip: () => void
}) {
  const [s, setS] = useState('')
  const ok = product.unit === 'ml' ? parseLiters(s) !== null : /^\d+$/.test(s.trim())
  const ml = product.unit === 'ml'
  const before = already !== null && already > 0 ? (ml ? formatLiters(already) : `${already} шт`) : null
  return (
    <div className="flex flex-col gap-2">
      {before && (
        <Notice tone="sky">
          В этой ревизии уже посчитано <b>{before}</b> — если товар лежит ещё и здесь, прибавьте.
        </Notice>
      )}
      <div className="flex flex-wrap items-end gap-2">
        <Field label={ml ? 'На полке, л' : 'На полке, шт'}>
          <input
            id="intake-count"
            className="w-32"
            inputMode={ml ? 'decimal' : 'numeric'}
            value={s}
            onChange={(e) => setS(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && ok && onSave(s, 'add')}
          />
        </Field>
        <Button disabled={busy || !ok} onClick={() => onSave(s, 'add')}>
          {before ? `Прибавить к ${before}` : 'Записать'}
        </Button>
        {before && (
          <Button variant="secondary" disabled={busy || !ok} onClick={() => onSave(s, 'set')}>
            Заменить
          </Button>
        )}
        <Button variant="ghost" onClick={onSkip}>
          Пропустить
        </Button>
      </div>
    </div>
  )
}
