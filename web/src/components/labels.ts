// Печать этикеток со штрихкодом через диалог печати браузера (SPEC-02, ADR-010).
import JsBarcode from 'jsbarcode'
import { get } from '../lib/api'
import { formatSom } from '../lib/format'
import type { LabelSettings, Product } from '../lib/types'

export interface LabelItem {
  product: Product
  copies: number
}

const escapeHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

function isEan13(code: string): boolean {
  if (!/^\d{13}$/.test(code)) return false
  const digits = code.split('').map(Number)
  const sum = digits.slice(0, 12).reduce((acc, d, i) => acc + (i % 2 === 0 ? d : d * 3), 0)
  return (10 - (sum % 10)) % 10 === digits[12]
}

function barcodeSvg(code: string, heightMm: number): string {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  JsBarcode(svg, code, {
    format: isEan13(code) ? 'EAN13' : 'CODE128',
    margin: 0,
    height: Math.max(20, heightMm * 2),
    fontSize: 12,
    displayValue: true,
  })
  svg.setAttribute('preserveAspectRatio', 'xMidYMid meet')
  svg.removeAttribute('width')
  svg.removeAttribute('height')
  return svg.outerHTML
}

/** HTML страницы печати: по одной этикетке на страницу заданного размера. */
export function labelsHtml(items: LabelItem[], s: LabelSettings): string {
  const labels: string[] = []
  for (const { product, copies } of items) {
    const code = product.barcodes[0]
    if (!code) continue
    const svg = barcodeSvg(code, s.height_mm)
    const name = s.show_name ? `<div class="name">${escapeHtml(product.name)}</div>` : ''
    const article = s.show_article && product.article ? `<div class="small">${escapeHtml(product.article)}</div>` : ''
    const price = s.show_price ? `<div class="price">${escapeHtml(formatSom(product.sale_price_tyiyn))}</div>` : ''
    for (let i = 0; i < copies; i++) {
      labels.push(`<div class="label">${name}${article}<div class="code">${svg}</div>${price}</div>`)
    }
  }
  return `<!doctype html><html><head><meta charset="utf-8"><title>Этикетки</title><style>
    @page { size: ${s.width_mm}mm ${s.height_mm}mm; margin: 0; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: Arial, sans-serif; }
    .label { width: ${s.width_mm}mm; height: ${s.height_mm}mm; padding: 1.5mm; overflow: hidden;
             display: flex; flex-direction: column; justify-content: space-between; page-break-after: always; }
    .name { font-size: 8pt; font-weight: bold; line-height: 1.1; max-height: 2.3em; overflow: hidden; }
    .small { font-size: 7pt; }
    .code { flex: 1; min-height: 0; display: flex; align-items: center; justify-content: center; }
    .code svg { width: 100%; height: 100%; }
    .price { font-size: 11pt; font-weight: bold; text-align: right; }
  </style></head><body>${labels.join('')}</body></html>`
}

/** Печатает этикетки через скрытый фрейм. Возвращает число этикеток. */
export async function printLabels(items: LabelItem[]): Promise<number> {
  const settings = await get<LabelSettings>('/settings/labels')
  const printable = items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
  const count = printable.reduce((acc, i) => acc + i.copies, 0)
  if (count === 0) return 0
  const frame = document.createElement('iframe')
  frame.style.position = 'fixed'
  frame.style.width = '0'
  frame.style.height = '0'
  frame.style.border = '0'
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  if (!doc || !frame.contentWindow) {
    frame.remove()
    throw new Error('Не удалось подготовить печать')
  }
  doc.open()
  doc.write(labelsHtml(printable, settings))
  doc.close()
  frame.contentWindow.focus()
  frame.contentWindow.print()
  setTimeout(() => frame.remove(), 60_000)
  return count
}
