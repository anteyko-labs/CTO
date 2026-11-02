// Кабинет юрлица (SPEC-12): покупки за период — «В Excel» (задача 003 Н13). Строка файла — строка
// чека с шапкой чека (дата, №, работник, машина, филиал); итог — суммы чеков, как на экране.
// Деньги и количества — числами из целых (I1).
import { formatLiters } from './format'
import { saleQtyText } from './units'
import { totalBy, xl, type ExportCol } from './exportTable'
import type { Cell } from './xlsxWrite'

export interface CabinetSaleLine {
  name: string
  kind: string
  qty: number
  unit: string
  container_ml: number | null
  amount_tyiyn: number
}

export interface CabinetSale {
  id: string
  number: number
  kind: 'sale' | 'return'
  business_date: string
  created_at: string
  contact_name: string | null
  vehicle_plate: string | null
  total_tyiyn: number
  debt_tyiyn: number
  /** Оплачено «По счёту» — перечислением фирмы (ADR-095). */
  invoice_tyiyn?: number
  /** Филиал покупки (SPEC-33). */
  branch_name?: string
  lines: CabinetSaleLine[]
}

/** Количество строки словами — как на экране кабинета. */
export function cabinetQtyText(l: CabinetSaleLine): string {
  return saleQtyText(l.kind, l.qty, l.container_ml)
}

/** Количество строки числом и единица: литры, килограммы и метры — с тремя знаками из мл, г и мм. */
function qtyCell(l: CabinetSaleLine): { qty: Cell; unit: string } {
  if (l.kind === 'pour') return { qty: xl.liters(l.qty), unit: 'л' }
  if (l.kind === 'weight') return { qty: { qty: l.qty, scale: 3 }, unit: 'кг' }
  if (l.kind === 'length') return { qty: { qty: l.qty, scale: 3 }, unit: 'м' }
  if (l.kind === 'container') return { qty: xl.int(l.qty), unit: l.container_ml ? `кан. × ${formatLiters(l.container_ml)}` : 'кан.' }
  if (l.kind === 'service') return { qty: xl.int(l.qty), unit: 'усл.' }
  return { qty: xl.int(l.qty), unit: 'шт' }
}

export interface PurchaseRow {
  sale: CabinetSale
  line: CabinetSaleLine
  /** Первая строка чека: на ней — долг чека (сумма столбца не задваивается). */
  first: boolean
}

const NO_LINE: CabinetSaleLine = { name: '', kind: '', qty: 0, unit: '', container_ml: null, amount_tyiyn: 0 }

/** Строки файла: каждая строка каждого чека; чек без строк — одной строкой шапки. */
export function purchaseRows(sales: CabinetSale[]): PurchaseRow[] {
  return sales.flatMap((sale) => (sale.lines.length ? sale.lines : [NO_LINE]).map((line, i) => ({ sale, line, first: i === 0 })))
}

export const PURCHASE_COLS: ExportCol<PurchaseRow>[] = [
  { label: 'Когда', value: (r) => xl.dateTime(r.sale.created_at) },
  { label: 'Документ', value: (r) => `${r.sale.kind === 'return' ? 'Возврат' : 'Чек'} № ${r.sale.number}` },
  { label: 'Брал', value: (r) => r.sale.contact_name ?? '' },
  { label: 'Машина', value: (r) => r.sale.vehicle_plate ?? '' },
  { label: 'Филиал', value: (r) => r.sale.branch_name ?? '' },
  { label: 'Товар или услуга', value: (r) => r.line.name },
  { label: 'Количество', value: (r) => (r.line.kind ? qtyCell(r.line).qty : null) },
  { label: 'Ед.', value: (r) => (r.line.kind ? qtyCell(r.line).unit : '') },
  { label: 'Сумма', value: (r) => xl.money(r.line.amount_tyiyn) },
  { label: 'В долг', value: (r) => (r.first && r.sale.debt_tyiyn ? xl.money(r.sale.debt_tyiyn) : null) },
]

/** «Итого» — сумма чеков периода, как подпись над списком. */
export function purchaseTotal(sales: CabinetSale[]): Cell[] {
  return totalBy(PURCHASE_COLS, { Когда: 'Итого', Сумма: xl.money(sales.reduce((a, s) => a + s.total_tyiyn, 0)) })
}
