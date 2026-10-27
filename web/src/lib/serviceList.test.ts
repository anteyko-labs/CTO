import { describe, expect, it } from 'vitest'
import { sameServices } from './serviceList'
import type { Service } from './types'

const svc = (id: string, price: number): Service => ({ id, name: `Услуга ${id}`, price_tyiyn: price, master_fee_tyiyn: 3_000, active: true }) as Service

describe('свежий список услуг (SPEC-36 §16)', () => {
  it('тот же список — окно не перерисовывается', () => {
    expect(sameServices([svc('a', 100), svc('b', 200)], [svc('a', 100), svc('b', 200)])).toBe(true)
  })
  it('новая услуга, другая цена или списка не было — берём свежий', () => {
    expect(sameServices([svc('a', 100)], [svc('a', 100), svc('b', 200)])).toBe(false)
    expect(sameServices([svc('a', 100)], [svc('a', 150)])).toBe(false)
    expect(sameServices(null, [svc('a', 100)])).toBe(false)
  })
})
