// Касса и замена масла (SPEC-29 §18, §23): спросить «Это замена масла?» и проверить «через сколько
// менять», который кассир ставит прямо в чеке.
import { toKm, type MileageUnit } from './mileage'

/**
 * «Это замена масла?» на вынос (ADR-118): моторного масла не меньше канистры его категории.
 * 0,5–1 л — доливка, без вопроса. `enough` — категории масла, которого в чеке не меньше канистры
 * (`oilEnough`); моторная — со строкой книжки «Масло двигателя». Категории ещё не загрузились —
 * спрашиваем от канистры любого масла.
 */
export function askOilChange(enough: ReadonlySet<string>, categories: readonly { id: string; book_slot: string }[] | null | undefined): boolean {
  if (!categories) return enough.size > 0
  return [...enough].some((id) => categories.find((c) => c.id === id)?.book_slot === 'oil')
}

/** В чеке есть услуга, которая требует «В сервис», — любая, кроме доливки (ADR-118). */
export function hasServiceWork(lines: readonly { kind: string; service?: { takeaway?: boolean } | null }[]): boolean {
  return lines.some((l) => l.kind === 'service' && !l.service?.takeaway)
}

/** В чеке есть доливка — на вынос можно выбрать мастера (ADR-118). */
export function hasTopup(lines: readonly { kind: string; service?: { takeaway?: boolean } | null }[]): boolean {
  return lines.some((l) => l.kind === 'service' && Boolean(l.service?.takeaway))
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
