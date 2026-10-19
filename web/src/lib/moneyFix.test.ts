import { describe, expect, it } from 'vitest'
import {
  adjustDoc,
  adjustFields,
  expenseDoc,
  expenseFields,
  movementDoc,
  movementFields,
  repaymentDoc,
  repaymentFields,
  transferDoc,
  transferFields,
  type ExpenseDoc,
  type RepaymentDoc,
} from './moneyFix'

describe('исправление расхода', () => {
  const doc: ExpenseDoc = {
    article_id: 'a1',
    amount_tyiyn: 150_050,
    source: 'account',
    account_id: 'till',
    expense_date: '2026-10-18',
    comment: 'вода',
  }

  it('касса смены в форме — пустой выбор, другой счёт — как есть', () => {
    expect(expenseFields(doc, 'till')).toEqual({ article: 'a1', sum: '1500,50', account: '', date: '2026-10-18', comment: 'вода' })
    expect(expenseFields({ ...doc, account_id: 'safe' }, 'till').account).toBe('safe')
  })

  it('обратно — исправленный документ; сумма в тыйынах', () => {
    const f = { ...expenseFields(doc, 'till'), sum: '2000', article: 'a2' }
    expect(expenseDoc(doc, f)).toEqual({ ...doc, article_id: 'a2', amount_tyiyn: 200_000, account_id: null })
  })

  it('неверная сумма — ничего', () => {
    expect(expenseDoc(doc, { ...expenseFields(doc, null), sum: 'abc' })).toBeNull()
    expect(expenseDoc(doc, { ...expenseFields(doc, null), sum: '0' })).toBeNull()
  })
})

describe('исправление внесения, изъятия и перевода', () => {
  it('изъятие из кассы смены', () => {
    const f = movementFields({ account_id: 'till', kind: 'cash_out', amount_tyiyn: 50_000, comment: 'размен' }, 'till')
    expect(f).toEqual({ kind: 'cash_out', sum: '500', comment: 'размен', from: '', to: '' })
    expect(movementDoc({ ...f, kind: 'cash_in', sum: '600' }, 'till')).toEqual({ account_id: 'till', kind: 'cash_in', amount_tyiyn: 60_000, comment: 'размен' })
  })

  it('перевод: откуда, куда и сумма', () => {
    const f = transferFields({ from_account_id: 'till', to_account_id: 'safe', amount_tyiyn: 10_000, comment: '' })
    expect(f.kind).toBe('transfer')
    expect(transferDoc({ ...f, sum: '120' })).toEqual({ from_account_id: 'till', to_account_id: 'safe', amount_tyiyn: 12_000, comment: '' })
    expect(transferDoc({ ...f, to: '' })).toBeNull()
    expect(movementDoc(f, 'till')).toBeNull()
  })
})

describe('исправление оплаты долга и правки баланса', () => {
  const pay: RepaymentDoc = {
    type: 'repayment',
    party_id: 'p1',
    amount_tyiyn: 30_000,
    method: 'cash',
    account_id: 'till',
    receipt_id: 'r1',
    comment: '',
  }

  it('оплата: способ и счёт меняются, накладная остаётся', () => {
    const f = repaymentFields(pay)
    expect(f).toEqual({ sum: '300', method: 'cash', account: 'till', comment: '' })
    expect(repaymentDoc(pay, { ...f, method: 'card', account: 'bank', sum: '250' })).toEqual({
      ...pay,
      method: 'card',
      account_id: 'bank',
      amount_tyiyn: 25_000,
    })
  })

  it('правка баланса: знак — кнопкой, сумма — без знака', () => {
    const doc = { type: 'adjust' as const, party_id: 'p1', amount_tyiyn: -7_000, comment: 'простили' }
    const f = adjustFields(doc)
    expect(f).toEqual({ sign: -1, sum: '70', comment: 'простили' })
    expect(adjustDoc(doc, { ...f, sign: 1 })).toEqual({ ...doc, amount_tyiyn: 7_000 })
    expect(adjustDoc(doc, { ...f, sum: '' })).toBeNull()
  })
})
