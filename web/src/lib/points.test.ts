import { describe, expect, it } from 'vitest'
import { effectivePoints, parsePointsRule, pointsChanged, pointsDraft, pointsInput, pointsRuleText } from './points'

describe('правило баллов в форме', () => {
  it('черновик из сохранённого и признак правки', () => {
    expect(pointsDraft(undefined, undefined)).toEqual({ mode: 'inherit', text: '' })
    expect(pointsDraft('percent', 150)).toEqual({ mode: 'percent', text: '1,5' })
    expect(pointsChanged({ mode: 'percent', text: '1,50' }, 'percent', 150)).toBe(false)
    expect(pointsChanged({ mode: 'percent', text: '2' }, 'percent', 150)).toBe(true)
    expect(pointsChanged({ mode: 'inherit', text: '' }, undefined, undefined)).toBe(false)
    expect(pointsChanged({ mode: 'none', text: '' }, 'inherit', 0)).toBe(true)
  })
})

describe('баллы товара и категории', () => {
  it('разбирает процент и баллы за единицу целыми', () => {
    expect(parsePointsRule('percent', '0,5')).toEqual({ mode: 'percent', value: 50 })
    expect(parsePointsRule('percent', '2')).toEqual({ mode: 'percent', value: 200 })
    expect(parsePointsRule('fixed', '10')).toEqual({ mode: 'fixed', value: 1000 })
    expect(parsePointsRule('fixed', '0,25')).toEqual({ mode: 'fixed', value: 25 })
    expect(parsePointsRule('none', 'что угодно')).toEqual({ mode: 'none', value: 0 })
    expect(typeof parsePointsRule('percent', '0')).toBe('string')
    expect(typeof parsePointsRule('percent', '101')).toBe('string')
    expect(typeof parsePointsRule('fixed', 'abc')).toBe('string')
  })

  it('показывает правило в поле и словами', () => {
    expect(pointsInput({ mode: 'percent', value: 50 })).toBe('0,5')
    expect(pointsInput({ mode: 'percent', value: 125 })).toBe('1,25')
    expect(pointsInput({ mode: 'percent', value: 200 })).toBe('2')
    expect(pointsInput({ mode: 'fixed', value: 1000 })).toBe('10')
    expect(pointsRuleText({ mode: 'percent', value: 50 })).toBe('0,5 % от оплаченного')
    expect(pointsRuleText({ mode: 'fixed', value: 1000 }, 'ml')).toBe('10 за литр')
    expect(pointsRuleText({ mode: 'fixed', value: 250 }, 'piece')).toBe('2,50 за штуку')
    expect(pointsRuleText({ mode: 'none', value: 0 })).toBe('не начислять')
  })

  it('товар → категория → общая ставка', () => {
    const fixed = { mode: 'fixed' as const, value: 500 }
    const pct = { mode: 'percent' as const, value: 200 }
    const inherit = { mode: 'inherit' as const, value: 0 }
    expect(effectivePoints(fixed, pct, 50)).toEqual({ rule: fixed, from: 'product' })
    expect(effectivePoints(inherit, pct, 50)).toEqual({ rule: pct, from: 'category' })
    expect(effectivePoints(inherit, inherit, 50)).toEqual({ rule: { mode: 'percent', value: 50 }, from: 'common' })
    expect(effectivePoints({ mode: 'none', value: 0 }, pct, 50).from).toBe('product')
  })
})
