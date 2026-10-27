import { describe, expect, it } from 'vitest'
import { groupQty, groupRows, rowQty, rowViscosity, stockSheetHtml, stockTotal } from './stockSheet'
import type { CategoryTotal, ProductRow } from './types'

const row = (o: Partial<ProductRow>): ProductRow =>
  ({
    id: 'p',
    category_id: 'oil',
    category_kind: 'oil',
    name: 'Масло',
    brand: '',
    article: '',
    unit: 'piece',
    container_ml: null,
    attrs: {},
    archived: false,
    barcodes: [],
    sale_price_tyiyn: 0,
    pour_price_per_l_tyiyn: null,
    min_stock: 0,
    stock_qty: 0,
    needs_review: false,
    last_purchase_price_tyiyn: null,
    code: '',
    stock_sale_tyiyn: 0,
    ...o,
  }) as ProductRow

const total = (o: Partial<CategoryTotal>): CategoryTotal => ({
  category_id: 'oil',
  name: 'Масла',
  kind: 'oil',
  count: 0,
  pieces: 0,
  ml: 0,
  grams: 0,
  container_ml: null,
  sale_tyiyn: 0,
  ...o,
})

describe('«Склад → Остатки» (SPEC-36 §14)', () => {
  it('остаток: масло — литры и «кан. + л», вес — кг, штуки', () => {
    expect(rowQty({ unit: 'ml', container_ml: 4000, stock_qty: 34_500 })).toBe('34,5 л · 8 кан. + 2,5 л')
    expect(rowQty({ unit: 'ml', container_ml: 4000, stock_qty: 2_500 })).toBe('2,5 л')
    expect(rowQty({ unit: 'g', container_ml: null, stock_qty: 12_350 })).toBe('12,35 кг')
    expect(rowQty({ unit: 'piece', container_ml: null, stock_qty: -2 })).toBe('-2 шт')
  })

  it('подытог группы: одна тара — «кан. + л», разная — только литры', () => {
    expect(groupQty(total({ ml: 38_500, container_ml: 4000 }))).toBe('38,5 л · 9 кан. + 2,5 л')
    expect(groupQty(total({ ml: 38_500 }))).toBe('38,5 л')
    expect(groupQty(total({ pieces: 3, grams: 1500 }))).toBe('3 шт · 1,5 кг')
    expect(stockTotal([total({ pieces: 3, ml: 1000, container_ml: 4000 }), total({ pieces: 2 })])).toEqual({
      pieces: 5,
      ml: 1000,
      grams: 0,
      container_ml: null,
    })
  })

  it('строки — по группам в порядке подытогов', () => {
    const items = [row({ id: 'a', category_id: 'f' }), row({ id: 'b', category_id: 'oil' }), row({ id: 'c', category_id: 'f' })]
    const g = groupRows(items, [total({ category_id: 'oil' }), total({ category_id: 'f', name: 'Фильтры' })])
    expect(g.map((x) => x.rows.map((r) => r.id))).toEqual([['b'], ['a', 'c']])
  })

  it('вязкость — из карточки, у масла без неё — из названия', () => {
    expect(rowViscosity(row({ attrs: { viscosity: '5W-30' } }))).toEqual({ text: '5W-30', fromName: false })
    expect(rowViscosity(row({ name: 'Shell 10W40 4L' }))).toEqual({ text: '10W-40', fromName: true })
    expect(rowViscosity(row({ name: 'Фильтр 5W30', category_kind: 'filter' }))).toBeNull()
  })

  it('лист: минус помечен «проверить», закупка — только если её дал сервер', () => {
    const groups = [{ total: total({ count: 1, pieces: -2, sale_tyiyn: -20_000 }), rows: [row({ name: 'Лампа <H4>', stock_qty: -2, stock_sale_tyiyn: -20_000 })] }]
    const admin = stockSheetHtml({ at: '2026-10-27T08:00:00Z', groups, total: { count: 1, sale_tyiyn: -20_000 } })
    expect(admin).toContain('Лампа &lt;H4&gt; <b>(проверить)</b>')
    expect(admin).not.toContain('закупка')
    expect(admin).toContain('Итого по складу: 1 поз.')
    const owner = stockSheetHtml({ at: '2026-10-27T08:00:00Z', groups, total: { count: 1, sale_tyiyn: -20_000, purchase_tyiyn: -15_000 } })
    expect(owner).toContain('Сумма (закупка)')
  })
})
