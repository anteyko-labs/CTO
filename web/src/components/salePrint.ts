// Печать товарного чека через диалог печати браузера (HTML-шаблон, лента 58–80 мм).
import { formatDateTime, formatKg, formatLiters, formatSomExact } from '../lib/format'
import { paymentLabel } from '../lib/battery'
import { printInFrame } from '../lib/printFrame'
import type { Sale, SaleLine } from '../lib/types'
import { branchLabel } from '../lib/branch'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

export function lineQtyText(l: Pick<SaleLine, 'kind' | 'qty' | 'container_ml'>): string {
  switch (l.kind) {
    case 'pour':
      return formatLiters(l.qty)
    case 'container':
      return `${l.qty} кан.${l.container_ml ? ` × ${formatLiters(l.container_ml)}` : ''}`
    case 'weight':
      return formatKg(l.qty)
    case 'service':
      return `${l.qty} усл.`
    default:
      return `${l.qty} шт`
  }
}

export function saleHtml(sale: Sale, change: number | null): string {
  const title = sale.kind === 'return' ? (sale.no_receipt ? 'Возврат без чека' : 'Возврат') : 'Товарный чек'
  // Удержание за замену (ADR-104) — отдельной строкой после возвращённого.
  const held = sale.lines
    .filter((l) => l.withheld_for != null)
    .map((l) => `<tr><td>Удержано за замену</td><td class="r">${esc(formatSomExact(l.amount_tyiyn))}</td></tr>`)
    .join('')
  const rows = sale.lines
    .filter((l) => l.withheld_for == null)
    .map(
      (l) => `<tr><td colspan="2">${esc(l.name)}</td></tr>
      <tr><td class="muted">${esc(lineQtyText(l))} × ${esc(formatSomExact(l.unit_price_tyiyn))}${l.kind === 'pour' ? '/л' : l.kind === 'weight' ? '/кг' : ''}</td>
      <td class="r">${esc(formatSomExact(l.amount_tyiyn))}</td></tr>`,
    )
    .join('') + held
  const pays = sale.payments
    .map((p) => `<tr><td>${esc(paymentLabel(p))}</td><td class="r">${esc(formatSomExact(p.amount_tyiyn))}</td></tr>`)
    .join('')
  const changeRow = change && change > 0 ? `<tr><td>Сдача</td><td class="r">${esc(formatSomExact(change))}</td></tr>` : ''
  const master = sale.master_name ? `<div>Мастер: ${esc(sale.master_name)}</div>` : ''
  return `<!doctype html><html><head><meta charset="utf-8"><title>Чек ${sale.number}</title><style>
    @page { size: 80mm auto; margin: 3mm; }
    body { font-family: Arial, sans-serif; font-size: 10pt; margin: 0; width: 72mm; }
    h1 { font-size: 12pt; text-align: center; margin: 0 0 2mm; }
    table { width: 100%; border-collapse: collapse; }
    td { padding: 0.5mm 0; vertical-align: top; }
    .r { text-align: right; white-space: nowrap; }
    .muted { color: #444; }
    .total td { font-weight: bold; font-size: 12pt; border-top: 1px dashed #000; padding-top: 1mm; }
    .center { text-align: center; margin-top: 3mm; }
  </style></head><body>
    <h1>Avtodom</h1>
    ${branchLabel() ? `<div class="center" style="margin:0 0 2mm">${esc(branchLabel() ?? '')}</div>` : ''}
    <div>${title} № ${sale.number}</div>
    <div>${esc(saleWhen(sale))}</div>
    <div>Кассир: ${esc(sale.cashier_name)}</div>${master}
    ${sale.delivery_address ? `<div>Доставка: ${esc(sale.delivery_address)}</div>` : ''}
    ${sale.kind === 'return' && sale.comment ? `<div>Причина: ${esc(sale.comment)}</div>` : ''}
    <table>${rows}<tr class="total"><td>Итого</td><td class="r">${esc(formatSomExact(sale.total_tyiyn))}</td></tr>${pays}${changeRow}</table>
    <div class="center">Спасибо за покупку!</div>
  </body></html>`
}

/** Дата чека: день и время записи; если учётный день другой (чек без сети после полуночи,
 *  исправленный чек) — учётный день, как в «Чеках за день» и акте сверки. */
function saleWhen(sale: Sale): string {
  const recorded = new Date(sale.created_at).toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  if (!sale.business_date || sale.business_date === recorded) return formatDateTime(sale.created_at)
  const [y, m, d] = sale.business_date.split('-')
  return `${d}.${m}.${y}`
}

export function printSale(sale: Sale, change: number | null = null): void {
  printInFrame(saleHtml(sale, change))
}
