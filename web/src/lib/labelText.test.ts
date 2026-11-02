import { describe, expect, it } from 'vitest'
import { labelPrice, labelText, previewPick, specText } from './labelText'

const on = { show_name: true, show_brand: true, show_spec: true }

describe('этикетка', () => {
  it('без бренда (null) и с числом в характеристиках — не падает (пробная этикетка станции)', () => {
    const p = { name: 'Пробная этикетка', brand: null, unit: 'piece', attrs: { capacity_ah: 60 } } as never
    expect(labelText(p, on).brand).toBe('')
    expect(labelText(p, on).name).toBe('Пробная этикетка')
  })

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

describe('образец для предпросмотра этикетки', () => {
  type P = { name: string; brand: string; attrs: Record<string, string>; unit: 'piece' | 'ml' | 'g'; container_ml: number | null; barcodes: string[]; sale_price_tyiyn: number }
  const base: Omit<P, 'name' | 'brand'> = { attrs: {}, unit: 'piece', container_ml: null, barcodes: ['1'], sale_price_tyiyn: 100 }
  it('масло с брендом и характеристиками, лучше со штрихкодом и ценой', () => {
    const rag: P = { ...base, name: 'Ветошь', brand: '' }
    const oilNoBrand: P = { ...base, name: 'Масло 5W-40', brand: '', unit: 'ml', container_ml: 4000, attrs: { viscosity: '5W-40' } }
    const oilNoCode: P = { ...base, name: 'Magnatec', brand: 'Castrol', unit: 'ml', container_ml: 4000, barcodes: [], attrs: { viscosity: '5W-30' } }
    const oil: P = { ...oilNoCode, name: 'Super 3000', brand: 'Mobil', barcodes: ['2'] }
    expect(previewPick([rag, oilNoBrand, oilNoCode, oil])).toBe(oil)
    expect(previewPick([rag, oilNoCode])).toBe(oilNoCode)
    expect(previewPick([rag, oilNoBrand])).toBeNull()
  })
})

describe('цена на этикетке (ADR-178, задача 021)', () => {
  const nb = (x: string) => x.replace(/\u00a0/g, ' ')
  const oil = { unit: 'ml' as const, container_ml: 4000, sale_price_tyiyn: 200_000, pour_price_per_l_tyiyn: 50_000 }
  it('по умолчанию «за шт»: подпись у любой единицы', () => {
    expect(nb(labelPrice({ unit: 'g', sale_price_tyiyn: 30_000 }))).toBe('300 с за кг')
    expect(nb(labelPrice({ unit: 'mm', sale_price_tyiyn: 12_000 }))).toBe('120 с за м')
    expect(nb(labelPrice({ unit: 'piece', sale_price_tyiyn: 12_000 }))).toBe('120 с за шт')
    expect(nb(labelPrice(oil))).toBe('2 000 с за шт')
  })
  it('масло: за канистру или за литр; коротко — «500 с/л»', () => {
    expect(nb(labelPrice(oil, 'piece'))).toBe('2 000 с за шт')
    expect(nb(labelPrice(oil, 'liter'))).toBe('500 с за л')
    expect(nb(labelPrice(oil, 'liter', true))).toBe('500 с/л')
    expect(nb(labelPrice(oil, 'piece', true))).toBe('2 000 с/шт')
  })
  it('цена литра без ответа сервера — от канистры, до тыйына, как на кассе', () => {
    expect(nb(labelPrice({ unit: 'ml', container_ml: 4000, sale_price_tyiyn: 199_999 }, 'liter'))).toBe('500 с за л')
    expect(nb(labelPrice({ unit: 'ml', container_ml: 3000, sale_price_tyiyn: 100_000 }, 'liter'))).toBe('333,33 с за л')
  })
  it('выбор не подходит товару — по его единице', () => {
    expect(nb(labelPrice({ unit: 'piece', sale_price_tyiyn: 12_000 }, 'liter'))).toBe('120 с за шт')
    expect(nb(labelPrice({ unit: 'piece', sale_price_tyiyn: 12_000 }, 'meter'))).toBe('120 с за шт')
    expect(nb(labelPrice(oil, 'meter'))).toBe('2 000 с за шт')
    expect(nb(labelPrice({ unit: 'mm', sale_price_tyiyn: 12_000 }, 'piece'))).toBe('120 с за м')
    expect(nb(labelPrice({ unit: 'mm', sale_price_tyiyn: 12_000 }, 'liter', true))).toBe('120 с/м')
    expect(nb(labelPrice({ unit: 'g', sale_price_tyiyn: 30_000 }, 'meter'))).toBe('300 с за кг')
  })
})
