import { describe, expect, it } from 'vitest'
import { can } from './permissions'

describe('права сверх роли', () => {
  it('владелец может всё, администратор — только выданное', () => {
    expect(can({ role: 'owner' }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin' }, 'oil_transfer')).toBe(false)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'oil_transfer')).toBe(true)
  })
})
