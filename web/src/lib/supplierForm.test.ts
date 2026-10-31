import { describe, expect, it } from 'vitest'
import { dueDaysOk, EMPTY_SUPPLIER, innOk, supplierBody, supplierForm } from './supplierForm'

describe('форма поставщика (задача 003 Н10)', () => {
  it('ИНН — только цифры, срок оплаты — целые дни до 3650', () => {
    expect(innOk('')).toBe(true)
    expect(innOk('01234567890123')).toBe(true)
    expect(innOk('12ab')).toBe(false)
    expect(dueDaysOk('')).toBe(true)
    expect(dueDaysOk('14')).toBe(true)
    expect(dueDaysOk('3651')).toBe(false)
    expect(dueDaysOk('1.5')).toBe(false)
  })

  it('пустой срок у нового — не шлётся, при правке — снимается', () => {
    const f = { ...EMPTY_SUPPLIER, name: 'ОсОО Масла', inn: ' 0123 ' }
    expect(supplierBody(f, false)).not.toHaveProperty('due_days')
    expect(supplierBody(f, false)).not.toHaveProperty('clear_due_days')
    expect(supplierBody(f, true)).toMatchObject({ clear_due_days: true, inn: '0123' })
    expect(supplierBody({ ...f, due_days: '14' }, true)).toMatchObject({ due_days: 14 })
  })

  it('из поставщика без реквизитов — фирма с пустыми полями', () => {
    const f = supplierForm({ id: '1', name: 'Поставщик', phone: '', comment: '', active: true })
    expect(f).toMatchObject({ kind: 'company', inn: '', due_days: '' })
    expect(supplierForm({ id: '1', name: 'П', phone: '', comment: '', active: true, due_days: 7 }).due_days).toBe('7')
  })
})
