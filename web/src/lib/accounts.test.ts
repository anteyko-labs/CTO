import { describe, expect, it } from 'vitest'
import { accountLabel, accountNote, accountTotals, accountType, bankChoices, defaultBankId, enoughOn, handoverChoices, isDefaultFor, isTill, payAccounts } from './accounts'
import { formatSom } from './format'
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
  acc('Оптима', 'bank', 30_000, { default_card: true }),
  acc('Счёт', 'bank', 20_000, { default_transfer: true }),
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
    expect(accountNote(list[3])).toBe('по умолчанию для карты')
    expect(accountNote(list[4])).toBe('по умолчанию для перевода')
    expect(accountNote(acc('Б', 'bank', 0, { default_card: true, default_transfer: true }))).toBe('по умолчанию для карты и перевода')
    expect(accountNote(acc('Б', 'bank', 0))).toBe('')
    expect(accountNote(list[1])).toBe('сейф')
    expect(isTill(list[0])).toBe(true)
    expect(isTill(acc('x', 'register', 0))).toBe(false)
  })

  it('банк для карты и перевода: действующие, счёт по умолчанию для способа — первым', () => {
    expect(bankChoices(list, 'card').map((a) => a.id)).toEqual(['Оптима', 'Счёт'])
    expect(bankChoices(list, 'transfer').map((a) => a.id)).toEqual(['Счёт', 'Оптима'])
    expect(defaultBankId(list, 'card')).toBe('Оптима')
    expect(defaultBankId(list, 'transfer')).toBe('Счёт')
    expect(defaultBankId([], 'card')).toBe('')
    expect(isDefaultFor(list[3], 'card')).toBe(true)
    expect(isDefaultFor(list[3], 'transfer')).toBe(false)
  })

  it('снимок кассы без сети: отметки по умолчанию у банков', () => {
    const snap = [
      { id: 'a', name: 'Счёт', kind: 'bank' as const },
      { id: 'b', name: 'Карта владельца', kind: 'bank' as const, default_transfer: true },
      { id: 'c', name: 'Оптима', kind: 'bank' as const, default_card: true },
    ]
    expect(defaultBankId(snap, 'card')).toBe('c')
    expect(defaultBankId(snap, 'transfer')).toBe('b')
  })

  it('хватит ли денег: с разрешённым минусом — всегда', () => {
    expect(enoughOn(list[0], 50_000)).toBe(true)
    expect(enoughOn(list[0], 50_001)).toBe(false)
    expect(enoughOn({ balance_tyiyn: 0, allow_negative: true }, 1_000_000)).toBe(true)
    expect(enoughOn(undefined, 1)).toBe(true)
  })
})

describe('выбор счёта с остатком (SPEC-29 §12–14)', () => {
  it('подпись счёта — с остатком', () => {
    expect(accountLabel(acc('Касса', 'register', 1_230_000))).toBe(`Касса — ${formatSom(1_230_000)}`)
  })

  it('откуда платить: владелец — любой действующий, касса первой; администратор — только касса', () => {
    expect(payAccounts(list, true).map((a) => a.id)).toEqual(['Касса', 'Сейф-1', 'СВИНЕЦ', 'Оптима', 'Счёт'])
    expect(payAccounts(list, false).map((a) => a.id)).toEqual(['Касса'])
  })

  it('куда сдать кассу: не сама касса, первым — предложенный, иначе сейф', () => {
    expect(handoverChoices(list, 'Оптима')).toMatchObject({ pick: 'Оптима' })
    expect(handoverChoices(list, 'Касса').pick).toBe('Сейф-1')
    expect(handoverChoices(list, null).list.map((a) => a.id)).toEqual(['Сейф-1', 'СВИНЕЦ', 'Оптима', 'Счёт'])
  })
})
