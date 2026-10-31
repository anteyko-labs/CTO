import { describe, expect, it } from 'vitest'
import { can, DEFAULT_PERMISSIONS, OWNER_ONLY, PERMISSION_GROUPS, PERMISSION_PRESETS, PERMISSIONS, presetOf, toggleGroup } from './permissions'

describe('права администратора', () => {
  it('владелец может всё, администратор — только выданное', () => {
    expect(can({ role: 'owner' }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin' }, 'oil_transfer')).toBe(false)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'expenses')).toBe(false)
  })

  it('по умолчанию — всё, кроме скидки, перелива, аванса, старых долгов, цен, архива и заработка кассиров (SPEC-30 §5, задача 003 Н8)', () => {
    expect(PERMISSIONS).toHaveLength(32)
    expect(new Set(PERMISSIONS.map((p) => p.key)).size).toBe(32)
    expect(DEFAULT_PERMISSIONS).toHaveLength(25)
    for (const off of ['discount', 'oil_transfer', 'payroll_advance', 'old_debts', 'prices', 'products_archive', 'payroll_view_all'] as const) {
      expect(DEFAULT_PERMISSIONS).not.toContain(off)
    }
    expect(DEFAULT_PERMISSIONS).toContain('acceptance')
    expect(PERMISSIONS.every((p) => PERMISSION_GROUPS.includes(p.group))).toBe(true)
  })

  it('разделы меню — отдельная группа, все по умолчанию включены', () => {
    const sections = PERMISSIONS.filter((p) => p.group === 'Разделы')
    expect(sections.map((p) => p.key)).toEqual([
      'view_sales',
      'view_debts',
      'view_payroll',
      'view_income',
      'view_accounts',
      'view_suppliers',
      'view_stock',
      'reconciliation',
    ])
    expect(sections.every((p) => p.default)).toBe(true)
    // У каждого права видно, что оно открывает.
    expect(PERMISSIONS.every((p) => (p.opens ?? '').length > 0)).toBe(true)
  })

  it('шаблоны состоят только из известных прав; «Старший» — всё, что можно выдать', () => {
    const keys = new Set(PERMISSIONS.map((p) => p.key))
    for (const preset of PERMISSION_PRESETS) {
      expect(preset.permissions.every((k) => keys.has(k))).toBe(true)
    }
    expect(PERMISSION_PRESETS.find((p) => p.key === 'default')?.permissions).toEqual(DEFAULT_PERMISSIONS)
    expect(PERMISSION_PRESETS.find((p) => p.key === 'senior')?.permissions).toHaveLength(PERMISSIONS.length)
    const till = PERMISSION_PRESETS.find((p) => p.key === 'till')?.permissions ?? []
    expect(till).not.toContain('cash_moves')
    expect(till).not.toContain('receipts')
    expect(OWNER_ONLY.length).toBeGreaterThan(0)
  })

  it('«Как у…» берёт только известные права; группа включается и выключается целиком', () => {
    expect(presetOf(['expenses', 'price_edit', 'sale_return'])).toEqual(['sale_return', 'expenses'])
    const money = PERMISSIONS.filter((p) => p.group === 'Деньги').map((p) => p.key)
    const off = toggleGroup(DEFAULT_PERMISSIONS, 'Деньги', false)
    expect(off.some((k) => money.includes(k))).toBe(false)
    expect(off).toContain('sale_return')
    const on = toggleGroup(off, 'Деньги', true)
    expect(money.every((k) => on.includes(k))).toBe(true)
    expect(on).toContain('payroll_view_all')
  })
})
