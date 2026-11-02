import { describe, expect, it } from 'vitest'
import { isKgUnit, isLiterUnit, presetFromHit } from './oldCatalog'
import type { Category, OldCatalogHit } from './types'

const cat = (id: string, name: string, kind: Category['kind']): Category => ({ id, name, kind, attributes: [] }) as unknown as Category
const cats = [cat('oil', 'Масла', 'oil'), cat('flt', 'Фильтры', 'filter')]
const hit = (over: Partial<OldCatalogHit>): OldCatalogHit => ({
  id: 'h',
  name: 'Фильтр W712',
  barcode: '4600000000017',
  article: 'W712',
  cost_tyiyn: 30_000,
  price_tyiyn: 50_000,
  unit: 'шт',
  supplier: '',
  category: 'Фильтры',
  exact: true,
  linked_product_id: null,
  ...over,
})

describe('карточка из старой программы', () => {
  it('штучный товар: категория по названию, цены как есть', () => {
    expect(presetFromHit(hit({}), cats)).toEqual({
      name: 'Фильтр W712',
      barcode: '4600000000017',
      article: 'W712',
      categoryId: 'flt',
      liters: '',
      price: '500',
      cost: '300',
    })
  })

  it('масло: литры из названия, цена за литр — за канистру', () => {
    const p = presetFromHit(hit({ name: 'AZMOL 5W40 4Л', unit: 'литр', price_tyiyn: 50_000, cost_tyiyn: 37_550, category: '' }), cats)
    expect(p).toMatchObject({ categoryId: 'oil', liters: '4', price: '2000', cost: '1502' })
  })

  it('строка «литр»: код карточки — заводской EAN, а не код литра Умага (276…, PLU)', () => {
    const liter = hit({ name: 'CASTROL MAGNATEC 5W30 4Л', unit: 'литр', barcode: '2760391', codes: ['2760391', '1392', '4008177071423'] })
    expect(presetFromHit(liter, cats).barcode).toBe('4008177071423')
    expect(presetFromHit(liter, cats, '', '1392').barcode).toBe('4008177071423')
    expect(presetFromHit(liter, cats, '', '4008177071423').barcode).toBe('4008177071423')
    // Без заводского кода — пусто: код выдаст программа.
    expect(presetFromHit(hit({ unit: 'литр', barcode: '2760391', codes: ['2760391', '1392'] }), cats, '', '2760391').barcode).toBe('')
    // Без кодов от сервера — по тому же правилу на устройстве.
    expect(presetFromHit(hit({ unit: 'кг', barcode: '2760445', article: '', codes: undefined }), cats).barcode).toBe('')
  })

  it('строка «литр» с наклейкой «1 литр» и без заводского кода — код карточки выдаст программа', () => {
    // Рабочая выгрузка: MULLER SN 5W40 5L — 2760320, наклейка 2760320010001 (код строки + 01000 + контрольная), PLU 1321.
    const muller = hit({ name: 'MULLER SN 5W40 5L', unit: 'литр', barcode: '2760320', article: '', codes: ['2760320', '2760320010001', '1321'] })
    expect(presetFromHit(muller, cats).barcode).toBe('')
    expect(presetFromHit(muller, cats, '', '2760320010001').barcode).toBe('')
    // Скан наклейки с потерянным нулём впереди — тоже код меры.
    expect(presetFromHit(muller, cats, '', '02760320010001').barcode).toBe('')
    // Код магазина 211… у строки «литр» — тоже не код канистры.
    expect(presetFromHit(hit({ unit: 'литр', barcode: '2760161', article: '', codes: ['2760161', '2110000000073'] }), cats).barcode).toBe('')
  })

  it('без права на закупку (сервер не отдал её) — закупка пустая', () => {
    expect(presetFromHit(hit({ cost_tyiyn: null }), cats).cost).toBe('')
  })

  it('код скана важнее кода из списка; неизвестная категория — запомненная', () => {
    const p = presetFromHit(hit({ category: 'Прочее' }), cats, 'flt', '222')
    expect(p.barcode).toBe('222')
    expect(p.categoryId).toBe('flt')
    expect(presetFromHit(hit({ category: '' }), cats, 'нет такой').categoryId).toBe('')
  })

  it('единица «литр»', () => {
    expect(isLiterUnit('литр')).toBe(true)
    expect(isLiterUnit('л')).toBe(true)
    expect(isLiterUnit('шт')).toBe(false)
  })

  it('строка «кг»: товар в килограммах, цена и закупка за кг как есть (ADR-174)', () => {
    expect(isKgUnit('кг')).toBe(true)
    expect(isKgUnit('Килограмм')).toBe(true)
    expect(isKgUnit('шт')).toBe(false)
    const p = presetFromHit(hit({ name: 'Солидол', unit: 'кг', price_tyiyn: 30_000, cost_tyiyn: 20_000 }), cats)
    expect(p).toMatchObject({ unit: 'g', liters: '', price: '300', cost: '200' })
    expect(presetFromHit(hit({}), cats).unit).toBeUndefined()
  })

  it('строка «м»: товар в метрах; единица сервера важнее колонки (ADR-178)', () => {
    const p = presetFromHit(hit({ name: 'Шланг 8 мм', unit: 'м', price_tyiyn: 25_000, cost_tyiyn: 15_000 }), cats)
    expect(p).toMatchObject({ unit: 'mm', liters: '', price: '250', cost: '150' })
    expect(presetFromHit(hit({ unit: 'метр' }), cats).unit).toBe('mm')
    expect(presetFromHit(hit({ unit: 'шт', product_unit: 'mm' }), cats).unit).toBe('mm')
    // «г» — штуки: цена у Умага за грамм.
    expect(presetFromHit(hit({ unit: 'г' }), cats).unit).toBeUndefined()
  })
})

describe('масло из Умага — в свою категорию масел (задача 007)', () => {
  const withSlot = (id: string, name: string, kind: Category['kind'], book_slot: string) => ({ ...cat(id, name, kind), book_slot }) as unknown as Category
  const list = [
    withSlot('af', 'АНТИФРИЗЫ', 'oil', 'antifreeze'),
    withSlot('gear', 'ТРАНСМИСИОННЫЕ МАСЛА', 'oil', 'gear_oil'),
    withSlot('motor', 'МОТОРНЫЕ МАСЛА', 'oil', 'oil'),
    withSlot('chem', 'АВТОХИМИЯ', 'other', 'none'),
  ]
  it('без категории в Умаге — моторные, а не первая по алфавиту', () => {
    expect(presetFromHit(hit({ name: 'LIQUI MOLY 5W40 4L', unit: 'литр', category: 'Незаданные' }), list).categoryId).toBe('motor')
  })
  it('категория Умага — масла: она', () => {
    expect(presetFromHit(hit({ name: 'AZMOL ATF 4L', unit: 'литр', category: 'ТРАНСМИСИОННЫЕ МАСЛА' }), list).categoryId).toBe('gear')
  })
  it('категория Умага — не масла (литры автохимии): моторные', () => {
    expect(presetFromHit(hit({ name: 'Очиститель 5Л', unit: 'литр', category: 'АВТОХИМИЯ' }), list).categoryId).toBe('motor')
  })
})
