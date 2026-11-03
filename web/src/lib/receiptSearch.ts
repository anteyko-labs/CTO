// Список приходов (SPEC-03, задача 002, П6): параметры запроса. Без поиска — период (по умолчанию
// 30 дней), с поиском — за всё время: накладную ищут по номеру, поставщику, товару или сумме, а
// сумму в сомах сервер сравнивает с итогом в тыйынах. Приём аккумуляторов на кассе — строками
// того же списка (задача 025).

import { formatSom } from './format'
import { qtyText } from './units'
import type { ReceiptListItem } from './types'

export interface ReceiptsFilter {
  from: string
  to: string
  kind: string
  q: string
  photo: boolean
}

/** Параметры `GET /receipts`: пустые не отправляются. */
export function receiptsQuery(f: ReceiptsFilter): Record<string, string | boolean | undefined> {
  const q = f.q.trim()
  return {
    ...(q ? { q } : { from: f.from, to: f.to }),
    kind: f.kind,
    photo: f.photo || undefined,
  }
}

/** Строка приёма аккумуляторов: за наличные или в оплату чека (задача 025). */
export function isBattery(r: ReceiptListItem): boolean {
  return r.kind === 'battery_intake' || r.kind === 'battery_tradein'
}

/** Куда ведёт строка: накладная, оприходование, чек; приём за наличные — окно на месте (null). */
export function receiptHref(r: ReceiptListItem): string | null {
  switch (r.kind) {
    case 'stock_in':
      return `/stock-ins/${r.id}`
    case 'battery_intake':
      return null
    case 'battery_tradein':
      return `/sales/${r.id}`
    default:
      return `/receipts/${r.id}`
  }
}

/** Вид строки словами — значок и столбец «Вид» выгрузки. */
export function receiptKindText(r: ReceiptListItem): string {
  switch (r.kind) {
    case 'stock_in':
      return 'Оприходование'
    case 'battery_intake':
      return 'Приём аккумуляторов'
    case 'battery_tradein':
      return (r.grams ?? 0) < 0 ? 'Аккумулятор: возврат чека' : 'Аккумулятор в оплату чека'
    default:
      return 'От поставщика'
  }
}

/** Что принято: «12,5 кг по 100 с/кг» у приёма, «чек № 7 · 3 кг» у оплаты чека. */
export function batteryText(r: ReceiptListItem): string {
  const grams = r.grams ?? 0
  if (r.kind === 'battery_tradein') {
    return grams < 0 ? `возврат чека № ${r.number} · −${qtyText('g', -grams)}` : `чек № ${r.number} · ${qtyText('g', grams)}`
  }
  const per = r.price_per_kg_tyiyn
  return per == null ? qtyText('g', grams) : `${qtyText('g', grams)} по ${formatSom(per)}/кг`
}
