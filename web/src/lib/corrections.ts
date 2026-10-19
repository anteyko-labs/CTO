// «Исправить» у документов владельца (SPEC-29 §24, ADR-085): сторно и верный документ одной
// операцией на сервере. Здесь — обращения к серверу и подписи для истории.
import { get, post, qs } from './api'

/** Виды документов, у которых есть «Исправить» (у каждого есть сторно). */
export type CorrectionKind =
  | 'sale'
  | 'receipt'
  | 'expense'
  | 'cash_movement'
  | 'cash_transfer'
  | 'ledger'
  | 'accrual'
  | 'payout'
  | 'stock_in'
  | 'supplier_return'
  | 'battery_intake'
  | 'oil_transfer'

/** Строка сравнения «что — было — стало». */
export interface DiffItem {
  what: string
  old: string
  new: string
}

export interface Correction {
  id: string
  kind: CorrectionKind
  kind_label: string
  original_id: string
  original_number: number | null
  reversal_id: string
  new_id: string
  new_number: number | null
  comment: string
  /** Администратору — пусто. */
  diff: DiffItem[]
  user_name: string
  created_at: string
}

/** Исходный документ для формы исправления — в виде запроса его проведения. */
export interface CorrectionDraft<T = Record<string, unknown>> {
  kind: CorrectionKind
  kind_label: string
  original_id: string
  number: number | null
  doc: T
}

export interface CorrectionLinks {
  corrected_by: Correction | null
  correction_of: Correction | null
}

export const loadCorrectionDraft = <T = Record<string, unknown>>(kind: CorrectionKind, id: string) =>
  get<CorrectionDraft<T>>(`/corrections/draft/${kind}/${id}`)

export const loadCorrectionLinks = (id: string) => get<CorrectionLinks>(`/corrections/doc/${id}`)

export const loadCorrections = (kind?: CorrectionKind) => get<Correction[]>(`/corrections${qs({ kind })}`)

/** Проводит исправление: `op_id` — один на попытку, повтор не проводит второй раз. */
export const postCorrection = (body: { op_id: string; kind: CorrectionKind; original_id: string; comment: string; doc: unknown }) =>
  post<Correction>('/corrections', body)

/** «№ 12» или ничего — у внесений, начислений и оплат долга номера нет. */
export const numberText = (n: number | null | undefined): string => (n == null ? '' : `№ ${n}`)

/** Отметка документа в списке: исправлен (на какой) или сам исправление (какого). */
export interface CorrectionMark {
  correctedTo: Correction | null
  correctionOf: Correction | null
}

/** Отметки по id документов — одним запросом на список, а не по строке. */
export function correctionMarks(list: Correction[]): Map<string, CorrectionMark> {
  const out = new Map<string, CorrectionMark>()
  const at = (id: string) => {
    const cur = out.get(id) ?? { correctedTo: null, correctionOf: null }
    out.set(id, cur)
    return cur
  }
  for (const c of list) {
    at(c.original_id).correctedTo = c
    at(c.new_id).correctionOf = c
  }
  return out
}

/** «исправлен → № 15» / «исправление № 12» — короткая подпись для строки списка. */
export function markText(m: CorrectionMark | undefined): string[] {
  if (!m) return []
  const out: string[] = []
  if (m.correctionOf) out.push(['исправление', numberText(m.correctionOf.original_number)].filter(Boolean).join(' '))
  if (m.correctedTo) out.push(['исправлен →', numberText(m.correctedTo.new_number) || 'новый'].join(' '))
  return out
}
