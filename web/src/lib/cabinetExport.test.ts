import { describe, expect, it } from 'vitest'
import { INVOICE_COLS, PURCHASE_COLS, cabinetQtyText, invoiceRows, invoiceTotal, purchaseRows, purchaseTotal, type CabinetSale, type SupplierCabinetInvoice } from './cabinetExport'

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

describe('кабинет поставщика: накладные в Excel', () => {
  const inv = (patch: Partial<SupplierCabinetInvoice>): SupplierCabinetInvoice => ({
    id: 'r1',
    number: 5,
    supplier_doc: '15',
    business_date: '2026-11-01',
    created_at: '2026-11-01T05:00:00Z',
    branch_name: '',
    total_tyiyn: 1_000_000,
    paid_tyiyn: 400_000,
    left_tyiyn: 600_000,
    status: 'partial',
    reversed: false,
    lines: [
      { name: 'Масло 5W-30', qty: 20_000, unit: 'ml', container_ml: 4_000, amount_tyiyn: 800_000 },
      { name: 'Фильтр', qty: 4, unit: 'piece', container_ml: null, amount_tyiyn: 200_000 },
    ],
    files: [],
    ...patch,
  })

  it('строка файла — строка накладной; суммы накладной — только на первой строке', () => {
    const rows = invoiceRows([inv({})])
    expect(rows).toHaveLength(2)
    const cells = rows.map((r) => INVOICE_COLS.map((c) => c.value(r)))
    expect(cells[0].slice(1, 11)).toEqual(['№ 15 (приход 5)', '', 'Масло 5W-30', { qty: 20_000, scale: 3 }, 'л', { money: 800_000 }, { money: 1_000_000 }, { money: 400_000 }, { money: 600_000 }, 'оплачена частично'])
    expect(cells[1][4]).toEqual({ int: 4 })
    expect(cells[1][5]).toBe('шт')
    expect(cells[1][7]).toBeNull()
    expect(cells[1][10]).toBe('')
  })

  it('отменённая не входит в итог; без номера поставщика — номер прихода', () => {
    const gone = inv({ id: 'r2', number: 6, supplier_doc: '', reversed: true, lines: [] })
    const rows = invoiceRows([inv({}), gone])
    expect(rows).toHaveLength(3)
    expect(INVOICE_COLS[1].value(rows[2])).toBe('Приход № 6')
    expect(INVOICE_COLS[10].value(rows[2])).toBe('отменена')
    expect(INVOICE_COLS[8].value(rows[2])).toBeNull()
    const total = invoiceTotal([inv({}), gone])
    expect(total[0]).toBe('Итого')
    expect(total[7]).toEqual({ money: 1_000_000 })
    expect(total[9]).toEqual({ money: 600_000 })
  })
})
