// В ревизию — товар, которого нет в программе (SPEC-29 §10): карточка заводится сразу с найденным
// количеством одной операцией; подсказки из старой программы заполняют карточку.
import { useEffect, useMemo, useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { canisterPourPrice } from '../lib/money'
import { formatSom, parseLiters, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { presetFromHit } from '../lib/oldCatalog'
import type { Category, OldCatalogHit, Product } from '../lib/types'
import { ProductPicker } from './ProductPicker'
import { Button, ErrorBox, Field, Missing, Modal, Notice, toast } from './ui'

interface Form {
  barcode: string
  name: string
  brand: string
  article: string
  categoryId: string
  liters: string
  price: string
  cost: string
  count: string
  from: string | null
}

const empty = (barcode = '', name = ''): Form => ({
  barcode,
  name,
  brand: '',
  article: '',
  categoryId: '',
  liters: '',
  price: '',
  cost: '',
  count: '',
  from: null,
})

/** Количество на полке: штуки целым, масло литрами. */
function countOf(oil: boolean, s: string): number | null {
  const t = s.trim()
  if (!t) return null
  if (oil) return parseLiters(t)
  return /^\d+$/.test(t) ? Number(t) : null
}

export function RevisionNewProduct({
  revisionId,
  categoryId,
  presetBarcode,
  onClose,
  onDone,
  onExisting,
}: {
  revisionId: string
  /** Ревизия по категории — новая карточка в ней же. */
  categoryId: string | null
  presetBarcode?: string
  onClose: () => void
  onDone: () => void
  /** Товар уже есть в программе — его считают в строке ревизии. */
  onExisting: (p: Product) => void
}) {
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const cats = useMemo(() => categories.data ?? [], [categories.data])
  const [form, setForm] = useState<Form | null>(() =>
    presetBarcode ? { ...empty(presetBarcode), categoryId: categoryId ?? '' } : null,
  )
  const [opId] = useState(newOpId)
  const act = useAction()
  const category = cats.find((c) => c.id === form?.categoryId)
  const oil = category?.kind === 'oil'
  const ml = oil && form ? parseLiters(form.liters) : null
  const price = form?.price.trim() ? parseSom(form.price) : 0
  const pour = ml && price ? canisterPourPrice(price, ml) : null

  // Скан незнакомого кода — сразу ищем его в списке старой программы.
  const ready = cats.length > 0
  useEffect(() => {
    if (!presetBarcode || !ready) return
    let alive = true
    void get<OldCatalogHit[]>(`/old-catalog/search${qs({ barcode: presetBarcode })}`)
      .then((hits) => {
        const exact = hits.find((h) => h.exact)
        if (alive && exact) fromOld(exact, presetBarcode)
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [presetBarcode, ready])

  function fromOld(h: OldCatalogHit, code: string) {
    const p = presetFromHit(h, cats, categoryId ?? '', code)
    setForm({ ...empty(), barcode: p.barcode, name: p.name, article: p.article, categoryId: categoryId ?? p.categoryId, liters: p.liters, price: p.price, cost: p.cost, from: h.name })
    setTimeout(() => document.getElementById('revnew-count')?.focus(), 50)
  }

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => (f ? { ...f, [k]: v } : f))
  const count = form ? countOf(oil, form.count) : null
  const missing = form
    ? missingWithFocus(
        [Boolean(form.name.trim()), 'название', '#revnew-name'],
        [Boolean(form.categoryId), 'категорию', '#revnew-category'],
        [!oil || (ml !== null && ml > 0), 'объём тары, л', '#revnew-liters'],
        // Без цены товар продался бы за 0 с (проверка 20.10.2026): цена обязательна, как в «Товарах».
        [(parseSom(form.price) ?? 0) > 0, 'цену продажи', '#revnew-price'],
        [!form.cost.trim() || parseSom(form.cost) !== null, 'закупку числом', '#revnew-cost'],
        [count !== null, oil ? 'сколько литров на полке' : 'сколько штук на полке', '#revnew-count'],
      )
    : []

  const save = () =>
    void act.run(async () => {
      if (!form || count === null) return
      const r = await post<{ product: Product }>(`/revisions/${revisionId}/new-product`, {
        op_id: opId,
        counted_qty: count,
        category_id: form.categoryId,
        name: form.name.trim(),
        brand: form.brand.trim(),
        article: form.article.trim(),
        container_ml: oil ? ml : null,
        barcodes: form.barcode.trim() ? [form.barcode.trim()] : [],
        sale_price_tyiyn: parseSom(form.price) ?? 0,
        initial_cost_tyiyn: form.cost.trim() ? parseSom(form.cost) : null,
      })
      toast(`Заведён и посчитан: ${r.product.name}`)
      onDone()
    })

  return (
    <Modal title="Товар, которого нет в программе" wide onClose={onClose}>
      <div className="flex flex-col gap-3">
        {!form && (
          <>
            <p className="text-sm text-slate-600">
              Напишите название или отсканируйте код. Если товар уже есть — посчитайте его в строке; из старой программы — карточка
              заполнится сама.
            </p>
            <ProductPicker
              placeholder="Название, артикул или штрихкод"
              onPick={(p) => onExisting(p)}
              onPickOld={fromOld}
              onCreate={(text) => setForm({ ...empty('', text), categoryId: categoryId ?? '' })}
              onUnknownCode={(code) => setForm({ ...empty(code), categoryId: categoryId ?? '' })}
            />
          </>
        )}
        {form && (
          <>
            {form.from && <Notice tone="sky">Данные из старой программы: {form.from}. Проверьте категорию и цену.</Notice>}
            <div className="grid gap-3 sm:grid-cols-2">
              <Field label="Название" required>
                <input id="revnew-name" autoFocus={!form.from} value={form.name} onChange={(e) => set('name', e.target.value)} />
              </Field>
              <Field label="Категория" required>
                <select id="revnew-category" value={form.categoryId} disabled={Boolean(categoryId)} onChange={(e) => set('categoryId', e.target.value)}>
                  <option value="">— выберите —</option>
                  {cats.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Бренд">
                <input value={form.brand} onChange={(e) => set('brand', e.target.value)} />
              </Field>
              <Field label="Штрихкод" hint="Без кода система выдаст свой">
                <input value={form.barcode} onChange={(e) => set('barcode', e.target.value)} />
              </Field>
              {oil && (
                <Field label="Объём тары, л" required>
                  <input id="revnew-liters" inputMode="decimal" value={form.liters} onChange={(e) => set('liters', e.target.value)} />
                </Field>
              )}
              <Field label={oil ? 'Цена канистры, с' : 'Цена продажи, с'} hint={pour !== null ? `на розлив: ${formatSom(pour)} за л` : undefined}>
                <input id="revnew-price" inputMode="decimal" value={form.price} onChange={(e) => set('price', e.target.value)} />
              </Field>
              <Field label={oil ? 'Закупка канистры, с' : 'Закупка, с'} hint="По ней ревизия оценит найденное">
                <input id="revnew-cost" inputMode="decimal" value={form.cost} onChange={(e) => set('cost', e.target.value)} />
              </Field>
              <Field label={oil ? 'На полке, л' : 'На полке, шт'} required>
                <input
                  id="revnew-count"
                  inputMode={oil ? 'decimal' : 'numeric'}
                  value={form.count}
                  onChange={(e) => set('count', e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && missing.length === 0 && save()}
                />
              </Field>
            </div>
            <Missing items={missing} />
          </>
        )}
        <ErrorBox error={act.error ?? categories.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          {form && (
            <Button disabled={act.busy || missing.length > 0} onClick={save}>
              Завести и посчитать
            </Button>
          )}
        </div>
      </div>
    </Modal>
  )
}
