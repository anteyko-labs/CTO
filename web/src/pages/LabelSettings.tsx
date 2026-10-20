// Настройка размера и содержимого этикеток с предпросмотром (SPEC-02, ADR-010).
import { useState } from 'react'
import { labelsHtml } from '../components/labels'
import { Button, Card, CardTitle, Checkbox, ErrorBox, Field, Loading, PageHeader } from '../components/ui'
import { get, put, qs } from '../lib/api'
import { useAction, useLoad } from '../lib/hooks'
import { previewPick } from '../lib/labelText'
import type { Category, LabelSettings, Product } from '../lib/types'

const PRESETS: [number, number][] = [
  [58, 40],
  [40, 30],
  [30, 20],
]

const MIN_MM = 10
const MAX_MM = 200
// Только для показа на экране: 1 мм ≈ 3,78 px
const PX_PER_MM = 3.78

const SAMPLE: Product = {
  id: 'sample',
  category_id: 'sample',
  category_kind: 'oil',
  name: 'Super 3000 5W-30',
  brand: 'Mobil',
  article: '152566',
  unit: 'ml',
  container_ml: 4000,
  attrs: { viscosity: '5W-30' },
  archived: false,
  barcodes: ['2200000000019'],
  sale_price_tyiyn: 245000,
  pour_price_per_l_tyiyn: null,
  min_stock: 0,
  stock_qty: 0,
  needs_review: false,
  last_purchase_price_tyiyn: null,
}

interface Form {
  width: string
  height: string
  show_name: boolean
  show_price: boolean
  show_article: boolean
  show_brand: boolean
  show_spec: boolean
}

const toForm = (s: LabelSettings): Form => ({
  width: String(s.width_mm),
  height: String(s.height_mm),
  show_name: s.show_name,
  show_price: s.show_price,
  show_article: s.show_article,
  show_brand: s.show_brand !== false,
  show_spec: s.show_spec !== false,
})

function parseMm(v: string): number | null {
  if (!/^\d+$/.test(v.trim())) return null
  const n = Number(v.trim())
  return n >= MIN_MM && n <= MAX_MM ? n : null
}

function toSettings(f: Form): LabelSettings | null {
  const width_mm = parseMm(f.width)
  const height_mm = parseMm(f.height)
  if (width_mm === null || height_mm === null) return null
  return {
    width_mm,
    height_mm,
    show_name: f.show_name,
    show_price: f.show_price,
    show_article: f.show_article,
    show_brand: f.show_brand,
    show_spec: f.show_spec,
  }
}

/** Для предпросмотра — настоящее масло с брендом и характеристиками (видно, что меняют отметки);
 *  такого нет — встроенный образец масла (SPEC-02). */
async function previewProduct(): Promise<Product> {
  try {
    const cats = await get<Category[]>('/categories')
    const oilCats = cats.filter((c) => c.kind === 'oil').slice(0, 3)
    const lists = await Promise.all(oilCats.map((c) => get<Product[]>(`/products${qs({ category_id: c.id, limit: 50 })}`)))
    return previewPick(lists.flat()) ?? SAMPLE
  } catch {
    return SAMPLE
  }
}

function Editor({ initial }: { initial: LabelSettings }) {
  const [form, setForm] = useState<Form>(() => toForm(initial))
  const [saved, setSaved] = useState(false)
  const { busy, error, run } = useAction()
  const settings = toSettings(form)
  const sample = useLoad(previewProduct, [])
  const product = sample.data ?? SAMPLE

  const preview = settings ? labelsHtml([{ product, copies: 1 }], settings) : null

  const update = (patchForm: Partial<Form>) => {
    setForm((f) => ({ ...f, ...patchForm }))
    setSaved(false)
  }

  const save = () =>
    run(async () => {
      if (!settings) throw new Error(`Размер этикетки от ${MIN_MM} до ${MAX_MM} мм`)
      const res = await put<LabelSettings>('/settings/labels', settings)
      setForm(toForm(res))
      setSaved(true)
    })

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card>
        <div className="flex flex-col gap-4">
          <div>
            <div className="mb-2 text-sm font-medium text-slate-700">Размер</div>
            <div className="flex flex-wrap gap-2">
              {PRESETS.map(([w, h]) => {
                const active = settings?.width_mm === w && settings.height_mm === h
                return (
                  <Button
                    key={`${w}x${h}`}
                    variant={active ? 'primary' : 'secondary'}
                    onClick={() => update({ width: String(w), height: String(h) })}
                  >
                    {w}×{h} мм
                  </Button>
                )
              })}
            </div>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <Field label="Ширина, мм">
              <input inputMode="numeric" value={form.width} onChange={(e) => update({ width: e.target.value })} />
            </Field>
            <Field label="Высота, мм">
              <input inputMode="numeric" value={form.height} onChange={(e) => update({ height: e.target.value })} />
            </Field>
          </div>
          {!settings && (
            <div className="text-sm text-rose-600">
              Размер — целое число от {MIN_MM} до {MAX_MM} мм
            </div>
          )}
          <div className="flex flex-col gap-2">
            <Checkbox label="Бренд" checked={form.show_brand} onChange={(v) => update({ show_brand: v })} />
            <Checkbox label="Название" checked={form.show_name} onChange={(v) => update({ show_name: v })} />
            <Checkbox
              label="Характеристики: вязкость и объём масла, тип фильтра, ёмкость аккумулятора"
              checked={form.show_spec}
              onChange={(v) => update({ show_spec: v })}
            />
            <Checkbox label="Цена" checked={form.show_price} onChange={(v) => update({ show_price: v })} />
            <Checkbox label="Артикул" checked={form.show_article} onChange={(v) => update({ show_article: v })} />
          </div>
          <ErrorBox error={error} />
          {saved && <div className="text-sm text-emerald-700">Сохранено</div>}
          <div>
            <Button disabled={busy || !settings} onClick={() => void save()}>
              Сохранить
            </Button>
          </div>
        </div>
      </Card>

      <Card>
        <CardTitle>Предпросмотр</CardTitle>
        <div className="mb-2 text-xs text-slate-500">{product === SAMPLE ? 'Образец: товаров в справочнике пока нет' : `На примере товара «${product.name}»`}</div>
        {settings && preview ? (
          <div className="overflow-x-auto">
            <iframe
              title="Предпросмотр этикетки"
              srcDoc={preview}
              className="border border-dashed border-slate-400 bg-white"
              style={{
                width: Math.round(settings.width_mm * PX_PER_MM),
                height: Math.round(settings.height_mm * PX_PER_MM),
              }}
            />
          </div>
        ) : (
          <div className="text-sm text-slate-500">Укажите размер этикетки</div>
        )}
        <p className="mt-2 text-xs text-slate-500">Размер на экране приблизительный; при печати — точно в миллиметрах.</p>
      </Card>
    </div>
  )
}

export default function LabelSettingsPage() {
  const data = useLoad(() => get<LabelSettings>('/settings/labels'), [])
  return (
    <div>
      <PageHeader title="Этикетки" />
      <ErrorBox error={data.error} />
      {data.loading && !data.data ? <Loading /> : data.data && <Editor initial={data.data} />}
    </div>
  )
}
