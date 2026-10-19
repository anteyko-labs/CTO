import { describe, expect, it } from 'vitest'
import { labelText, specText } from './labelText'

const on = { show_name: true, show_brand: true, show_spec: true }

describe('этикетка', () => {
  it('масло: бренд, название, вязкость и объём', () => {
    const p = { name: 'Magnatec 5W-30', brand: 'Castrol', attrs: { viscosity: '5W-30' }, unit: 'ml' as const, container_ml: 4000, category_kind: 'oil' }
    expect(labelText(p, on)).toEqual({ brand: 'Castrol', name: 'Magnatec 5W-30', spec: '5W-30 · 4 л' })
  })

  it('бренд не повторяется, если название с него начинается; выключенное не печатается', () => {
    const p = { name: 'Mobil Super 10W-40', brand: 'Mobil', attrs: {}, unit: 'ml' as const, container_ml: 1000, category_kind: 'oil' }
    expect(labelText(p, on).brand).toBe('')
    expect(labelText(p, { show_name: false, show_brand: false, show_spec: false })).toEqual({ brand: '', name: '', spec: '' })
  })

  it('фильтр — тип, аккумулятор — ёмкость и ток с полярностью в названии', () => {
    expect(specText({ name: 'W712', brand: 'MANN', attrs: { filter_type: 'масляный' }, unit: 'piece', container_ml: null, category_kind: 'filter' })).toBe('масляный')
    const bat = { name: 'Varta Blue', brand: 'Varta', attrs: { capacity_ah: '60', cranking_a: '540', polarity: 'обратная' }, unit: 'piece' as const, container_ml: null, category_kind: 'battery' }
    expect(labelText(bat, on)).toEqual({ brand: '', name: 'Varta Blue (П)', spec: '60 А·ч · 540 А' })
  })

  it('прочее — без характеристик', () => {
    expect(specText({ name: 'Ветошь', brand: '', attrs: {}, unit: 'piece', container_ml: null, category_kind: 'other' })).toBe('')
  })
})
