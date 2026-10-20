// Печать этикеток со штрихкодом через диалог печати браузера (SPEC-02, ADR-010).
import { printInFrame } from '../lib/printFrame'
import JsBarcode from 'jsbarcode'
import { get } from '../lib/api'
import { formatSom } from '../lib/format'
import { labelText } from '../lib/labelText'
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
    // Свободное поле по краям: сканер не читает код, прижатый к краю этикетки (EAN-13 — 11 и
    // 7 модулей; модуль — 2 единицы рисунка).
    margin: 0,
    marginLeft: 22,
    marginRight: 14,
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
    // Бренд, название и характеристики — вязкость и объём масла, тип фильтра (SPEC-29 §6).
    const t = labelText(product, s)
    const brand = t.brand ? `<div class="brand">${escapeHtml(t.brand)}</div>` : ''
    const name = t.name ? `<div class="name">${escapeHtml(t.name)}</div>` : ''
    const spec = t.spec ? `<div class="spec">${escapeHtml(t.spec)}</div>` : ''
    const article = s.show_article && product.article ? `<div class="small">${escapeHtml(product.article)}</div>` : ''
    const price = s.show_price ? `<div class="price">${escapeHtml(formatSom(product.sale_price_tyiyn))}</div>` : ''
    // Маленькая этикетка (до 25 мм в высоту): штрихкоду — не меньше 9 мм, текст ужимается в
    // строку названия и строку «бренд · характеристики — цена» (проверка 20.10.2026).
    const compact = s.height_mm <= 25
    const meta = [t.brand, t.spec].filter(Boolean).join(' · ')
    const html = compact
      ? `<div class="label">${t.name ? `<div class="name one">${escapeHtml(t.name)}</div>` : ''}<div class="code">${svg}</div>` +
        `<div class="foot"><span class="meta">${escapeHtml(meta)}</span>${s.show_price ? `<span class="price">${escapeHtml(formatSom(product.sale_price_tyiyn))}</span>` : ''}</div></div>`
      : `<div class="label">${brand}${name}${spec}${article}<div class="code">${svg}</div>${price}</div>`
    for (let i = 0; i < copies; i++) {
      labels.push(html)
    }
  }
  return `<!doctype html><html><head><meta charset="utf-8"><title>Этикетки</title><style>
    @page { size: ${s.width_mm}mm ${s.height_mm}mm; margin: 0; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: Arial, sans-serif; }
    .label { width: ${s.width_mm}mm; height: ${s.height_mm}mm; padding: 1.5mm; overflow: hidden;
             display: flex; flex-direction: column; justify-content: space-between; page-break-after: always; }
    .brand { font-size: 7pt; font-weight: bold; text-transform: uppercase; line-height: 1.1; }
    .name { font-size: 8pt; font-weight: bold; line-height: 1.1; max-height: 2.3em; overflow: hidden; }
    .spec { font-size: 8pt; font-weight: bold; line-height: 1.1; }
    .small { font-size: 7pt; }
    .name.one { font-size: 7pt; white-space: nowrap; text-overflow: ellipsis; max-height: none; }
    .code { flex: 1; min-height: ${Math.min(9, Math.max(6, s.height_mm * 0.45))}mm; display: flex; align-items: center; justify-content: center; }
    .code svg { width: 100%; height: 100%; }
    .price { font-size: 11pt; font-weight: bold; text-align: right; }
    .foot { display: flex; align-items: baseline; justify-content: space-between; gap: 1mm; }
    .foot .meta { font-size: 6.5pt; font-weight: bold; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
    .foot .price { font-size: 9pt; white-space: nowrap; }
  </style></head><body>${labels.join('')}</body></html>`
}

/** Печатает этикетки через скрытый фрейм. Возвращает число этикеток. */
export async function printLabels(items: LabelItem[]): Promise<number> {
  const settings = await get<LabelSettings>('/settings/labels')
  const printable = items.filter((i) => i.copies > 0 && i.product.barcodes.length > 0)
  const count = printable.reduce((acc, i) => acc + i.copies, 0)
  if (count === 0) return 0
  printInFrame(labelsHtml(printable, settings))
  return count
}
