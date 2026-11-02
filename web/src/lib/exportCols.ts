// Столбцы выгрузки в Excel для таблиц, где важны права (задача 003 Н13, ADR-159): закупка, стоимость
// остатка и прибыль — только владельцу (I13). Условие повторяет столбцы экрана; тесты —
// exportCols.test.ts. Простые таблицы описывают столбцы рядом с собой.
import { colsIf, exportTitle, tableSheet, unitName, xl, type ExportCol } from './exportTable'
import { actLines, actVerdict, type Reconciliation } from './debtDocs'
import { formatDate, formatSom } from './format'
import { oilAmountText } from './serviceSale'
import { groupQty, pricePer, rowViscosity, type QtyTotal, type StockGroup } from './stockSheet'
import type { DebtRow, InvoicePayment, ProductRow, ServiceKind, TopProduct } from './types'
import { paymentSource } from './supplierPay'
import type { Cell, Sheet, SheetRow } from './xlsxWrite'

// ---------- Товары ----------

/** Отметки строки товара — отдельным столбцом (на экране — значки). */
export function productMarks(p: ProductRow): string {
  const out = p.stock_qty <= 0
  const low = !out && p.stock_qty < p.min_stock
  return [
    out && !p.archived && 'нет',
    low && 'ниже минимума',
    p.needs_review && 'проверить',
    p.cost_pending && 'без цены закупки',
    p.stale && `${p.stale.sold_ever ? 'не продавался' : 'ни разу не продан'} ${p.stale.days} дн.`,
    p.archived && 'архив',
  ]
    .filter(Boolean)
    .join(', ')
}

/** «Товары»: как в таблице экрана; закупка и сумма по закупке — только владельцу (ADR-008). */
export function productCols(o: { owner: boolean; category: (id: string) => string; branch?: (p: ProductRow) => string }): ExportCol<ProductRow>[] {
  return [
    ...colsIf<ProductRow>(Boolean(o.branch), [{ label: 'Филиал', value: (p) => o.branch?.(p) ?? '' }]),
    { label: 'Название', value: (p) => p.name },
    { label: 'Бренд', value: (p) => p.brand },
    { label: 'Категория', value: (p) => o.category(p.category_id) },
    { label: 'Код', value: (p) => p.code },
    { label: 'Штрихкоды', value: (p) => p.barcodes.join(', ') },
    { label: 'Остаток', value: (p) => xl.units(p.stock_qty, p.unit) },
    { label: 'Ед.', value: (p) => unitName(p.unit) },
    { label: 'Минимум', value: (p) => (p.min_stock > 0 ? xl.units(p.min_stock, p.unit) : null) },
    ...colsIf<ProductRow>(o.owner, [{ label: 'Закупка', value: (p) => xl.maybeMoney(p.purchase_price_tyiyn) }]),
    { label: 'Продажа', value: (p) => xl.money(p.sale_price_tyiyn) },
    { label: 'Цена за', value: (p) => pricePer(p) || 'шт' },
    ...colsIf<ProductRow>(o.owner, [{ label: 'Сумма по закупке', value: (p) => xl.money(p.stock_purchase_tyiyn ?? 0) }]),
    { label: 'Сумма по продаже', value: (p) => xl.money(p.stock_sale_tyiyn) },
    { label: 'Отметки', value: productMarks },
  ]
}

// ---------- Остатки ----------

/** Количество подытога: одной единицей — числом, смесь штук, литров, кг и м — текстом. */
function groupQtyCell(t: QtyTotal): Cell {
  const mm = t.mm ?? 0
  const kinds = [t.pieces, t.ml, t.grams, mm].filter((x) => x !== 0).length
  if (kinds > 1) return groupQty(t)
  if (t.ml !== 0) return xl.liters(t.ml)
  if (t.grams !== 0) return { qty: t.grams, scale: 3 }
  if (mm !== 0) return { qty: mm, scale: 3 }
  return xl.int(t.pieces)
}

/**
 * «Остатки»: категория у каждой строки, подытоги — строками «Итого по категории»; закупка — владельцу.
 * Суммы по продаже в подытогах и итоге — по наличию поля (сервер отдаёт только владельцу, ADR-167).
 */
export function stockSheet(o: {
  groups: readonly StockGroup[]
  total: { count: number; sale_tyiyn?: number; purchase_tyiyn?: number }
  owner: boolean
  filter?: (string | false | null | undefined)[]
  now?: Date
}): Sheet {
  const owner = o.owner
  const columns = [
    'Категория',
    'Товар',
    'Вязкость',
    'Остаток',
    'Ед.',
    'Минимум',
    'Цена',
    'Цена за',
    'Сумма (продажа)',
    ...(owner ? ['Сумма (закупка)'] : []),
    'Отметка',
  ].map((label) => ({ label }))
  const rows: SheetRow[] = []
  const all = { pieces: 0, ml: 0, grams: 0, mm: 0, container_ml: null as number | null }
  for (const g of o.groups) {
    for (const r of g.rows) {
      rows.push({
        cells: [
          g.total.name,
          r.name,
          rowViscosity(r)?.text ?? '',
          xl.units(r.stock_qty, r.unit),
          unitName(r.unit),
          r.min_stock > 0 ? xl.units(r.min_stock, r.unit) : null,
          xl.money(r.sale_price_tyiyn),
          pricePer(r) || 'шт',
          xl.money(r.stock_sale_tyiyn),
          ...(owner ? [xl.maybeMoney(r.stock_purchase_tyiyn)] : []),
          r.stock_qty < 0 ? 'проверить' : '',
        ],
      })
    }
    const t = g.total
    all.pieces += t.pieces
    all.ml += t.ml
    all.grams += t.grams
    all.mm += t.mm ?? 0
    rows.push({
      cells: [g.total.name, `Итого по категории: ${t.count} поз.`, null, groupQtyCell(t), null, null, null, null, xl.maybeMoney(t.sale_tyiyn), ...(owner ? [xl.maybeMoney(t.purchase_tyiyn)] : [])],
      bold: true,
    })
  }
  rows.push({
    cells: ['', `Итого по складу: ${o.total.count} поз.`, null, groupQtyCell(all), null, null, null, null, xl.maybeMoney(o.total.sale_tyiyn), ...(owner ? [xl.maybeMoney(o.total.purchase_tyiyn)] : [])],
    bold: true,
  })
  return { name: 'Остатки', columns, rows, title: exportTitle('Остатки на складе', o.filter, o.now) }
}

// ---------- Продаваемые товары ----------

/** «Продаваемые товары»: прибыль — только если сервер её отдал (владельцу, SPEC-28 §4). */
export function topProductCols(showProfit: boolean): ExportCol<TopProduct & { rank: number }>[] {
  return [
    { label: '№', value: (r) => xl.int(r.rank) },
    { label: 'Товар', value: (r) => r.name },
    { label: 'Продано', value: (r) => xl.units(r.qty, r.unit) },
    { label: 'Ед.', value: (r) => unitName(r.unit) },
    { label: 'Выручка', value: (r) => xl.money(r.amount_tyiyn) },
    ...colsIf<TopProduct & { rank: number }>(showProfit, [{ label: 'Прибыль', value: (r) => xl.money(r.profit_tyiyn ?? 0) }]),
    { label: 'Чеков', value: (r) => xl.int(r.checks) },
  ]
}

export const SERVICE_KIND_COLS: ExportCol<ServiceKind>[] = [
  { label: 'Вид замены', value: (k) => k.name },
  { label: 'Раз', value: (k) => xl.int(k.count) },
  { label: 'Заплатили клиенты', value: (k) => xl.money(k.clients_tyiyn) },
  { label: 'Мастерам', value: (k) => xl.money(k.masters_tyiyn) },
  { label: 'Масла, л', value: (k) => xl.liters(k.oils.reduce((a, x) => a + x.total_ml, 0)) },
  { label: 'Масла в этих чеках', value: (k) => (k.oils.length === 0 ? 'масло клиента' : k.oils.map((x) => `${x.name}: ${oilAmountText(x)}`).join('; ')) },
]

// ---------- Долги ----------

/** Долги: контрагент, реквизиты текстом, долг и просрочка числами. */
export const DEBT_COLS: ExportCol<DebtRow>[] = [
  { label: 'Контрагент', value: (p) => p.name },
  { label: 'Вид', value: (p) => (p.kind === 'company' ? 'Юрлицо' : 'Физлицо') },
  { label: 'ИНН / ПИН', value: (p) => p.inn },
  { label: 'Телефон', value: (p) => p.phone },
  { label: 'Долг', value: (p) => xl.money(Math.abs(p.balance_tyiyn)) },
  { label: 'Просрочено', value: (p) => (p.overdue_tyiyn > 0 ? xl.money(p.overdue_tyiyn) : null) },
  { label: 'Просрочка, дн.', value: (p) => (p.overdue_tyiyn > 0 ? xl.int(p.overdue_days) : null) },
  { label: 'Срок оплаты', value: (p) => xl.date(p.due_date) },
  { label: 'Последняя оплата', value: (p) => xl.dateTime(p.last_payment_at) },
  { label: 'Лимит', value: (p) => xl.maybeMoney(p.credit_limit_tyiyn) },
  // Долг клиента по филиалам (SPEC-33 §2) — подписью, как под суммой на экране.
  { label: 'По филиалам', value: (p) => ((p.branch_balances ?? []).length < 2 ? '' : (p.branch_balances ?? []).map((b) => `${b.name}: ${formatSom(b.balance_tyiyn)}`).join('; ')) },
]

/** Лист долгов одной стороны с итогом, как над таблицей экрана. */
export function debtSheet(name: string, rows: DebtRow[], filter: string[] = [], now?: Date): Sheet {
  const total = rows.reduce((a, p) => a + Math.abs(p.balance_tyiyn), 0)
  const overdue = rows.reduce((a, p) => a + p.overdue_tyiyn, 0)
  return tableSheet(name, DEBT_COLS, rows, {
    title: exportTitle(name, filter, now),
    total: ['Итого', null, null, null, xl.money(total), xl.money(overdue)],
  })
}

// ---------- Акт сверки ----------

/** Акт сверки листом: сальдо, строки, обороты и сальдо на конец — те же `actLines`, что на экране и в печати. */
export function reconciliationSheet(act: Reconciliation, us: string, now?: Date): Sheet {
  const rows: SheetRow[] = actLines(act).map((l) => ({
    cells: [l.kind === 'row' ? xl.date(l.date) : null, l.document, l.comment, xl.money(l.debit_tyiyn), xl.money(l.credit_tyiyn)],
    bold: l.kind !== 'row',
  }))
  return {
    name: 'Акт сверки',
    columns: ['Дата', 'Документ', 'Комментарий', 'Дебет', 'Кредит'].map((label) => ({ label })),
    rows,
    title: [
      ...exportTitle(`Акт сверки взаимных расчётов: ${act.name}${act.inn ? `, ИНН ${act.inn}` : ''}`, [`с ${formatDate(act.from)} по ${formatDate(act.to)}`], now),
    ],
    footer: [actVerdict(act, us), `Дебет — в пользу точки, кредит — в пользу ${act.role === 'supplier' ? 'поставщика' : 'клиента'}.`],
    filter: false,
  }
}

// ---------- Мелкие таблицы (задача 003 Н13, хвост) ----------

/** «Что покупает чаще всего» в карточке клиента — как на экране: товар, сколько, чеков. */
export const FREQUENT_COLS: ExportCol<TopProduct>[] = [
  { label: 'Товар', value: (r) => r.name },
  { label: 'Сколько', value: (r) => xl.units(r.qty, r.unit) },
  { label: 'Ед.', value: (r) => unitName(r.unit) },
  { label: 'Чеков', value: (r) => xl.int(r.checks) },
]

/** Платежи по накладной: сколько легло на неё и весь платёж; сторно — отдельным столбцом. */
export const INVOICE_PAYMENT_COLS: ExportCol<InvoicePayment>[] = [
  { label: 'Когда', value: (p) => xl.dateTime(p.at) },
  { label: 'Кто', value: (p) => p.user_name },
  { label: 'На накладную', value: (p) => xl.money(p.amount_tyiyn) },
  { label: 'Весь платёж', value: (p) => xl.money(p.payment_tyiyn) },
  { label: 'Откуда', value: (p) => paymentSource(p) },
  { label: 'Комментарий', value: (p) => p.comment },
  { label: 'Отметка', value: (p) => (p.reversed ? 'сторно' : '') },
]
