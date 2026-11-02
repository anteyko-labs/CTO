/**
 * Остаток на кассе (задача 015, ADR-184): без «Продажи в минус» товар с остатком 0 в чек не
 * кладётся, а чек, где строк товара больше остатка, не пробивается. Единицы — как у остатка:
 * штуки, мл, граммы, миллиметры (I1); канистры уже пересчитаны в мл. Окончательно решает сервер.
 */

export interface StockItem {
  id: string
  name: string
  stock_qty: number
}

/** Можно ли положить товар в чек. */
export const canAddToCart = (p: Pick<StockItem, 'stock_qty'>, allowNegative: boolean): boolean => allowNegative || p.stock_qty > 0

/** Товары чека, которых не хватает: строки одного товара складываются. Порядок — как в чеке. */
export function shortages<T extends StockItem>(lines: { product: T | null | undefined; units: number }[], allowNegative: boolean): T[] {
  if (allowNegative) return []
  const need = new Map<string, number>()
  const items = new Map<string, T>()
  for (const l of lines) {
    if (!l.product) continue
    need.set(l.product.id, (need.get(l.product.id) ?? 0) + l.units)
    items.set(l.product.id, l.product)
  }
  return [...items.values()].filter((p) => (need.get(p.id) ?? 0) > p.stock_qty)
}
