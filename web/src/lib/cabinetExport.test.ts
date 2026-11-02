import { describe, expect, it } from 'vitest'
import { PURCHASE_COLS, cabinetQtyText, purchaseRows, purchaseTotal, type CabinetSale } from './cabinetExport'

const sale = (patch: Partial<CabinetSale>): CabinetSale => ({
  id: 's1',
  number: 17,
  kind: 'sale',
  business_date: '2026-10-30',
  created_at: '2026-10-30T05:00:00Z',
  contact_name: 'Азамат',
  vehicle_plate: '01KG123ABC',
  total_tyiyn: 350_000,
  debt_tyiyn: 350_000,
  branch_name: 'Главный',
  lines: [
    { name: 'Масло 5W-30', kind: 'pour', qty: 4_500, unit: 'ml', container_ml: 4_000, amount_tyiyn: 300_000 },
    { name: 'Замена масла', kind: 'service', qty: 1, unit: 'piece', container_ml: null, amount_tyiyn: 50_000 },
  ],
  ...patch,
})

describe('кабинет: покупки в Excel', () => {
  it('строка файла — строка чека с шапкой; долг чека — только на первой строке', () => {
    const rows = purchaseRows([sale({})])
    expect(rows).toHaveLength(2)
    const cells = rows.map((r) => PURCHASE_COLS.map((c) => c.value(r)))
    expect(cells[0].slice(1, 10)).toEqual(['Чек № 17', 'Азамат', '01KG123ABC', 'Главный', 'Масло 5W-30', { qty: 4_500, scale: 3 }, 'л', { money: 300_000 }, { money: 350_000 }])
    expect(cells[1][6]).toEqual({ int: 1 })
    expect(cells[1][7]).toBe('усл.')
    expect(cells[1][9]).toBeNull()
  })

  it('возврат и чек без строк; итог — сумма чеков', () => {
    const ret = sale({ id: 's2', number: 18, kind: 'return', total_tyiyn: -50_000, debt_tyiyn: 0, lines: [] })
    const rows = purchaseRows([sale({}), ret])
    expect(rows).toHaveLength(3)
    expect(PURCHASE_COLS[1].value(rows[2])).toBe('Возврат № 18')
    expect(PURCHASE_COLS[6].value(rows[2])).toBeNull()
    const total = purchaseTotal([sale({}), ret])
    expect(total[0]).toBe('Итого')
    expect(total[8]).toEqual({ money: 300_000 })
  })

  it('количество словами — как на экране', () => {
    expect(cabinetQtyText({ name: '', kind: 'container', qty: 2, unit: 'ml', container_ml: 4_000, amount_tyiyn: 0 })).toBe('2 кан. × 4 л')
    expect(cabinetQtyText({ name: '', kind: 'piece', qty: 3, unit: 'piece', container_ml: null, amount_tyiyn: 0 })).toBe('3 шт')
    expect(cabinetQtyText({ name: '', kind: 'length', qty: 1500, unit: 'mm', container_ml: null, amount_tyiyn: 0 })).toBe('1,5\u00a0м')
  })
})
