// Блок «Умаг» в карточке товара (SPEC-35 §3): связанная строка старой базы — название, коды, цена в
// Умаге; «Связать другую» и «Отвязать». Отвязка оставляет коды у товара (их убирают в списке кодов
// карточки). «Связать другую» связывает новую и отвязывает прежнюю; если строк Умага у товара
// несколько (дубли в Умаге), видны все — лишнюю можно отвязать.
import { useState } from 'react'
import { get } from '../lib/api'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import type { OldCatalogHit, Product, ProductUmag } from '../lib/types'
import { useUmagOn } from '../lib/umagSwitch'
import { hitCodes, linkSummary, linkUmag, opIds, sameCode, searchUmag, umagPer, umagUnitWord, unlinkUmag } from '../lib/umagLink'
import { Button, Empty, ErrorBox, Field, Loading, toast } from './ui'

export function UmagCard({
  productId,
  codes,
  mayEdit,
  onProduct,
}: {
  productId: string
  /** Коды товара сейчас (список в карточке); без них — из ответа сервера. */
  codes?: readonly string[]
  mayEdit: boolean
  onProduct: (p: Product) => void
}) {
  const [tick, setTick] = useState(0)
  const load = useLoad(() => get<Product>(`/products/${productId}`), [productId, tick])
  const [choosing, setChoosing] = useState(false)
  const act = useAction()
  const [ops] = useState(opIds)
  const umag = load.data?.umag ?? null
  const more = load.data?.umag_more ?? []
  // Умаг выключен (SPEC-36 §15) — блока нет.
  const on = useUmagOn()
  if (!load.data || !on) return null
  // Коды строки, которых у товара нет: заняты другим товаром или убраны из карточки (SPEC-35 §1).
  // Коды меры (литр, кг, метр) товару не переходят — они отдельно (ADR-113). Товара без кода не
  // бывает: пустой список — ещё не известен, берём ответ сервера.
  const own = codes && codes.length > 0 ? codes : load.data.barcodes
  const measure = umag?.measure_codes ?? []
  const missing = (umag?.codes ?? []).filter((c) => !measure.includes(c) && !own.some((b) => sameCode(b, c)))

  const choose = (h: OldCatalogHit) =>
    void act.run(async () => {
      const key = `link:${h.id}`
      const r = await linkUmag(h.id, productId, ops.get(key))
      ops.done(key)
      if (umag && umag.id !== h.id) {
        const prev = `unlink:${umag.id}`
        await unlinkUmag(umag.id, ops.get(prev))
        ops.done(prev)
      }
      toast(`Связано со строкой Умага «${h.name}»: ${linkSummary(r)}`)
      setChoosing(false)
      onProduct(r.product)
      setTick((t) => t + 1)
    })

  const unlink = (row: ProductUmag | null) => {
    if (!row) return
    if (!window.confirm(`Отвязать строку Умага «${row.name}»? Коды останутся у товара.`)) return
    void act.run(async () => {
      const key = `unlink:${row.id}`
      await unlinkUmag(row.id, ops.get(key))
      ops.done(key)
      toast('Строка Умага отвязана, коды остались у товара')
      setTick((t) => t + 1)
    })
  }

  const small = 'min-h-[34px] px-3 py-1 text-xs'
  return (
    <section className="mt-4 rounded-md border border-slate-200 p-3" data-testid="umag-card" aria-label="Умаг">
      <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold">Умаг</h3>
        {mayEdit && (
          <div className="flex flex-wrap gap-2">
            <Button variant="secondary" className={small} disabled={act.busy} onClick={() => setChoosing((c) => !c)}>
              {umag ? 'Связать другую' : 'Связать со строкой Умага'}
            </Button>
            {umag && (
              <Button variant="danger-outline" className={small} disabled={act.busy} onClick={() => unlink(umag)}>
                Отвязать
              </Button>
            )}
          </div>
        )}
      </div>
      {umag ? (
        <div className="flex flex-col gap-0.5 text-sm">
          <div className="font-medium">{umag.name}</div>
          <div className="text-slate-600">Коды: {umag.codes.length > 0 ? umag.codes.join(', ') : umag.barcode || '—'}</div>
          <div className="text-slate-600">
            Цена в Умаге {umag.price_tyiyn ? `${formatSom(umag.price_tyiyn)}${umagPer(umag.unit ?? '')}` : '—'}
            {umag.cost_tyiyn != null && umag.cost_tyiyn > 0 && ` · закупка ${formatSom(umag.cost_tyiyn)}${umagPer(umag.unit ?? '')}`}
          </div>
          {measure.length > 0 && (
            <div className="text-slate-600" data-testid="umag-card-measure">
              Коды {measure.join(', ')} в Умаге продавали {umagUnitWord(umag.unit ?? '')} — у товара их нет: на кассе они добавили бы всю тару
            </div>
          )}
          {missing.length > 0 && (
            <div className="text-amber-800" data-testid="umag-card-missing">
              Не у этого товара: {missing.join(', ')} — код у другого товара (конфликт) или убран из карточки
            </div>
          )}
          {more.length > 0 && (
            <div className="mt-2 border-t border-slate-100 pt-2" data-testid="umag-card-more">
              <div className="text-xs font-medium uppercase text-slate-500">Ещё связаны со строками Умага: {more.length}</div>
              <ul className="mt-1 flex flex-col gap-1">
                {more.map((m) => (
                  <li key={m.id} className="flex flex-wrap items-center justify-between gap-2">
                    <span className="min-w-0">
                      <span className="font-medium">{m.name}</span>
                      <span className="ml-2 text-xs text-slate-500">
                        {m.codes.join(', ') || m.barcode}
                        {m.price_tyiyn ? ` · ${formatSom(m.price_tyiyn)}${umagPer(m.unit ?? '')}` : ''}
                      </span>
                    </span>
                    {mayEdit && (
                      <Button variant="danger-outline" className={small} disabled={act.busy} onClick={() => unlink(m)}>
                        Отвязать
                      </Button>
                    )}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      ) : (
        <div className="text-sm text-slate-500">Не связан со строкой старой базы Умаг.</div>
      )}
      {choosing && <UmagRowChooser productId={productId} busy={act.busy} onChoose={choose} />}
      <ErrorBox error={act.error} />
    </section>
  )
}

/** Поиск строки Умага по названию или коду: несвязанную — связать с этим товаром. */
function UmagRowChooser({ productId, busy, onChoose }: { productId: string; busy: boolean; onChoose: (h: OldCatalogHit) => void }) {
  const [q, setQ] = useState('')
  const query = useDebounced(q.trim(), 250)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<OldCatalogHit[]>([]) : searchUmag({ q: query, barcode: query })),
    [query],
  )
  const rows = found.data ?? []
  return (
    <div className="mt-3 flex flex-col gap-2 border-t border-slate-200 pt-3" data-testid="umag-chooser">
      <Field label="Строка Умага">
        <input autoFocus placeholder="Название или код из Умага" value={q} onChange={(e) => setQ(e.target.value)} />
      </Field>
      <ErrorBox error={found.error} />
      {found.loading && !found.data ? (
        <Loading />
      ) : query.length < 2 ? (
        <Empty>Введите хотя бы две буквы или код</Empty>
      ) : rows.length === 0 ? (
        <Empty>В базе Умаг ничего не нашлось</Empty>
      ) : (
        <ul className="flex max-h-72 flex-col divide-y divide-slate-100 overflow-y-auto">
          {rows.map((h) => {
            const mine = h.product_id === productId
            const other = Boolean(h.product_id) && !mine
            return (
              <li key={h.id} className="flex items-center justify-between gap-3 py-2 text-sm">
                <div className="min-w-0">
                  <div className="font-medium">{h.name}</div>
                  <div className="text-xs text-slate-500">
                    {[hitCodes(h).join(', '), h.price_tyiyn ? formatSom(h.price_tyiyn) : '', other ? `связана с «${h.product_name ?? 'другим товаром'}»` : '']
                      .filter(Boolean)
                      .join(' · ')}
                  </div>
                </div>
                {mine ? (
                  <span className="shrink-0 text-xs text-emerald-700">связана с этим товаром</span>
                ) : (
                  <Button variant="secondary" className="min-h-[34px] shrink-0 px-3 py-1 text-xs" disabled={busy || other} onClick={() => onChoose(h)}>
                    Связать
                  </Button>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
