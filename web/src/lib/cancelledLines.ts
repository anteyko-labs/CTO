// Отменённые товары (SPEC-30 §8, ADR-099): строки, удалённые из чека до проведения, — крестиком,
// «Очистить» или вместе с отложенным чеком. Удалённые крестиком уходят с проведённым чеком
// (`removed_lines`), остальное — отдельной записью через очередь устройства, как чеки.
// Уменьшение количества кнопкой «−» не записывается.
import { newOpId } from './api'
import { enqueueOp, type QueueUser } from './offline'
import type { SaleLineKind } from './types'

/** Куда уходит отдельная запись об отменённых строках. */
export const CANCELLED_PATH = '/sales/cancelled-lines'

/** Почему строку убрали: крестиком, «Очистить», удалили отложенный чек. */
export type CancelReason = 'remove' | 'clear' | 'held'

export interface RemovedLine {
  /** Строка кассы: «Вернуть» снимает её запись. На сервер не уходит. */
  key: string
  reason: CancelReason
  kind: SaleLineKind
  product_id: string | null
  service_id: string | null
  name: string
  /** Как в строке чека: штуки, канистры, мл розлива, граммы. */
  qty: number
  unit_price_tyiyn: number
  removed_at: string
}

/** Строка кассы, как её видит этот модуль. */
export interface CartLineView {
  key: string
  kind: SaleLineKind
  product_id: string | null
  service_id: string | null
  name: string
  qty: number | null
  price: number | null
  gift?: boolean
}

/** Запись об удалённой строке; строку без количества (ещё не набрали) не записываем. */
export function removedLine(l: CartLineView, reason: CancelReason, at: Date = new Date()): RemovedLine | null {
  if (l.qty === null || !Number.isSafeInteger(l.qty) || l.qty <= 0) return null
  const price = l.gift ? 0 : Math.max(0, l.price ?? 0)
  return {
    key: l.key,
    reason,
    kind: l.kind,
    product_id: l.product_id,
    service_id: l.service_id,
    name: l.name,
    qty: l.qty,
    unit_price_tyiyn: Number.isSafeInteger(price) ? price : 0,
    removed_at: at.toISOString(),
  }
}

/** Строки для сервера: без ключа строки кассы. */
export function forServer(lines: RemovedLine[]): Omit<RemovedLine, 'key'>[] {
  return lines.map(({ key: _key, ...rest }) => rest)
}

/** «Вернуть» удалённую строку: её запись снимается (последняя с этим ключом). */
export function unremove(lines: RemovedLine[], key: string): RemovedLine[] {
  const i = lines.map((l) => l.key).lastIndexOf(key)
  return i < 0 ? lines : [...lines.slice(0, i), ...lines.slice(i + 1)]
}

/** Тело отдельной записи: «Очистить» и удаление отложенного чека. */
export function cancelledBody(lines: RemovedLine[], shiftId: string | null, opId: string, now: Date = new Date()) {
  return { op_id: opId, shift_id: shiftId, client_time: now.toISOString(), lines: forServer(lines) }
}

/**
 * Отправляет строки отдельной записью через очередь устройства: без сети они уйдут позже, как
 * чеки. Ошибки не мешают кассе — это след для владельца, а не продажа.
 */
export async function sendCancelled(lines: RemovedLine[], shiftId: string | null, user: QueueUser | null): Promise<void> {
  if (lines.length === 0) return
  const opId = newOpId()
  await enqueueOp(opId, CANCELLED_PATH, cancelledBody(lines, shiftId, opId), user)
}
