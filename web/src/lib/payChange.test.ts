import { describe, expect, it } from 'vitest'
import {
  canFixPay,
  diffWords,
  fixItemsFor,
  formPayments,
  initialForm,
  matchingQr,
  remaining,
  swapPayments,
  swapTarget,
  transferLimit,
  transferWords,
  type PayChangeInfo,
} from './payChange'

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

describe('наличные ↔ QR (задача 003 Н1)', () => {
  const limits = {
    limit_tyiyn: 400_000,
    bank_limits: [
      { account_id: 'optima', name: 'Optima', card_sales_tyiyn: 300_000, card_returns_tyiyn: 0, moved_tyiyn: -100_000, limit_tyiyn: 200_000 },
    ],
  }

  it('предел по направлению: в QR — наличные смены, в наличные — по счёту', () => {
    expect(transferLimit(limits, 'to_bank', 'optima')).toBe(400_000)
    expect(transferLimit(limits, 'to_cash', 'optima')).toBe(200_000)
    expect(transferLimit(limits, 'to_cash', 'mbank')).toBe(0)
  })

  it('направление словами по знаку', () => {
    expect(transferWords(100_000, 'Optima')).toBe('из наличных в QR: Касса → «Optima»')
    expect(transferWords(-100_000, 'Optima')).toBe('из QR в наличные: «Optima» → Касса')
  })

  it('«⇄» по чеку: вся сумма способа на другой; смешанная, долг, QR банка — только «Подробно»', () => {
    const cash = { method: 'cash' as const, amount_tyiyn: 100_000, account_id: null, account_name: 'Касса', bank_qr: false }
    const card = { method: 'card' as const, amount_tyiyn: 50_000, account_id: 'optima', account_name: 'Optima', bank_qr: false }
    const bonus = { method: 'bonus' as const, amount_tyiyn: 5_000, account_id: null, account_name: null, bank_qr: false }
    expect(swapTarget([cash])).toBe('card')
    expect(swapTarget([cash, bonus])).toBe('card')
    expect(swapTarget([card])).toBe('cash')
    expect(swapTarget([{ ...card, bank_qr: true }])).toBeNull()
    expect(swapTarget([cash, card])).toBeNull()
    expect(swapTarget([{ ...cash, method: 'debt' }])).toBeNull()
    expect(swapPayments([cash, bonus], 'card', 'optima')).toEqual([{ method: 'card', amount_tyiyn: 100_000, account_id: 'optima' }])
    expect(swapPayments([card], 'cash', '')).toEqual([{ method: 'cash', amount_tyiyn: 50_000 }])
  })

  it('кнопка: владелец — при связи; администратор — с правом и в открытой смене', () => {
    const owner = { role: 'owner' as const, permissions: [] }
    const admin = { role: 'admin' as const, permissions: ['till_pay_fix' as const] }
    const plain = { role: 'admin' as const, permissions: [] }
    expect(canFixPay(owner, true, false)).toBe(true)
    expect(canFixPay(owner, false, true)).toBe(false)
    expect(canFixPay(admin, true, true)).toBe(true)
    expect(canFixPay(admin, true, false)).toBe(false)
    expect(canFixPay(plain, true, true)).toBe(false)
    expect(canFixPay(null, true, true)).toBe(false)
  })

  it('расшифровка строки движения денег: чеки по знаку, переносы по направлению с отменами', () => {
    const base = {
      created_at: '2026-10-31T08:00:00Z',
      user_name: 'Азиз',
      original_id: null,
      original_number: null,
      new_id: null,
      new_number: null,
      old_text: '',
      new_text: '',
      diff_before_tyiyn: null,
      diff_after_tyiyn: null,
      withhold_tyiyn: 0,
      comment: '',
    }
    const items = [
      { ...base, id: 'a', kind: 'sale' as const, reversal: false, cash_delta_tyiyn: -100_000 },
      { ...base, id: 'b', kind: 'sale' as const, reversal: false, cash_delta_tyiyn: 40_000 },
      { ...base, id: 'c', kind: 'shift' as const, reversal: false, cash_delta_tyiyn: -30_000, amount_tyiyn: 30_000 },
      { ...base, id: 'd', kind: 'shift' as const, reversal: false, cash_delta_tyiyn: 20_000, amount_tyiyn: -20_000 },
      { ...base, id: 'e', kind: 'shift' as const, reversal: true, cash_delta_tyiyn: 30_000, amount_tyiyn: -30_000 },
    ]
    const ids = (key: string) => fixItemsFor(key, items).map((i) => i.id)
    expect(ids('pay_fix_out')).toEqual(['a'])
    expect(ids('pay_fix_in')).toEqual(['b'])
    expect(ids('cash_to_bank')).toEqual(['c', 'e'])
    expect(ids('bank_to_cash')).toEqual(['d'])
    expect(ids('cash_sales')).toEqual([])
  })
})
