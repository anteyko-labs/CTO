import { describe, expect, it } from 'vitest'
import { isLiterUnit, presetFromHit } from './oldCatalog'
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
})
