// Вид строки товара при добавлении в чек (задача 010): бочка — сразу розливом в литрах.
import type { Product, SaleLineKind } from './types'
import { lineKindOf } from './units'

/** Товар в литрах с тарой строго больше `overL` литров — розлив, иначе канистра; 0 — всегда канистра. */
export const defaultLineKind = (p: Pick<Product, 'unit' | 'container_ml'>, overL: number): SaleLineKind => {
  if (p.unit !== 'ml') return lineKindOf(p.unit)
  return overL > 0 && (p.container_ml ?? 0) > overL * 1000 ? 'pour' : 'container'
}
