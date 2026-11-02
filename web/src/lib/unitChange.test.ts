import { describe, expect, it } from 'vitest'
import { formatKg } from './format'
import { priceAfter, recount, stockText, unitPlan, unitText, zeroOnly, type UnitPlan } from './unitChange'

const plan = (p: UnitPlan | string): UnitPlan => {
  if (typeof p === 'string') throw new Error(p)
  return p
}

describe('единица и объём канистры', () => {
  it('литры → штуки: остаток делится на «1 шт = N мл», цена канистры → цена штуки', () => {
    const p = plan(unitPlan('ml', 4000, { unit: 'piece', containerMl: null, pieceMl: 1000, keep: 'count' }))
    expect(recount(12_000, p)).toBe(12)
    expect(recount(4_500, p)).toBeNull()
    expect(priceAfter(200_000, p)).toBe(50_000)
  })

  it('штуки → литры: штука — канистра по умолчанию', () => {
    const p = plan(unitPlan('piece', null, { unit: 'ml', containerMl: 4000, pieceMl: null, keep: 'count' }))
    expect(recount(3, p)).toBe(12_000)
    expect(priceAfter(200_000, p)).toBe(200_000)
    const half = plan(unitPlan('piece', null, { unit: 'ml', containerMl: 4000, pieceMl: 500, keep: 'count' }))
    expect(recount(30, half)).toBe(15_000)
    expect(priceAfter(20_000, half)).toBe(160_000)
  })

  it('другая канистра: канистр столько же или литров столько же', () => {
    const count = plan(unitPlan('ml', 4000, { unit: 'ml', containerMl: 5000, pieceMl: null, keep: 'count' }))
    expect(recount(12_000, count)).toBe(15_000)
    expect(recount(14_500, count)).toBe(18_125)
    expect(recount(14_501, count)).toBeNull()
    const volume = plan(unitPlan('ml', 4000, { unit: 'ml', containerMl: 5000, pieceMl: null, keep: 'volume' }))
    expect(recount(14_500, volume)).toBe(14_500)
  })

  it('килограммы — как на сервере: «1 шт = 1 кг», «1 л = 1 кг», только без остатка', () => {
    const toKg = plan(unitPlan('piece', null, { unit: 'g', containerMl: null, pieceMl: null, keep: 'count' }))
    expect(priceAfter(40_000, toKg)).toBe(40_000)
    const oilKg = plan(unitPlan('ml', 4000, { unit: 'g', containerMl: null, pieceMl: null, keep: 'count' }))
    expect(priceAfter(200_000, oilKg)).toBe(50_000)
    expect(zeroOnly('piece', 'g')).toBe(true)
    expect(zeroOnly('g', 'ml')).toBe(true)
    expect(zeroOnly('ml', 'piece')).toBe(false)
    expect(stockText(1250, 'g')).toBe(formatKg(1250))
  })

  it('отказы — словами', () => {
    expect(unitPlan('ml', 4000, { unit: 'ml', containerMl: 4000, pieceMl: null, keep: 'count' })).toMatch(/тот же/)
    expect(unitPlan('piece', null, { unit: 'piece', containerMl: null, pieceMl: null, keep: 'count' })).toMatch(/уже в штуках/)
    expect(unitPlan('ml', 4000, { unit: 'piece', containerMl: null, pieceMl: null, keep: 'count' })).toMatch(/Укажите/)
    expect(unitPlan('g', null, { unit: 'g', containerMl: null, pieceMl: null, keep: 'count' })).toMatch(/уже в килограммах/)
    expect(unitPlan('g', null, { unit: 'ml', containerMl: null, pieceMl: null, keep: 'count' })).toMatch(/Укажите/)
    expect(unitText('ml', 4000)).toBe('литры, канистра 4 л')
    expect(unitText('piece', null)).toBe('штуки')
  })
})
