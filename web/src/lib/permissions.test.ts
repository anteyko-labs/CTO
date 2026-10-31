import { describe, expect, it } from 'vitest'
import { can, cashierDenied, CASHIER_PERMISSIONS, DEFAULT_PERMISSIONS, PERMISSION_GROUPS, PERMISSIONS, rememberShiftCashier } from './permissions'

describe('права администратора', () => {
  it('владелец может всё, администратор — только выданное', () => {
    expect(can({ role: 'owner' }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin' }, 'oil_transfer')).toBe(false)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'expenses')).toBe(false)
  })

  it('по умолчанию — всё, кроме скидки, перелива, аванса и старых долгов; приёмка — по умолчанию (SPEC-30 §5, ADR-122, ADR-133, ADR-134)', () => {
    expect(PERMISSIONS).toHaveLength(19)
    expect(new Set(PERMISSIONS.map((p) => p.key)).size).toBe(19)
    expect(DEFAULT_PERMISSIONS).toHaveLength(15)
    expect(DEFAULT_PERMISSIONS).not.toContain('discount')
    expect(DEFAULT_PERMISSIONS).not.toContain('oil_transfer')
    expect(DEFAULT_PERMISSIONS).not.toContain('payroll_advance')
    expect(DEFAULT_PERMISSIONS).not.toContain('old_debts')
    expect(DEFAULT_PERMISSIONS).toContain('acceptance')
    expect(PERMISSIONS.every((p) => PERMISSION_GROUPS.includes(p.group))).toBe(true)
  })
})

describe('права кассира без входа (задача 003 Н10)', () => {
  const admin = { role: 'admin' as const, permissions: ['discount', 'sale_debt', 'expenses', 'receipts'] as const }

  it('снимаются только права кассы и денег', () => {
    expect(CASHIER_PERMISSIONS.every((p) => p.group === 'Касса' || p.group === 'Деньги')).toBe(true)
    expect(CASHIER_PERMISSIONS.map((p) => p.key)).toContain('discount')
    expect(CASHIER_PERMISSIONS.map((p) => p.key)).not.toContain('receipts')
  })

  it('пересечение прав входа и кассира открытой смены; владельца не ограничивает', () => {
    const nurlan = { name: 'Нурлан', denied: ['discount'] }
    const user = { ...admin, permissions: [...admin.permissions] }
    expect(can(user, 'discount', nurlan)).toBe(false)
    expect(cashierDenied(user, 'discount', nurlan)).toBe('Нурлан')
    expect(can(user, 'sale_debt', nurlan)).toBe(true)
    // Ограничение кассира не добавляет того, чего нет у входа.
    expect(can(user, 'oil_transfer', null)).toBe(false)
    expect(can({ role: 'owner' }, 'discount', nurlan)).toBe(true)
    expect(cashierDenied({ role: 'owner' }, 'discount', nurlan)).toBeNull()
  })

  it('смена с ограничением запоминается, закрытая или без ограничений — снимает его', () => {
    const user = { ...admin, permissions: [...admin.permissions] }
    rememberShiftCashier({ cashier_name: 'Нурлан', cashier_denied: ['expenses'], closed_at: null })
    expect(can(user, 'expenses')).toBe(false)
    rememberShiftCashier({ cashier_name: 'Нурлан', cashier_denied: ['expenses'], closed_at: '2026-10-31T10:00:00Z' })
    expect(can(user, 'expenses')).toBe(true)
    rememberShiftCashier({ cashier_name: 'Нурлан', cashier_denied: ['expenses'] })
    expect(can(user, 'expenses')).toBe(false)
    rememberShiftCashier({ cashier_name: 'Айбек', cashier_denied: [] })
    expect(can(user, 'expenses')).toBe(true)
    rememberShiftCashier(null)
    expect(can(user, 'expenses')).toBe(true)
  })
})
