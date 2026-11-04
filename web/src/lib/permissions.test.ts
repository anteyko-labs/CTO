import { describe, expect, it } from 'vitest'
import { can, cashierDenied, CASHIER_PERMISSIONS, DEFAULT_PERMISSIONS, OWNER_ONLY, PERMISSION_GROUPS, PERMISSION_PRESETS, PERMISSIONS, presetOf, rememberShiftCashier, toggleGroup } from './permissions'

describe('права администратора', () => {
  it('владелец может всё, администратор — только выданное', () => {
    expect(can({ role: 'owner' }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin' }, 'oil_transfer')).toBe(false)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'oil_transfer')).toBe(true)
    expect(can({ role: 'admin', permissions: ['oil_transfer'] }, 'expenses')).toBe(false)
  })

  it('по умолчанию — всё, кроме скидки, перелива, аванса, старых долгов, «Наличные ↔ QR», цен, архива и заработка кассиров (SPEC-30 §5, задача 003 Н1 и Н8, ADR-149)', () => {
    expect(PERMISSIONS).toHaveLength(35)
    expect(new Set(PERMISSIONS.map((p) => p.key)).size).toBe(35)
    expect(DEFAULT_PERMISSIONS).toHaveLength(26)
    for (const off of ['discount', 'oil_transfer', 'payroll_advance', 'old_debts', 'till_pay_fix', 'prices', 'products_archive', 'payroll_view_all'] as const) {
      expect(DEFAULT_PERMISSIONS).not.toContain(off)
    }
    expect(DEFAULT_PERMISSIONS).toContain('acceptance')
    // Коды на кассе — любой кассир (задача 005 Н3): группа «Касса», по умолчанию включено.
    expect(PERMISSIONS.find((p) => p.key === 'till_barcode')).toMatchObject({ group: 'Касса', default: true })
    expect(DEFAULT_PERMISSIONS).toContain('till_barcode')
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

  it('личный вход сотрудника — по своим правам: снятое у кассира смены его не урезает (задача 005 Н1)', () => {
    const nurlan = { name: 'Нурлан', denied: ['discount'] }
    const personal = { ...admin, permissions: [...admin.permissions], employee_id: 'e-1' }
    expect(cashierDenied(personal, 'discount', nurlan)).toBeNull()
    expect(can(personal, 'discount', nurlan)).toBe(true)
    // Своих прав не добавляет: чего нет у входа — нет.
    expect(can(personal, 'oil_transfer', nurlan)).toBe(false)
    // Общий вход кассы (без сотрудника) — пересечение, как раньше.
    expect(can({ ...personal, employee_id: null }, 'discount', nurlan)).toBe(false)
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
