// «Склад → Остатки» (SPEC-36 §14, ADR-125): строки по категориям, подытоги групп и лист остатков на
// дату. Подытоги и суммы считает сервер (`category_totals`); здесь — только показ и печать.
import { formatDateTime, formatKg, formatLiters, formatSomExact } from './format'
import type { CategoryTotal, ProductRow } from './types'
import { oilCountText, qtyText, viscosityFromName } from './units'

export interface StockGroup {
  total: CategoryTotal
  rows: ProductRow[]
}

/** Строки по группам в порядке подытогов сервера (по названию категории). */
export function groupRows(items: readonly ProductRow[], totals: readonly CategoryTotal[]): StockGroup[] {
  const by = new Map<string, ProductRow[]>()
  for (const r of items) {
    const list = by.get(r.category_id)
    if (list) list.push(r)
    else by.set(r.category_id, [r])
  }
  return totals.map((t) => ({ total: t, rows: by.get(t.category_id) ?? [] }))
}

/** Остаток строки: масло в таре — «34,5 л · 8 кан. + 2,5 л», вес — кг, остальное — штуки. */
export function rowQty(p: Pick<ProductRow, 'unit' | 'container_ml' | 'stock_qty'>): string {
  if (p.unit === 'ml' && p.container_ml) {
    const cans = oilCountText(p.stock_qty, p.container_ml)
    const liters = formatLiters(p.stock_qty)
    return cans === liters ? liters : `${liters} · ${cans}`
  }
  return qtyText(p.unit, p.stock_qty)
}

/** Количество группы: штуки, масло литрами (одна тара — и «кан. + л»), вес в кг. */
export function groupQty(t: Pick<CategoryTotal, 'pieces' | 'ml' | 'grams' | 'container_ml'>): string {
  const parts: string[] = []
  if (t.pieces !== 0) parts.push(`${t.pieces} шт`)
  if (t.ml !== 0) parts.push(t.container_ml ? rowQty({ unit: 'ml', container_ml: t.container_ml, stock_qty: t.ml }) : formatLiters(t.ml))
  if (t.grams !== 0) parts.push(formatKg(t.grams))
  return parts.join(' · ') || '0'
}

/** Итог склада — сумма подытогов (целые числа, без тары: она у групп разная). */
export function stockTotal(totals: readonly CategoryTotal[]): Pick<CategoryTotal, 'pieces' | 'ml' | 'grams' | 'container_ml'> {
  return totals.reduce(
    (a, t) => ({ pieces: a.pieces + t.pieces, ml: a.ml + t.ml, grams: a.grams + t.grams, container_ml: null }),
    { pieces: 0, ml: 0, grams: 0, container_ml: null as number | null },
  )
}

/** Характеристика: вязкость из карточки, у масла без неё — из названия (`fromName`). */
export function rowViscosity(p: Pick<ProductRow, 'attrs' | 'name' | 'category_kind'>): { text: string; fromName: boolean } | null {
  const own = typeof p.attrs?.viscosity === 'string' ? p.attrs.viscosity.trim() : ''
  if (own) return { text: own, fromName: false }
  if (p.category_kind !== 'oil') return null
  const guess = viscosityFromName(p.name)
  return guess ? { text: guess, fromName: true } : null
}

/** За что цена: канистра, килограмм, штука. */
export function pricePer(p: Pick<ProductRow, 'unit' | 'container_ml'>): string {
  if (p.unit === 'ml' && p.container_ml) return `за ${formatLiters(p.container_ml)}`
  if (p.unit === 'g') return 'за кг'
  if (p.unit === 'ml') return 'за л'
  return ''
}

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

/**
 * Лист остатков на дату и время (A4): группы с подытогами и итог склада. Закупка — только если её отдал
 * сервер (владельцу, ADR-008).
 */
export function stockSheetHtml(o: {
  /** Время листа (ISO). */
  at: string
  groups: readonly StockGroup[]
  total: { count: number; sale_tyiyn: number; purchase_tyiyn?: number }
  filter?: string
}): string {
  const owner = o.total.purchase_tyiyn != null
  const money = (v: number | null | undefined) => (v == null ? '' : esc(formatSomExact(v)))
  const cols = owner ? 7 : 6
  const body = o.groups
    .map((g) => {
      const rows = g.rows
        .map((r) => {
          const v = rowViscosity(r)
          const minus = r.stock_qty < 0
          return `<tr${minus ? ' class="minus"' : ''}>
<td>${esc(r.name)}${minus ? ' <b>(проверить)</b>' : ''}</td>
<td>${v ? esc(v.text) : ''}</td>
<td class="n">${esc(rowQty(r))}</td>
<td class="n">${r.min_stock > 0 ? esc(qtyText(r.unit, r.min_stock)) : ''}</td>
<td class="n">${money(r.sale_price_tyiyn)} ${esc(pricePer(r))}</td>
<td class="n">${money(r.stock_sale_tyiyn)}</td>${owner ? `\n<td class="n">${money(r.stock_purchase_tyiyn)}</td>` : ''}
</tr>`
        })
        .join('\n')
      const t = g.total
      return `<tr class="group"><td colspan="${cols}">${esc(t.name)}</td></tr>
${rows}
<tr class="sub"><td>Итого «${esc(t.name)}»: ${t.count} поз.</td><td></td><td class="n">${esc(groupQty(t))}</td><td></td><td></td>
<td class="n">${money(t.sale_tyiyn)}</td>${owner ? `<td class="n">${money(t.purchase_tyiyn)}</td>` : ''}</tr>`
    })
    .join('\n')
  const all = stockTotal(o.groups.map((g) => g.total))
  return `<!doctype html><html lang="ru"><head><meta charset="utf-8"><title>Остатки на ${esc(formatDateTime(o.at))}</title>
<style>
@page { size: A4; margin: 12mm; }
body { font: 11px/1.35 Arial, sans-serif; color: #000; }
h1 { font-size: 16px; margin: 0 0 4px; }
.meta { margin-bottom: 8px; color: #333; }
table { width: 100%; border-collapse: collapse; }
th, td { border: 1px solid #999; padding: 2px 4px; vertical-align: top; }
th { background: #eee; text-align: left; }
.n { text-align: right; white-space: nowrap; }
.group td { background: #f3f3f3; font-weight: bold; }
.sub td { font-weight: bold; }
.minus td { color: #b00; }
.total td { font-weight: bold; border-top: 2px solid #000; }
</style></head><body>
<h1>Остатки на ${esc(formatDateTime(o.at))}</h1>
<div class="meta">${o.filter ? `${esc(o.filter)} · ` : ''}позиций: ${o.total.count}</div>
<table>
<thead><tr><th>Товар</th><th>Вязкость</th><th>Остаток</th><th>Минимум</th><th>Цена</th><th>Сумма (продажа)</th>${owner ? '<th>Сумма (закупка)</th>' : ''}</tr></thead>
<tbody>
${body}
<tr class="total"><td>Итого по складу: ${o.total.count} поз.</td><td></td><td class="n">${esc(groupQty(all))}</td><td></td><td></td>
<td class="n">${money(o.total.sale_tyiyn)}</td>${owner ? `<td class="n">${money(o.total.purchase_tyiyn)}</td>` : ''}</tr>
</tbody></table>
</body></html>`
}
