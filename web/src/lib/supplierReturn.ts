// Возврат поставщику (SPEC-29 §9): количество из поля и сумма по цене последнего прихода — как на сервере.
import { divRound } from './money'
import type { Product } from './types'
import { parseQty, priceUnits } from './units'

/** Единиц в цене: канистра масла, килограмм, метр, штука. */
const per = (p: Pick<Product, 'unit' | 'container_ml'>): number => priceUnits(p.unit, p.container_ml)

/** Количество: штуки целым, масло литрами, кг и м — десятичным; в единицах учёта. */
export function returnQty(p: Pick<Product, 'unit'>, text: string): number | null {
  const v = parseQty(p.unit, text)
  return v !== null && v > 0 ? v : null
}

/** Сумма строки по цене за штуку/канистру/кг/м: цена × количество / единицу, до тыйына. */
export const returnAmount = (p: Pick<Product, 'unit' | 'container_ml'>, qty: number, price: number): number => divRound(price * qty, per(p))
