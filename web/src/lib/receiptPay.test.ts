import { describe, expect, it } from 'vitest'
import { OUTSIDE, defaultPaySource, notEnough, paidFromText, paidNow, partProblem, paySources } from './receiptPay'
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

  it('владелец выбирает сам из касс и счетов; «из своих» больше нет (ADR-079)', () => {
    const s = paySources(accounts, true)
    expect(s.map((x) => x.value)).toEqual(['till', 'bank', 'safe'])
    expect(s.some((x) => x.value === OUTSIDE)).toBe(false)
    expect(defaultPaySource(s, true)).toBe('')
  })

  it('не хватает денег на счёте', () => {
    const s = paySources(accounts, true)
    expect(notEnough(s, 'till', 60_000)).toBe(true)
    expect(notEnough(s, 'till', 50_000)).toBe(false)
  })

  it('счёт с разрешённым минусом — хватает всегда (ADR-076)', () => {
    const s = paySources([{ ...acc('opt', 'Оптима', 'bank', false, 0), allow_negative: true }], true)
    expect(notEnough(s, 'opt', 1_000_000)).toBe(false)
  })

  it('часть сейчас — больше нуля и меньше суммы (SPEC-29 §4)', () => {
    expect(paidNow('paid', 100_000, null)).toBe(100_000)
    expect(paidNow('part', 100_000, 30_000)).toBe(30_000)
    expect(paidNow('debt', 100_000, 30_000)).toBe(0)
    expect(partProblem(30_000, 100_000)).toBeNull()
    expect(partProblem(0, 100_000)).toContain('больше нуля')
    expect(partProblem(100_000, 100_000)).toContain('меньше суммы')
    expect(partProblem(null, 100_000)).toContain('числом')
  })

  it('в документе: прежние «из своих» читаются', () => {
    expect(paidFromText('Касса')).toBe('из «Касса»')
    expect(paidFromText(OUTSIDE)).toBe('из своих, мимо касс')
    expect(paidFromText(null)).toBe('—')
  })
})
