import { describe, expect, it } from 'vitest'
import { accountsText, bankParts, bankTotals, barHeights, changeText, debtKindText, debtTotals, filterSales, methodName, methodsIn, paymentsText, tabOf, type DebtRow, type SaleRow } from './shiftReport'

const sale = (number: number, methods: string[], over: Partial<SaleRow> = {}): SaleRow => ({
  id: String(number),
  number,
  kind: 'sale',
  sale_type: 'takeaway',
  created_at: '2026-10-20T03:07:00Z',
  cashier_name: 'Иман',
  user_name: 'Админ',
  party_name: null,
  total_tyiyn: 100,
  reversal_of: null,
  reversal_of_number: null,
  payments: methods.map((m) => ({ method: m, amount_tyiyn: 100, account_name: m === 'cash' ? 'Касса' : m === 'card' ? 'Счёт' : null })),
  ...over,
})

describe('отчёт смены (SPEC-30 §7)', () => {
  it('вкладка из адреса; неизвестная — обзор', () => {
    expect(tabOf('cancelled')).toBe('cancelled')
    expect(tabOf('что-то')).toBe('overview')
    expect(tabOf(null)).toBe('overview')
  })

  it('способы оплаты словами, «По счёту» — тоже; новый способ — как пришёл', () => {
    expect(methodName('invoice')).toBe('По счёту')
    expect(methodName('transfer')).toBe('QR владельцу')
    expect(methodName('crypto')).toBe('crypto')
    const mixed = sale(5, ['cash', 'card', 'cash'])
    expect(paymentsText(mixed.payments)).toBe('Наличные + Карта / QR банка')
    expect(accountsText(mixed.payments)).toBe('Касса, Счёт')
    expect(accountsText(sale(6, ['debt']).payments)).toBe('В долг')
  })

  it('поиск по номеру чека и отбор по способу оплаты', () => {
    const rows = [sale(101, ['cash']), sale(102, ['card']), sale(203, ['debt']), sale(7, ['cash'], { kind: 'return', reversal_of: '102', reversal_of_number: 102 })]
    expect(filterSales(rows, '10', '').map((r) => r.number)).toEqual([101, 102, 7])
    expect(filterSales(rows, '№ 203', '').map((r) => r.number)).toEqual([203])
    expect(filterSales(rows, '', 'cash').map((r) => r.number)).toEqual([101, 7])
    expect(methodsIn(rows)).toEqual(['cash', 'card', 'debt'])
  })

  it('изменение к прошлой смене со знаком', () => {
    // Пробел перед «%» — неразрывный, как во всех процентах (formatPercentBp).
    expect(changeText(1234).replace(/\s/g, ' ')).toBe('+12,34 %')
    expect(changeText(-8000).replace(/\s/g, ' ')).toBe('−80,00 %')
    expect(changeText(null)).toBe('')
  })

  it('столбики: лучший час — во всю высоту, маленький не пропадает', () => {
    expect(barHeights([0, 50, 100, 1])).toEqual([0, 50, 100, 2])
    expect(barHeights([0, 0])).toEqual([0, 0])
    expect(barHeights([])).toEqual([])
  })

  it('долги смены: продано в долг за вычетом возвратов, оплачено', () => {
    const d = (kind: DebtRow['kind'], amount: number, reversal = false): DebtRow => ({
      id: kind + amount,
      created_at: '2026-10-20T03:07:00Z',
      kind,
      reversal,
      party_id: 'p',
      party_name: 'Эрлан',
      sale_id: null,
      sale_number: null,
      amount_tyiyn: amount,
      account_name: null,
      user_name: 'Админ',
      comment: '',
    })
    expect(debtTotals([d('sale', 7600), d('return', -600), d('repayment', -770), d('adjust', 100)])).toEqual({ sold: 7000, repaid: 770, adjusted: 100 })
    expect(debtKindText(d('repayment', -770))).toBe('Оплата долга')
    expect(debtKindText(d('repayment', 770, true))).toBe('Сторно: оплата долга')
  })
})

describe('безнал смены по счетам (SPEC-36 §9)', () => {
  const b = { account_id: 'a', name: 'Счёт', card_tyiyn: 50_000, transfer_tyiyn: 0, invoice_tyiyn: 0, fee_tyiyn: 300, total_tyiyn: 60_000, debt_tyiyn: 30_000, returns_tyiyn: 20_000, landed_tyiyn: 59_700 }

  it('по чекам, оплатам долга и возвратам минусом; нулевых нет', () => {
    expect(bankParts(b)).toEqual([
      { key: 'card', label: 'Карта / QR банка', value: 50_000 },
      { key: 'debt', label: 'Оплаты долга', value: 30_000 },
      { key: 'returns', label: 'Возвраты', value: -20_000 },
    ])
  })

  it('весь безнал: итог, комиссия и «упало» = итог − комиссия', () => {
    expect(bankTotals([b, { ...b, account_id: 'b', total_tyiyn: 10_000, fee_tyiyn: 50 }])).toEqual({ total: 70_000, fee: 350, landed: 69_650 })
    expect(bankTotals([])).toEqual({ total: 0, fee: 0, landed: 0 })
  })
})
