import { describe, expect, it } from 'vitest'
import { shiftHtml } from './shiftPrint'
import type { ShiftReport } from '../lib/shiftReport'

const plain = (html: string) => html.replace(/<[^>]+>/g, ' ').replace(/&nbsp;| /g, ' ').replace(/\s+/g, ' ')

function report(owner: boolean): ShiftReport {
  const revenue = {
    sales_count: 3,
    sales_tyiyn: 150_000,
    returns_count: 0,
    returns_tyiyn: 0,
    net_tyiyn: 150_000,
    methods: [
      { method: 'cash', amount_tyiyn: 100_000 },
      { method: 'card', amount_tyiyn: 50_000 },
    ],
    rest_tyiyn: 0,
    bank_fee_tyiyn: 250,
  }
  return {
    shift: {
      id: 's1',
      number: 7,
      business_date: '2026-10-27',
      account_id: 'till',
      account_name: 'Касса',
      cashier_name: 'Иман',
      opened_by: 'Админ',
      opened_at: '2026-10-27T03:00:00Z',
      opening_expected_tyiyn: 0,
      expected_tyiyn: 97_000,
      closed_at: '2026-10-27T12:00:00Z',
      closed_by: 'Админ',
      counted_tyiyn: 96_000,
      diff_tyiyn: -1_000,
      breakdown: [],
      cash_sales_tyiyn: 100_000,
      card_tyiyn: 50_000,
      transfer_tyiyn: 0,
      debt_tyiyn: 0,
      bank_fee_tyiyn: 250,
      bank_split: [{ account_id: 'b', name: 'Счёт', card_tyiyn: 50_000, transfer_tyiyn: 0, fee_tyiyn: 250, total_tyiyn: 50_000, debt_tyiyn: 0, returns_tyiyn: 0, landed_tyiyn: 49_750 }],
      handover_pending: false,
      to_safe_tyiyn: 90_000,
      left_tyiyn: 6_000,
    },
    owner_view: owner,
    cash: {
      opening_expected_tyiyn: 0,
      opening_tyiyn: 0,
      income: [{ key: 'cash_sales', label: 'Продажи наличными', amount_tyiyn: 100_000 }],
      income_tyiyn: 100_000,
      outgo: [
        { key: 'expenses', label: 'Расходы', amount_tyiyn: 3_000 },
        { key: 'payouts', label: 'Выплаты сотрудникам', amount_tyiyn: 0 },
      ],
      outgo_tyiyn: 3_000,
      expected_tyiyn: 97_000,
      counted_tyiyn: 96_000,
      diff_tyiyn: -1_000,
      handover_tyiyn: 90_000,
      handover_to: 'Сейф магазина',
      left_tyiyn: 6_000,
    },
    revenue: owner
      ? {
          ...revenue,
          cost_tyiyn: 3_000,
          gross_tyiyn: 147_000,
          net_profit_tyiyn: 140_750,
          profit: [
            { key: 'gross', label: 'Валовая прибыль', amount_tyiyn: 147_000 },
            { key: 'payroll', label: 'Начисления сотрудникам (без оклада)', amount_tyiyn: -2_000 },
            { key: 'bank_fee', label: 'Комиссия банка', amount_tyiyn: -250 },
            { key: 'expenses', label: 'Расходы', amount_tyiyn: -3_000 },
            { key: 'cash_diff', label: 'Недостача или излишек кассы', amount_tyiyn: -1_000 },
          ],
        }
      : revenue,
    compare: null,
    hours: [],
    sales: [],
    sales_totals: { count: 3, sales_tyiyn: 150_000, paid_tyiyn: 150_000, bonus_tyiyn: 0, debt_tyiyn: 0 },
    returns_totals: { count: 0, total_tyiyn: 0, methods: [] },
    receipts: [],
    money: [],
    money_totals: [],
    debts: [],
    payouts: [],
    payouts_tyiyn: 0,
    cancelled: [],
    cancelled_tyiyn: 0,
  }
}

describe('чек смены (SPEC-36 §10)', () => {
  it('шапка, выручка, безнал по счёту, деньги кассы и сдача', () => {
    const t = plain(shiftHtml(report(true)))
    expect(t).toContain('Чек смены № 7')
    expect(t).toContain('Кассир: Иман')
    expect(t).toContain('Админ')
    expect(t).toContain('Выручка 1 500,00 с')
    expect(t).toContain('Наличные 1 000,00 с')
    expect(t).toContain('Безнал по счетам')
    expect(t).toContain('«Счёт» 500,00 с')
    expect(t).toContain('Упало на счёт 497,50 с')
    expect(t).toContain('Должно быть 970,00 с')
    expect(t).toContain('Насчитали 960,00 с')
    expect(t).toContain('Недостача −10,00 с')
    expect(t).toContain('Сдано в «Сейф магазина» 900,00 с')
    expect(t).toContain('Осталось в кассе 60,00 с')
    // Нулевые виды расхода не печатаются.
    expect(t).not.toContain('Выплаты сотрудникам')
  })

  it('владельцу — валовая и чистая прибыль', () => {
    const t = plain(shiftHtml(report(true)))
    expect(t).toContain('Валовая прибыль 1 470,00 с')
    expect(t).toContain('Чистая прибыль 1 407,50 с')
    expect(t).toContain('без оклада')
  })

  it('администратору — без себестоимости и прибыли', () => {
    const t = plain(shiftHtml(report(false)))
    expect(t).not.toContain('Себестоимость')
    expect(t).not.toContain('прибыль')
    expect(t).toContain('Выручка 1 500,00 с')
  })

  it('авансы — отдельной строкой от выплат, у выплаты — за что (ADR-122)', () => {
    const r = report(false)
    r.cash.outgo = [
      { key: 'payouts', label: 'Выплаты сотрудникам', amount_tyiyn: 50_000 },
      { key: 'advances', label: 'Авансы', amount_tyiyn: 20_000 },
    ]
    const base = { created_at: '2026-10-27T05:00:00Z', comment: '', account_name: 'Касса', reversal: false, user_name: 'Админ' }
    r.payouts = [
      { ...base, id: 'p1', number: 1, employee_name: 'Иман', amount_tyiyn: 50_000, purpose: 'percent' },
      { ...base, id: 'p2', number: 2, employee_name: 'Нурлан', amount_tyiyn: 20_000, purpose: 'advance' },
    ]
    const t = plain(shiftHtml(r))
    expect(t).toMatch(/Выплаты сотрудникам 500,00 с Иман, проценты 500,00 с Авансы 200,00 с Нурлан 200,00 с/)
  })

  it('безнал печатается и без движений по счёту', () => {
    const r = report(false)
    r.shift.bank_split = [{ account_id: 'b', name: 'Счёт', card_tyiyn: 0, transfer_tyiyn: 0, fee_tyiyn: 0, total_tyiyn: 0, landed_tyiyn: 0 }]
    expect(plain(shiftHtml(r))).toContain('«Счёт» 0,00 с')
  })
})
