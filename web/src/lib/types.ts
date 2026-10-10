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
  unit: 'piece' | 'ml' | 'g'
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

export type SaleLineKind = 'piece' | 'container' | 'pour' | 'service' | 'weight'
export type PaymentMethod = 'cash' | 'card' | 'transfer' | 'debt' | 'bonus'

export interface SaleLine {
  line_no: number
  kind: SaleLineKind
  gift: boolean
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
  /** Адрес доставки; пусто — забрали сами. */
  delivery_address: string
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
  party_name: string | null
  debt_tyiyn: number
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
    debt_tyiyn: number
    bonus_tyiyn?: number
  }
}

export const PAYMENT_LABELS: Record<PaymentMethod, string> = {
  cash: 'Наличные',
  card: 'Карта',
  // Код прежний, чтобы не трогать проведённые чеки: на точке это оплата QR через терминал.
  transfer: 'QR',
  debt: 'В долг',
  bonus: 'Баллы',
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
  /** Долг старше срока оплаты, ещё не погашенный. */
  overdue_tyiyn: number
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
  /** Запись долга; погашение и правку владелец может сторнировать (SPEC-10). */
  ledger_id: string | null
  reversible: boolean
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

// ---------- Уведомления владельцу ----------

export interface NotificationItem {
  id: string
  action: string
  title: string
  details: string
  user_name: string | null
  at: string
  entity_id: string | null
  new: boolean
}

export interface NotificationsOut {
  unseen: number
  items: NotificationItem[]
}

// ---------- Подарки (SPEC-11) ----------

export interface GiftRule {
  id: string
  trigger_product_id: string
  trigger_name: string
  /** `ml` — масло, порог в миллилитрах; иначе в штуках. */
  trigger_unit: string
  min_units: number
  active: boolean
  items: { gift_product_id: string; name: string; gift_qty: number }[]
}

// ---------- Кассы и смена (SPEC-05) ----------

export interface CashAccount {
  id: string
  name: string
  kind: 'register' | 'bank' | 'safe' | 'other'
  owner_only: boolean
  is_default: boolean
  balance_tyiyn: number
}

export interface Shift {
  id: string
  number: number
  business_date: string
  account_id: string
  account_name: string
  cashier_name: string
  cashier_employee_id?: string
  opened_by: string
  opened_at: string
  opening_expected_tyiyn: number
  /** Сколько должно быть в кассе сейчас. */
  expected_tyiyn: number
  closed_at: string | null
  counted_tyiyn: number | null
  diff_tyiyn: number | null
  breakdown: { kind: string; sum_tyiyn: number }[]
  cash_sales_tyiyn: number
  card_tyiyn: number
  transfer_tyiyn: number
  debt_tyiyn: number
  bonus_tyiyn?: number
  bank_fee_tyiyn: number
  sales_count?: number
  sales_total_tyiyn?: number
  returns_tyiyn?: number
  pay?: StaffPay[]
  /** Смена закрыта, а что сделали с деньгами (сейф или размен) ещё не отмечено. */
  handover_pending: boolean
  to_safe_tyiyn: number | null
  left_tyiyn: number | null
}

export interface CashMovement {
  id: string
  kind: string
  amount_tyiyn: number
  comment: string
  doc_type: string
  doc_id: string | null
  user_name: string
  created_at: string
  /** Внесение, изъятие или перемещение, которое ещё можно отменить сторно. */
  reversible: boolean
}

// ---------- Расходы (SPEC-06) ----------

export interface ExpenseArticle {
  id: string
  name: string
  owner_only: boolean
  active: boolean
}

export interface Expense {
  id: string
  number: number
  article_id: string
  article_name: string
  amount_tyiyn: number
  source: 'account' | 'outside'
  account_name: string | null
  expense_date: string
  comment: string
  reversal_of: string | null
  reversed: boolean
  user_name: string
  created_at: string
}

// ---------- Расчёт сотрудников (SPEC-07) ----------

export interface PayrollRow {
  employee_id: string
  full_name: string
  opening_tyiyn: number
  accrued_tyiyn: number
  service_fee_tyiyn: number
  percent_tyiyn: number
  other_tyiyn: number
  /** База процента — валовая прибыль; администратору не отдаётся. */
  base_tyiyn: number | null
  paid_tyiyn: number
  balance_tyiyn: number
}

// ---------- Прибыль и сводка (SPEC-08) ----------

export interface ProfitTotals {
  goods_tyiyn: number
  services_tyiyn: number
  cost_tyiyn: number
  gross_tyiyn: number
  payroll_tyiyn: number
  bank_fee_tyiyn: number
  expenses_tyiyn: number
  bonus_tyiyn: number
  net_tyiyn: number
  margin_bp: number | null
  sales_count: number
}

export interface ProfitReport {
  from: string
  to: string
  totals: ProfitTotals
  categories: { name: string; revenue_tyiyn: number; cost_tyiyn: number; gross_tyiyn: number }[]
  days: {
    date: string
    revenue_tyiyn: number
    gross_tyiyn: number
    payroll_tyiyn: number
    expenses_tyiyn: number
    fee_tyiyn?: number
    bonus_tyiyn?: number
    net_tyiyn: number
  }[]
  articles: { name: string; amount_tyiyn: number }[]
  warnings: string[]
  staff?: StaffPay[]
  hours?: { hour: number; revenue_tyiyn: number; gross_tyiyn: number; sales_count: number }[]
}

export interface Dashboard {
  date: string
  totals: ProfitTotals
  returns_tyiyn: number
  average_check_tyiyn: number
  accounts: { name: string; balance_tyiyn: number }[]
  money_total_tyiyn: number
  shift_open: boolean
  shift_cashier: string | null
  to_pay_tyiyn: number
  debts_in_tyiyn: number
  debts_out_tyiyn: number
  low_stock: number
  needs_review: number
  stale_stock: number
  staff: StaffPay[]
  /** Продажи к этому же часу: сегодня, вчера, неделю назад. */
  compare?: { today: DayPart; yesterday: DayPart; week_ago: DayPart }
}

export interface DayPart {
  revenue_tyiyn: number
  gross_tyiyn: number
  sales_count: number
}

/** Тепловая карта продаж: день недели (1 — пн … 7 — вс) и час. */
export interface HeatCell {
  dow: number
  hour: number
  revenue_tyiyn: number
  sales_count: number
}

/** Плитка «ходовое» на кассе: товар или услуга. */
export interface FavoriteTile {
  kind: 'product' | 'service'
  product?: Product
  service?: Service
}

/** Кто сколько заработал: виды начислений с количеством. */
export interface StaffPay {
  employee_id: string
  name: string
  items: { kind: string; count: number; amount_tyiyn: number }[]
  total_tyiyn: number
  paid_tyiyn?: number
  owed_tyiyn?: number
}
