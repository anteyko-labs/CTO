// Типы ответов сервера (docs/tier-3/arch-core.md). Деньги — тыйыны, объёмы — мл.

export type Role = 'owner' | 'admin'

export interface User {
  id: string
  branch_id: string
  login: string
  full_name: string
  role: Role
}

export interface UserRow {
  id: string
  login: string
  full_name: string
  role: Role
  active: boolean
}

export type CategoryKind = 'oil' | 'filter' | 'battery' | 'other'

export interface AttributeDef {
  key: string
  label: string
  type: 'text' | 'number' | 'select'
  options?: string[]
  filterable: boolean
}

export interface Category {
  id: string
  name: string
  kind: CategoryKind
  attributes: AttributeDef[]
}

export interface Product {
  id: string
  category_id: string
  category_kind: CategoryKind
  name: string
  brand: string
  article: string
  unit: 'piece' | 'ml'
  container_ml: number | null
  attrs: Record<string, string>
  archived: boolean
  barcodes: string[]
  sale_price_tyiyn: number
  pour_price_per_l_tyiyn: number | null
  min_stock: number
  stock_qty: number
  needs_review: boolean
  /** Цена последнего прихода за штуку или канистру. */
  last_purchase_price_tyiyn: number | null
  avg_cost_tyiyn?: number
  stock_value_tyiyn?: number
}

export interface Employee {
  id: string
  full_name: string
  is_cashier: boolean
  is_master: boolean
  active: boolean
}

export interface Service {
  id: string
  name: string
  price_tyiyn: number
  master_fee_tyiyn: number
  active: boolean
}

export interface Supplier {
  id: string
  name: string
  phone: string
  comment: string
  active: boolean
}

/** Строка «что привозил поставщик» для его карточки. */
export interface SupplierSupply {
  product_id: string
  name: string
  unit: 'piece' | 'ml'
  container_ml: number | null
  receipts: number
  qty: number
  amount_tyiyn: number
  last_price_tyiyn: number
  last_at: string
}

export interface LabelSettings {
  width_mm: number
  height_mm: number
  show_name: boolean
  show_price: boolean
  show_article: boolean
}

export interface ReceiptLine {
  line_no: number
  product_id: string
  product_name: string
  unit: 'piece' | 'ml'
  container_ml: number | null
  qty: number
  cost_tyiyn: number
}

export interface Receipt {
  id: string
  number: number
  supplier_id: string | null
  supplier_name: string | null
  supplier_doc: string
  comment: string
  total_tyiyn: number
  reversal_of: string | null
  reversed_by: string | null
  user_name: string
  created_at: string
  lines: ReceiptLine[]
}

export interface ReceiptListItem {
  id: string
  number: number
  supplier_name: string | null
  supplier_doc: string
  total_tyiyn: number
  reversal_of: string | null
  reversed: boolean
  user_name: string
  created_at: string
}

export interface StockMismatch {
  product_id: string
  cached_qty: number
  moved_qty: number
  cached_value_tyiyn: number
  moved_value_tyiyn: number
}

export type SaleLineKind = 'piece' | 'container' | 'pour' | 'service'
export type PaymentMethod = 'cash' | 'card' | 'transfer' | 'debt'

export interface SaleLine {
  line_no: number
  kind: SaleLineKind
  product_id: string | null
  service_id: string | null
  name: string
  container_ml: number | null
  qty: number
  units: number
  unit_price_tyiyn: number
  list_price_tyiyn: number
  amount_tyiyn: number
  cost_tyiyn?: number
  master_fee_tyiyn: number
}

export interface Payment {
  method: PaymentMethod
  amount_tyiyn: number
}

export interface Sale {
  id: string
  number: number
  kind: 'sale' | 'return'
  sale_type: 'takeaway' | 'service'
  cashier_id: string
  cashier_name: string
  master_id: string | null
  master_name: string | null
  party_id: string | null
  party_name: string | null
  contact_name: string | null
  vehicle_plate: string | null
  /** Баланс клиента после чека: сколько он теперь должен. */
  party_balance_tyiyn: number | null
  total_tyiyn: number
  /** Начисление мастеру за замену: ставка на чек, а не строка услуги. */
  master_fee_tyiyn: number
  comment: string
  reversal_of: string | null
  user_name: string
  created_at: string
  lines: SaleLine[]
  payments: Payment[]
  returns: { id: string; number: number; total_tyiyn: number }[]
}

export interface SaleListItem {
  id: string
  number: number
  kind: 'sale' | 'return'
  sale_type: 'takeaway' | 'service'
  cashier_name: string
  master_name: string | null
  total_tyiyn: number
  reversal_of: string | null
  created_at: string
}

export interface SalesDay {
  date: string
  sales: SaleListItem[]
  totals: {
    count: number
    total_tyiyn: number
    cash_tyiyn: number
    card_tyiyn: number
    transfer_tyiyn: number
  }
}

export const PAYMENT_LABELS: Record<PaymentMethod, string> = {
  cash: 'Наличные',
  card: 'Карта',
  transfer: 'Перевод',
  debt: 'В долг',
}

export const KIND_LABELS: Record<CategoryKind, string> = {
  oil: 'Масло',
  filter: 'Фильтр',
  battery: 'Аккумулятор',
  other: 'Прочее',
}

// ---------- Контрагенты и долги (SPEC-10) ----------

export interface Party {
  id: string
  role: 'customer' | 'supplier'
  kind: 'person' | 'company'
  name: string
  phone: string
  inn: string
  comment: string
  credit_limit_tyiyn: number | null
  due_days: number | null
  active: boolean
  /** > 0 должен нам, < 0 аванс клиента. */
  balance_tyiyn: number
}

export interface PartyContact {
  id: string
  full_name: string
  phone: string
  position: string
  inn: string
  active: boolean
}

export interface PartyVehicle {
  id: string
  plate: string
  brand: string
  model: string
  comment: string
  active: boolean
}

export interface PartyTimelineItem {
  at: string
  kind: 'sale' | 'sale_return' | 'debt' | 'repayment' | 'adjust'
  title: string
  amount_tyiyn: number
  number: number | null
  doc_id: string | null
  comment: string
}

export interface PartyCard {
  party: Party
  contacts: PartyContact[]
  vehicles: PartyVehicle[]
  purchases: number
  purchases_tyiyn: number
  debt_taken_tyiyn: number
  repaid_tyiyn: number
  last_at: string | null
  timeline: PartyTimelineItem[]
}
