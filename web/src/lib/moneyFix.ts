// «Исправить» у денежных документов (SPEC-29 §24, ADR-085): исходный документ с сервера → поля
// той же формы, что его создаёт, и обратно — исправленный документ в виде запроса проведения.
// Права, остатки и суммы окончательно проверяет сервер.
import { parseSom, somInput } from './format'

// ---------- Расход ----------

export interface ExpenseDoc {
  article_id: string
  amount_tyiyn: number
  source: string
  account_id: string | null
  expense_date: string | null
  comment: string
}

/** Поля формы расхода: пустой счёт — касса смены, как при записи. */
export interface ExpenseFields {
  article: string
  sum: string
  account: string
  date: string
  comment: string
}

export function expenseFields(doc: ExpenseDoc, tillId: string | null): ExpenseFields {
  return {
    article: doc.article_id,
    sum: somInput(doc.amount_tyiyn),
    account: !doc.account_id || doc.account_id === tillId ? '' : doc.account_id,
    date: doc.expense_date ?? '',
    comment: doc.comment,
  }
}

/** Исправленный расход; null — сумма не числом или не больше нуля. */
export function expenseDoc(base: ExpenseDoc, f: ExpenseFields): ExpenseDoc | null {
  const sum = parseSom(f.sum)
  if (sum === null || sum <= 0) return null
  return {
    ...base,
    article_id: f.article,
    amount_tyiyn: sum,
    account_id: f.account || null,
    expense_date: f.date || base.expense_date,
    comment: f.comment,
  }
}

// ---------- Внесение, изъятие, перевод ----------

export interface MovementDoc {
  account_id: string | null
  kind: 'cash_in' | 'cash_out'
  amount_tyiyn: number
  comment: string
}

export interface TransferDoc {
  from_account_id: string
  to_account_id: string
  amount_tyiyn: number
  comment: string
}

/** Поля окна «Внести / Изъять / Перевести» на экране «Смена»: `from` пустой — касса смены. */
export interface MoneyFields {
  kind: 'cash_in' | 'cash_out' | 'transfer'
  sum: string
  comment: string
  from: string
  to: string
}

export function movementFields(doc: MovementDoc, tillId: string | null): MoneyFields {
  return {
    kind: doc.kind,
    sum: somInput(Math.abs(doc.amount_tyiyn)),
    comment: doc.comment,
    from: !doc.account_id || doc.account_id === tillId ? '' : doc.account_id,
    to: '',
  }
}

export function transferFields(doc: TransferDoc): MoneyFields {
  return {
    kind: 'transfer',
    sum: somInput(doc.amount_tyiyn),
    comment: doc.comment,
    from: doc.from_account_id,
    to: doc.to_account_id,
  }
}

/** Исправленное внесение/изъятие: вид меняется в пределах внесения и изъятия. */
export function movementDoc(f: MoneyFields, tillId: string | null): MovementDoc | null {
  const sum = parseSom(f.sum)
  if (sum === null || sum <= 0 || f.kind === 'transfer') return null
  return { account_id: f.from || tillId, kind: f.kind, amount_tyiyn: sum, comment: f.comment }
}

export function transferDoc(f: MoneyFields): TransferDoc | null {
  const sum = parseSom(f.sum)
  if (sum === null || sum <= 0 || f.kind !== 'transfer' || !f.from || !f.to) return null
  return { from_account_id: f.from, to_account_id: f.to, amount_tyiyn: sum, comment: f.comment }
}

// ---------- Оплата долга и правка баланса ----------

export type PayMethod = 'cash' | 'card' | 'transfer' | 'invoice'

export interface RepaymentDoc {
  type?: 'repayment'
  party_id: string
  amount_tyiyn: number
  method: PayMethod
  account_id: string | null
  receipt_id: string | null
  comment: string
}

export interface AdjustDoc {
  type?: 'adjust'
  party_id: string
  /** Со знаком: плюс — долг больше, минус — простили. */
  amount_tyiyn: number
  comment: string
}

export interface RepaymentFields {
  sum: string
  method: PayMethod
  account: string
  comment: string
}

export function repaymentFields(doc: RepaymentDoc): RepaymentFields {
  return {
    sum: somInput(Math.abs(doc.amount_tyiyn)),
    method: doc.method,
    account: doc.account_id ?? '',
    comment: doc.comment,
  }
}

/** Исправленная оплата: накладная и сторона долга — как в исходной. */
export function repaymentDoc(base: RepaymentDoc, f: RepaymentFields): RepaymentDoc | null {
  const sum = parseSom(f.sum)
  if (sum === null || sum <= 0) return null
  return { ...base, amount_tyiyn: sum, method: f.method, account_id: f.account || null, comment: f.comment }
}

export interface AdjustFields {
  sign: 1 | -1
  sum: string
  comment: string
}

export function adjustFields(doc: AdjustDoc): AdjustFields {
  return { sign: doc.amount_tyiyn < 0 ? -1 : 1, sum: somInput(Math.abs(doc.amount_tyiyn)), comment: doc.comment }
}

export function adjustDoc(base: AdjustDoc, f: AdjustFields): AdjustDoc | null {
  const sum = parseSom(f.sum)
  if (sum === null || sum <= 0) return null
  return { ...base, amount_tyiyn: f.sign * sum, comment: f.comment }
}
