// Остатки → Разбор из Умага (задача 042): временный экран переноса товара и остатков из Умага.
// Каждая строка выгрузки — «Новый товар» (карточка заполнена из строки) или «Уже есть в Автодом»
// (похожие из общей базы, «Это он»); количество строки уходит в черновик ревизии «весь склад».
import { useState } from 'react'
import { Pager, usePagedLoad } from '../components/Pager'
import { ProductFormModal } from '../components/ProductFormModal'
import type { SimilarProduct } from '../components/ProductSimilarBlock'
import { Badge, Button, Card, Empty, ErrorBox, Modal, Money, PageHeader, toast } from '../components/ui'
import { get, newOpId, post, put, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { presetFromHit } from '../lib/oldCatalog'
import type { Category, OldCatalogHit, Product } from '../lib/types'
import { mapUmagRows } from '../lib/umag'
import { readXlsx } from '../lib/xlsx'

type Status = 'todo' | 'new' | 'linked' | 'skipped'

export interface IntakeRow extends OldCatalogHit {
  qty: number | null
  intake_status: Status
  intake_product_id: string | null
  intake_product_name: string | null
  intake_product_unit?: string | null
  intake_counted: number | null
}

interface IntakeOut {
  intake_status: Status
  revision_id: string | null
  intake_counted: number | null
}

const FILTERS: [Status, string][] = [
  ['todo', 'Не разобрано'],
  ['new', 'Новые'],
  ['linked', 'Привязаны'],
  ['skipped', 'Пропущены'],
]

const qtyText = (q: number | null) => (q === null ? '—' : q.toLocaleString('ru-RU'))
/** Записанное в ревизию в единицах товара: мл → л, г → кг, мм → м. */
function countedText(counted: number, unit: string | null | undefined): string {
  const [div, u] = unit === 'ml' ? [1000, 'л'] : unit === 'g' ? [1000, 'кг'] : unit === 'mm' ? [1000, 'м'] : [1, 'шт.']
  return `${(counted / div).toLocaleString('ru-RU', { maximumFractionDigits: 3 })} ${u}`
}

/** Итог действия: записано в ревизию или количество ждёт черновика. */
function savedText(r: IntakeOut, what: string): string {
  if (r.revision_id && r.intake_counted !== null) return `${what}; количество записано в ревизию`
  return `${what}; количество ждёт ревизию — её начинает владелец`
}

function SimilarModal({ row, onClose, onDone, onNew }: { row: IntakeRow; onClose: () => void; onDone: () => void; onNew: () => void }) {
  const list = useLoad(() => get<SimilarProduct[]>(`/old-catalog/${row.id}/similar`), [row.id])
  const [find, setFind] = useState('')
  const term = find.trim()
  const found = useLoad(
    () => (term.length < 2 ? Promise.resolve([] as Product[]) : get<Product[]>(`/products?q=${encodeURIComponent(term)}&limit=30`)),
    [term],
  )
  const act = useAction()
  const pick = (p: { id: string; name: string }) =>
    void act.run(async () => {
      const r = await post<IntakeOut>(`/old-catalog/${row.id}/intake/link`, { op_id: newOpId(), product_id: p.id })
      toast(savedText(r, `Привязано к «${p.name}»`))
      onDone()
    })
  return (
    <Modal title={`Уже есть в Автодом: ${row.name}`} onClose={onClose} wide>
      <div className="flex flex-col gap-2" data-testid="intake-similar">
        <ErrorBox error={list.error ?? act.error} />
        {list.data?.length === 0 && <div className="text-sm text-slate-500">Похожих товаров не нашли.</div>}
        {list.data?.map((p) => (
          <div key={p.id} className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-slate-200 px-3 py-2">
            <div className="min-w-0">
              <div className="font-medium">
                {p.name} {p.reason === 'code' && <Badge tone="green">тот же код</Badge>}
              </div>
              <div className="text-xs text-slate-500">
                {p.category_name}
                {p.article && ` · ${p.article}`} · {formatSom(p.sale_price_tyiyn)}
              </div>
            </div>
            <Button disabled={act.busy} onClick={() => pick(p)}>
              Это он
            </Button>
          </div>
        ))}
        <div className="border-t border-slate-200 pt-3">
          <input
            className="w-full rounded-md border border-slate-300 px-3 py-2"
            placeholder="Поиск по всем товарам: название, код, артикул"
            value={find}
            onChange={(e) => setFind(e.target.value)}
            data-testid="intake-find"
          />
        </div>
        <ErrorBox error={found.error} />
        {term.length >= 2 && found.data?.length === 0 && <div className="text-sm text-slate-500">Ничего не нашли.</div>}
        {found.data?.map((p) => (
          <div key={p.id} className="flex flex-wrap items-center justify-between gap-2 rounded-md border border-slate-200 px-3 py-2">
            <div className="min-w-0">
              <div className="font-medium">{p.name}</div>
              <div className="text-xs text-slate-500">
                {p.article && `${p.article} · `}
                {p.barcodes[0] && `${p.barcodes[0]} · `}
                {formatSom(p.sale_price_tyiyn)}
              </div>
            </div>
            <Button disabled={act.busy} onClick={() => pick(p)}>
              Это он
            </Button>
          </div>
        ))}
        <div className="pt-2">
          <Button variant="secondary" onClick={onNew}>
            Нет, это новый
          </Button>
        </div>
      </div>
    </Modal>
  )
}

export default function UmagIntake() {
  const [status, setStatus] = useState<Status>('todo')
  const [q, setQ] = useState('')
  const rows = usePagedLoad<IntakeRow>(`/old-catalog/intake${qs({ status, q: q.trim() || undefined })}`, 'umag-intake')
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  const [creating, setCreating] = useState<IntakeRow | null>(null)
  const [similar, setSimilar] = useState<IntakeRow | null>(null)
  const act = useAction()
  const upload = useAction()
  const preset = creating ? presetFromHit(creating, categories.data ?? []) : null

  const op = (row: IntakeRow, what: 'skip' | 'undo') =>
    void act.run(async () => {
      await post(`/old-catalog/${row.id}/intake/${what}`, { op_id: newOpId() })
      toast(what === 'skip' ? 'Пропущено' : 'Строка снова не разобрана')
      rows.reload()
    })

  const onUpload = (file: File) =>
    void upload.run(async () => {
      const list = mapUmagRows(await readXlsx(file))
      const r = await put<{ rows: number }>('/old-catalog', list)
      toast(`Загружено позиций: ${r.rows.toLocaleString('ru-RU')}`)
      rows.reload()
    })

  return (
    <div>
      <PageHeader
        title="Разбор из Умага"
        subtitle="Каждая строка выгрузки — новый товар или уже заведённый. Количество попадает в ревизию «весь склад»: остатки встанут, когда владелец её проведёт."
        actions={
          <label className="inline-flex cursor-pointer items-center gap-2 rounded-md border border-slate-300 bg-white px-3 py-2 text-sm">
            Загрузить выгрузку .xlsx
            <input
              type="file"
              accept=".xlsx"
              className="hidden"
              data-testid="intake-upload"
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) onUpload(f)
                e.target.value = ''
              }}
            />
          </label>
        }
      />
      <Card>
        <div className="mb-3 flex flex-wrap items-center gap-2">
          {FILTERS.map(([s, label]) => (
            <Button key={s} variant={s === status ? 'primary' : 'secondary'} onClick={() => setStatus(s)}>
              {label}
            </Button>
          ))}
          <input
            className="ml-auto min-w-[200px] rounded-md border border-slate-300 px-3 py-2 text-sm"
            placeholder="Поиск: название, штрихкод, артикул"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </div>
        <ErrorBox error={rows.error ?? act.error ?? upload.error} />
        {upload.busy && <div className="text-sm text-slate-500">Загрузка файла…</div>}
        {!rows.loading && rows.items.length === 0 ? (
          <Empty>Строк нет. Загрузите выгрузку Умага с колонкой «Остаток».</Empty>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm" data-testid="intake-table">
              <thead className="text-left text-xs text-slate-500">
                <tr>
                  <th className="py-2 pr-2">Название</th>
                  <th className="pr-2">Штрихкод / артикул</th>
                  <th className="pr-2 text-right">Кол-во</th>
                  <th className="pr-2 text-right">Закупка</th>
                  <th className="pr-2 text-right">Цена</th>
                  <th className="pr-2">Категория Умага</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {rows.items.map((r) => (
                  <tr key={r.id} className="border-t border-slate-100 align-top" data-testid="intake-row">
                    <td className="py-2 pr-2">
                      <div className="font-medium">{r.name}</div>
                      {r.intake_product_name && <div className="text-xs text-slate-500">→ {r.intake_product_name}</div>}
                    </td>
                    <td className="pr-2 text-xs">
                      {r.barcode}
                      {r.article && <div className="text-slate-500">{r.article}</div>}
                    </td>
                    <td className="pr-2 text-right tabular-nums">
                      {qtyText(r.qty)} {r.unit}
                      {r.intake_counted !== null && r.intake_status !== 'todo' && (
                        <div className="text-xs text-slate-500">в ревизии: {countedText(r.intake_counted, r.intake_product_unit)}</div>
                      )}
                    </td>
                    <td className="pr-2 text-right">{r.cost_tyiyn === null ? '—' : <Money value={r.cost_tyiyn} />}</td>
                    <td className="pr-2 text-right">
                      <Money value={r.price_tyiyn} />
                    </td>
                    <td className="pr-2 text-xs">{r.category}</td>
                    <td className="py-1">
                      {r.intake_status === 'todo' ? (
                        <div className="flex flex-wrap justify-end gap-1">
                          <Button onClick={() => setCreating(r)}>Новый товар</Button>
                          <Button variant="secondary" onClick={() => setSimilar(r)}>
                            Уже есть в Автодом
                          </Button>
                          <Button variant="ghost" disabled={act.busy} onClick={() => op(r, 'skip')}>
                            Пропустить
                          </Button>
                        </div>
                      ) : (
                        <div className="flex justify-end">
                          <Button variant="ghost" disabled={act.busy} onClick={() => op(r, 'undo')}>
                            Отменить
                          </Button>
                        </div>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        <Pager page={rows.page} size={rows.size} total={rows.total} onPage={rows.setPage} onSize={rows.setSize} />
      </Card>
      {similar && (
        <SimilarModal
          row={similar}
          onClose={() => setSimilar(null)}
          onDone={() => {
            setSimilar(null)
            rows.reload()
          }}
          onNew={() => {
            setCreating(similar)
            setSimilar(null)
          }}
        />
      )}
      {creating && preset && (
        <ProductFormModal
          preset={preset}
          {...(preset.cost ? { presetCost: { text: preset.cost, per: 'can' as const } } : {})}
          umagRow={creating}
          createUrl={`/old-catalog/${creating.id}/intake/new`}
          onClose={() => setCreating(null)}
          onSaved={() => {
            setCreating(null)
            toast('Товар заведён; количество — в ревизии')
            rows.reload()
          }}
        />
      )}
    </div>
  )
}
