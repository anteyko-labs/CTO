// Типы ответов сервера (docs/tier-3/arch-core.md). Деньги — тыйыны, объёмы — мл.

export type Role = 'owner' | 'admin'

/** Права администратора, которые выбирает владелец (ADR-082, ADR-097); список — lib/permissions.ts. */
export type Permission =
  | 'sale_return'
  | 'sale_debt'
  | 'discount'
  | 'debt_payment'
  | 'cash_moves'
  | 'expenses'
  | 'payroll_pay'
  | 'receipts'
  | 'stock_in'
  | 'supplier_returns'
  | 'defects'
  | 'revision'
  | 'oil_transfer'
  | 'battery_intake'
  | 'products'
  | 'clients'
  | 'payroll_advance'

/** Филиал (SPEC-33). */
export interface Branch {
  id: string
  name: string
  address: string
  phone: string
  archived: boolean
  is_main: boolean
}

export interface User {
  id: string
  /** Филиал запроса: у владельца — выбранный, у администратора — свой. */
  branch_id: string
  login: string
  full_name: string
  role: Role
  /** Права администратора (ADR-082, ADR-097); у старого ответа сервера — нет. */
  permissions?: Permission[]
  /** Свой филиал пользователя и что он видит (SPEC-33); у старого ответа сервера — нет. */
  home_branch_id?: string
  branch_name?: string
  branches?: Branch[]
  several_branches?: boolean
}

export interface UserRow {
  id: string
  login: string
  full_name: string
  role: Role
  active: boolean
  permissions?: Permission[]
}

export type CategoryKind = 'oil' | 'filter' | 'battery' | 'other'

export interface AttributeDef {
  key: string
  label: string
  type: 'text' | 'number' | 'select'
  options?: string[]
  filterable: boolean
}

/** Строка масляной книжки (SPEC-27): масло, фильтры по видам, прочие работы; `none` — не в книжку. */
export type BookSlot =
  | 'oil'
  | 'gear_oil'
  | 'oil_filter'
  | 'air_filter'
  | 'cabin_filter'
  | 'fuel_filter'
  | 'antifreeze'
  | 'brake_fluid'
  | 'other'

export const BOOK_SLOT_LABELS: Record<BookSlot, string> = {
  oil: 'Масло двигателя',
  gear_oil: 'Масло в коробке',
  oil_filter: 'Фильтр масляный',
  air_filter: 'Фильтр воздушный',
  cabin_filter: 'Фильтр салонный',
  fuel_filter: 'Фильтр топливный',
  antifreeze: 'Антифриз',
  brake_fluid: 'Тормозная жидкость',
  other: 'Прочие работы',
}

export interface Category {
  id: string
  name: string
  kind: CategoryKind
  attributes: AttributeDef[]
  /** Строка книжки товаров категории и интервал по умолчанию. */
  book_slot: BookSlot | 'none'
  book_interval_km: number | null
  book_interval_days: number | null
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
  /** Общая цена всех филиалов (SPEC-33): `sale_price_tyiyn` — цена этого филиала. */
  shared_sale_price_tyiyn?: number
  /** У филиала своя цена: общая её не меняет. */
  own_price?: boolean
  /** Розлив за литр у масла — от цены канистры, сервер считает сам (ADR-074). */
  pour_price_per_l_tyiyn: number | null
  min_stock: number
  stock_qty: number
  needs_review: boolean
  /** Продан без цены закупки: 2 % кассира ждут, пока владелец укажет цену (ADR-062). */
  cost_pending?: boolean
  /** Цена последнего прихода за штуку или канистру. */
  last_purchase_price_tyiyn: number | null
  avg_cost_tyiyn?: number
  stock_value_tyiyn?: number
  /** Связанная строка Умага — только в ответе `GET /products/{id}` (SPEC-35 §3). */
  umag?: ProductUmag | null
  /** Остальные связанные строки Умага филиала (их бывает несколько). */
  umag_more?: ProductUmag[]
}

export interface Employee {
  id: string
  full_name: string
  is_cashier: boolean
  is_master: boolean
  active: boolean
  /** Телефон для кабинета в боте (SPEC-23); сервер отдаёт его только владельцу. */
  phone?: string
  /** PIN кассира задан (ADR-081); сам PIN сервер не отдаёт. */
  has_pin?: boolean
}

export interface Service {
  id: string
  name: string
  /** Цена клиенту — со своим маслом (ADR-096). */
  price_tyiyn: number
  /** Ставка мастеру за одну услугу. */
  master_fee_tyiyn: number
  /** «Бесплатно с нашим маслом»: с маслом в чеке услуга стоит 0 (ADR-096). */
  free_with_oil?: boolean
  /** Категория масла, с которым услуга бесплатна: его в чеке не меньше канистры (ADR-104). */
  oil_category_id?: string | null
  active: boolean
  /** Строка книжки (SPEC-34): что услуга меняет; «прочее» — без напоминания. */
  book_slot?: BookSlot
  /** Интервал, заданный владельцем; пусто — по строке книжки. */
  book_interval_km?: number | null
  book_interval_days?: number | null
  /** Интервал для кассы: заданный или по умолчанию строки книжки. */
  interval_km?: number | null
  interval_days?: number | null
}

export interface Supplier {
  id: string
  name: string
  phone: string
  comment: string
  active: boolean
}

export interface LabelSettings {
  width_mm: number
  height_mm: number
  show_name: boolean
  show_price: boolean
  show_article: boolean
  /** Бренд строкой над названием (SPEC-29 §6). */
  show_brand?: boolean
  /** Характеристики: вязкость и объём масла, тип фильтра, ёмкость и ток аккумулятора. */
  show_spec?: boolean
}

export interface ReceiptLine {
  line_no: number
  product_id: string
  product_name: string
  unit: 'piece' | 'ml'
  container_ml: number | null
  qty: number
  cost_tyiyn: number
  /** Цена продажи товара после этого прихода (SPEC-28 §1); null — приход до этой версии. */
  sale_price_tyiyn?: number | null
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
  /** Откуда оплачена: название кассы или `outside`; null — в долг или старая накладная (ADR-062). */
  paid_from?: string | null
  /** Накладная в долг: оплачено, осталось, статус и платежи (SPEC-27, ADR-073). */
  debt?: ReceiptDebt | null
}

export type InvoiceStatus = 'unpaid' | 'partial' | 'paid'

/** Платёж по одной накладной: сколько из него легло на неё. */
export interface InvoicePayment {
  ledger_id: string
  at: string
  user_name: string
  /** `payment` — оплата, `defect` — зачёт брака, `return` — возврат товара (SPEC-29), `adjust` — правка владельца. */
  kind: 'payment' | 'defect' | 'return' | 'adjust'
  /** Касса; `outside` — мимо касс (до ADR-079); пусто — зачёт, возврат или правка. */
  source: string
  amount_tyiyn: number
  payment_tyiyn: number
  reversed: boolean
  comment: string
}

export interface ReceiptDebt {
  total_tyiyn: number
  paid_tyiyn: number
  left_tyiyn: number
  status: InvoiceStatus
  payments: InvoicePayment[]
}

/** Долг поставщику: накладная в долг или правка владельца. */
export interface SupplierInvoice {
  ledger_id: string
  receipt_id: string | null
  number: number | null
  supplier_doc: string
  business_date: string
  created_at: string
  total_tyiyn: number
  paid_tyiyn: number
  left_tyiyn: number
  status: InvoiceStatus
  comment: string
}

export interface SupplierPayment {
  ledger_id: string
  at: string
  user_name: string
  kind: 'payment' | 'defect' | 'return' | 'adjust'
  amount_tyiyn: number
  source: string
  reversed: boolean
  comment: string
  applied: { ledger_id: string; receipt_id: string | null; number: number | null; amount_tyiyn: number; gross_tyiyn: number }[]
}

export interface SupplierInvoices {
  invoices: SupplierInvoice[]
  debt_tyiyn: number
  advance_tyiyn: number
  payments: SupplierPayment[]
}

/** Прошлая закупка товара: «в прошлый раз: 350 с, 12.10, Автоснаб». */
export interface LastReceipt {
  receipt_id: string
  number: number
  price_tyiyn: number
  created_at: string
  supplier_name: string | null
}

/** Строка списка товаров: суммы остатка считает сервер (SPEC-27). */
export interface ProductRow extends Product {
  code: string
  /** Залежалый товар (ADR-039): дней без продаж; нет — не залежался. */
  stale?: { days: number; sold_ever: boolean }
  /** Только владельцу. */
  purchase_price_tyiyn?: number | null
  stock_purchase_tyiyn?: number
  stock_sale_tyiyn: number
}

/** Отбор по складу на экране «Товары» — бывшие «Остатки» (SPEC-30 §11). */
export type StockChip = 'all' | 'low' | 'out' | 'stale' | 'review'

export interface ProductList {
  items: ProductRow[]
  totals: {
    count: number
    purchase_tyiyn?: number
    sale_tyiyn: number
    /** Сколько товаров в каждом отборе по складу — при тех же поиске и категории. */
    counts?: Record<StockChip, number>
    /** Срок залежалости, дней. */
    stale_days?: number
  }
}

export interface TopProduct {
  product_id: string
  name: string
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  category_id: string
  qty: number
  amount_tyiyn: number
  checks: number
  /** Валовая прибыль строк — только владельцу (SPEC-28 §4). */
  profit_tyiyn?: number
}

export interface TopProducts {
  from: string
  to: string
  items: TopProduct[]
}

export interface ReceiptListItem {
  /** `receipt` — от поставщика, `stock_in` — оприходование (ADR-086). */
  kind?: 'receipt' | 'stock_in'
  id: string
  number: number
  supplier_name: string | null
  supplier_doc: string
  total_tyiyn: number
  reversal_of: string | null
  reversed: boolean
  user_name: string
  created_at: string
  /** Взято в долг у поставщика по накладной; 0 — оплачено сразу. */
  on_debt_tyiyn?: number
  /** Осталось заплатить по накладной в долг (ADR-073). */
  debt_left_tyiyn?: number
  comment?: string
  /** Товары оприходования словами; у прихода — пусто. */
  goods?: string
}

export interface StockMismatch {
  product_id: string
  cached_qty: number
  moved_qty: number
  cached_value_tyiyn: number
  moved_value_tyiyn: number
}

export type SaleLineKind = 'piece' | 'container' | 'pour' | 'service' | 'weight'
export type PaymentMethod = 'cash' | 'card' | 'transfer' | 'invoice' | 'debt' | 'bonus' | 'battery'

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
  /** Строка возврата — удержание за замену: номер строки услуги исходного чека (ADR-104). */
  withheld_for?: number | null
  /** Категория масла: у масла — его, у услуги «бесплатно с нашим маслом» — её масла. */
  oil_category_id?: string | null
  /** Бесплатная по маслу услуга, ещё не удержанная: цена, которую удержит возврат масла. */
  withhold_price_tyiyn?: number | null
}

export interface Payment {
  method: PaymentMethod
  amount_tyiyn: number
  /** Аккумулятор в оплату: вес в граммах (у возврата — с минусом). */
  grams?: number | null
  /** Карта, перевод и «По счёту»: безналичный счёт, куда пришли деньги (ADR-076, ADR-095). */
  account_id?: string | null
  account_name?: string | null
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
  /** Начисление мастеру: сумма ставок услуг чека (ADR-096); у старых чеков — ставка за чек. */
  master_fee_tyiyn: number
  comment: string
  reversal_of: string | null
  /** Возврат без чека (ADR-105). */
  no_receipt?: boolean
  user_name: string
  created_at: string
  /** Учётный день чека: его печатают чек и документ о долге. */
  business_date?: string | null
  lines: SaleLine[]
  payments: Payment[]
  returns: { id: string; number: number; total_tyiyn: number }[]
  /** Масляная книжка чека «В сервис» для печати (SPEC-34 §4). */
  book?: SaleBook | null
}

/** Масляная книжка в печатном чеке: машина, пробег сейчас и следующие замены (SPEC-34 §4). */
export interface SaleBook {
  brand: string
  model: string
  plate: string
  mileage: number | null
  mileage_unit: 'km' | 'mi'
  lines: { label: string; next_mileage: number; unit: 'km' | 'mi'; next_date: string | null }[]
}

export interface SaleListItem {
  id: string
  number: number
  kind: 'sale' | 'return'
  sale_type: 'takeaway' | 'service'
  cashier_name: string
  master_name: string | null
  party_name: string | null
  party_phone?: string | null
  debt_tyiyn: number
  total_tyiyn: number
  reversal_of: string | null
  /** Возврат без чека (ADR-105). */
  no_receipt?: boolean
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
    invoice_tyiyn?: number
    debt_tyiyn: number
    bonus_tyiyn?: number
    battery_tyiyn?: number
  }
}

export const PAYMENT_LABELS: Record<PaymentMethod, string> = {
  cash: 'Наличные',
  // Карта и официальный QR банка — на «Счёт» с комиссией банка (SPEC-29 §25).
  card: 'Карта / QR банка',
  // Код прежний, чтобы не трогать проведённые чеки: QR на личный счёт владельца, без комиссии.
  transfer: 'QR владельцу',
  // Юрлицо перечислением по выставленному счёту, без комиссии банка (ADR-095).
  invoice: 'По счёту',
  debt: 'В долг',
  bonus: 'Баллы',
  battery: 'Аккумулятор',
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
  /** Имя и фамилия человека (ADR-083); у прежних записей — пусто. */
  first_name?: string
  last_name?: string
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
  /** Фирма не хочет сообщений о закупах (SPEC-32). */
  no_purchase_messages?: boolean
  /** Баланс в филиале запроса (SPEC-33): у клиента `balance_tyiyn` — общий по всем филиалам. */
  branch_balance_tyiyn?: number
  /** Долг клиента по филиалам — только ненулевые. */
  branch_balances?: BranchBalance[]
}

/** Долг клиента в одном филиале (SPEC-33 §2). */
export interface BranchBalance {
  branch_id: string
  name: string
  balance_tyiyn: number
}

export interface PartyContact {
  id: string
  full_name: string
  phone: string
  position: string
  inn: string
  active: boolean
  /** Руководитель фирмы: ему — сообщение о закупе (SPEC-32). */
  is_head?: boolean
}

export interface PartyVehicle {
  id: string
  plate: string
  brand: string
  model: string
  comment: string
  active: boolean
  /** Пробег в км или милях (ADR-084). */
  mileage_unit?: 'km' | 'mi'
}

/** Справочник марок и моделей (ADR-083). */
export interface CarMake {
  id: string
  name: string
  models: { id: string; name: string }[]
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
  /** Запись другого филиала (SPEC-33): его название; документ открывается только в нём. */
  branch_name?: string
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
  /** `register` — касса смены, `safe`/`other` — наличные без смены, `bank` — безналичный счёт. */
  kind: 'register' | 'bank' | 'safe' | 'other'
  owner_only: boolean
  /** Касса смены (у безналичных счетов — всегда false, ADR-077). */
  is_default: boolean
  /** Безналичный счёт по умолчанию для карты (ADR-077). */
  default_card?: boolean
  /** Безналичный счёт по умолчанию для перевода / QR (ADR-077). */
  default_transfer?: boolean
  /** Безналичный счёт по умолчанию для «По счёту» (ADR-095); ни у одного — счёт карты. */
  default_invoice?: boolean
  /** Ставка комиссии банка на сегодня, сотые доли процента (ADR-077). */
  fee_rate_bp?: number
  /** Отрицательный баланс разрешён (ADR-076; по умолчанию — да, ADR-077). */
  allow_negative: boolean
  /** В архиве — false. */
  active: boolean
  balance_tyiyn: number
}

/** Безнал чеков одного безналичного счёта (ADR-076). */
export interface BankSplit {
  account_id: string
  name: string
  card_tyiyn: number
  transfer_tyiyn: number
  invoice_tyiyn?: number
  fee_tyiyn: number
  total_tyiyn: number
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
  /** «По счёту» (ADR-095). */
  invoice_tyiyn?: number
  debt_tyiyn: number
  bonus_tyiyn?: number
  battery_tyiyn?: number
  bank_fee_tyiyn: number
  /** Безнал смены по безналичным счетам. */
  bank_split?: BankSplit[]
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
  /** Оклад за месяц и за смену за день (ADR-122). */
  salary_tyiyn?: number
  bonus_tyiyn?: number
  /** Удержания и недостача за день — с минусом. */
  withheld_tyiyn?: number
  /** Из выданного за день — авансом. */
  advance_tyiyn?: number
}

/** Вид выплаты (ADR-122); `general` — прежние выплаты без вида. */
export type PayoutPurpose = 'percent' | 'service_fee' | 'bonus' | 'salary' | 'advance'

/** Остатки сотрудника по видам — `GET /payroll/remains`. */
export interface PayoutRemains {
  employee_id: string
  full_name: string
  is_cashier: boolean
  is_master: boolean
  remains: {
    percent_tyiyn: number
    service_fee_tyiyn: number
    bonus_tyiyn: number
    /** Оклад минус авансы — с минусом, пока аванс не закрыт окладом. */
    salary_tyiyn: number
    /** Удержание, которое ещё не из чего вычесть (с минусом). */
    withheld_tyiyn: number
  }
  balance_tyiyn: number
  /** Аванс в счёт оклада или оплаты за услуги. */
  advance_to: 'salary' | 'service_fee'
}

// ---------- Прибыль и сводка (SPEC-08) ----------

export interface ProfitTotals {
  goods_tyiyn: number
  services_tyiyn: number
  cost_tyiyn: number
  /** В том числе переоценка остатков (ревизия, сторно закупок). */
  revaluation_tyiyn?: number
  gross_tyiyn: number
  payroll_tyiyn: number
  /** В том числе оклады за месяц — расшифровка оплаты труда. */
  salary_tyiyn?: number
  bank_fee_tyiyn: number
  expenses_tyiyn: number
  bonus_tyiyn: number
  /** Недостача (минус) или излишек (плюс) кассы по пересчётам смен (ADR-065). */
  cash_diff_tyiyn?: number
  /** «Списание брака» — убыток (SPEC-25). */
  defect_writeoff_tyiyn?: number
  /** «Брак: разница возмещения»: минус — убыток, плюс — доход (SPEC-25). */
  defect_diff_tyiyn?: number
  /** «Возврат поставщику: разница»: минус — убыток, плюс — доход (SPEC-29 §9). */
  return_diff_tyiyn?: number
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
    cash_diff_tyiyn?: number
    defect_writeoff_tyiyn?: number
    defect_diff_tyiyn?: number
    return_diff_tyiyn?: number
    net_tyiyn: number
  }[]
  articles: { name: string; amount_tyiyn: number }[]
  warnings: string[]
  staff?: StaffPay[]
  hours?: { hour: number; revenue_tyiyn: number; gross_tyiyn: number; sales_count: number }[]
}

export interface Dashboard {
  /** Последний день периода. */
  date: string
  from: string
  to: string
  today: string
  totals: ProfitTotals
  returns_tyiyn: number
  average_check_tyiyn: number
  /** Остатки касс — всегда на сейчас. */
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
  /** Баллы у всех клиентов на сейчас (SPEC-30 §12). */
  points?: PointsTotal
  staff: StaffPay[]
  /** «Сколько по карте ушло»: безнал чеков за период (SPEC-27 §6). */
  noncash: Noncash
  /** Один день: к предыдущему дню и тому же дню неделю назад; сегодня — к этому часу. */
  compare: {
    today: DayPart
    yesterday: DayPart
    week_ago: DayPart
    /** Чистая прибыль предыдущего дня за весь день. */
    yesterday_net_tyiyn: number
    until_now: boolean
  } | null
  /** Период: те же числа за предыдущий период такой же длины. */
  previous: { from: string; to: string; revenue_tyiyn: number; gross_tyiyn: number; net_tyiyn: number; sales_count: number } | null
}

/** Баллы у клиентов вместе: 1 балл = 1 сом, в тыйынах (SPEC-30 §12). */
export interface PointsTotal {
  clients: number
  points_tyiyn: number
}

export interface Noncash {
  card_tyiyn: number
  transfer_tyiyn: number
  invoice_tyiyn?: number
  total_tyiyn: number
  /** По безналичным счетам (ADR-076). */
  by_account?: BankSplit[]
}

// ---------- Деньги и отчёты (SPEC-27 §6) ----------

export interface KindRow {
  kind: string
  in_tyiyn: number
  out_tyiyn: number
}

export interface BankIncome {
  sales_tyiyn: number
  repayments_tyiyn: number
  total_tyiyn: number
}

/** Экран «Счёт»: только владельцу. */
export interface BankReport {
  account_id: string
  name: string
  from: string
  to: string
  balance_tyiyn: number
  opening_tyiyn: number
  closing_tyiyn: number
  card: BankIncome
  transfer: BankIncome
  invoice?: BankIncome
  fee_tyiyn: number
  other_in_tyiyn: number
  outgoing_tyiyn: number
  kinds: KindRow[]
  days: { date: string; card_tyiyn: number; transfer_tyiyn: number; invoice_tyiyn?: number; fee_tyiyn: number; other_tyiyn: number; net_tyiyn: number }[]
  movements: CashMovement[]
  movements_count: number
}

export interface MethodRow {
  method: string
  sales_tyiyn: number
  returns_tyiyn: number
  net_tyiyn: number
  fee_tyiyn: number
}

export interface AccountFlow {
  account_id: string
  name: string
  kind: string
  opening_tyiyn: number
  in_tyiyn: number
  out_tyiyn: number
  closing_tyiyn: number
  kinds: KindRow[]
}

export interface ReceiptSoldItem {
  product_id: string
  name: string
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  qty: number
  sold_qty: number
  left_qty: number
  cost_tyiyn: number
  left_cost_tyiyn: number
  sold_cost_tyiyn: number
  revenue_tyiyn: number
  earned_tyiyn: number
}

export interface ReceiptSold {
  receipt_id: string
  number: number
  created_at: string
  supplier_name: string | null
  cost_tyiyn: number
  sold_cost_tyiyn: number
  left_cost_tyiyn: number
  revenue_tyiyn: number
  earned_tyiyn: number
  items: ReceiptSoldItem[]
}

/** «Куда сколько поступило»: администратору — без сейфа владельца и без накладных. */
export interface IncomeReport {
  from: string
  to: string
  methods: MethodRow[]
  revenue_tyiyn: number
  goods_tyiyn: number
  services_tyiyn: number
  sales_tyiyn: number
  returns_tyiyn: number
  fee_tyiyn: number
  accounts: AccountFlow[]
  categories: { name: string; revenue_tyiyn: number }[]
  products: TopProduct[]
  receipts: ReceiptSold[] | null
}

/** Строка экрана «Долги»: карточка со сроком и последней оплатой. */
export interface DebtRow extends Party {
  due_date: string | null
  overdue_days: number
  last_payment_at: string | null
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

/** Решение владельца по браку (SPEC-25). Суммы — только владельцу. */
export interface DefectResolution {
  id: string
  kind: 'write_off' | 'replace' | 'credit' | 'cash'
  supplier_id: string | null
  supplier_name: string | null
  replace_qty: number | null
  refund_tyiyn: number | null
  account_name: string | null
  writeoff_tyiyn: number | null
  diff_tyiyn: number | null
  comment: string
  user_name: string
  created_at: string
}

export interface Defect {
  id: string
  number: number
  product_id: string
  product_name: string
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  qty: number
  /** Стоимость по средней на момент отметки — только владельцу. */
  cost_tyiyn: number | null
  reason: string
  supplier_id: string | null
  supplier_name: string | null
  user_name: string
  created_at: string
  status: 'pending' | 'written_off' | 'returned' | 'cancelled'
  resolved_at: string | null
  resolution: DefectResolution | null
}

/** Строка «Поставки» в карточке поставщика (SPEC-26). */
export interface DeliveryRow {
  product_id: string
  name: string
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  receipts: number
  delivered_qty: number
  returned_qty: number
  amount_tyiyn: number
  last_price_tyiyn: number | null
  last_at: string | null
  stock_qty: number
  estimate_qty: number
  /** Себестоимость оценки — только владельцу. */
  estimate_value_tyiyn: number | null
}

export interface SupplierDeliveries {
  supplier_id: string
  supplier_name: string
  from: string | null
  to: string | null
  rows: DeliveryRow[]
  amount_tyiyn: number
  balance_tyiyn: number
  pending_defects: number
  pending_defects_tyiyn: number | null
}

export interface DeliveryReceipt {
  receipt_id: string
  number: number
  created_at: string
  supplier_doc: string
  qty: number
  cost_tyiyn: number
  price_tyiyn: number | null
  reversed: boolean
}

/** Неоплаченный долг клиента в окне приёма долга (SPEC-28 §3). */
export interface OpenDebt {
  ledger_id: string
  business_date: string
  created_at: string
  kind: 'debt' | 'adjust'
  sale_id: string | null
  sale_number: number | null
  comment: string
  amount_tyiyn: number
  left_tyiyn: number
  overdue: boolean
  /** Филиал долга, когда долг клиента есть в нескольких филиалах (SPEC-33). */
  branch_name?: string
  other_branch?: boolean
}

export interface OpenDebts {
  balance_tyiyn: number
  overdue_tyiyn: number
  due_date: string | null
  overdue_days: number
  items: OpenDebt[]
}

/** Фото накладной (SPEC-29 §3, ADR-080). */
export interface ReceiptFile {
  id: string
  receipt_id: string
  name: string
  content_type: string
  size_bytes: number
  user_name: string
  created_at: string
  /** Скрыт владельцем; администратор скрытых не видит. */
  hidden_at: string | null
  hidden_by: string | null
  hidden_reason: string | null
}

/** Оприходование без поставщика (SPEC-29 §8). */
export interface StockInLine {
  line_no: number
  product_id: string
  product_name: string
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  /** В единицах учёта; у сторно — с минусом. */
  qty: number
  /** Цена закупки за штуку, канистру или килограмм. */
  price_tyiyn: number
  cost_tyiyn: number
  /** Цена продажи после оприходования; пусто — документ до ADR-086 или сторно. */
  sale_price_tyiyn: number | null
}

/** Оприходование без поставщика — вариант прихода, строками (SPEC-29 §8, ADR-086). */
export interface StockIn {
  id: string
  number: number
  /** Итог документа; у сторно — с минусом. */
  cost_tyiyn: number
  comment: string
  reversal_of: string | null
  reversed_by: string | null
  user_name: string
  created_at: string
  lines: StockInLine[]
}

/** Возврат товара поставщику, не брак (SPEC-29 §9). */
export interface SupplierReturn {
  id: string
  number: number
  supplier_id: string
  supplier_name: string
  settle: 'debt' | 'money'
  account_id: string | null
  account_name: string | null
  amount_tyiyn: number
  /** Себестоимость и разница — только владельцу. */
  cost_tyiyn?: number
  diff_tyiyn?: number
  comment: string
  reversal_of: string | null
  reversed_by: string | null
  user_name: string
  created_at: string
  lines: {
    line_no: number
    product_id: string
    product_name: string
    unit: 'piece' | 'ml' | 'g'
    container_ml: number | null
    qty: number
    amount_tyiyn: number
    cost_tyiyn?: number
  }[]
}

/** Код строки Умага, который уже у другого товара: не перенесён, строка помечена (SPEC-35 §1). */
export interface UmagConflict {
  code: string
  product_id: string
  product_name: string
}

/** Позиция списка старой программы (SPEC-20) — строка старой базы Умаг (SPEC-35). */
export interface OldCatalogHit {
  id: string
  name: string
  barcode: string
  article: string
  /** Закупка в Умаге — владельцу и правам «Приход», «Ревизия», «Товары и штрихкоды»; остальным — null (ADR-113). */
  cost_tyiyn: number | null
  price_tyiyn: number
  unit: string
  supplier: string
  category: string
  /** Совпал штрихкод. */
  exact: boolean
  /** Товар учёта с этим штрихкодом уже заведён. */
  linked_product_id: string | null
  /** Связанный товар учёта (SPEC-35 §1); у прежнего ответа сервера — нет. */
  product_id?: string | null
  product_name?: string | null
  /** Все коды строки: штрихкод, артикул-код, доп. коды. */
  codes?: string[]
  /** Коды, занятые другим товаром: при связывании не перенесены. */
  conflicts?: UmagConflict[]
}

/** Строка Умага связана с товаром: товар, перенесённые коды и не перенесённые (SPEC-35). */
export interface UmagLinkResult {
  product: Product
  copied: string[]
  conflicts: UmagConflict[]
}

/** Сводка списка Умага филиала (SPEC-20, SPEC-35 §3). */
export interface UmagStats {
  rows: number
  imported_at: string | null
  linked?: number
  unlinked?: number
  conflicts?: number
}

/** Итог «Связать автоматически»: всего, по коду, по названию. */
export interface UmagAutoLink {
  linked: number
  by_code: number
  by_name: number
}

/** Похожая пара «товар без связи ↔ строка Умага без связи»; сходство названий 0…1 — для показа. */
export interface UmagPair {
  product: Pick<Product, 'id' | 'name' | 'brand' | 'article' | 'sale_price_tyiyn'>
  umag: OldCatalogHit
  similarity: number
}

/** Строка Умага, связанная с товаром, — в карточке товара (SPEC-35 §3). */
export interface ProductUmag {
  id: string
  name: string
  barcode: string
  codes: string[]
  /** Коды меры (строка «литр», «кг», «метр»: 276…, PLU) — товару не переходят (ADR-113). */
  measure_codes?: string[]
  unit?: string
  price_tyiyn: number
  /** Закупка в Умаге — только владельцу (ADR-008). */
  cost_tyiyn?: number | null
}

// ---------- Замены по видам (SPEC-30 §2) ----------

/** Масло в чеках вида замены: канистрами, розливом (мл) и всего (мл). */
export interface ServiceOil {
  product_id: string
  name: string
  container_ml: number | null
  canisters: number
  pour_ml: number
  total_ml: number
}

/** Вид замены за период; без `service_id` — чеки «в сервис» без вида замены (ставка за чек). */
export interface ServiceKind {
  service_id: string | null
  name: string
  count: number
  checks: number
  clients_tyiyn: number
  masters_tyiyn: number
  oils: ServiceOil[]
}

export interface ServiceKinds {
  from: string
  to: string
  kinds: ServiceKind[]
}
