// Вид строки товара при добавлении в чек (задача 007): бочка — сразу розливом в литрах.
import type { Product, SaleLineKind } from './types'

/** Товар в литрах с тарой строго больше `overL` литров — розлив, иначе канистра; 0 — всегда канистра. */
export const defaultLineKind = (p: Pick<Product, 'unit' | 'container_ml'>, overL: number): SaleLineKind => {
  if (p.unit === 'g') return 'weight'
  if (p.unit !== 'ml') return 'piece'
  return overL > 0 && (p.container_ml ?? 0) > overL * 1000 ? 'pour' : 'container'
}
