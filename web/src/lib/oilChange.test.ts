import { describe, expect, it } from 'vitest'
import { hasEngineOil, nextChangeProblem } from './oilChange'

describe('замена масла в кассе', () => {
  it('спрашиваем только при моторном масле не в подарок', () => {
    expect(hasEngineOil([{ product: { category_kind: 'oil' } }])).toBe(true)
    expect(hasEngineOil([{ product: { category_kind: 'filter' } }, { product: { category_kind: 'oil' }, gift: true }])).toBe(false)
    expect(hasEngineOil([])).toBe(false)
  })

  it('интервал из чека — в пределах, мили переводятся в км', () => {
    expect(nextChangeProblem('8000', '180', 'km')).toBeNull()
    expect(nextChangeProblem('3000', '90', 'mi')).toBeNull()
    expect(nextChangeProblem('300', '90', 'mi')).toContain('миль')
    expect(nextChangeProblem('400', '90', 'km')).toContain('500')
    expect(nextChangeProblem('8000', '0', 'km')).toContain('дней')
    expect(nextChangeProblem('', '30', 'km')).toContain('числами')
  })
})
