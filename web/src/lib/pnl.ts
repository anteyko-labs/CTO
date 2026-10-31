// «Прибыль и убытки» (задача 003 Н12, SPEC-08, ADR-{B10.1}): ответ `GET /reports/profit?detail=1`
// → дерево строк для экрана и листы для выгрузки в Excel. Только раскладка: итоги — серверные
// `totals` (`gross_tyiyn`, `net_tyiyn`), раскрытие каждой строки складывается в её сумму.
import { shiftDate } from './format'
import { PURPOSE_LABELS } from './payouts'
import type { ProfitDetail, ProfitReport } from './types'
import { periodText, xl } from './exportTable'
import type { Cell, Sheet, SheetRow } from './xlsxWrite'

// ---------- Период ----------

export type QuickPeriod = 'today' | 'yesterday' | 'week' | 'month' | 'last_month'

export const QUICK_PERIODS: [QuickPeriod, string][] = [
  ['today', 'Сегодня'],
  ['yesterday', 'Вчера'],
  ['week', '7 дней'],
  ['month', 'Этот месяц'],
  ['last_month', 'Прошлый месяц'],
]

/** Быстрый период по сегодняшней дате Бишкека: «Этот месяц» — с 1-го по сегодня. */
export function quickPeriod(kind: QuickPeriod, today: string): { from: string; to: string } {
  const first = `${today.slice(0, 8)}01`
  switch (kind) {
    case 'today':
      return { from: today, to: today }
    case 'yesterday': {
      const d = shiftDate(today, -1)
      return { from: d, to: d }
    }
    case 'week':
      return { from: shiftDate(today, -6), to: today }
    case 'month':
      return { from: first, to: today }
    case 'last_month': {
      const end = shiftDate(first, -1)
      return { from: `${end.slice(0, 8)}01`, to: end }
    }
  }
}

/** «С 1 октября 2026 по 31 октября 2026» — подпись периода, как у заказчика. */
export function periodTitle(from: string, to: string): string {
  const months = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря']
  const day = (iso: string) => `${Number(iso.slice(8, 10))} ${months[Number(iso.slice(5, 7)) - 1]} ${iso.slice(0, 4)}`
  return from === to ? `За ${day(from)}` : `С ${day(from)} по ${day(to)}`
}

// ---------- Дерево ----------

export interface PnlNode {
  /** Ключ строки: раскрытие на экране, testid в проверках. */
  id: string
  label: string
  /** Сумма строки в тыйынах; `null` — справочная строка без суммы. */
  amount: number | null
  /** Итоговая строка (валовая, чистая) — жирнее и без раскрытия. */
  total?: boolean
  /** Справочно: в сумму родителя не входит (возвраты, выдано, «к выплате»). */
  info?: boolean
  /** Строка-документ: дата и подробности. */
  doc?: {
    date?: string
    payee?: string
    account?: string | null
    user?: string | null
    comment?: string
    branch?: string
    /** «сторно», «сторнирован». */
    mark?: string
  }
  /** Сторно и исправления: строка серая и скрыта, пока не включён показ. */
  reversal?: boolean
  children?: PnlNode[]
  /** Сколько строк сторно скрыто в раскрытии. */
  hidden?: number
}

const KIND_LABELS: Record<string, string> = {
  revenue_percent: '% с чеков',
  service_fee: 'Мастеру за работы',
  shift_fee: 'За смену',
  monthly_salary: 'Оклад',
  bonus: 'Бонус / доп. доход',
  penalty: 'Удержание',
  shortage: 'Удержание недостачи',
}

export const kindLabel = (k: string): string => KIND_LABELS[k] ?? k
const purposeLabel = (p: string): string => PURPOSE_LABELS[p] ?? p

const sumOf = <T>(rows: T[], pick: (r: T) => number): number => rows.reduce((a, r) => a + pick(r), 0)

function groupBy<T>(rows: T[], key: (r: T) => string): [string, T[]][] {
  const m = new Map<string, T[]>()
  for (const r of rows) {
    const k = key(r)
    const list = m.get(k)
    if (list) list.push(r)
    else m.set(k, [r])
  }
  return [...m.entries()]
}

const EMPTY_DETAIL: ProfitDetail = {
  products: [],
  services: [],
  returns_tyiyn: 0,
  revision_tyiyn: 0,
  revaluation_only_tyiyn: 0,
  revisions: [],
  revaluations: [],
  expenses: [],
  payouts: [],
  fees: [],
  cash_diffs: [],
  defects: [],
  supplier_returns: [],
  not_in_profit: [],
}

/** Отметка сторно у строки-документа. */
function mark(r: { reversal_of?: string | null; reversed?: boolean }): string | undefined {
  if (r.reversal_of) return 'сторно'
  if (r.reversed) return 'сторнирован'
  return undefined
}

/** Строки сторно и их исходные — пары гасятся, поэтому по умолчанию скрыты. */
function withReversals(nodes: PnlNode[], show: boolean): { children: PnlNode[]; hidden: number } {
  const hidden = nodes.filter((n) => n.reversal).length
  return { children: show ? nodes : nodes.filter((n) => !n.reversal), hidden: show ? 0 : hidden }
}

/**
 * Дерево «Прибыли и убытков» по образцу заказчика: Выручка → Себестоимость → Валовая →
 * Операционные расходы → Списание и брак → Чистая прибыль. `showReversals` — показывать пары
 * «расход + сторно» (их суммы гасятся в статье, итоги от показа не меняются).
 */
export function pnlTree(r: ProfitReport, showReversals = false): PnlNode[] {
  const t = r.totals
  const d = r.detail ?? EMPTY_DETAIL
  const staff = r.staff ?? []
  const revaluation = t.revaluation_tyiyn ?? 0
  const cashDiff = t.cash_diff_tyiyn ?? 0
  const writeoff = t.defect_writeoff_tyiyn ?? 0
  const defectDiff = t.defect_diff_tyiyn ?? 0
  const returnDiff = t.return_diff_tyiyn ?? 0
  const branchOf = (b?: string) => (r.all_branches ? b : undefined)

  // 1. Выручка: товары по категориям и товарам, работы по услугам.
  const categories = groupBy(d.products, (p) => p.category).map(([name, items]) => ({ name, items }))
  const goods: PnlNode = {
    id: 'revenue.goods',
    label: 'Товары',
    amount: t.goods_tyiyn,
    children: categories.map((c) => ({
      id: `revenue.goods.${c.name}`,
      label: c.name,
      amount: sumOf(c.items, (p) => p.revenue_tyiyn),
      children: c.items.map((p) => ({
        id: `revenue.product.${p.product_id ?? p.name}`,
        label: `${p.name} — ${unitsText(p.units, p.unit)}`,
        amount: p.revenue_tyiyn,
      })),
    })),
  }
  const works: PnlNode = {
    id: 'revenue.services',
    label: 'Работы',
    amount: t.services_tyiyn,
    children: d.services.map((s) => ({
      id: `revenue.service.${s.service_id ?? s.name}`,
      label: `${s.name} — ${s.count} шт.`,
      amount: s.revenue_tyiyn,
    })),
  }
  const revenue: PnlNode = {
    id: 'revenue',
    label: 'Выручка',
    amount: t.goods_tyiyn + t.services_tyiyn,
    children: [
      goods,
      works,
      ...(d.returns_tyiyn ? [{ id: 'revenue.returns', label: 'в т. ч. возвраты покупателей (уже вычтены)', amount: d.returns_tyiyn, info: true }] : []),
    ],
  }

  // 2. Себестоимость: проданное, ревизия, переоценка.
  const sold: PnlNode = {
    id: 'cost.sold',
    label: 'Себестоимость проданного',
    amount: t.cost_tyiyn - revaluation,
    children: categories.map((c) => ({
      id: `cost.sold.${c.name}`,
      label: c.name,
      amount: sumOf(c.items, (p) => p.cost_tyiyn),
      children: c.items.map((p) => ({
        id: `cost.product.${p.product_id ?? p.name}`,
        label: `${p.name} — ${unitsText(p.units, p.unit)}`,
        amount: p.cost_tyiyn,
      })),
    })),
  }
  const cost: PnlNode = {
    id: 'cost',
    label: 'Себестоимость',
    amount: t.cost_tyiyn,
    children: [
      sold,
      ...(d.revision_tyiyn
        ? [
            {
              id: 'cost.revision',
              label: 'Ревизия: недостача / излишек',
              amount: d.revision_tyiyn,
              children: d.revisions.map((x) => ({
                id: `cost.revision.${x.id}`,
                label: x.number ? `Ревизия № ${x.number}` : 'Ревизия',
                amount: x.amount_tyiyn,
                doc: { date: x.date, user: x.user_name, branch: branchOf(x.branch) },
              })),
            },
          ]
        : []),
      ...(d.revaluation_only_tyiyn
        ? [
            {
              id: 'cost.revaluation',
              label: 'Переоценка остатков',
              amount: d.revaluation_only_tyiyn,
              children: d.revaluations.map((x) => ({ id: `cost.revaluation.${x.id}`, label: x.name, amount: x.amount_tyiyn, doc: { date: x.date } })),
            },
          ]
        : []),
    ],
  }

  // 4. Операционные расходы: статьи, зарплата, комиссия, баллы, касса.
  const articles: PnlNode[] = groupBy(d.expenses, (e) => e.article).map(([name, rows]) => {
    const docs: PnlNode[] = rows.map((e) => ({
      id: `opex.expense.${e.id}`,
      label: e.payee || e.comment || `Расход № ${e.number}`,
      amount: e.amount_tyiyn,
      reversal: Boolean(e.reversal_of || e.reversed),
      doc: { date: e.date, payee: e.payee, account: e.account_name, user: e.user_name, comment: e.comment, branch: branchOf(e.branch), mark: mark(e) },
    }))
    return { id: `opex.article.${name}`, label: name, amount: sumOf(rows, (e) => e.amount_tyiyn), ...withReversals(docs, showReversals) }
  })
  const payoutsBy = groupBy(d.payouts, (p) => p.employee_id)
  const employees = [...new Set([...staff.map((s) => s.employee_id), ...payoutsBy.map(([id]) => id)])]
  const salaryNodes: PnlNode[] = employees.map((id) => {
    const s = staff.find((x) => x.employee_id === id)
    const paid = payoutsBy.find(([k]) => k === id)?.[1] ?? []
    const name = s?.name ?? paid[0]?.employee_name ?? 'Работник'
    const docs: PnlNode[] = paid.map((p) => ({
      id: `opex.payout.${p.id}`,
      label: purposeLabel(p.purpose),
      amount: p.amount_tyiyn,
      reversal: Boolean(p.reversal_of || p.reversed),
      doc: { date: p.date, account: p.account_name, user: p.user_name, comment: p.comment, branch: branchOf(p.branch), mark: mark(p) },
    }))
    const shown = withReversals(docs, showReversals)
    return {
      id: `opex.staff.${id}`,
      label: name,
      amount: s?.total_tyiyn ?? 0,
      children: [
        ...(s?.items ?? []).map((i) => ({ id: `opex.staff.${id}.${i.kind}`, label: kindLabel(i.kind), amount: i.amount_tyiyn })),
        ...(paid.length
          ? [{ id: `opex.staff.${id}.paid`, label: 'Выдано за период (прибыль не меняет)', amount: sumOf(paid, (p) => p.amount_tyiyn), info: true, ...shown }]
          : []),
        ...(s?.owed_tyiyn !== undefined && s.owed_tyiyn !== 0
          ? [{ id: `opex.staff.${id}.owed`, label: 'Остаток к выплате на сегодня', amount: s.owed_tyiyn, info: true }]
          : []),
      ],
    }
  })
  const salary: PnlNode = {
    id: 'opex.salary',
    label: 'Зарплата работникам',
    amount: t.payroll_tyiyn,
    children: salaryNodes,
  }
  const opexParts: PnlNode[] = [
    ...articles,
    ...(t.payroll_tyiyn || salaryNodes.length ? [salary] : []),
    ...(t.bank_fee_tyiyn
      ? [
          {
            id: 'opex.fee',
            label: 'Комиссия банка',
            amount: t.bank_fee_tyiyn,
            children: d.fees.map((f) => ({ id: `opex.fee.${f.id ?? f.name}.${f.branch}`, label: r.all_branches ? `${f.name} · ${f.branch}` : f.name, amount: f.amount_tyiyn })),
          },
        ]
      : []),
    ...(t.bonus_tyiyn ? [{ id: 'opex.bonus', label: 'Скидки баллами', amount: t.bonus_tyiyn }] : []),
    ...(cashDiff
      ? [
          {
            id: 'opex.cash',
            label: 'Недостача / излишек кассы',
            amount: -cashDiff,
            children: d.cash_diffs.map((c) => ({
              id: `opex.cash.${c.id}`,
              label: `Смена № ${c.number}${c.cashier ? `, ${c.cashier}` : ''}`,
              amount: -c.amount_tyiyn,
              doc: { date: c.date, branch: branchOf(c.branch) },
            })),
          },
        ]
      : []),
  ]
  const opex: PnlNode = {
    id: 'opex',
    label: 'Операционные расходы',
    amount: t.expenses_tyiyn + t.payroll_tyiyn + t.bank_fee_tyiyn + t.bonus_tyiyn - cashDiff,
    children: opexParts,
  }

  // 5. Списание и брак.
  const losses: PnlNode = {
    id: 'writeoff',
    label: 'Списание и брак',
    amount: writeoff - defectDiff - returnDiff,
    children: [
      ...(writeoff
        ? [
            {
              id: 'writeoff.defects',
              label: 'Списание брака',
              amount: writeoff,
              children: d.defects
                .filter((x) => x.writeoff_tyiyn)
                .map((x) => ({
                  id: `writeoff.defect.${x.id}`,
                  label: `Брак № ${x.number}: ${x.product}`,
                  amount: x.writeoff_tyiyn,
                  doc: { date: x.date, user: x.user_name, branch: branchOf(x.branch), mark: x.reversal_of ? 'отмена решения' : undefined },
                })),
            },
          ]
        : []),
      ...(defectDiff
        ? [
            {
              id: 'writeoff.defect_diff',
              label: 'Брак: разница возмещения',
              amount: -defectDiff,
              children: d.defects
                .filter((x) => x.diff_tyiyn)
                .map((x) => ({
                  id: `writeoff.diff.${x.id}`,
                  label: `Брак № ${x.number}: ${x.product}`,
                  amount: -x.diff_tyiyn,
                  doc: { date: x.date, user: x.user_name, branch: branchOf(x.branch) },
                })),
            },
          ]
        : []),
      ...(returnDiff
        ? [
            {
              id: 'writeoff.returns',
              label: 'Возврат поставщику: разница',
              amount: -returnDiff,
              children: d.supplier_returns.map((x) => ({
                id: `writeoff.return.${x.id}`,
                label: `Возврат № ${x.number}${x.supplier ? `: ${x.supplier}` : ''}`,
                amount: -x.diff_tyiyn,
                doc: { date: x.date, user: x.user_name, branch: branchOf(x.branch), mark: x.reversal_of ? 'сторно' : undefined },
              })),
            },
          ]
        : []),
    ],
  }

  return [
    revenue,
    cost,
    { id: 'gross', label: 'Валовая прибыль', amount: t.gross_tyiyn, total: true },
    opex,
    ...(losses.amount || losses.children?.length ? [losses] : []),
    { id: 'net', label: 'Чистая прибыль', amount: t.net_tyiyn, total: true },
  ]
}

const MONEY_KINDS: [string, string][] = [
  ['supplier_payment', 'Оплата поставщикам за товар'],
  ['cash_out', 'Изъято из касс'],
  ['transfer_out', 'Переводы между кассами и счетами'],
]

/** «Не входит в прибыль — движение денег» (SPEC-08): деньги переместились, прибыль не меняется. */
export function notInProfit(r: ProfitReport): PnlNode[] {
  const d = r.detail ?? EMPTY_DETAIL
  const branchOf = (b?: string) => (r.all_branches ? b : undefined)
  const out: PnlNode[] = MONEY_KINDS.map(([kind, label]) => {
    const rows = d.not_in_profit.filter((m) => m.kind === kind)
    return {
      id: `money.${kind}`,
      label,
      amount: -sumOf(rows, (m) => m.amount_tyiyn),
      children: rows.map((m) => ({
        id: `money.${m.id}`,
        label: m.payee || m.comment || label,
        amount: -m.amount_tyiyn,
        doc: { date: m.date, payee: m.payee ?? '', account: m.account_name, user: m.user_name, comment: m.comment, branch: branchOf(m.branch), mark: m.reversed ? 'отменено' : undefined },
      })),
    }
  })
  out.push({
    id: 'money.payouts',
    label: 'Выдано работникам',
    amount: sumOf(d.payouts, (p) => p.amount_tyiyn),
    children: d.payouts.map((p) => ({
      id: `money.payout.${p.id}`,
      label: `${p.employee_name}: ${purposeLabel(p.purpose)}`,
      amount: p.amount_tyiyn,
      doc: { date: p.date, account: p.account_name, user: p.user_name, comment: p.comment, branch: branchOf(p.branch), mark: mark(p) },
    })),
  })
  return out.filter((n) => n.amount !== 0 || (n.children?.length ?? 0) > 0)
}

/** Количество по единице товара для подписи: мл — литрами, остальное — штуками. Без float. */
function unitsText(units: number, unit: string): string {
  if (unit === 'ml') {
    const abs = Math.abs(units)
    const frac = String(abs % 1000).padStart(3, '0').replace(/0+$/, '')
    return `${units < 0 ? '−' : ''}${Math.trunc(abs / 1000)}${frac ? ',' + frac : ''} л`
  }
  if (unit === 'g') {
    const abs = Math.abs(units)
    const frac = String(abs % 1000).padStart(3, '0').replace(/0+$/, '')
    return `${units < 0 ? '−' : ''}${Math.trunc(abs / 1000)}${frac ? ',' + frac : ''} кг`
  }
  return `${units} шт.`
}

// ---------- Выгрузка в Excel ----------

/** Строки дерева для листа, всё раскрыто; уровень — группировка Excel. */
function flatten(nodes: PnlNode[], level = 0): SheetRow[] {
  const out: SheetRow[] = []
  for (const n of nodes) {
    const d = n.doc
    out.push({
      level,
      bold: level === 0,
      cells: [
        n.label,
        xl.date(d?.date),
        n.amount === null ? null : xl.money(n.amount),
        d?.payee ?? (d?.comment ? d.comment : null),
        d?.account ?? null,
        d?.user ?? null,
        [d?.mark, n.info ? 'справочно' : undefined, d?.branch].filter(Boolean).join(', ') || null,
      ],
    })
    if (n.children?.length) out.push(...flatten(n.children, level + 1))
  }
  return out
}

/**
 * Книга «Прибыль и убытки»: дерево целиком (раскрыто, со сторно), расходы, зарплата, по дням,
 * товары и работы, движение денег вне прибыли. Деньги — числами из тыйынов (I1).
 */
export function pnlSheets(r: ProfitReport, opts: { branch: string; now: string }): Sheet[] {
  const d = r.detail ?? EMPTY_DETAIL
  const title = ['Прибыль и убытки', `${periodTitle(r.from, r.to)} (${periodText(r.from, r.to)})`, `Филиал: ${opts.branch}`, `Выгружено: ${opts.now}`]
  const allBranches = Boolean(r.all_branches)
  const branchCol = (b: string): Cell[] => (allBranches ? [b] : [])
  const branchHead = allBranches ? [{ label: 'Филиал' }] : []
  const tree: Sheet = {
    name: 'Прибыль и убытки',
    title,
    filter: false,
    columns: [{ label: 'Строка', width: 48 }, { label: 'Дата' }, { label: 'Сумма' }, { label: 'Кому / комментарий' }, { label: 'Касса' }, { label: 'Кто провёл' }, { label: 'Отметка' }],
    rows: [
      ...flatten(pnlTree(r, true)),
      { cells: [] },
      { cells: ['Не входит в прибыль — движение денег'], bold: true },
      ...flatten(notInProfit(r), 1),
    ],
    footer: [`Чеков: ${r.totals.sales_count}`, ...r.warnings],
  }
  const expenses: Sheet = {
    name: 'Расходы',
    title: title.slice(0, 2),
    columns: [{ label: '№' }, { label: 'Дата' }, { label: 'Статья' }, { label: 'Кому' }, { label: 'Сумма' }, { label: 'Касса' }, { label: 'Комментарий' }, { label: 'Кто провёл' }, { label: 'Отметка' }, ...branchHead],
    rows: [
      ...d.expenses.map((e) => [
        xl.int(e.number),
        xl.date(e.date),
        e.article,
        e.payee,
        xl.money(e.amount_tyiyn),
        e.account_name ?? '',
        e.comment,
        e.user_name,
        mark(e) ?? '',
        ...branchCol(e.branch),
      ]),
      { cells: ['Итого', null, null, null, xl.money(r.totals.expenses_tyiyn)], bold: true },
    ],
  }
  const staff = r.staff ?? []
  const salaryRows: SheetRow[] = []
  for (const s of staff) {
    for (const i of s.items) salaryRows.push({ cells: [s.name, `Начислено: ${kindLabel(i.kind)}`, null, xl.money(i.amount_tyiyn)] })
  }
  for (const p of d.payouts) {
    salaryRows.push({
      cells: [p.employee_name, `Выдано: ${purposeLabel(p.purpose)}`, xl.date(p.date), xl.money(p.amount_tyiyn), p.account_name ?? '', p.user_name, p.comment, mark(p) ?? '', ...branchCol(p.branch)],
    })
  }
  for (const s of staff) {
    salaryRows.push({ cells: [s.name, 'Итого начислено', null, xl.money(s.total_tyiyn)], bold: true })
    if (s.owed_tyiyn !== undefined) salaryRows.push({ cells: [s.name, 'Остаток к выплате на сегодня', null, xl.money(s.owed_tyiyn)] })
  }
  salaryRows.push({ cells: ['Всего начислено за период', null, null, xl.money(r.totals.payroll_tyiyn)], bold: true })
  salaryRows.push({ cells: ['Всего выдано за период', null, null, xl.money(sumOf(d.payouts, (p) => p.amount_tyiyn))], bold: true })
  const salary: Sheet = {
    name: 'Зарплата',
    title: title.slice(0, 2),
    columns: [{ label: 'Работник' }, { label: 'Что' }, { label: 'Дата' }, { label: 'Сумма' }, { label: 'Касса' }, { label: 'Кто выдал' }, { label: 'Комментарий' }, { label: 'Отметка' }, ...branchHead],
    rows: salaryRows,
    footer: ['Зарплата входит в прибыль по начислению (ADR-023); выдача денег прибыль не меняет.'],
  }
  const days: Sheet = {
    name: 'По дням',
    title: title.slice(0, 2),
    columns: ['День', 'Выручка', 'Валовая', 'Оплата труда', 'Комиссия', 'Расходы', 'Баллы', 'Касса ±', 'Брак и возвраты', 'Чистая'].map((label) => ({ label })),
    rows: [...r.days]
      .sort((a, b) => a.date.localeCompare(b.date))
      .map((x) => [
        xl.date(x.date),
        xl.money(x.revenue_tyiyn),
        xl.money(x.gross_tyiyn),
        xl.money(x.payroll_tyiyn),
        xl.money(x.fee_tyiyn ?? 0),
        xl.money(x.expenses_tyiyn),
        xl.money(x.bonus_tyiyn ?? 0),
        xl.money(x.cash_diff_tyiyn ?? 0),
        xl.money((x.defect_diff_tyiyn ?? 0) - (x.defect_writeoff_tyiyn ?? 0) + (x.return_diff_tyiyn ?? 0)),
        xl.money(x.net_tyiyn),
      ]),
  }
  const goods: Sheet = {
    name: 'Товары и работы',
    title: title.slice(0, 2),
    columns: [{ label: 'Категория' }, { label: 'Товар / работа' }, { label: 'Количество' }, { label: 'Выручка' }, { label: 'Себестоимость' }, { label: 'Валовая' }],
    rows: [
      ...d.products.map((p) => [p.category, p.name, xl.units(p.units, p.unit), xl.money(p.revenue_tyiyn), xl.money(p.cost_tyiyn), xl.money(p.revenue_tyiyn - p.cost_tyiyn)]),
      ...d.services.map((s) => ['Работы', s.name, xl.int(s.count), xl.money(s.revenue_tyiyn), null, null]),
      { cells: ['Итого', null, null, xl.money(r.totals.goods_tyiyn + r.totals.services_tyiyn)], bold: true },
    ],
  }
  const money: Sheet = {
    name: 'Не входит в прибыль',
    title: [...title.slice(0, 2), 'Деньги переместились, прибыль не меняется'],
    columns: [{ label: 'Что' }, { label: 'Дата' }, { label: 'Сумма' }, { label: 'Кому' }, { label: 'Касса' }, { label: 'Кто провёл' }, { label: 'Комментарий' }, ...branchHead],
    rows: [
      ...d.not_in_profit.map((m) => [
        MONEY_KINDS.find(([k]) => k === m.kind)?.[1] ?? m.kind,
        xl.date(m.date),
        xl.money(-m.amount_tyiyn),
        m.payee ?? '',
        m.account_name,
        m.user_name,
        m.comment,
        ...branchCol(m.branch),
      ]),
      ...d.payouts.map((p) => [`Выдано работнику: ${p.employee_name}`, xl.date(p.date), xl.money(p.amount_tyiyn), p.employee_name, p.account_name ?? '', p.user_name, p.comment, ...branchCol(p.branch)]),
    ],
  }
  return [tree, expenses, salary, days, goods, money]
}
