// Этикетки со штрихкодом: HTML для станции этикеток склада (SPEC-02, ADR-010, задача 014).
import JsBarcode from 'jsbarcode'
import { labelPrice, labelText, type PricePer } from '../lib/labelText'
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

/** Точка термопринтера 203 dpi, мм. Полоса штрихкода — целое число точек: дробная (1,7 точки)
 *  печатается рваной, и сканер код не читает. */
const DOT_MM = 25.4 / 203
/** Подпись цифрами под полосами, мм. */
const DIGITS_MM = 2.4

/** Штрихкод шириной не больше `availMm`, высотой `heightMm`: полоса — целое число точек, не меньше 2. */
function barcodeSvg(code: string, availMm: number, heightMm: number): string {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
  const opts = {
    format: isEan13(code) ? 'EAN13' : 'CODE128',
    // Свободное поле по краям: сканер не читает код, прижатый к краю этикетки (EAN-13 — 11 и
    // 7 модулей). Единица рисунка — один модуль.
    margin: 0,
    marginLeft: 11,
    marginRight: 7,
    width: 1,
    // Ровная раскладка: первая цифра EAN-13 не выносится влево — рисунок ровно 113 модулей.
    flat: true,
    textMargin: 1,
    displayValue: true,
  }
  JsBarcode(svg, code, { ...opts, height: 50, fontSize: 10 })
  const modules = parseFloat(svg.getAttribute('width') ?? '') || 113
  const moduleMm = Math.max(2, Math.floor(availMm / (modules * DOT_MM))) * DOT_MM
  const fontSize = Math.max(6, Math.round(DIGITS_MM / moduleMm))
  const bars = Math.max(10, Math.round(heightMm / moduleMm) - fontSize - opts.textMargin)
  JsBarcode(svg, code, { ...opts, height: bars, fontSize })
  svg.setAttribute('preserveAspectRatio', 'none')
  // Полосы по точкам принтера, без размытых краёв.
  svg.setAttribute('shape-rendering', 'crispEdges')
  svg.setAttribute('width', `${(modules * moduleMm).toFixed(2)}mm`)
  svg.setAttribute('height', `${((bars + fontSize + opts.textMargin) * moduleMm).toFixed(2)}mm`)
  return svg.outerHTML
}

/** HTML страницы печати: по одной этикетке на страницу заданного размера; цена — `per` (задача 021). */
export function labelsHtml(items: LabelItem[], s: LabelSettings, per: PricePer = 'piece'): string {
  const labels: string[] = []
  // Маленькая этикетка (до 25 мм в высоту): текст ужимается в строку названия и строку
  // «бренд · характеристики — цена» (проверка 20.10.2026), поля по бокам 0,5 мм — штрихкоду ширина.
  const compact = s.height_mm <= 25
  const padX = compact ? 0.5 : 1.5
  // Штрихкоду — половина высоты (маленькой — 55 %), текст мельче: на бумаге код выходил мелким.
  const codeMm = Math.round(s.height_mm * (compact ? 0.55 : 0.5) * 10) / 10
  for (const { product, copies } of items) {
    const code = product.barcodes[0]
    if (!code) continue
    const svg = barcodeSvg(code, s.width_mm - 2 * padX, codeMm)
    // Бренд, название и характеристики — вязкость и объём масла, тип фильтра (SPEC-29 §6).
    const t = labelText(product, s)
    const brand = t.brand ? `<div class="brand">${escapeHtml(t.brand)}</div>` : ''
    const name = t.name ? `<div class="name">${escapeHtml(t.name)}</div>` : ''
    const spec = t.spec ? `<div class="spec">${escapeHtml(t.spec)}</div>` : ''
    const article = s.show_article && product.article ? `<div class="small">${escapeHtml(product.article)}</div>` : ''
    const price = s.show_price ? `<div class="price">Цена: ${escapeHtml(labelPrice(product, per))}</div>` : ''
    // На маленькой — коротко и без «Цена:»: «500 с/л» влезает в 30 мм (задача 021).
    const short = escapeHtml(labelPrice(product, per, true))
    const meta = [t.brand, t.spec].filter(Boolean).join(' · ')
    // Без названия и бренда (маленькая 30×20, задача 014) — штрихкод и под ним цена крупно.
    const bare = !t.name && !meta
    const html = compact
      ? bare
        ? `<div class="label"><div class="code">${svg}</div>${s.show_price ? `<div class="price big">${short}</div>` : ''}</div>`
        : `<div class="label">${t.name ? `<div class="name one">${escapeHtml(t.name)}</div>` : ''}<div class="code">${svg}</div>` +
          `<div class="foot"><span class="meta">${escapeHtml(meta)}</span>${s.show_price ? `<span class="price">${short}</span>` : ''}</div></div>`
      : `<div class="label">${brand}${name}${spec}${article}<div class="code">${svg}</div>${price}</div>`
    for (let i = 0; i < copies; i++) {
      labels.push(html)
    }
  }
  // Текст не растёт с этикеткой: высоту забирает штрихкод.
  const pt = (n: number) => `${n}pt`
  return `<!doctype html><html><head><meta charset="utf-8"><title>Этикетки</title><style>
    @page { size: ${s.width_mm}mm ${s.height_mm}mm; margin: 0; }
    * { box-sizing: border-box; }
    body { margin: 0; font-family: Arial, sans-serif; }
    /* На 1 мм ниже наклейки: ровно в её высоту Chrome переносит низ этикетки (цифры штрихкода)
       на следующую наклейку (проверка на XP-365B 03.11.2026). */
    .label { width: ${s.width_mm}mm; height: ${s.height_mm - 1}mm; padding: ${compact ? 0.5 : 1}mm ${padX}mm; overflow: hidden;
             display: flex; flex-direction: column; justify-content: space-between; page-break-after: always; }
    .label:last-child { page-break-after: auto; }
    .brand { font-size: ${pt(6)}; font-weight: bold; text-transform: uppercase; line-height: 1.1; }
    .name { font-size: ${pt(7)}; font-weight: bold; line-height: 1.1; max-height: 2.3em; overflow: hidden; }
    .spec { font-size: ${pt(7)}; font-weight: bold; line-height: 1.1; }
    .small { font-size: ${pt(6)}; }
    .name.one { font-size: 6pt; white-space: nowrap; text-overflow: ellipsis; max-height: none; }
    .code { flex: 1; min-height: ${codeMm}mm; display: flex; overflow: hidden; align-items: center; justify-content: center; }
    .code svg { flex: none; display: block; }
    .price { font-size: ${pt(9)}; font-weight: bold; text-align: right; }
    .foot { display: flex; align-items: baseline; justify-content: space-between; gap: 1mm; }
    .foot .meta { font-size: 6pt; font-weight: bold; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
    .foot .price { font-size: 8pt; white-space: nowrap; }
    .price.big { font-size: 9pt; text-align: center; white-space: nowrap; }
  </style></head><body>${labels.join('')}</body></html>`
}
