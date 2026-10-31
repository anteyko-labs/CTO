import { describe, expect, it } from 'vitest'
import { notInProfit, periodTitle, pnlSheets, pnlTree, quickPeriod, type PnlNode } from './pnl'
import type { ProfitReport } from './types'

const base = { branch: 'Основной', created_at: '2026-10-31T05:00:00Z', comment: '', user_name: 'Владелец', account_name: 'Касса' }

/** Отчёт за день: два товара двух категорий, работа, возврат, ревизия, расходы со сторно, зарплата. */
const report: ProfitReport = {
  from: '2026-10-01',
  to: '2026-10-31',
  totals: {
    goods_tyiyn: 150_000,
    services_tyiyn: 30_000,
    cost_tyiyn: 12_000,
    revaluation_tyiyn: 3_000,
    gross_tyiyn: 168_000,
    payroll_tyiyn: 26_650,
    bank_fee_tyiyn: 500,
    expenses_tyiyn: 12_000,
    bonus_tyiyn: 1_000,
    cash_diff_tyiyn: -100_000,
    defect_writeoff_tyiyn: 2_000,
    defect_diff_tyiyn: -500,
    return_diff_tyiyn: 300,
    net_tyiyn: 168_000 - 26_650 - 500 - 12_000 - 1_000 - 100_000 - 2_000 - 500 + 300,
    margin_bp: null,
    sales_count: 2,
  },
  categories: [],
  days: [{ date: '2026-10-31', revenue_tyiyn: 180_000, gross_tyiyn: 168_000, payroll_tyiyn: 26_650, expenses_tyiyn: 12_000, net_tyiyn: 25_650 }],
  articles: [],
  warnings: [],
  staff: [
    {
      employee_id: 'e1',
      name: 'Рома',
      items: [
        { kind: 'bonus', count: 1, amount_tyiyn: 21_650 },
        { kind: 'penalty', count: 1, amount_tyiyn: -5_000 },
        { kind: 'revenue_percent', count: 2, amount_tyiyn: 10_000 },
      ],
      total_tyiyn: 26_650,
      owed_tyiyn: 11_650,
    },
  ],
  detail: {
    products: [
      { product_id: 'p1', name: 'Масло', category: 'Масла', unit: 'ml', units: 4500, revenue_tyiyn: 100_000, cost_tyiyn: 6_000 },
      { product_id: 'p2', name: 'Фильтр', category: 'Фильтры', unit: 'piece', units: 1, revenue_tyiyn: 50_000, cost_tyiyn: 3_000 },
    ],
    services: [{ service_id: 's1', name: 'Замена масла', count: 1, revenue_tyiyn: 30_000, master_fee_tyiyn: 10_000 }],
    returns_tyiyn: 50_000,
    revision_tyiyn: 3_000,
    revaluation_only_tyiyn: 0,
    revisions: [{ id: 'r1', number: 1, date: '2026-10-31', user_name: 'Владелец', amount_tyiyn: 3_000, branch: 'Основной' }],
    revaluations: [],
    expenses: [
      { ...base, id: 'x1', number: 1, article: 'Хозтовары', date: '2026-10-31', amount_tyiyn: 12_000, payee: 'Хозмаг', reversal_of: null, reversed: false },
      { ...base, id: 'x2', number: 2, article: 'Налоги', date: '2026-10-31', amount_tyiyn: 40_000, payee: 'Налоговая', reversal_of: null, reversed: true },
      { ...base, id: 'x3', number: 3, article: 'Налоги', date: '2026-10-31', amount_tyiyn: -40_000, payee: 'Налоговая', reversal_of: 'x2', reversed: false },
    ],
    payouts: [
      { ...base, id: 'o1', number: 1, employee_id: 'e1', employee_name: 'Рома', date: '2026-10-31', amount_tyiyn: 15_000, purpose: 'bonus', reversal_of: null, reversed: false },
    ],
    fees: [{ id: 'a1', name: 'Банк', amount_tyiyn: 500, branch: 'Основной' }],
    cash_diffs: [{ id: 'sh1', number: 7, date: '2026-10-31', cashier: 'Рома', amount_tyiyn: -100_000, branch: 'Основной' }],
    defects: [{ ...base, id: 'd1', number: 1, date: '2026-10-31', product: 'Фильтр', kind: 'write_off', writeoff_tyiyn: 2_000, diff_tyiyn: -500, reversal_of: null }],
    supplier_returns: [{ ...base, id: 'sr1', number: 1, date: '2026-10-31', supplier: 'Поставщик', diff_tyiyn: 300, reversal_of: null }],
    not_in_profit: [
      { ...base, id: 'm1', kind: 'supplier_payment', date: '2026-10-31', amount_tyiyn: -90_000, account_name: 'Сейф', payee: 'Поставщик', reversed: false },
      { ...base, id: 'm2', kind: 'cash_out', date: '2026-10-31', amount_tyiyn: -10_000, payee: null, reversed: false },
    ],
  },
}

/** Сумма строки = сумма её раскрытия (справочные строки не считаются). */
function checkTies(nodes: PnlNode[], path = ''): void {
  for (const n of nodes) {
    const kids = (n.children ?? []).filter((c) => !c.info)
    if (kids.length && !n.info && n.hidden === undefined) {
      expect(kids.reduce((a, c) => a + (c.amount ?? 0), 0), `${path}/${n.label}`).toBe(n.amount)
    }
    if (n.children) checkTies(n.children, `${path}/${n.label}`)
  }
}

const find = (nodes: PnlNode[], id: string): PnlNode | undefined => {
  for (const n of nodes) {
    if (n.id === id) return n
    const hit = find(n.children ?? [], id)
    if (hit) return hit
  }
  return undefined
}

describe('прибыль и убытки', () => {
  it('дерево по образцу заказчика, чистая — серверная', () => {
    const tree = pnlTree(report)
    expect(tree.map((n) => n.label)).toEqual(['Выручка', 'Себестоимость', 'Валовая прибыль', 'Операционные расходы', 'Списание и брак', 'Чистая прибыль'])
    const [revenue, cost, gross, opex, losses, net] = tree
    expect(revenue.amount).toBe(180_000)
    expect(cost.amount).toBe(12_000)
    expect(gross.amount).toBe(168_000)
    expect(net.amount).toBe(report.totals.net_tyiyn)
    // Валовая − операционные − списание = чистая: дерево не придумывает своих чисел.
    expect((gross.amount ?? 0) - (opex.amount ?? 0) - (losses.amount ?? 0)).toBe(net.amount)
    checkTies(tree)
  })

  it('раскрытие до документов: статья → дата, сумма, кому, кто провёл', () => {
    const tree = pnlTree(report)
    const household = find(tree, 'opex.article.Хозтовары')
    expect(household?.amount).toBe(12_000)
    expect(household?.children?.[0].doc).toMatchObject({ date: '2026-10-31', payee: 'Хозмаг', account: 'Касса', user: 'Владелец' })
    expect(find(tree, 'revenue.product.p1')?.label).toBe('Масло — 4,5 л')
    expect(find(tree, 'revenue.returns')?.info).toBe(true)
    expect(find(tree, 'cost.revision.r1')?.label).toBe('Ревизия № 1')
    expect(find(tree, 'opex.cash.sh1')).toMatchObject({ label: 'Смена № 7, Рома', amount: 100_000 })
  })

  it('сторно скрыто парой и показывается переключателем; итог статьи тот же', () => {
    const hidden = find(pnlTree(report), 'opex.article.Налоги')
    expect(hidden?.amount).toBe(0)
    expect(hidden?.children).toEqual([])
    expect(hidden?.hidden).toBe(2)
    const shown = find(pnlTree(report, true), 'opex.article.Налоги')
    expect(shown?.amount).toBe(0)
    expect(shown?.children?.map((c) => c.doc?.mark)).toEqual(['сторнирован', 'сторно'])
    expect(shown?.hidden).toBe(0)
  })

  it('зарплата по работнику: начислено по видам, выдано списком', () => {
    const roma = find(pnlTree(report), 'opex.staff.e1')
    expect(roma?.amount).toBe(26_650)
    expect(roma?.children?.map((c) => c.label)).toEqual(['Бонус / доп. доход', 'Удержание', '% с чеков', 'Выдано за период (прибыль не меняет)', 'Остаток к выплате на сегодня'])
    const paid = find(pnlTree(report), 'opex.staff.e1.paid')
    expect(paid?.info).toBe(true)
    expect(paid?.children?.[0]).toMatchObject({ label: 'Бонус', amount: 15_000 })
  })

  it('не входит в прибыль: поставщикам, изъятия, выдано работникам', () => {
    const money = notInProfit(report)
    expect(money.map((n) => [n.label, n.amount])).toEqual([
      ['Оплата поставщикам за товар', 90_000],
      ['Изъято из касс', 10_000],
      ['Выдано работникам', 15_000],
    ])
  })

  it('книга Excel: листы, расходы сходятся, сторно в файле есть', () => {
    const sheets = pnlSheets(report, { branch: 'Основной', now: '31.10.2026 18:00' })
    expect(sheets.map((s) => s.name)).toEqual(['Прибыль и убытки', 'Расходы', 'Зарплата', 'По дням', 'Товары и работы', 'Не входит в прибыль'])
    expect(sheets[0].title?.[0]).toBe('Прибыль и убытки')
    expect(sheets[0].title?.[1]).toBe('С 1 октября 2026 по 31 октября 2026 (01.10.2026–31.10.2026)')
    const exp = sheets[1].rows.filter(Array.isArray)
    expect(exp.length).toBe(3)
    const sum = exp.reduce((a, r) => a + ((r[4] as { money: number }).money ?? 0), 0)
    expect(sum).toBe(report.totals.expenses_tyiyn)
    expect(exp.map((r) => r[8])).toEqual(['', 'сторнирован', 'сторно'])
    // Дерево — всё раскрыто, вложенность — уровнем строки.
    const tree = sheets[0].rows.map((r) => (Array.isArray(r) ? { cells: r, level: 0 } : r))
    const x3 = tree.find((r) => r.cells[0] === 'Налоговая' && (r.cells[2] as { money: number }).money === -40_000)
    expect(x3?.level).toBe(2)
  })

  it('быстрые периоды по дате Бишкека', () => {
    expect(quickPeriod('month', '2026-10-31')).toEqual({ from: '2026-10-01', to: '2026-10-31' })
    expect(quickPeriod('last_month', '2026-10-31')).toEqual({ from: '2026-09-01', to: '2026-09-30' })
    expect(quickPeriod('last_month', '2026-03-15')).toEqual({ from: '2026-02-01', to: '2026-02-28' })
    expect(quickPeriod('last_month', '2026-01-05')).toEqual({ from: '2025-12-01', to: '2025-12-31' })
    expect(quickPeriod('yesterday', '2026-11-01')).toEqual({ from: '2026-10-31', to: '2026-10-31' })
    expect(quickPeriod('week', '2026-10-31')).toEqual({ from: '2026-10-25', to: '2026-10-31' })
    expect(periodTitle('2026-10-01', '2026-10-31')).toBe('С 1 октября 2026 по 31 октября 2026')
    expect(periodTitle('2026-10-31', '2026-10-31')).toBe('За 31 октября 2026')
  })
})
