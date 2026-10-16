import { describe, expect, it } from 'vitest'
import { accountNote, accountTotals, accountType, bankChoices, defaultBankId, enoughOn, isTill } from './accounts'
import type { CashAccount } from './types'

const acc = (id: string, kind: CashAccount['kind'], balance: number, extra: Partial<CashAccount> = {}): CashAccount => ({
  id,
  name: id,
  kind,
  owner_only: false,
  is_default: false,
  allow_negative: false,
  active: true,
  balance_tyiyn: balance,
  ...extra,
})

const list = [
  acc('Касса', 'register', 50_000, { is_default: true }),
  acc('Сейф-1', 'safe', 100_000),
  acc('СВИНЕЦ', 'other', 0),
  acc('Оптима', 'bank', 30_000),
  acc('Счёт', 'bank', 20_000, { is_default: true }),
  acc('МБИЗНЕС', 'bank', 0, { active: false }),
]

describe('обзор счетов', () => {
  it('итог: наличные, безнал и всего — только действующие', () => {
    expect(accountTotals(list)).toEqual({ cash: 150_000, bank: 50_000, total: 200_000 })
  })

  it('тип и пометка словами', () => {
    expect(accountType(list[0])).toBe('Наличный')
    expect(accountType(list[3])).toBe('Безналичный')
    expect(accountNote(list[0])).toBe('касса смены')
    expect(accountNote(list[4])).toBe('основной')
    expect(accountNote(list[1])).toBe('сейф')
    expect(isTill(list[0])).toBe(true)
    expect(isTill(acc('x', 'register', 0))).toBe(false)
  })

  it('банк для карты: действующие, основной первым', () => {
    expect(bankChoices(list).map((a) => a.id)).toEqual(['Счёт', 'Оптима'])
    expect(defaultBankId(list)).toBe('Счёт')
    expect(defaultBankId([])).toBe('')
  })

  it('хватит ли денег: с разрешённым минусом — всегда', () => {
    expect(enoughOn(list[0], 50_000)).toBe(true)
    expect(enoughOn(list[0], 50_001)).toBe(false)
    expect(enoughOn({ balance_tyiyn: 0, allow_negative: true }, 1_000_000)).toBe(true)
    expect(enoughOn(undefined, 1)).toBe(true)
  })
})
