// Что пишется на этикетке (SPEC-02, SPEC-29 §6): бренд, название и главное о товаре — у масла
// вязкость и объём тары, у фильтра тип, у аккумулятора ёмкость и пусковой ток.
import { productTitle } from './battery'
import { formatLiters } from './format'
import type { LabelSettings, Product } from './types'

export interface LabelText {
  brand: string
  name: string
  spec: string
}

type LabelProduct = Pick<Product, 'name' | 'brand' | 'attrs' | 'unit' | 'container_ml'> & { category_kind?: string }

/** Характеристики строкой: «5W-30 · 4 л», «масляный», «60 А·ч · 540 А». */
export function specText(p: LabelProduct): string {
  const a = (p.attrs ?? {}) as Record<string, string>
  const val = (k: string) => (a[k] ?? '').trim()
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
  const brand = p.brand.trim()
  const named = p.name.trim().toLowerCase().startsWith(brand.toLowerCase())
  return {
    brand: s.show_brand !== false && brand && !named ? brand : '',
    name: s.show_name ? productTitle(p) : '',
    spec: s.show_spec !== false ? specText(p) : '',
  }
}
