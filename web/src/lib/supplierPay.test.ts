import { describe, expect, it } from 'vitest'
import { INVOICE_STATUS_LABELS, invoicePayPreview, paymentSource, repayNote } from './supplierPay'

describe('оплата накладных', () => {
  it('статус и откуда платили', () => {
    expect(INVOICE_STATUS_LABELS.partial).toBe('частично')
    expect(paymentSource({ kind: 'payment', source: 'Касса' })).toBe('Касса')
    expect(paymentSource({ kind: 'payment', source: 'outside' })).toBe('вне кассы (из своих)')
    expect(paymentSource({ kind: 'defect', source: '' })).toBe('зачёт брака (скидка)')
    expect(paymentSource({ kind: 'adjust', source: '' })).toBe('правка владельца')
    expect(paymentSource({ kind: 'return', source: '' })).toBe('возврат товара поставщику')
  })

  it('по накладной и всего поставщику — отдельно', () => {
    // Накладная 3000 с из общего долга 10 360 с: платим 1000 с.
    expect(invoicePayPreview(300_000, 1_036_000, 100_000)).toEqual({ invoiceAfter: 200_000, supplierAfter: 936_000 })
    expect(invoicePayPreview(300_000, null, 300_000)).toEqual({ invoiceAfter: 0, supplierAfter: null })
    expect(invoicePayPreview(300_000, 200_000, 300_000)).toEqual({ invoiceAfter: 0, supplierAfter: 0 })
  })

  it('пояснение поставщику — о нашем долге, клиенту — о выручке', () => {
    expect(repayNote(true)).toContain('уменьшает наш долг')
    expect(repayNote(true)).not.toContain('выручка')
    expect(repayNote(false)).toContain('выручка')
  })
})
