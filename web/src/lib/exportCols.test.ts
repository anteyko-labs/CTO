// Столбцы выгрузки в Excel (задача 003 Н13): у администратора нет закупки, себестоимости и прибыли
// (I13); количества и деньги — числами из целых, коды — текстом, сторно — отдельным столбцом.
import { describe, expect, it } from 'vitest'
import { FREQUENT_COLS, INVOICE_PAYMENT_COLS, debtSheet, productCols, reconciliationSheet, stockSheet, topProductCols } from './exportCols'
import { exportTitle, totalBy, xl } from './exportTable'
import { kindQty, shiftReportSheets } from './shiftReportExport'
import type { ShiftReport } from './shiftReport'
import type { Reconciliation } from './debtDocs'
import type { CategoryTotal, DebtRow, ProductRow, TopProduct } from './types'
import type { Cell, Sheet, SheetRow } from './xlsxWrite'

const labels = (cols: { label: string }[]) => cols.map((c) => c.label)
const cells = (r: Cell[] | SheetRow) => (Array.isArray(r) ? r : r.cells)

const oil: ProductRow = {
  id: 'p1',
  category_id: 'c1',
  category_kind: 'oil',
  name: 'Mobil 5W-30',
  brand: 'Mobil',
  article: 'M530',
  unit: 'ml',
  container_ml: 4000,
  attrs: { viscosity: '5W-30' },
  archived: false,
  barcodes: ['0888602305', '4601234567890'],
  sale_price_tyiyn: 350_000,
  pour_price_per_l_tyiyn: null,
  min_stock: 8000,
  stock_qty: 34_500,
  needs_review: false,
  last_purchase_price_tyiyn: null,
  code: '0888602305',
  purchase_price_tyiyn: 280_000,
  stock_purchase_tyiyn: 2_415_000,
  stock_sale_tyiyn: 3_018_750,
}

describe('Товары', () => {
  it('владелец видит закупку и сумму по закупке, администратор — нет', () => {
    const owner = productCols({ owner: true, category: () => 'Масла' })
    const admin = productCols({ owner: false, category: () => 'Масла' })
    expect(labels(owner)).toContain('Закупка')
    expect(labels(owner)).toContain('Сумма по закупке')
    expect(labels(admin)).not.toContain('Закупка')
    expect(labels(admin)).not.toContain('Сумма по закупке')
    expect(labels(admin)).toEqual(labels(owner).filter((l) => l !== 'Закупка' && l !== 'Сумма по закупке'))
  })

  it('код и штрихкоды — текстом, остаток — литрами из мл, деньги — тыйынами', () => {
    const cols = productCols({ owner: true, category: () => 'Масла' })
    const row = Object.fromEntries(cols.map((c) => [c.label, c.value(oil)]))
    expect(row['Код']).toBe('0888602305')
    expect(row['Штрихкоды']).toBe('0888602305, 4601234567890')
    expect(row['Остаток']).toEqual({ qty: 34_500, scale: 3 })
    expect(row['Ед.']).toBe('л')
    expect(row['Закупка']).toEqual({ money: 280_000 })
    expect(row['Сумма по продаже']).toEqual({ money: 3_018_750 })
  })

  it('отметки — отдельным столбцом; архив и «нет»', () => {
    const cols = productCols({ owner: false, category: () => '' })
    const marks = cols.find((c) => c.label === 'Отметки')
    expect(marks?.value({ ...oil, stock_qty: 0, archived: true })).toBe('архив')
    expect(marks?.value({ ...oil, stock_qty: 0, min_stock: 1 })).toBe('нет')
    expect(marks?.value({ ...oil, stock_qty: 4000 })).toBe('ниже минимума')
  })

  it('итог по подписям: столбца закупки у администратора нет — и ячейки нет', () => {
    const admin = productCols({ owner: false, category: () => '' })
    const total = totalBy(admin, { Название: 'Итого', 'Сумма по закупке': xl.money(1), 'Сумма по продаже': xl.money(2) })
    expect(total.length).toBe(admin.length)
    expect(total).not.toContainEqual({ money: 1 })
    expect(total[labels(admin).indexOf('Сумма по продаже')]).toEqual({ money: 2 })
  })
})

describe('Остатки', () => {
  const group: CategoryTotal = { category_id: 'c1', name: 'Масла', kind: 'oil', count: 1, pieces: 0, ml: 34_500, grams: 0, container_ml: 4000, purchase_tyiyn: 2_415_000, sale_tyiyn: 3_018_750 }
  const args = { groups: [{ total: group, rows: [oil] }], total: { count: 1, sale_tyiyn: 3_018_750, purchase_tyiyn: 2_415_000 } }

  it('администратору — без «Сумма (закупка)» и без закупки в подытогах', () => {
    const admin = stockSheet({ ...args, owner: false })
    expect(labels(admin.columns)).not.toContain('Сумма (закупка)')
    for (const r of admin.rows) expect(cells(r)).not.toContainEqual({ money: 2_415_000 })
    const owner = stockSheet({ ...args, owner: true })
    expect(labels(owner.columns)).toContain('Сумма (закупка)')
  })

  it('без суммы по продаже от сервера (не владелец) — подытог и итог без денег, строки с суммой', () => {
    const { sale_tyiyn: _g, ...bare } = group
    const s = stockSheet({ groups: [{ total: bare, rows: [oil] }], total: { count: 1 }, owner: false })
    const sale = labels(s.columns).indexOf('Сумма (продажа)')
    expect(cells(s.rows[0])[sale]).toEqual({ money: 3_018_750 })
    expect(cells(s.rows[1])[sale]).toBeNull()
    expect(cells(s.rows[2])[sale]).toBeNull()
    expect(cells(s.rows[2])[1]).toBe('Итого по складу: 1 поз.')
    expect(cells(s.rows[2])[3]).toEqual({ qty: 34_500, scale: 3 })
  })

  it('категория у строки, подытог категории и итог склада — жирными строками', () => {
    const s = stockSheet({ ...args, owner: true })
    expect(s.rows.length).toBe(3)
    expect(cells(s.rows[0]).slice(0, 5)).toEqual(['Масла', 'Mobil 5W-30', '5W-30', { qty: 34_500, scale: 3 }, 'л'])
    expect(s.rows[1]).toMatchObject({ bold: true })
    expect(cells(s.rows[1])[1]).toBe('Итого по категории: 1 поз.')
    expect(cells(s.rows[2])[1]).toBe('Итого по складу: 1 поз.')
    expect(cells(s.rows[2])[3]).toEqual({ qty: 34_500, scale: 3 })
  })
})

describe('Остатки в метрах (ADR-175)', () => {
  it('подытог только метрами — числом с тремя знаками, итог склада складывает мм', () => {
    const hose = { ...oil, id: 'h', name: 'Шланг', unit: 'mm', container_ml: null, stock_qty: 1250, min_stock: 0 } as ProductRow
    const g: CategoryTotal = { category_id: 'c2', name: 'Шланги', kind: 'other', count: 1, pieces: 0, ml: 0, grams: 0, mm: 1250, container_ml: null }
    const s = stockSheet({ groups: [{ total: g, rows: [hose] }], total: { count: 1 }, owner: false })
    expect(cells(s.rows[0]).slice(3, 5)).toEqual([{ qty: 1250, scale: 3 }, 'м'])
    expect(cells(s.rows[1])[3]).toEqual({ qty: 1250, scale: 3 })
    expect(cells(s.rows[2])[3]).toEqual({ qty: 1250, scale: 3 })
  })
})

describe('Продаваемые товары', () => {
  const r: TopProduct & { rank: number } = { rank: 1, product_id: 'p', name: 'Фильтр', unit: 'piece', container_ml: null, category_id: 'c', qty: 7, amount_tyiyn: 70_000, checks: 5, profit_tyiyn: 20_000 }
  it('прибыль — только когда её показывает экран', () => {
    expect(labels(topProductCols(true))).toEqual(['№', 'Товар', 'Продано', 'Ед.', 'Выручка', 'Прибыль', 'Чеков'])
    expect(labels(topProductCols(false))).toEqual(['№', 'Товар', 'Продано', 'Ед.', 'Выручка', 'Чеков'])
    expect(topProductCols(false).map((c) => c.value(r))).not.toContainEqual({ money: 20_000 })
  })
})

describe('Долги', () => {
  const p: DebtRow = {
    id: 'd',
    role: 'customer',
    kind: 'company',
    name: 'ОсОО Транс',
    phone: '0555123456',
    inn: '01234567890123',
    comment: '',
    credit_limit_tyiyn: null,
    due_days: 10,
    active: true,
    balance_tyiyn: 1_500_000,
    overdue_tyiyn: 500_000,
    due_date: '2026-10-20',
    overdue_days: 11,
    last_payment_at: null,
  }
  it('ИНН и телефон — текстом, долг и просрочка — числами, итог — по столбцам', () => {
    const s = debtSheet('Должны нам', [p, { ...p, id: 'e', balance_tyiyn: 100, overdue_tyiyn: 0 }])
    const row = cells(s.rows[0])
    expect(row.slice(0, 7)).toEqual(['ОсОО Транс', 'Юрлицо', '01234567890123', '0555123456', { money: 1_500_000 }, { money: 500_000 }, { int: 11 }])
    expect(cells(s.rows[1])[5]).toBeNull()
    expect(s.rows[2]).toEqual({ cells: ['Итого', null, null, null, { money: 1_500_100 }, { money: 500_000 }], bold: true })
  })
})

describe('Акт сверки', () => {
  const act: Reconciliation = {
    party_id: 'p',
    name: 'ОсОО Транс',
    kind: 'company',
    role: 'customer',
    inn: '0123',
    phone: '',
    from: '2026-10-01',
    to: '2026-10-31',
    opening_tyiyn: 10_000,
    rows: [{ date: '2026-10-05', document: 'Чек № 12', comment: '', debit_tyiyn: 50_000, credit_tyiyn: 0, doc_type: 'sale', doc_id: 's' }],
    debit_total_tyiyn: 50_000,
    credit_total_tyiyn: 0,
    closing_tyiyn: 60_000,
  } as Reconciliation
  it('сальдо, строки, обороты и сальдо на конец — числами', () => {
    const s = reconciliationSheet(act, 'Автодом')
    expect(labels(s.columns)).toEqual(['Дата', 'Документ', 'Комментарий', 'Дебет', 'Кредит'])
    expect(s.rows.map(cells)).toEqual([
      [null, 'Сальдо на 01.10.2026', '', { money: 10_000 }, { money: 0 }],
      [{ date: '2026-10-05' }, 'Чек № 12', '', { money: 50_000 }, { money: 0 }],
      [null, 'Обороты за период', '', { money: 50_000 }, { money: 0 }],
      [null, 'Сальдо на 31.10.2026', '', { money: 60_000 }, { money: 0 }],
    ])
    expect(s.title?.[0]).toContain('ОсОО Транс')
  })
})

describe('Отчёт смены', () => {
  const base = {
    shift: { id: 's', number: 7, cashier_name: 'Айгуль', account_name: 'Касса', opened_at: '2026-10-31T03:00:00Z', closed_at: null },
    owner_view: false,
    cash: { opening_expected_tyiyn: 0, opening_tyiyn: 0, income: [], income_tyiyn: 0, outgo: [], outgo_tyiyn: 0, expected_tyiyn: 0, counted_tyiyn: null, diff_tyiyn: null, handover_tyiyn: null, handover_to: null, left_tyiyn: null },
    revenue: { sales_count: 1, sales_tyiyn: 100_000, returns_count: 0, returns_tyiyn: 0, net_tyiyn: 100_000, methods: [{ method: 'cash', amount_tyiyn: 100_000 }], rest_tyiyn: 0, bank_fee_tyiyn: 0 },
    compare: null,
    hours: [],
    sales: [
      { id: 'a', number: 1, kind: 'sale', sale_type: 'takeaway', created_at: '2026-10-31T04:00:00Z', cashier_name: 'Айгуль', user_name: 'admin', party_name: null, total_tyiyn: 100_000, reversal_of: null, reversal_of_number: null, payments: [] },
    ],
    sales_totals: { count: 1, sales_tyiyn: 100_000, paid_tyiyn: 100_000, bonus_tyiyn: 0, debt_tyiyn: 0 },
    returns_totals: { count: 0, total_tyiyn: 0, methods: [] },
    receipts: [],
    money: [],
    money_totals: [],
    debts: [],
    payouts: [],
    payouts_tyiyn: 0,
    cancelled: [],
    cancelled_tyiyn: 0,
  } as unknown as ShiftReport
  const flat = (sheets: Sheet[]) => sheets.flatMap((s) => s.rows.map(cells).flat())

  it('все вкладки — листами одного файла', () => {
    const s = shiftReportSheets(base)
    expect(s.map((x) => x.name)).toEqual(['Обзор', 'Продажи', 'Возвраты', 'Приходы', 'Деньги', 'Долги', 'Выплаты', 'Отменённое'])
    expect(cells(s[1].rows[0])).toContainEqual({ money: 100_000 })
  })

  it('администратору (сервер не прислал прибыль) — без себестоимости и прибыли', () => {
    const texts = flat(shiftReportSheets(base)).filter((c): c is string => typeof c === 'string')
    expect(texts.some((t) => /прибыль|себестоимость/i.test(t))).toBe(false)
    const owner = { ...base, owner_view: true, revenue: { ...base.revenue, cost_tyiyn: 60_000, gross_tyiyn: 40_000, net_profit_tyiyn: 30_000, profit: [] } }
    const ownerTexts = flat(shiftReportSheets(owner)).filter((c): c is string => typeof c === 'string')
    expect(ownerTexts).toContain('Валовая прибыль')
    expect(ownerTexts).toContain('Чистая прибыль (без оклада)')
  })

  it('количество строки по виду: розлив — литры, вес — кг, канистры, штуки', () => {
    expect(kindQty('pour', 1500)).toEqual({ qty: { qty: 1500, scale: 3 }, unit: 'л' })
    expect(kindQty('weight', 2350)).toEqual({ qty: { qty: 2350, scale: 3 }, unit: 'кг' })
    expect(kindQty('length', 1250)).toEqual({ qty: { qty: 1250, scale: 3 }, unit: 'м' })
    expect(kindQty('container', 2)).toEqual({ qty: { int: 2 }, unit: 'кан.' })
    expect(kindQty('piece', 3)).toEqual({ qty: { int: 3 }, unit: 'шт' })
  })
})

describe('Подпись отбора', () => {
  it('название, отбор без пустых частей и время выгрузки', () => {
    const t = exportTitle('Приход', ['период 01.10.2026–31.10.2026', false, '', 'с фото'], new Date('2026-10-31T09:00:00Z'))
    expect(t[0]).toBe('Приход')
    expect(t[1]).toBe('Отбор: период 01.10.2026–31.10.2026; с фото')
    expect(t[2]).toMatch(/^Выгружено: 31\.10\.2026/)
    expect(exportTitle('Товары', [])).toHaveLength(2)
  })

  it('граммы — килограммами с тремя знаками', () => {
    expect(xl.units(2350, 'g')).toEqual({ qty: 2350, scale: 3 })
  })
})

describe('Мелкие таблицы (задача 003, хвост Н13)', () => {
  it('частые товары клиента: литры с тремя знаками, чеки числом', () => {
    const r = { product_id: 'p', name: 'Масло', unit: 'ml' as const, container_ml: 4000, category_id: 'c', qty: 12_500, amount_tyiyn: 1, checks: 3 }
    expect(labels(FREQUENT_COLS)).toEqual(['Товар', 'Сколько', 'Ед.', 'Чеков'])
    expect(FREQUENT_COLS.map((c) => c.value(r))).toEqual(['Масло', { qty: 12_500, scale: 3 }, 'л', { int: 3 }])
  })

  it('платежи по накладной: на накладную и весь платёж — деньгами, сторно — отметкой', () => {
    const p = { ledger_id: 'l', at: '2026-10-30T05:00:00Z', user_name: 'Владелец', kind: 'payment' as const, source: 'Касса', amount_tyiyn: 50_000, payment_tyiyn: 80_000, reversed: true, comment: 'часть' }
    expect(labels(INVOICE_PAYMENT_COLS)).toEqual(['Когда', 'Кто', 'На накладную', 'Весь платёж', 'Откуда', 'Комментарий', 'Отметка'])
    const v = INVOICE_PAYMENT_COLS.map((c) => c.value(p))
    expect(v.slice(1)).toEqual(['Владелец', { money: 50_000 }, { money: 80_000 }, 'Касса', 'часть', 'сторно'])
    expect(INVOICE_PAYMENT_COLS[4].value({ ...p, kind: 'defect' })).toBe('зачёт брака (скидка)')
  })
})
