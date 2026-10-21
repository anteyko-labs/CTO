// «Остатки» вошли в «Товары» (SPEC-30 §11): старый адрес `/stock` ведёт на «Товары» с теми же
// отборами, ссылки сводки — сразу на нужный отбор.
import type { StockChip } from './types'

const CHIPS: readonly StockChip[] = ['low', 'out', 'stale', 'review']

/** Адрес «Товаров» с отбором по складу (`all` — без отбора). */
export function productsLink(chip: StockChip = 'all'): string {
  return chip === 'all' ? '/products' : `/products?stock=${chip}`
}

/**
 * Адрес «Товаров» из адреса «Остатков»: `filter=low` (и прежнее `low=true`) → `stock=low`;
 * поиск, категория и вкладка «Брак» — как были.
 */
export function stockToProducts(search: string): string {
  const from = new URLSearchParams(search)
  const to = new URLSearchParams()
  const filter = from.get('stock') ?? from.get('filter') ?? (from.get('low') === 'true' ? 'low' : null)
  if (filter && (CHIPS as readonly string[]).includes(filter)) to.set('stock', filter)
  for (const key of ['q', 'category_id']) {
    const v = from.get(key)
    if (v) to.set(key, v)
  }
  if (from.get('tab') === 'defects') to.set('tab', 'defects')
  const s = to.toString()
  return s ? `/products?${s}` : '/products'
}
