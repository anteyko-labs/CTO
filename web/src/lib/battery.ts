// Аккумуляторы (SPEC-27): полярность в названии и аккумулятор, сданный в оплату чека.
import { formatKg } from './format'
import { divRound } from './money'
import { PAYMENT_LABELS, type Payment } from './types'

/** Полярность — характеристика `polarity` товара: значение и как её показывать. */
export const POLARITY_LABELS: Record<string, string> = {
  прямая: 'прямая (+ слева, Л)',
  обратная: 'обратная (+ справа, П)',
}

/** «Л» — прямая, «П» — обратная; у товаров без полярности — `null`. */
export function polarityMark(attrs: Record<string, string> | null | undefined): 'Л' | 'П' | null {
  const v = attrs?.polarity
  if (v === 'прямая') return 'Л'
  if (v === 'обратная') return 'П'
  return null
}

/** Название товара с полярностью: «Varta 60 (П)» — как в строке чека на сервере. */
export function productTitle(p: { name: string; attrs?: Record<string, string> | null }): string {
  const mark = polarityMark(p.attrs)
  return mark ? `${p.name} (${mark})` : p.name
}

/** Сумма за сданный аккумулятор: вес × цена приёма за кг — предпросмотр, итог считает сервер. */
export const batteryAmount = (grams: number, perKg: number): number => divRound(grams * perKg, 1000)

/** Способ оплаты в чеке: «Аккумулятор 12 кг» — с весом, остальные — названием. */
export function paymentLabel(p: Pick<Payment, 'method' | 'grams'>): string {
  if (p.method === 'battery' && p.grams) return `${PAYMENT_LABELS.battery} ${formatKg(Math.abs(p.grams))}`
  return PAYMENT_LABELS[p.method]
}

/**
 * Часть возврата, которую отдают аккумулятором (SPEC-27): деньгами возвращается не больше,
 * чем по чеку платили не аккумулятором; остальное — аккумулятором (вес уходит со склада).
 */
export function batteryShare(total: number, batteryLeft: number, otherLeft: number): number {
  return Math.max(0, Math.min(batteryLeft, total - Math.max(0, otherLeft)))
}
