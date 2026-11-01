// Поиск в списке ревизии по названию или артикулу находит и то, чего в списке нет (задача 005, Н7):
// товар программы вне списка — «Посчитать», строку старой базы Умаг — «Завести» с найденным
// количеством (как скан незнакомого кода, ADR-114). Строки Умага — с правом «Товары и штрихкоды»
// (SPEC-35 §4), без него — только заведённые товары.
import { get, qs } from '../lib/api'
import { formatSom } from '../lib/format'
import { useDebounced, useLoad } from '../lib/hooks'
import type { OldCatalogHit, Product } from '../lib/types'
import { codesShort, searchUmag, umagPer, unlinkedHit } from '../lib/umagLink'
import { Button, Card } from './ui'

export function RevisionSearchHits({
  text,
  shown,
  fits,
  mayUmag,
  onCount,
  onUmag,
}: {
  /** Набранное в «Найти в списке». */
  text: string
  /** Товары, чьи строки уже есть в ревизии на экране. */
  shown: ReadonlySet<string>
  /** Товар входит в эту ревизию (весь склад или её категория). */
  fits: (p: Product) => boolean
  /** Право заводить из Умага: владелец и «Товары и штрихкоды». */
  mayUmag: boolean
  onCount: (p: Product) => void
  onUmag: (hit: OldCatalogHit) => void
}) {
  const query = useDebounced(text.trim(), 250)
  const on = query.length >= 2
  // Подсказка: сбой поиска не мешает считать — блока просто нет.
  const own = useLoad(() => (on ? get<Product[]>(`/products${qs({ q: query, limit: 15 })}`).catch(() => []) : Promise.resolve([])), [query])
  const old = useLoad(
    () => (on && mayUmag ? searchUmag({ q: query }).catch(() => []) : Promise.resolve<OldCatalogHit[]>([])),
    [query, mayUmag],
  )
  if (!on || query !== text.trim()) return null
  const outside = (own.data ?? []).filter((p) => !shown.has(p.id) && fits(p)).slice(0, 8)
  const umag = (old.data ?? []).filter(unlinkedHit).slice(0, 8)
  if (outside.length === 0 && umag.length === 0) return null
  return (
    <Card className="flex flex-col gap-3" data-testid="revision-search-hits">
      {outside.length > 0 && (
        <div>
          <div className="text-sm font-medium text-slate-700">Есть в программе, нет в списке ревизии</div>
          <ul className="divide-y divide-slate-100">
            {outside.map((p) => (
              <li key={p.id} className="flex flex-wrap items-center justify-between gap-2 py-2" data-own-row>
                <div className="min-w-0 flex-1">
                  <div className="font-medium">{p.name}</div>
                  {p.article && <div className="text-xs text-slate-500">{p.article}</div>}
                </div>
                <Button variant="secondary" className="min-h-[34px] px-3 py-1" onClick={() => onCount(p)}>
                  Посчитать
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
      {umag.length > 0 && (
        <div data-testid="revision-umag">
          <div className="text-sm font-medium text-slate-700">Из старой базы Умаг — ещё не заведены</div>
          <ul className="divide-y divide-slate-100">
            {umag.map((h) => (
              <li key={h.id} className="flex flex-wrap items-center justify-between gap-2 py-2" data-umag-row>
                <div className="min-w-0 flex-1">
                  <div className="font-medium">{h.name}</div>
                  <div className="text-xs text-slate-500">{[codesShort(h), h.category].filter(Boolean).join(' · ')}</div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <span className="text-sm tabular-nums">{h.price_tyiyn ? `${formatSom(h.price_tyiyn)}${umagPer(h.unit)}` : '—'}</span>
                  <Button variant="secondary" className="min-h-[34px] px-3 py-1" onClick={() => onUmag(h)}>
                    Завести
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Card>
  )
}
