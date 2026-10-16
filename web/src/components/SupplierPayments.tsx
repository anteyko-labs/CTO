// Оплата накладных поставщику: статус и история платежей — кто, когда, сколько, откуда (SPEC-27, ADR-073).
import { Link } from 'react-router-dom'
import { formatDateTime, formatSom } from '../lib/format'
import { INVOICE_STATUS_LABELS, paymentSource } from '../lib/supplierPay'
import type { InvoicePayment, InvoiceStatus, SupplierPayment } from '../lib/types'
import { Pager, usePager } from './Pager'
import { Badge, Table } from './ui'

export function InvoiceStatusBadge({ status }: { status: InvoiceStatus }) {
  return <Badge tone={status === 'paid' ? 'green' : status === 'partial' ? 'amber' : 'rose'}>{INVOICE_STATUS_LABELS[status]}</Badge>
}

/** Платежи по одной накладной: сколько из каждого легло на неё. */
export function InvoicePayments({ payments }: { payments: InvoicePayment[] }) {
  if (payments.length === 0) return <div className="text-sm text-slate-500">Оплат по накладной пока не было.</div>
  return (
    <Table head={['Когда', 'Кто', 'На накладную', 'Весь платёж', 'Откуда']}>
      {payments.map((p) => (
        <tr key={p.ledger_id} className={p.reversed ? 'text-slate-400 line-through' : ''}>
          <td className="whitespace-nowrap px-2 py-2">{formatDateTime(p.at)}</td>
          <td className="px-2 py-2">{p.user_name}</td>
          <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(p.amount_tyiyn)}</td>
          <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(p.payment_tyiyn)}</td>
          <td className="px-2 py-2">
            {paymentSource(p)}
            {p.reversed && (
              <>
                {' '}
                <Badge tone="rose">сторно</Badge>
              </>
            )}
            {p.comment && <div className="text-xs text-slate-500">{p.comment}</div>}
          </td>
        </tr>
      ))}
    </Table>
  )
}

/** Все платежи и зачёты поставщику: на какие накладные легли. */
export function SupplierPaymentHistory({ payments }: { payments: SupplierPayment[] }) {
  const pager = usePager(payments, 'supplier.payments')
  if (payments.length === 0) return <div className="text-sm text-slate-500">Платежей поставщику не было.</div>
  return (
    <>
      <Table head={['Когда', 'Кто', 'Сумма', 'Откуда', 'Закрыло']}>
        {pager.shown.map((p) => {
          const applied = p.applied.filter((a) => (p.reversed ? a.gross_tyiyn : a.amount_tyiyn) > 0)
          return (
            <tr key={p.ledger_id} className={p.reversed ? 'text-slate-400' : ''}>
              <td className="whitespace-nowrap px-2 py-2">{formatDateTime(p.at)}</td>
              <td className="px-2 py-2">{p.user_name}</td>
              <td className={`whitespace-nowrap px-2 py-2 text-right tabular-nums ${p.reversed ? 'line-through' : ''}`}>{formatSom(p.amount_tyiyn)}</td>
              <td className="px-2 py-2">
                {paymentSource(p)}
                {p.reversed && (
                  <>
                    {' '}
                    <Badge tone="rose">сторно</Badge>
                </>
              )}
              {p.comment && <div className="text-xs text-slate-500">{p.comment}</div>}
            </td>
            <td className="px-2 py-2">
              {applied.length === 0
                ? p.reversed
                  ? '—'
                  : 'аванс'
                : applied.map((a) => (
                    <div key={a.ledger_id} className="whitespace-nowrap">
                      {a.receipt_id ? (
                        <Link className="text-sky-700 underline" to={`/receipts/${a.receipt_id}`}>
                          № {a.number}
                        </Link>
                      ) : (
                        'старый долг'
                      )}
                      {' — '}
                      {formatSom(p.reversed ? a.gross_tyiyn : a.amount_tyiyn)}
                    </div>
                  ))}
            </td>
          </tr>
        )
      })}
    </Table>
    <Pager page={pager.page} size={pager.size} total={pager.total} onPage={pager.setPage} onSize={pager.setSize} label="Страницы платежей" />
    </>
  )
}
