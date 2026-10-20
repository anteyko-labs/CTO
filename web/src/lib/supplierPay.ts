// Оплата накладных поставщику: подписи статуса и источника платежа (SPEC-27, ADR-073).
import type { InvoicePayment, InvoiceStatus } from './types'

export const INVOICE_STATUS_LABELS: Record<InvoiceStatus, string> = {
  unpaid: 'не оплачена',
  partial: 'частично',
  paid: 'оплачена',
}

/** Откуда платили: касса, зачёт брака, возврат товара, правка владельца; «вне кассы» — только у прежних (ADR-079). */
export function paymentSource(p: Pick<InvoicePayment, 'kind' | 'source'>): string {
  if (p.kind === 'defect') return 'зачёт брака (скидка)'
  if (p.kind === 'return') return 'возврат товара поставщику'
  if (p.kind === 'adjust') return 'правка владельца'
  return p.source === 'outside' ? 'вне кассы (из своих)' : p.source
}

/**
 * Оплата накладной: по накладной и всего поставщику — отдельно, чтобы «было → стало» не смешивало
 * остаток накладной с общим долгом (проверка 20.10.2026). Общий долг ещё не загружен — null.
 */
export function invoicePayPreview(
  invoiceLeft: number,
  supplierDebt: number | null,
  value: number,
): { invoiceAfter: number; supplierAfter: number | null } {
  const paid = Math.max(0, value)
  return {
    invoiceAfter: Math.max(0, invoiceLeft - paid),
    supplierAfter: supplierDebt === null ? null : Math.max(0, supplierDebt - paid),
  }
}

/** Пояснение под оплатой долга: поставщику — наш долг и прибыль, клиенту — выручка в день продажи. */
export function repayNote(supplier: boolean): string {
  return supplier
    ? 'Оплата поставщику уменьшает наш долг ему. Это не расход: прибыль не меняется, себестоимость товара учтена при продаже.'
    : 'В прибыль оплата долга не идёт: выручка и прибыль по чеку учтены в день продажи.'
}
