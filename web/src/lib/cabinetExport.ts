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

// ---------- Кабинет поставщика (задача 028): накладные в Excel ----------

export interface SupplierInvoiceLine {
  name: string
  qty: number
  unit: 'piece' | 'ml' | 'g' | 'mm'
  container_ml: number | null
  amount_tyiyn: number
}

export interface SupplierInvoiceFile {
  id: string
  name: string
  content_type: string
  size_bytes: number
  source: 'staff' | 'supplier'
  created_at: string
}

export type SupplierInvoiceStatus = 'now' | 'unpaid' | 'partial' | 'paid'

export interface SupplierCabinetInvoice {
  id: string
  number: number
  supplier_doc: string
  business_date: string
  created_at: string
  branch_name: string
  total_tyiyn: number
  paid_tyiyn: number
  left_tyiyn: number
  status: SupplierInvoiceStatus
  reversed: boolean
  lines: SupplierInvoiceLine[]
  files: SupplierInvoiceFile[]
}

export const INVOICE_STATUS_TEXT: Record<SupplierInvoiceStatus, string> = {
  now: 'оплачена сразу',
  unpaid: 'не оплачена',
  partial: 'оплачена частично',
  paid: 'оплачена',
}

/** Статус накладной словами; отменённая — «отменена». */
export function invoiceStatusText(i: SupplierCabinetInvoice): string {
  return i.reversed ? 'отменена' : INVOICE_STATUS_TEXT[i.status]
}

/** Номер накладной: номер поставщика, если записан, и наш номер прихода. */
export function invoiceTitle(i: SupplierCabinetInvoice): string {
  return i.supplier_doc.trim() ? `№ ${i.supplier_doc.trim()} (приход ${i.number})` : `Приход № ${i.number}`
}

function invoiceQty(l: SupplierInvoiceLine): { qty: Cell; unit: string } {
  if (l.unit === 'ml') return { qty: xl.liters(l.qty), unit: 'л' }
  if (l.unit === 'g') return { qty: { qty: l.qty, scale: 3 }, unit: 'кг' }
  if (l.unit === 'mm') return { qty: { qty: l.qty, scale: 3 }, unit: 'м' }
  return { qty: xl.int(l.qty), unit: 'шт' }
}

export interface InvoiceRow {
  inv: SupplierCabinetInvoice
  line: SupplierInvoiceLine | null
  /** Первая строка накладной: на ней сумма, оплачено и осталось (столбцы не задваиваются). */
  first: boolean
}

/** Строки файла: каждая строка каждой накладной; накладная без строк — одной строкой шапки. */
export function invoiceRows(list: SupplierCabinetInvoice[]): InvoiceRow[] {
  return list.flatMap((inv): InvoiceRow[] => (inv.lines.length ? inv.lines.map((line, i) => ({ inv, line, first: i === 0 })) : [{ inv, line: null, first: true }]))
}

export const INVOICE_COLS: ExportCol<InvoiceRow>[] = [
  { label: 'Дата', value: (r) => xl.dateTime(r.inv.created_at) },
  { label: 'Накладная', value: (r) => invoiceTitle(r.inv) },
  { label: 'Филиал', value: (r) => r.inv.branch_name },
  { label: 'Товар', value: (r) => r.line?.name ?? '' },
  { label: 'Количество', value: (r) => (r.line ? invoiceQty(r.line).qty : null) },
  { label: 'Ед.', value: (r) => (r.line ? invoiceQty(r.line).unit : '') },
  { label: 'Сумма строки', value: (r) => (r.line ? xl.money(r.line.amount_tyiyn) : null) },
  { label: 'Сумма накладной', value: (r) => (r.first ? xl.money(r.inv.total_tyiyn) : null) },
  { label: 'Оплачено', value: (r) => (r.first && !r.inv.reversed ? xl.money(r.inv.paid_tyiyn) : null) },
  { label: 'Осталось', value: (r) => (r.first && !r.inv.reversed ? xl.money(r.inv.left_tyiyn) : null) },
  { label: 'Статус', value: (r) => (r.first ? invoiceStatusText(r.inv) : '') },
]

/** «Итого» — по действующим накладным (отменённые не считаются). */
export function invoiceTotal(list: SupplierCabinetInvoice[]): Cell[] {
  const live = list.filter((i) => !i.reversed)
  const sum = (f: (i: SupplierCabinetInvoice) => number) => xl.money(live.reduce((a, i) => a + f(i), 0))
  return totalBy(INVOICE_COLS, {
    Дата: 'Итого',
    'Сумма накладной': sum((i) => i.total_tyiyn),
    Оплачено: sum((i) => i.paid_tyiyn),
    Осталось: sum((i) => i.left_tyiyn),
  })
}
