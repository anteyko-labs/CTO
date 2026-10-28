import { describe, expect, it } from 'vitest'
import { diffWords, formPayments, initialForm, matchingQr, remaining, type PayChangeInfo } from './payChange'

const info: PayChangeInfo = {
  sale_id: 's',
  number: 15,
  business_date: '2026-10-27',
  created_at: '2026-10-27T08:30:00Z',
  cashier_name: 'Айбек',
  total_tyiyn: 120_000,
  payments: [{ method: 'cash', amount_tyiyn: 100_000 }],
  fixed: [{ method: 'bonus', amount_tyiyn: 20_000 }],
  to_split_tyiyn: 100_000,
  party: null,
  accounts: [
    { id: 'mbank', name: 'Мбанк', default_card: false, default_transfer: true, default_invoice: false, fee_rate_bp: 0 },
    { id: 'optima', name: 'Optima', default_card: true, default_transfer: false, default_invoice: true, fee_rate_bp: 50 },
  ],
  qr: [],
  counted: null,
  diff_tyiyn: null,
  withheld_tyiyn: null,
  open_shift_number: 7,
  repaid_tyiyn: 0,
}

describe('изменить оплату', () => {
  it('поля — по оплате сейчас, счета — по умолчанию', () => {
    const f = initialForm(info)
    expect(f.cash).toBe('1000')
    expect(f.card).toBe('')
    expect(f.cardAccount).toBe('optima')
    expect(f.transferAccount).toBe('mbank')
  })

  it('оплаты запроса и остаток распределить', () => {
    const f = { ...initialForm(info), cash: '400', card: '600', qr: 'q1' }
    const lines = formPayments(f)
    expect(lines).toEqual([
      { method: 'cash', amount_tyiyn: 40_000 },
      { method: 'card', amount_tyiyn: 60_000, account_id: 'optima', bank_qr_id: 'q1' },
    ])
    expect(remaining(info.to_split_tyiyn, lines)).toBe(0)
    expect(remaining(info.to_split_tyiyn, formPayments({ ...f, card: '500' }))).toBe(10_000)
  })

  it('неверная сумма — нет оплат, нули не входят', () => {
    expect(formPayments({ ...initialForm(info), cash: 'abc' })).toBeNull()
    expect(formPayments({ ...initialForm(info), cash: '-5' })).toBeNull()
    expect(formPayments({ ...initialForm(info), cash: '0' })).toEqual([])
    expect(remaining(100, null)).toBeNull()
  })

  it('QR без чека — той же суммы, что карта', () => {
    const qr = [
      { id: 'a', amount_tyiyn: 100_000, paid_at: null },
      { id: 'b', amount_tyiyn: 50_000, paid_at: null },
    ]
    expect(matchingQr(qr, 100_000).map((q) => q.id)).toEqual(['a'])
    expect(matchingQr(qr, 0)).toEqual([])
  })

  it('расхождение словами', () => {
    const som = (v: number) => `${v / 100} с`
    expect(diffWords(-100_000, som)).toBe('недостача 1000 с')
    expect(diffWords(70_000, som)).toBe('излишек 700 с')
    expect(diffWords(0, som)).toBe('без расхождения')
  })
})
