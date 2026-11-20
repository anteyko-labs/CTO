import { describe, expect, it } from 'vitest'
import { cartGoods, productSlot, type CategoryBook } from './nextChange'
import { isFilterSlot, placedSlot, type Product } from './types'

describe('«Куда поставили» фильтр и новые строки книжки (задача 041 A)', () => {
  it('масляный — как фильтр коробки, воздушный — как салонный; другое место не берётся', () => {
    expect(placedSlot('oil_filter', 'gearbox_filter')).toBe('gearbox_filter')
    expect(placedSlot('air_filter', 'cabin_filter')).toBe('cabin_filter')
    expect(placedSlot('fuel_filter', 'gearbox_filter')).toBe('fuel_filter')
    expect(placedSlot('oil_filter', null)).toBe('oil_filter')
  })

  it('сепаратор и влагоотделитель — фильтры; тип товара не меняет строку фильтра коробки', () => {
    expect(isFilterSlot('fuel_separator') && isFilterSlot('water_separator') && isFilterSlot('hydraulic_filter')).toBe(true)
    expect(isFilterSlot('oil')).toBe(false)
    expect(productSlot('gearbox_filter', 'масляный')).toBe('gearbox_filter')
  })

  it('место строки — в товарах чека для книжки', () => {
    const cats = [{ id: 'c', book_slot: 'oil_filter' }] as unknown as CategoryBook[]
    const p = { id: 'p', name: 'W712', category_id: 'c', attrs: {} } as unknown as Product
    expect(cartGoods([{ product: p, kind: 'piece', qtyText: '1', place: 'gearbox_filter' }], cats).map((g) => g.slot)).toEqual(['gearbox_filter'])
    expect(cartGoods([{ product: p, kind: 'piece', qtyText: '1' }], cats).map((g) => g.slot)).toEqual(['oil_filter'])
  })
})
