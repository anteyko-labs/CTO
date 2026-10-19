// Оприходование без поставщика (SPEC-29 §8): количество из поля и стоимость — как считает сервер.
import { parseLiters } from './format'
import { divRound } from './money'
import type { Product } from './types'

/** Единиц в цене: канистра масла, килограмм, штука — как цена товара. */
const pricePer = (p: Pick<Product, 'unit' | 'container_ml'>): number =>
  p.unit === 'ml' ? (p.container_ml ?? 1000) : p.unit === 'g' ? 1000 : 1

/** Количество из поля: штуки целым, масло литрами, на вес килограммами. */
export function stockInQty(p: Pick<Product, 'unit'>, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (p.unit === 'piece') return /^\d+$/.test(t) && Number(t) > 0 ? Number(t) : null
  const v = parseLiters(t)
  return v !== null && v > 0 ? v : null
}

/** Стоимость: цена × количество / единицу цены, до тыйына — как на сервере. */
export const stockInCost = (p: Pick<Product, 'unit' | 'container_ml'>, qty: number, price: number): number =>
  divRound(price * qty, pricePer(p))

