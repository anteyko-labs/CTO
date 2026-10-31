// «Расходы» — все траты, кроме оплаты поставщикам за товар (задача 003 Н12б, ADR-161): расходы
// по статьям и выплаты работникам одним списком, начисленные бонусы — отдельной строкой. Только
// раскладка: строки и итоги отдаёт сервер (`GET /expenses/all`, `/expenses/all/summary`).
import { qs } from './api'
import { formatDateTime } from './format'
import { PURPOSE_LABELS } from './payouts'
import { xl, type ExportCol } from './exportTable'
import type { SpendingRow, SpendingSummary } from './types'

export type SpendingKind = 'all' | 'expenses' | 'payouts'

/** Отбор списка; по умолчанию «Всё». */
export const SPENDING_KINDS: [SpendingKind, string][] = [
  ['all', 'Всё'],
  ['expenses', 'Расходы'],
  ['payouts', 'Выплаты работникам'],
]

/** Отборы, которые этому пользователю видны: без «Расчёта» выплат нет, без «Расходов» — расходов. */
export function spendingKinds(may: { expenses: boolean; payroll: boolean }): [SpendingKind, string][] {
  if (may.expenses && may.payroll) return SPENDING_KINDS
  return SPENDING_KINDS.filter(([k]) => k === (may.payroll ? 'payouts' : 'expenses'))
}

/** Адрес списка (и выгрузки) по отбору. */
export function spendingPath(p: { from: string; to: string; kind: SpendingKind; reversals: boolean }, summary = false): string {
  return `/expenses/all${summary ? '/summary' : ''}${qs({ from: p.from, to: p.to, kind: p.kind === 'all' ? undefined : p.kind, reversals: p.reversals ? 1 : undefined })}`
}

/** «Что»: статья расхода, вид выплаты или «начислен бонус». */
export function spendingWhat(r: SpendingRow): string {
  switch (r.kind) {
    case 'expense':
      return r.article
    case 'payout':
      return `Выплата: ${PURPOSE_LABELS[r.purpose ?? 'general'] ?? r.purpose}`
    case 'bonus_accrual':
      return 'начислен бонус'
  }
}

/** Отметка сторно у строки. */
export function spendingMark(r: Pick<SpendingRow, 'reversal_of' | 'reversed'>): string {
  if (r.reversal_of) return 'сторно'
  if (r.reversed) return 'сторнирован'
  return ''
}

/** Номер документа для показа: у начисления номера нет. */
export function spendingNumber(r: SpendingRow): string {
  return r.number === null ? '—' : String(r.number)
}

export interface SummaryLine {
  key: string
  label: string
  amount: number
  /** `article` — статья, `group` — «Зарплата работникам», `sub` — «в т. ч.», `info` — в итог не входит, `total`. */
  tone: 'article' | 'group' | 'sub' | 'info' | 'total'
}

/**
 * Итог по статьям: статьи расходов, группа «Зарплата работникам» (выданные деньги) со строкой
 * «в т. ч. бонусы»; начисленные и не выданные бонусы — отдельной строкой, в «Итого» не входят.
 */
export function summaryLines(s: SpendingSummary): SummaryLine[] {
  const out: SummaryLine[] = s.articles.map((a) => ({ key: `article.${a.article}`, label: a.article, amount: a.amount_tyiyn, tone: 'article' }))
  if (s.payouts) {
    out.push({ key: 'salary', label: 'Зарплата работникам', amount: s.payouts_tyiyn, tone: 'group' })
    out.push({ key: 'salary.bonus', label: 'в т. ч. бонусы', amount: s.bonus_paid_tyiyn, tone: 'sub' })
    if (s.bonus_unpaid_tyiyn) {
      out.push({ key: 'bonus.unpaid', label: 'Бонусы: начислено, не выдано (в итог не входит)', amount: s.bonus_unpaid_tyiyn, tone: 'info' })
    }
  }
  out.push({ key: 'total', label: 'Итого', amount: s.total_tyiyn, tone: 'total' })
  return out
}

/** Столбцы выгрузки — как в таблице экрана; сторно — отметкой. */
export const SPENDING_EXPORT_COLS: ExportCol<SpendingRow>[] = [
  { label: '№', value: (r) => (r.number === null ? '' : xl.int(r.number)) },
  { label: 'Дата', value: (r) => xl.date(r.date) },
  { label: 'Что', value: (r) => spendingWhat(r) },
  { label: 'Кому', value: (r) => r.payee },
  { label: 'За что', value: (r) => r.comment },
  { label: 'Касса', value: (r) => (r.source === 'outside' ? 'не из денег точки' : (r.account_name ?? '')) },
  { label: 'Сумма', value: (r) => xl.money(r.amount_tyiyn) },
  { label: 'Кто провёл', value: (r) => r.user_name },
  { label: 'Когда', value: (r) => formatDateTime(r.created_at) },
  { label: 'Отметка', value: (r) => [spendingMark(r), r.kind === 'bonus_accrual' ? 'в итог не входит' : ''].filter(Boolean).join(', ') },
]

/** Строки итога для листа выгрузки: подпись и сумма. */
export function summarySheetRows(s: SpendingSummary): { label: string; amount: number; bold: boolean }[] {
  return summaryLines(s).map((l) => ({ label: l.tone === 'sub' ? `  ${l.label}` : l.label, amount: l.amount, bold: l.tone === 'group' || l.tone === 'total' }))
}
