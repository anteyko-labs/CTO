// Деление возврата долгового чека (SPEC-36 §8): сначала снимается долг — не больше, чем этот чек
// дал в долг за вычетом прежних возвратов, и не больше, чем клиент должен сейчас; остаток
// отдаётся деньгами тем способом, которым платили. Обе суммы кассир может поправить.

/** Сколько можно снять с долга: min(по чеку в долг − уже снятое, долг клиента сейчас). */
export function returnDebtCap(debtLeftOnSale: number, owesNow: number): number {
  return Math.max(0, Math.min(debtLeftOnSale, owesNow))
}

/** Какую сумму кассир правил последней; другая — остаток. */
export interface ReturnEdit {
  field: 'debt' | 'money'
  /** null — в поле не число. */
  value: number | null
}

export interface ReturnSplit {
  debt: number
  money: number
  /** Обе части от 0, с долга — не больше `cap`, вместе — вся сумма. */
  ok: boolean
}

/** `total` — сколько возвращается долгом и деньгами (без части «аккумулятором»). */
export function splitReturn(total: number, cap: number, edit: ReturnEdit | null): ReturnSplit {
  const amount = Math.max(0, total)
  if (!edit) {
    const debt = Math.min(cap, amount)
    return { debt, money: amount - debt, ok: true }
  }
  if (edit.value === null) return { debt: 0, money: 0, ok: false }
  const debt = edit.field === 'debt' ? edit.value : amount - edit.value
  const money = amount - debt
  return { debt, money, ok: debt >= 0 && money >= 0 && debt <= cap }
}
