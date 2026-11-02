// Брак и поставки (SPEC-25, SPEC-26): ввод количества и подписи. Только показ и ввод —
// стоимость, возмещение и разницу считает сервер.
import { formatSom } from './format'
import { parseQty as parseUnitQty, qtyInputText, qtyText as unitQtyText, unitShort, type StockUnit } from './units'

export type Unit = StockUnit

/** В чём вводится количество брака: штуки, литры (мл), килограммы (г), метры (мм). */
export const qtyInputLabel = (unit: Unit): string => unitShort(unit)

/** «2» шт → 2; «1,5» л → 1500 мл; «0,25» кг → 250 г; «1,25» м → 1250 мм. Ноль, минус и мусор — null. */
export function parseQty(input: string, unit: Unit): number | null {
  const v = parseUnitQty(unit, input)
  return v !== null && v > 0 ? v : null
}

/** Количество для показа: «2 шт», «4 л», «0,25 кг», «1,25 м». */
export const qtyText = (qty: number, unit: Unit): string => unitQtyText(unit, qty)

/** Количество для поля ввода: 1500 мл → «1,5». */
export const qtyInput = (qty: number, unit: Unit): string => qtyInputText(unit, qty)

export type ResolutionKind = 'write_off' | 'replace' | 'credit' | 'cash'

export const RESOLUTION_LABELS: Record<ResolutionKind, string> = {
  write_off: 'Списать в убыток',
  replace: 'Вернуть → новый товар',
  credit: 'Вернуть → скидка / из долга',
  cash: 'Вернуть → деньгами',
}

export const STATUS_LABELS: Record<string, string> = {
  pending: 'ждёт решения',
  written_off: 'списан',
  returned: 'возвращён поставщику',
  cancelled: 'отметка снята',
}

/** Что будет с прибылью при возмещении: «доход 20 с», «убыток 50 с» или «без разницы». */
export function refundEffect(cost: number, refund: number): string {
  const d = refund - cost
  if (d === 0) return 'без разницы со стоимостью'
  return d > 0 ? `доход ${formatSom(d)}` : `убыток ${formatSom(-d)}`
}

/** Баланс поставщика словами: меньше нуля — должны мы, больше — он нам. */
export function balanceText(balance: number): string {
  if (balance === 0) return 'расчёты закрыты'
  return balance < 0 ? `мы должны ${formatSom(-balance)}` : `он должен нам ${formatSom(balance)}`
}
