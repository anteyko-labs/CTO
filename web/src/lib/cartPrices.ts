// Цена в чеке сверяется с каталогом (проверка 27.10.2026): плитки, черновик и отложенные чеки
// хранят товар целиком, и после смены цены владельцем касса продавала бы по старой — дороже
// (клиент переплачивает) или дешевле («Скидку ставит владелец»). Перед проведением, при возврате
// отложенного чека и черновика касса берёт свежие товары и услуги (без сети — из снимка).
import { parseSom, somInput } from './format'
import type { Product, Service } from './types'

export interface PricedLine {
  key: string
  product?: Product
  service?: Service
  priceText: string
  gift?: boolean
}

/** Свежие карточки по id: с сервера или из снимка. */
export interface FreshCatalog {
  products: ReadonlyMap<string, Product>
  services: ReadonlyMap<string, Service>
}

/** Цена строки сменилась: было → стало по каталогу; `kept` — цену правили руками, она осталась. */
export interface PriceChange {
  key: string
  name: string
  before: number
  after: number
  kept: boolean
}

/**
 * Подставляет в строки свежие товары и услуги. Цена, совпадавшая с прежним прайсом, становится
 * новой; цену, поправленную руками, касса не трогает (её проверит сервер). `listOf` — прайс строки
 * с учётом всего чека (замена бесплатно с нашим маслом, ADR-096).
 */
export function repriceLines<L extends PricedLine>(
  lines: readonly L[],
  fresh: FreshCatalog,
  listOf: (l: L, all: readonly L[]) => number,
): { lines: L[]; changes: PriceChange[] } {
  let touched = false
  const next = lines.map((l) => {
    if (l.gift) return l
    const product = l.product ? fresh.products.get(l.product.id) : undefined
    const service = l.service ? fresh.services.get(l.service.id) : undefined
    if (!product && !service) return l
    touched = true
    return { ...l, ...(product ? { product } : {}), ...(service ? { service } : {}) }
  })
  if (!touched) return { lines: [...lines], changes: [] }
  const changes: PriceChange[] = []
  const out = next.map((l, i) => {
    const old = lines[i]
    if (l === old) return l
    const before = listOf(old, lines)
    const after = listOf(l, next)
    if (before === after) return l
    const kept = parseSom(old.priceText) !== before
    changes.push({ key: l.key, name: l.product?.name ?? l.service?.name ?? '', before, after, kept })
    return kept ? l : { ...l, priceText: somInput(after) }
  })
  return { lines: out, changes }
}

/** Строка предупреждения: «Масло — было 400 с, стало 350 с». */
export function priceChangeText(c: PriceChange, money: (v: number) => string): string {
  const base = `${c.name} — было ${money(c.before)}, стало ${money(c.after)}`
  return c.kept ? `${base} (цена в чеке поправлена вручную и осталась)` : base
}
