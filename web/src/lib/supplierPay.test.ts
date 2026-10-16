import { describe, expect, it } from 'vitest'
import { INVOICE_STATUS_LABELS, paymentSource } from './supplierPay'

describe('оплата накладных', () => {
  it('статус и откуда платили', () => {
    expect(INVOICE_STATUS_LABELS.partial).toBe('частично')
    expect(paymentSource({ kind: 'payment', source: 'Касса' })).toBe('Касса')
    expect(paymentSource({ kind: 'payment', source: 'outside' })).toBe('вне кассы (из своих)')
    expect(paymentSource({ kind: 'defect', source: '' })).toBe('зачёт брака (скидка)')
    expect(paymentSource({ kind: 'adjust', source: '' })).toBe('правка владельца')
  })
})
