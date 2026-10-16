// Оплата накладных поставщику: подписи статуса и источника платежа (SPEC-27, ADR-073).
import type { InvoicePayment, InvoiceStatus } from './types'

export const INVOICE_STATUS_LABELS: Record<InvoiceStatus, string> = {
  unpaid: 'не оплачена',
  partial: 'частично',
  paid: 'оплачена',
}

/** Откуда платили: касса, «вне кассы», зачёт брака или правка владельца. */
export function paymentSource(p: Pick<InvoicePayment, 'kind' | 'source'>): string {
  if (p.kind === 'defect') return 'зачёт брака (скидка)'
  if (p.kind === 'adjust') return 'правка владельца'
  return p.source === 'outside' ? 'вне кассы (из своих)' : p.source
}
