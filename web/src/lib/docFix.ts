// «Исправить» у начислений, выплат и складских документов (SPEC-29 §24, ADR-085): исходный документ
// с сервера → поля формы, которой его создают. Сервер решает окончательно; здесь — только показ.
import { numberText, type Correction } from './corrections'
import { litersInput, somInput } from './format'

/** Количество в единицах учёта → текст поля: штуки целым, масло литрами, на вес килограммами. */
export function qtyInput(unit: 'piece' | 'ml' | 'g', qty: number): string {
  return unit === 'piece' ? String(qty) : litersInput(qty)
}

/** Сумма документа → поле в сомах; нет суммы — пусто. */
export const amountInput = (tyiyn: number | null | undefined): string => (tyiyn == null ? '' : somInput(tyiyn))

/** Уведомление после исправления: «Исправлено: Выплата → № 15». */
export function fixedText(c: Pick<Correction, 'kind_label' | 'new_number'>): string {
  const n = numberText(c.new_number)
  return `Исправлено: ${c.kind_label}${n ? ` → ${n}` : ''}`
}

/** Строка возврата поставщику из исходного документа: сумма уже задана — пересчёт её не трогает. */
export interface DraftReturnLine {
  product_id: string
  qty: number
  amount_tyiyn?: number | null
}

export function returnLineFromDraft<P extends { id: string; unit: 'piece' | 'ml' | 'g' }>(
  l: DraftReturnLine,
  product: P,
  price: number | null,
): { key: string; product: P; qty: string; amount: string; manual: boolean; price: number | null } {
  const amount = amountInput(l.amount_tyiyn)
  return { key: `fix-${l.product_id}`, product, qty: qtyInput(product.unit, l.qty), amount, manual: amount !== '', price }
}

/**
 * Остаток масла, доступный исправленному переливу: исходный перелив сторнируется в той же операции,
 * поэтому его литры возвращаются источнику и уходят у получателя.
 */
export function oilStockForFix(productId: string, stockMl: number, original: { from_product_id: string; to_product_id: string; qty_ml: number } | null): number {
  if (!original) return stockMl
  let out = stockMl
  if (productId === original.from_product_id) out += original.qty_ml
  if (productId === original.to_product_id) out -= original.qty_ml
  return out
}
