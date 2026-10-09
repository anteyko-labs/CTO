// Печать товарного чека через диалог печати браузера (HTML-шаблон, лента 58–80 мм).
import { formatDateTime, formatKg, formatLiters, formatSom } from '../lib/format'
import { PAYMENT_LABELS, type Sale, type SaleLine } from '../lib/types'

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
  const title = sale.kind === 'return' ? 'Возврат' : 'Товарный чек'
  const rows = sale.lines
    .map(
      (l) => `<tr><td colspan="2">${esc(l.name)}</td></tr>
      <tr><td class="muted">${esc(lineQtyText(l))} × ${esc(formatSom(l.unit_price_tyiyn))}${l.kind === 'pour' ? '/л' : l.kind === 'weight' ? '/кг' : ''}</td>
      <td class="r">${esc(formatSom(l.amount_tyiyn))}</td></tr>`,
    )
    .join('')
  const pays = sale.payments
    .map((p) => `<tr><td>${PAYMENT_LABELS[p.method]}</td><td class="r">${esc(formatSom(p.amount_tyiyn))}</td></tr>`)
    .join('')
  const changeRow = change && change > 0 ? `<tr><td>Сдача</td><td class="r">${esc(formatSom(change))}</td></tr>` : ''
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
    <div>${title} № ${sale.number}</div>
    <div>${esc(formatDateTime(sale.created_at))}</div>
    <div>Кассир: ${esc(sale.cashier_name)}</div>${master}
    ${sale.delivery_address ? `<div>Доставка: ${esc(sale.delivery_address)}</div>` : ''}
    <table>${rows}<tr class="total"><td>Итого</td><td class="r">${esc(formatSom(sale.total_tyiyn))}</td></tr>${pays}${changeRow}</table>
    <div class="center">Спасибо за покупку!</div>
  </body></html>`
}

export function printSale(sale: Sale, change: number | null = null): void {
  const frame = document.createElement('iframe')
  Object.assign(frame.style, { position: 'fixed', width: '0', height: '0', border: '0' })
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  const win = frame.contentWindow
  if (!doc || !win) {
    frame.remove()
    return
  }
  doc.open()
  doc.write(saleHtml(sale, change))
  doc.close()
  win.focus()
  win.print()
  setTimeout(() => frame.remove(), 60_000)
}
