// Цена прихода (закупки) «за кан. | за л» и себестоимость в карточке товара (задача 003, Н7,
// ADR-150). Только целые тыйыны и мл; перевод литр ↔ канистра — perCan/perLiter (одно округление).
// Введённое хранится как есть: переключение «кан. | л» показывает пересчёт, не накапливая ошибку.
import { formatLiters, formatPermille, formatSom, parseSom, somInput } from './format'
import { divRound, markupPermille } from './money'
import { avgPrice, countText, perCan, perLiter } from './units'
import type { Product } from './types'

/** За что введена цена масла: за канистру или за литр. У товара без тары — за его единицу. */
export type PricePer = 'can' | 'l'

/** Введённая цена: текст и за что он введён. */
export interface PriceEntry {
  text: string
  per: PricePer
}

/**
 * Цена за канистру (у товара без тары — за штуку или кг) из ввода, тыйыны: пусто — `'empty'`,
 * неверно или меньше нуля — null. Введено за литр — perCan(цена, тара).
 */
export function entryUnitPrice(e: PriceEntry, containerMl: number | null): number | 'empty' | null {
  const t = e.text.trim()
  if (!t) return 'empty'
  const v = parseSom(t)
  if (v === null || v < 0) return null
  return e.per === 'l' && containerMl ? perCan(v, containerMl) : v
}

/** Для запроса: цена числом, пусто или неверно — null (неверное не пропускает список «чего не хватает»). */
export const unitCostOrNull = (v: number | 'empty' | null): number | null => (typeof v === 'number' ? v : null)

/** Текст поля в показанном режиме: введено в нём — как есть, иначе пересчёт из введённого. */
export function entryText(e: PriceEntry, shown: PricePer, containerMl: number | null): string {
  if (e.per === shown || !containerMl) return e.text
  const v = parseSom(e.text.trim())
  if (v === null || !e.text.trim()) return ''
  return somInput(shown === 'l' ? perLiter(v, containerMl) : perCan(v, containerMl))
}

/** Ввели в показанном режиме: он и становится режимом цены. */
export const typed = (text: string, shown: PricePer): PriceEntry => ({ text, per: shown })

/** Вторая цена под полем: «= 450 с за л» или «= 1 800 с за канистру 4 л»; без тары или ввода — пусто. */
export function otherPriceText(e: PriceEntry, shown: PricePer, containerMl: number | null): string {
  if (!containerMl) return ''
  const v = parseSom(e.text.trim())
  if (v === null || !e.text.trim()) return ''
  if (shown === 'can') return `= ${formatSom(e.per === 'l' ? v : perLiter(v, containerMl))} за л`
  return `= ${formatSom(e.per === 'can' ? v : perCan(v, containerMl))} за канистру ${formatLiters(containerMl)}`
}

/** «наценка к цене продажи: 25,0 %» — закупка и продажа за одну единицу (канистру, штуку, кг). */
export function markupText(unitPrice: number | 'empty' | null, sale: number | null): string {
  if (typeof unitPrice !== 'number' || sale === null) return ''
  const m = markupPermille(unitPrice, sale)
  return m === null ? '' : `наценка к цене продажи: ${formatPermille(m)}`
}

/** Режим поля нового товара из строки накладной: литры — «за л», остальное — «за кан.». */
export const perOfInvoice = (liters: boolean): PricePer => (liters ? 'l' : 'can')

/** Цена строки накладной для карточки: цена за единицу, без неё — сумма / количество. */
export function invoiceEntry(l: { price_tyiyn: number | null; sum_tyiyn: number | null; qty_milli: number | null; liters: boolean }): PriceEntry | undefined {
  let each = l.price_tyiyn
  if (each === null && l.sum_tyiyn !== null && l.qty_milli) {
    // Сумма за литры — цена литра; за штуки — цена штуки (qty_milli — в тысячных).
    const v = l.sum_tyiyn * 1000
    each = Number.isSafeInteger(v) ? divRound(v, l.qty_milli) : null
  }
  return each === null || each < 0 ? undefined : { text: somInput(each), per: perOfInvoice(l.liters) }
}

type CostProduct = Pick<Product, 'unit' | 'container_ml' | 'stock_qty' | 'sale_price_tyiyn' | 'last_purchase_price_tyiyn'> & {
  avg_cost_tyiyn?: number | null
  stock_value_tyiyn?: number | null
  cost_pending?: boolean
}

/** Единица цены закупки в подписи: «за канистру 4 л», «за кг», «за м», «за шт». */
export function costPer(p: Pick<Product, 'unit' | 'container_ml'>): string {
  if (p.unit === 'ml' && p.container_ml) return `за канистру ${formatLiters(p.container_ml)}`
  if (p.unit === 'g') return 'за кг'
  if (p.unit === 'mm') return 'за м'
  if (p.unit === 'ml') return 'за л'
  return 'за шт'
}

/** Цена за единицу и, у масла в таре, за литр: «1 800 с за канистру 4 л · 450 с за л». */
export function priceWithLiter(tyiyn: number, p: Pick<Product, 'unit' | 'container_ml'>): string {
  const base = `${formatSom(tyiyn)} ${costPer(p)}`
  return p.unit === 'ml' && p.container_ml ? `${base} · ${formatSom(perLiter(tyiyn, p.container_ml))} за л` : base
}

/** Что показать в блоке «Себестоимость» владельцу (I13: администратору сервер этих полей не отдаёт). */
export interface CostSummary {
  /** «1 750 с за канистру 4 л · 437,50 с за л»; нет средней — null. */
  average: string | null
  /** «8 кан. + 2 л — 14 875 с»; остатка нет — null. */
  stockValue: string | null
  /** «цена продажи 2 000 с — наценка 14,3 %» к средней, без неё — к последней закупке. */
  markup: string | null
  /** Пояснение: остатка нет, остаток без цены, цены нет. */
  note: string | null
  /** Показать «Указать цену закупки». */
  askPrice: boolean
}

export function costSummary(p: CostProduct): CostSummary {
  const avg = avgPrice({ unit: p.unit, container_ml: p.container_ml, stock_qty: p.stock_qty, avg_cost_tyiyn: p.avg_cost_tyiyn, stock_value_tyiyn: p.stock_value_tyiyn })
  let average: string | null = null
  if (avg) {
    // У масла в таре — и за литр: из стоимости остатка, без неё — из средней канистры.
    const liter =
      p.unit === 'ml' && p.container_ml
        ? p.stock_qty > 0 && p.stock_value_tyiyn != null && p.stock_value_tyiyn > 0
          ? divRound(p.stock_value_tyiyn * 1000, p.stock_qty)
          : perLiter(avg.tyiyn, p.container_ml)
        : null
    const per = p.unit === 'ml' && p.container_ml ? `за канистру ${formatLiters(p.container_ml)}` : avg.per
    average = `${formatSom(avg.tyiyn)} ${per}${liter !== null ? ` · ${formatSom(liter)} за л` : ''}`
  }
  const value = p.stock_value_tyiyn ?? 0
  const stockValue = p.stock_qty > 0 ? `${countText(p, p.stock_qty)} — ${formatSom(value)}` : null
  const last = p.last_purchase_price_tyiyn
  // Средняя из avgPrice — за канистру, штуку, кг или м, как цена продажи; прошлая закупка — сервер
  // отдаёт за единицу цены (`catalog::price_unit`: кг, м, канистра, штука), как есть (задача 006).
  const base = avg ? avg.tyiyn : last
  const m = base != null ? markupPermille(base, p.sale_price_tyiyn) : null
  const markup = m === null ? null : `цена продажи ${formatSom(p.sale_price_tyiyn)} — наценка ${formatPermille(m)} к ${avg ? 'средней' : 'последней закупке'}`
  let note: string | null = null
  const noValue = p.stock_qty > 0 && value <= 0
  if (noValue) note = 'остаток без цены закупки — укажите её, остаток оценится по ней'
  else if (p.stock_qty <= 0) note = last != null ? 'остатка нет — продажа пойдёт по последней цене прихода' : 'остатка нет и цены закупки нет'
  else if (last == null && !avg) note = 'цены закупки нет'
  return { average, stockValue, markup, note, askPrice: noValue || last == null || Boolean(p.cost_pending) }
}

/** Цена прошлой закупки за литр — для подписи «(450 с за л)»; не масло в таре — null. */
export const literOf = (tyiyn: number, p: Pick<Product, 'unit' | 'container_ml'>): number | null =>
  p.unit === 'ml' && p.container_ml ? perLiter(tyiyn, p.container_ml) : null
