// Возврат поставщику (SPEC-29 §9): количество из поля и сумма по цене последнего прихода — как на сервере.
import { parseLiters } from './format'
import { divRound } from './money'
import type { Product } from './types'

/** Единиц в цене: канистра масла, килограмм, штука. */
const per = (p: Pick<Product, 'unit' | 'container_ml'>): number => (p.unit === 'ml' ? (p.container_ml ?? 1000) : p.unit === 'g' ? 1000 : 1)

/** Количество: штуки целым, масло литрами, на вес килограммами; в единицах учёта. */
export function returnQty(p: Pick<Product, 'unit'>, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (p.unit === 'piece') return /^\d+$/.test(t) && Number(t) > 0 ? Number(t) : null
  const v = parseLiters(t)
  return v !== null && v > 0 ? v : null
}

/** Сумма строки по цене за штуку/канистру/кг: цена × количество / единицу, до тыйына. */
export const returnAmount = (p: Pick<Product, 'unit' | 'container_ml'>, qty: number, price: number): number => divRound(price * qty, per(p))
