// Печать товарного чека через диалог печати браузера (HTML-шаблон, лента 58–80 мм).
import { formatDateTime, formatSomExact } from '../lib/format'
import { salePriceSuffix, saleQtyText } from '../lib/units'
import { paymentLabel } from '../lib/battery'
import { printInFrame } from '../lib/printFrame'
import { lineSum } from '../lib/discount'
import type { Sale, SaleLine } from '../lib/types'
import { branchLabel } from '../lib/branch'
import { mileageText } from '../lib/mileage'
import { cachedOilBookSettings } from '../lib/oilBookSettings'
import { bookGroups, bookGroupsHtml } from '../lib/oilBook'

const esc = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

export function lineQtyText(l: Pick<SaleLine, 'kind' | 'qty' | 'container_ml'>): string {
  return saleQtyText(l.kind, l.qty, l.container_ml)
}

/** Скидка чека для печати (задача 003 Н3): на сколько строки дешевле прайса, без подарков. */
export function saleDiscount(sale: Pick<Sale, 'lines'>): number {
  return sale.lines
    .filter((l) => !l.gift && l.withheld_for == null && l.unit_price_tyiyn < l.list_price_tyiyn)
    .reduce((acc, l) => acc + lineSum(l.kind, l.qty, l.list_price_tyiyn) - l.amount_tyiyn, 0)
}

/** `discount` — строка «Скидка» перед «Итого» (настройка «Печатать строку «Скидка» в чеке»);
 *  `bookBlock: false` — без блока «Масляная книжка» (задача 003 Н5). */
export function saleHtml(sale: Sale, change: number | null, opts: { bookBlock?: boolean; discount?: boolean } = {}): string {
  const title = sale.kind === 'return' ? (sale.no_receipt ? 'Возврат без чека' : 'Возврат') : 'Товарный чек'
  // Удержание за замену (ADR-104) — отдельной строкой после возвращённого.
  const held = sale.lines
    .filter((l) => l.withheld_for != null)
    .map((l) => `<tr><td>Удержано за замену</td><td class="r">${esc(formatSomExact(l.amount_tyiyn))}</td></tr>`)
    .join('')
  const rows = sale.lines
    .filter((l) => l.withheld_for == null)
    .map((l) =>
      // Услуга — название и сумма одной строкой; «N × цена» — только при количестве больше одного (ADR-130).
      l.kind === 'service'
        ? `<tr><td>${esc(l.name)}</td><td class="r">${esc(formatSomExact(l.amount_tyiyn))}</td></tr>${
            l.qty > 1 ? `<tr><td class="muted" colspan="2">${l.qty} × ${esc(formatSomExact(l.unit_price_tyiyn))}</td></tr>` : ''
          }`
        : `<tr><td colspan="2">${esc(l.name)}</td></tr>
      <tr><td class="muted">${esc(lineQtyText(l))} × ${esc(formatSomExact(l.unit_price_tyiyn))}${salePriceSuffix(l.kind)}</td>
      <td class="r">${esc(formatSomExact(l.amount_tyiyn))}</td></tr>`,
    )
    .join('') + held
  const pays = sale.payments
    .map((p) => `<tr><td>${esc(paymentLabel(p))}</td><td class="r">${esc(formatSomExact(p.amount_tyiyn))}</td></tr>`)
    .join('')
  const changeRow = change && change > 0 ? `<tr><td>Сдача</td><td class="r">${esc(formatSomExact(change))}</td></tr>` : ''
  const off = opts.discount && sale.kind !== 'return' ? saleDiscount(sale) : 0
  const discountRow = off > 0 ? `<tr><td>Скидка</td><td class="r">−${esc(formatSomExact(off))}</td></tr>` : ''
  const master = sale.master_name ? `<div>Мастер: ${esc(sale.master_name)}</div>` : ''
  return `<!doctype html><html><head><meta charset="utf-8"><title>Чек ${sale.number}</title><style>
    @page { margin: 2mm; }
    body { font-family: Arial, sans-serif; font-size: 10pt; margin: 0; width: auto; max-width: 72mm; overflow-wrap: anywhere; }
    h1 { font-size: 12pt; text-align: center; margin: 0 0 2mm; }
    table { width: 100%; border-collapse: collapse; }
    td { padding: 0.5mm 0; vertical-align: top; }
    .r { text-align: right; white-space: nowrap; }
    .muted { color: #444; }
    .total td { font-weight: bold; font-size: 12pt; border-top: 1px dashed #000; padding-top: 1mm; }
    .center { text-align: center; margin-top: 3mm; }
    .book { border-top: 1px dashed #000; margin-top: 2mm; padding-top: 1mm; }
    .book b { display: block; }
    .book .bk { margin-top: 1mm; }
    .book .grp, .book .nx { font-weight: bold; }
  </style></head><body>
    <h1>Avtodom</h1>
    ${branchLabel() ? `<div class="center" style="margin:0 0 2mm">${esc(branchLabel() ?? '')}</div>` : ''}
    <div>${title} № ${sale.number}</div>
    <div>${esc(saleWhen(sale))}</div>
    <div>Кассир: ${esc(sale.cashier_name)}</div>${master}
    ${sale.delivery_address ? `<div>Доставка: ${esc(sale.delivery_address)}</div>` : ''}
    ${sale.kind === 'return' && sale.comment ? `<div>Причина: ${esc(sale.comment)}</div>` : ''}
    <table>${rows}${discountRow}<tr class="total"><td>Итого</td><td class="r">${esc(formatSomExact(sale.total_tyiyn))}</td></tr>${pays}${changeRow}</table>
    ${opts.bookBlock === false ? '' : bookHtml(sale)}
    <div class="center">Спасибо за покупку!</div>
  </body></html>`
}

/** Масляная книжка чека «В сервис» (SPEC-34 §4, ADR-116, задача 003 Н5): машина, пробег сейчас и
 *  что поменяли по видам замены — товаром, а не строкой книжки, — со следующей заменой, только пробег
 *  (ADR-115). Печатать нечего — блока нет. */
export function bookHtml(sale: Pick<Sale, 'sale_type' | 'kind' | 'book'>): string {
  const b = sale.book
  if (!b || sale.kind === 'return' || sale.sale_type !== 'service') return ''
  const car = [b.brand, b.model].map((x) => x.trim()).filter(Boolean).join(' ')
  const same = (x: { slot?: string; service_id?: string | null }, y: { slot?: string; service_id?: string | null }) =>
    x.slot === y.slot && (x.service_id ?? null) === (y.service_id ?? null)
  const nextOf = (x: { slot?: string; service_id?: string | null }) => {
    const l = b.lines.find((l) => same(l, x))
    return l ? `на ${mileageText(l.next_mileage, l.unit)}` : ''
  }
  const filled = b.filled ?? []
  const items = [
    ...filled.map((f) => ({ title: f.label, group: f.group ?? null, next: nextOf(f) })),
    // Следующая замена без строки «что залито» (чек из очереди старой кассы) — своей строкой.
    ...b.lines.filter((l) => !filled.some((f) => same(f, l))).map((l) => ({ title: l.label, group: l.group ?? null, next: nextOf(l) })),
  ]
  if (items.length === 0) return ''
  return `<div class="book"><b>Масляная книжка</b>
    <div>Машина: ${esc([car, b.plate.trim()].filter(Boolean).join(', '))}</div>
    ${b.mileage !== null ? `<div>Пробег сейчас: ${esc(mileageText(b.mileage, b.mileage_unit))}</div>` : ''}
    ${bookGroupsHtml(bookGroups(items))}</div>`
}

/** Дата чека: день и время записи; если учётный день другой (чек без сети после полуночи,
 *  исправленный чек) — учётный день, как в «Чеках за день» и акте сверки. */
function saleWhen(sale: Sale): string {
  const recorded = new Date(sale.created_at).toLocaleDateString('sv-SE', { timeZone: 'Asia/Bishkek' })
  if (!sale.business_date || sale.business_date === recorded) return formatDateTime(sale.created_at)
  const [y, m, d] = sale.business_date.split('-')
  return `${d}.${m}.${y}`
}

/** `bookBlock: false` — без блока «Масляная книжка» (настройка «Масляная книжка и печать», задача 003 Н5);
 *  `discount` — строка «Скидка» (задача 003 Н3). */
export function printSale(sale: Sale, change: number | null = null, opts: { bookBlock?: boolean; discount?: boolean } = {}): void {
  printInFrame(saleHtml(sale, change, { bookBlock: cachedOilBookSettings().receipt_block, ...opts }))
}
