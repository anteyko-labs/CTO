// Касса и замена масла (SPEC-29 §18, §23): спросить «Это замена масла?» и проверить «через сколько
// менять», который кассир ставит прямо в чеке.
import { toKm, type MileageUnit } from './mileage'
import type { CategoryKind } from './types'

/** В чеке есть моторное масло (товар категории масел): тогда на вынос спрашиваем, не замена ли это. */
export function hasEngineOil(lines: readonly { product?: { category_kind?: CategoryKind } | null; gift?: boolean }[]): boolean {
  return lines.some((l) => !l.gift && l.product?.category_kind === 'oil')
}

/** Интервал из чека: от 500 до 200 000 км (мили переводятся) и от 1 до 1095 дней — как на сервере;
 *  null — всё верно, иначе — что не так. */
export function nextChangeProblem(dist: string, days: string, unit: MileageUnit): string | null {
  if (!/^\d+$/.test(dist) || !/^\d+$/.test(days)) return 'через сколько менять — числами'
  const km = toKm(Number(dist), unit)
  if (km < 500 || km > 200_000) return unit === 'mi' ? 'через сколько менять: от 311 до 124 274 миль' : 'через сколько менять: от 500 до 200 000 км'
  const d = Number(days)
  if (d < 1 || d > 1095) return 'через сколько менять: от 1 до 1095 дней'
  return null
}
