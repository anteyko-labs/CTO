// Что пишется на этикетке (SPEC-02, SPEC-29 §6): бренд, название и главное о товаре — у масла
// вязкость и объём тары, у фильтра тип, у аккумулятора ёмкость и пусковой ток.
import { productTitle } from './battery'
import { formatLiters, formatSom } from './format'
import { pourPricePerL } from './money'
import type { LabelSettings, Product } from './types'

export interface LabelText {
  brand: string
  name: string
  spec: string
}

type LabelProduct = Pick<Product, 'name' | 'brand' | 'attrs' | 'unit' | 'container_ml'> & { category_kind?: string }

/** За что цена на этикетке — выбор при печати (задача 021). */
export type PricePer = 'piece' | 'liter' | 'meter'

export const PRICE_PER: Record<PricePer, string> = { piece: 'за шт', liter: 'за литр', meter: 'за метр' }

export const isPricePer = (v: unknown): v is PricePer => v === 'piece' || v === 'liter' || v === 'meter'

type PricedProduct = Pick<Product, 'unit' | 'sale_price_tyiyn'> & Partial<Pick<Product, 'container_ml' | 'pour_price_per_l_tyiyn'>>

/**
 * Цена на этикетке по выбору (задача 021): масло — за канистру («за шт») или за литр (цена литра,
 * как на кассе при розливе, ADR-074); товар в метрах — за метр, в кг — за кг, штучный — за шт.
 * Выбор не подходит товару — цена и подпись по его единице. Коротко — «500 с/л» (маленькая этикетка).
 */
export function labelPrice(p: PricedProduct, per: PricePer = 'piece', short = false): string {
  const liter = per === 'liter' ? (p.pour_price_per_l_tyiyn ?? pourPricePerL({ ...p, container_ml: p.container_ml ?? null })) : null
  const [price, unit] = p.unit === 'g' ? [p.sale_price_tyiyn, 'кг'] : p.unit === 'mm' ? [p.sale_price_tyiyn, 'м'] : liter != null ? [liter, 'л'] : [p.sale_price_tyiyn, 'шт']
  return short ? `${formatSom(price)}/${unit}` : `${formatSom(price)} за ${unit}`
}

/** Характеристики строкой: «5W-30 · 4 л», «масляный», «60 А·ч · 540 А». */
export function specText(p: LabelProduct): string {
  const a = (p.attrs ?? {}) as Record<string, string>
  const val = (k: string) => String(a[k] ?? '').trim()
  const parts: string[] = []
  if (p.unit === 'ml' || p.category_kind === 'oil') {
    if (val('viscosity')) parts.push(val('viscosity'))
    if (p.container_ml) parts.push(formatLiters(p.container_ml))
  } else if (p.category_kind === 'filter' || val('filter_type')) {
    if (val('filter_type')) parts.push(val('filter_type'))
  } else if (p.category_kind === 'battery' || val('capacity_ah') || val('cranking_a')) {
    if (val('capacity_ah')) parts.push(`${val('capacity_ah')} А·ч`)
    if (val('cranking_a')) parts.push(`${val('cranking_a')} А`)
  }
  return parts.join(' · ')
}

/** Строки этикетки по настройкам. Бренд не повторяется, если название уже с него начинается. */
export function labelText(p: LabelProduct, s: Pick<LabelSettings, 'show_name' | 'show_brand' | 'show_spec'>): LabelText {
  const brand = (p.brand ?? '').trim()
  const named = p.name.trim().toLowerCase().startsWith(brand.toLowerCase())
  return {
    brand: s.show_brand !== false && brand && !named ? brand : '',
    name: s.show_name ? productTitle(p) : '',
    spec: s.show_spec !== false ? specText(p) : '',
  }
}

/**
 * Товар для предпросмотра этикетки: масло (литры или категория «масло») с брендом и
 * характеристиками, чтобы отметки «Бренд» и «Характеристики» были видны на образце. Лучше —
 * со штрихкодом и ценой. Подходящего нет — null (берётся встроенный образец масла).
 */
export function previewPick<T extends LabelProduct & Pick<Product, 'barcodes' | 'sale_price_tyiyn'>>(list: readonly T[]): T | null {
  const oil = list.filter((p) => {
    if (p.unit !== 'ml' && p.category_kind !== 'oil') return false
    const t = labelText(p, { show_name: true, show_brand: true, show_spec: true })
    return Boolean(t.brand) && Boolean(t.spec)
  })
  return oil.find((p) => p.barcodes.length > 0 && p.sale_price_tyiyn > 0) ?? oil[0] ?? null
}
