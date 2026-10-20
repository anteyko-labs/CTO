// Сортировка «Остатков» по заголовкам столбцов: название, остаток, стоимость (стоимость — только
// владельцу, её поле администратору сервер не отдаёт). Без выбора — порядок сервера.
import type { Product } from './types'

export type StockSortKey = 'name' | 'qty' | 'value'

export interface StockSort {
  key: StockSortKey | null
  desc: boolean
}

/** Щелчок по заголовку: тот же столбец — обратный порядок; числа сначала большие, название — по алфавиту. */
export function nextStockSort(cur: StockSort, key: StockSortKey): StockSort {
  return cur.key === key ? { key, desc: !cur.desc } : { key, desc: key !== 'name' }
}

/** Строки в выбранном порядке; пустые значения — в конце, при равенстве — по названию. */
export function sortStock<T extends Pick<Product, 'name' | 'stock_qty'> & { stock_value_tyiyn?: number | null }>(rows: readonly T[], sort: StockSort): T[] {
  if (!sort.key) return [...rows]
  const val = (p: T): number | string | null =>
    sort.key === 'name' ? p.name : sort.key === 'qty' ? p.stock_qty : (p.stock_value_tyiyn ?? null)
  return [...rows].sort((a, b) => {
    const x = val(a)
    const y = val(b)
    if (x === null && y === null) return a.name.localeCompare(b.name, 'ru')
    if (x === null) return 1
    if (y === null) return -1
    const c = typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y), 'ru')
    return (sort.desc ? -c : c) || a.name.localeCompare(b.name, 'ru')
  })
}
