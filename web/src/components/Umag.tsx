// Строки старой базы Умаг там, где они видны (SPEC-35 §2): «Товары», «Приход», ревизия, «Сканер
// товаров». На кассе и в остальных выборах товара Умага нет — там код Умага находит только
// связанный товар (его коды — коды товара).
// Окно строки, которой нет среди товаров: «Завести» — полная карточка из Умага (`presetFromHit`), после
// сохранения товар связан и коды строки у него; «Это уже есть» — выбрать свой товар и связать.
// Без права «Товары и штрихкоды» (ADR-097) — только текст.
import { useState } from 'react'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom } from '../lib/format'
import { useAction, useDebounced, useLoad } from '../lib/hooks'
import { presetFromHit } from '../lib/oldCatalog'
import { can } from '../lib/permissions'
import type { Category, OldCatalogHit, Product } from '../lib/types'
import { codesShort, hitCodes, hitMeasureCodes, linkSummary, linkUmag, opIds, searchUmag, UMAG_NO_RIGHT, umagPer, umagUnitWord, unlinkedHit } from '../lib/umagLink'
import { qtyText } from '../lib/units'
import { ProductFormModal } from './ProductFormModal'
import { Button, Card, CardTitle, Empty, ErrorBox, Field, Loading, Modal, Notice, toast } from './ui'

/** Товар заведён из строки Умага или связан с ней. `cost` — закупка из Умага (сомы строкой, за штуку
 *  или канистру) для строки прихода, пока у товара нет своих приходов. */
export interface UmagDone {
  cost?: string
  created: boolean
}

type Stage = 'info' | 'create' | 'pick'

/**
 * Окно «Товар из старой базы Умаг». `start` — с чего начать: строку выбрали в списке — сразу
 * карточка (`create`) или выбор своего товара (`pick`); скан — сведения и кнопки (`info`).
 */
export function UmagHitModal({
  hit,
  code = '',
  start = 'info',
  addLabel = 'Завести',
  onClose,
  onDone,
}: {
  hit: OldCatalogHit
  /** Отсканированный код — первым кодом новой карточки. */
  code?: string
  start?: Stage
  /** Подпись «Завести»: в приходе — «Завести и добавить». */
  addLabel?: string
  onClose: () => void
  onDone: (p: Product, done: UmagDone) => void
}) {
  const user = useUser()
  const mayEdit = can(user, 'products')
  // Закупка в Умаге — владельцу и тому, кто вводит приход: в накладной она его (ADR-008, ADR-113).
  const showCost = can(user, 'receipts') && hit.cost_tyiyn !== null
  const [stage, setStage] = useState<Stage>(mayEdit ? start : 'info')
  const categories = useLoad(() => get<Category[]>('/categories'), [])
  // Код строки уже у товара, а строка не связана: скорее всего, это он — предложим первым.
  const same = useLoad(
    () => (mayEdit && hit.linked_product_id && !hit.product_id ? get<Product>(`/products/${hit.linked_product_id}`) : Promise.resolve(null)),
    [hit.id],
  )
  const act = useAction()
  const [ops] = useState(opIds)

  const link = (p: Product) =>
    void act.run(async () => {
      const key = `${hit.id}:${p.id}`
      const r = await linkUmag(hit.id, p.id, ops.get(key))
      ops.done(key)
      toast(`Связано с «${r.product.name}»: ${linkSummary(r)}`)
      onDone(r.product, { created: false })
    })

  if (stage === 'create') {
    if (!categories.data) {
      return (
        <Modal title="Новый товар" onClose={onClose}>
          {categories.error ? <ErrorBox error={categories.error} /> : <Loading />}
        </Modal>
      )
    }
    const preset = presetFromHit(hit, categories.data, '', code)
    return <ProductFormModal preset={preset} umagRow={hit} onClose={onClose} onSaved={(p) => onDone(p, { cost: preset.cost, created: true })} />
  }

  if (stage === 'pick') {
    return <PickOwnProduct hit={hit} busy={act.busy} error={act.error} onBack={() => setStage('info')} onClose={onClose} onPick={link} />
  }

  const codes = hitCodes(hit)
  const measure = hitMeasureCodes(hit)
  return (
    <Modal title="Товар из старой базы Умаг" onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="umag-modal">
        <div>
          <div className="text-lg font-semibold">{hit.name}</div>
          <div className="text-sm text-slate-600">
            {[hit.supplier && `поставщик ${hit.supplier}`, hit.category && `категория ${hit.category}`, hit.unit && `ед. ${hit.unit}`]
              .filter(Boolean)
              .join(' · ') || 'без поставщика и категории'}
          </div>
        </div>
        <dl className="grid grid-cols-2 gap-3 text-sm">
          <div>
            <dt className="text-slate-500">Цена продажи в Умаге</dt>
            <dd className="text-base font-semibold tabular-nums">
              {hit.price_tyiyn ? formatSom(hit.price_tyiyn) : '—'}
              {hit.price_tyiyn ? <span className="text-sm font-normal text-slate-500">{umagPer(hit.unit)}</span> : null}
            </dd>
          </div>
          {showCost && (
            <div data-testid="umag-cost">
              <dt className="text-slate-500">Закупка в Умаге</dt>
              <dd className="text-base font-semibold tabular-nums">
                {hit.cost_tyiyn ? formatSom(hit.cost_tyiyn) : '—'}
                {hit.cost_tyiyn ? <span className="text-sm font-normal text-slate-500">{umagPer(hit.unit)}</span> : null}
              </dd>
            </div>
          )}
        </dl>
        <div className="text-sm">
          <span className="text-slate-500">Коды:</span> {codes.length > 0 ? codes.join(', ') : '—'}
        </div>
        {measure.length > 0 && (
          <div className="text-sm text-slate-600" data-testid="umag-measure-codes">
            Коды {measure.join(', ')} в Умаге продавали {umagUnitWord(hit.unit)}: к товару они не перейдут — на кассе они добавили бы
            всю тару.
          </div>
        )}
        {code && <div className="text-sm text-slate-600">Код {code} есть в базе Умаг, а среди товаров его нет.</div>}
        {mayEdit ? (
          <>
            {same.data && (
              <Notice tone="amber">
                Код этой строки уже у товара «{same.data.name}». Если это он — свяжите: остальные коды Умага перейдут к нему.
                <div className="mt-2">
                  <Button variant="secondary" disabled={act.busy} onClick={() => same.data && link(same.data)}>
                    Это он — связать
                  </Button>
                </div>
              </Notice>
            )}
            <ErrorBox error={act.error} />
            <div className="flex flex-wrap justify-end gap-2 border-t border-slate-200 pt-3">
              <Button variant="secondary" onClick={onClose}>
                Отмена
              </Button>
              <Button variant="secondary" data-testid="umag-pick-existing" disabled={act.busy} onClick={() => setStage('pick')}>
                Это уже есть — выбрать товар
              </Button>
              {/* Без автофокуса: Enter сканера остаётся в поле поиска и не заводит товар сам. */}
              <Button data-testid="umag-create" disabled={act.busy} onClick={() => setStage('create')}>
                {addLabel}
              </Button>
            </div>
          </>
        ) : (
          <>
            <p className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-900" data-testid="umag-noright">
              {UMAG_NO_RIGHT}
            </p>
            <div className="flex justify-end">
              <Button variant="secondary" onClick={onClose}>
                Закрыть
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}

/** «Это уже есть»: найти свой товар и связать с ним строку Умага. */
function PickOwnProduct({
  hit,
  busy,
  error,
  onBack,
  onClose,
  onPick,
}: {
  hit: OldCatalogHit
  busy: boolean
  error: string | null
  onBack: () => void
  onClose: () => void
  onPick: (p: Product) => void
}) {
  const [q, setQ] = useState(hit.name)
  const query = useDebounced(q.trim(), 250)
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<Product[]>([]) : get<Product[]>(`/products${qs({ q: query, limit: 10 })}`)),
    [query],
  )
  const rows = found.data ?? []
  return (
    <Modal title="Какой это товар?" onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="umag-pick">
        <p className="text-sm text-slate-600">
          Строка Умага «{hit.name}» свяжется с выбранным товаром, её коды станут его кодами — по ним товар найдут касса, приход и ревизия.
        </p>
        <Field label="Найти свой товар">
          <input autoFocus placeholder="Название, бренд, артикул, код" value={q} onFocus={(e) => e.target.select()} onChange={(e) => setQ(e.target.value)} />
        </Field>
        <ErrorBox error={found.error ?? error} />
        {found.loading && !found.data ? (
          <Loading />
        ) : query.length < 2 ? (
          <Empty>Введите хотя бы две буквы</Empty>
        ) : rows.length === 0 ? (
          <Empty>Ничего не нашлось — измените запрос</Empty>
        ) : (
          <ul className="flex max-h-80 flex-col gap-2 overflow-y-auto">
            {rows.map((p) => (
              <li key={p.id}>
                <Card className="flex items-center justify-between gap-3 py-2">
                  <div className="min-w-0">
                    <div className="font-medium">{p.name}</div>
                    <div className="text-xs text-slate-500">
                      {[p.brand, p.article, qtyText(p.unit, p.stock_qty), formatSom(p.sale_price_tyiyn)].filter(Boolean).join(' · ')}
                    </div>
                  </div>
                  <Button variant="secondary" disabled={busy} onClick={() => onPick(p)}>
                    Связать
                  </Button>
                </Card>
              </li>
            ))}
          </ul>
        )}
        <div className="flex justify-between gap-2 border-t border-slate-200 pt-3">
          <Button variant="ghost" onClick={onBack}>
            ← Назад
          </Button>
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/**
 * Блок «Из старой базы Умаг» в выпадающем списке поиска товара (приход, ревизия): строки, ещё не
 * заведённые, после своих товаров. Нумерация `active` — общая со своими товарами.
 */
export function UmagPickerBlock({
  hits,
  offset,
  active,
  onHover,
  onChoose,
}: {
  hits: OldCatalogHit[]
  offset: number
  active: number
  onHover: (i: number) => void
  onChoose: (h: OldCatalogHit) => void
}) {
  const mayEdit = can(useUser(), 'products')
  return (
    <li className="border-t border-slate-200" data-testid="umag-block">
      <div className="bg-slate-50 px-3 py-1 text-xs font-medium uppercase text-slate-500">Из старой базы Умаг — ещё не заведены</div>
      <ul>
        {hits.map((h, j) => {
          const i = offset + j
          return (
            <li key={h.id}>
              <button
                type="button"
                data-old-hit
                className={`flex w-full items-center justify-between gap-3 px-3 py-2 text-left text-sm ${i === active ? 'bg-sky-50' : 'hover:bg-slate-50'}`}
                onMouseEnter={() => onHover(i)}
                onClick={() => onChoose(h)}
              >
                <span className="min-w-0">
                  <span className="font-medium">{h.name}</span>
                  <span className="ml-2 text-xs text-slate-500">{codesShort(h)}</span>
                </span>
                <span className="flex shrink-0 items-center gap-2 text-right text-xs text-slate-500">
                  {h.price_tyiyn ? `${formatSom(h.price_tyiyn)}${umagPer(h.unit)}` : ''}
                  {mayEdit && <span className="rounded border border-sky-300 px-1.5 py-0.5 font-medium text-sky-700">Завести</span>}
                </span>
              </button>
            </li>
          )
        })}
      </ul>
    </li>
  )
}

/**
 * «Товары»: под списком при поиске — строки Умага, ещё не заведённые (до 8), с «Завести» и
 * «Это уже есть». `version` — перечитать после заведения или связывания.
 */
export function UmagSearchBlock({
  query,
  version = 0,
  onOpen,
}: {
  query: string
  version?: number
  onOpen: (hit: OldCatalogHit, start: 'create' | 'pick') => void
}) {
  const mayEdit = can(useUser(), 'products')
  const found = useLoad(
    () => (query.length < 2 ? Promise.resolve<OldCatalogHit[]>([]) : searchUmag({ q: query, barcode: query }).catch(() => [])),
    [query, version],
  )
  const rows = (found.data ?? []).filter(unlinkedHit).slice(0, 8)
  if (query.length < 2 || rows.length === 0) return null
  return (
    <Card className="mt-4" data-testid="umag-block">
      <CardTitle>Из старой базы Умаг — ещё не заведены</CardTitle>
      <ul className="divide-y divide-slate-100">
        {rows.map((h) => (
          <li key={h.id} className="flex flex-wrap items-center justify-between gap-2 py-2" data-umag-row>
            <div className="min-w-0 flex-1">
              <div className="font-medium">{h.name}</div>
              <div className="text-xs text-slate-500">{[codesShort(h), h.supplier, h.category].filter(Boolean).join(' · ')}</div>
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <span className="text-sm tabular-nums">{h.price_tyiyn ? `${formatSom(h.price_tyiyn)}${umagPer(h.unit)}` : '—'}</span>
              {mayEdit && (
                <>
                  <Button variant="secondary" className="min-h-[34px] px-3 py-1" onClick={() => onOpen(h, 'pick')}>
                    Это уже есть
                  </Button>
                  <Button variant="secondary" className="min-h-[34px] px-3 py-1" onClick={() => onOpen(h, 'create')}>
                    Завести
                  </Button>
                </>
              )}
            </div>
          </li>
        ))}
      </ul>
      {!mayEdit && <p className="mt-2 text-xs text-slate-500">{UMAG_NO_RIGHT}.</p>}
    </Card>
  )
}
