import { describe, expect, it } from 'vitest'
import { parseSom, somInput } from './format'
import { initialLines, payoutLines } from './payouts'
import type { PayoutRemains } from './types'

const remains: PayoutRemains = {
  employee_id: 'e1',
  full_name: 'Тимур',
  is_cashier: true,
  is_master: true,
  remains: { percent_tyiyn: 50_000, service_fee_tyiyn: 3_000, bonus_tyiyn: 0, salary_tyiyn: -1_000_000, withheld_tyiyn: 0 },
  balance_tyiyn: -947_000,
  advance_to: 'salary',
}

describe('окно «Выплата» по видам (ADR-122)', () => {
  it('владельцу — виды с остатком отмечены, аванс снят', () => {
    const lines = initialLines(remains, { owner: true, advance: true }, somInput)
    expect(lines.map((l) => [l.purpose, l.checked, l.sum])).toEqual([
      ['percent', true, somInput(50_000)],
      ['service_fee', true, somInput(3_000)],
      ['advance', false, ''],
    ])
  })

  it('администратору — только за услуги, аванс с правом', () => {
    expect(initialLines(remains, { owner: false, advance: false }, somInput).map((l) => l.purpose)).toEqual(['service_fee'])
    expect(initialLines(remains, { owner: false, advance: true }, somInput).map((l) => l.purpose)).toEqual(['service_fee', 'advance'])
  })

  it('строки уходят одной выплатой; больше остатка вида — ошибка, аванс — сверх', () => {
    const lines = initialLines(remains, { owner: true, advance: true }, somInput)
    lines[2] = { ...lines[2], checked: true, sum: '10000' }
    const ok = payoutLines(lines, parseSom)
    expect(ok.error).toBeNull()
    expect(ok.lines).toEqual([
      { purpose: 'percent', amount_tyiyn: 50_000 },
      { purpose: 'service_fee', amount_tyiyn: 3_000 },
      { purpose: 'advance', amount_tyiyn: 1_000_000 },
    ])
    expect(ok.total).toBe(1_053_000)
    lines[0] = { ...lines[0], sum: '600' }
    expect(payoutLines(lines, parseSom).error).toBe('Проценты: больше, чем к выдаче')
    expect(payoutLines(lines.map((l) => ({ ...l, checked: false })), parseSom).error).toBe('Отметьте, за что выплатить')
  })
})
