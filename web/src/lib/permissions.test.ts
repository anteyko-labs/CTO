import { describe, expect, it } from 'vitest'
import { can, DEFAULT_PERMISSIONS, PERMISSION_GROUPS, PERMISSIONS } from './permissions'

describe('права администратора', () => {
  it('владелец может всё, администратор — только выданное', () => {
    expect(can({ role: 'owner' }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin' }, 'oil_transfer')).toBe(false)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'expenses')).toBe(false)
  })

  it('по умолчанию — всё, кроме скидки и перелива (SPEC-30 §5)', () => {
    expect(PERMISSIONS).toHaveLength(16)
    expect(new Set(PERMISSIONS.map((p) => p.key)).size).toBe(16)
    expect(DEFAULT_PERMISSIONS).toHaveLength(14)
    expect(DEFAULT_PERMISSIONS).not.toContain('discount')
    expect(DEFAULT_PERMISSIONS).not.toContain('oil_transfer')
    expect(PERMISSIONS.every((p) => PERMISSION_GROUPS.includes(p.group))).toBe(true)
  })
})
