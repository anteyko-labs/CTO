import { describe, expect, it } from 'vitest'
import { OUTSIDE, defaultPaySource, notEnough, paidFromText, paySources } from './receiptPay'
import type { CashAccount } from './types'

const acc = (id: string, name: string, kind: CashAccount['kind'], owner_only: boolean, balance: number): CashAccount =>
  ({ id, name, kind, owner_only, is_default: kind === 'register', balance_tyiyn: balance }) as CashAccount

const accounts = [acc('till', 'Касса', 'register', false, 50_000), acc('bank', 'Счёт', 'bank', false, 0), acc('safe', 'Сейф владельца', 'safe', true, 10)]

describe('откуда оплачена накладная', () => {
  it('администратору — только касса, она и выбрана', () => {
    const s = paySources(accounts, false)
    expect(s.map((x) => x.value)).toEqual(['till'])
    expect(defaultPaySource(s, false)).toBe('till')
  })

  it('владелец выбирает сам из всех касс и «из своих»', () => {
    const s = paySources(accounts, true)
    expect(s.map((x) => x.value)).toEqual(['till', 'bank', 'safe', OUTSIDE])
    expect(defaultPaySource(s, true)).toBe('')
  })

  it('не хватает денег — только у кассы, у «из своих» предела нет', () => {
    const s = paySources(accounts, true)
    expect(notEnough(s, 'till', 60_000)).toBe(true)
    expect(notEnough(s, 'till', 50_000)).toBe(false)
    expect(notEnough(s, OUTSIDE, 1_000_000)).toBe(false)
  })

  it('счёт с разрешённым минусом — хватает всегда (ADR-076)', () => {
    const s = paySources([{ ...acc('opt', 'Оптима', 'bank', false, 0), allow_negative: true }], true)
    expect(notEnough(s, 'opt', 1_000_000)).toBe(false)
  })

  it('в документе', () => {
    expect(paidFromText('Касса')).toBe('из «Касса»')
    expect(paidFromText(OUTSIDE)).toBe('из своих, мимо касс')
    expect(paidFromText(null)).toBe('—')
  })
})
