// Масляная книжка: типы ответа и тексты строк книжки (SPEC-16, SPEC-27).
import type { ClientNote } from './clientNotes'
import { formatDate } from './format'
import { BOOK_SLOT_LABELS, type BookSlot } from './types'

/** Строка замены по книжке: масло, фильтр по виду, прочие работы. */
export interface BookLine {
  slot: BookSlot
  text: string
}

/** Следующая замена по строке книжки. */
export interface SlotNext {
  slot: BookSlot
  label: string
  interval_km: number | null
  interval_days: number | null
  /** Интервал задан у этой машины, а не у категории. */
  own_interval: boolean
  last_date: string | null
  last_km: number | null
  last_text: string | null
  next_km: number | null
  next_date: string | null
}

export interface OilRecord {
  id: string
  change_date: string
  mileage_km: number | null
  oil_text: string
  filter_text: string
  /** Строки по книжке (SPEC-27); у старого ответа сервера их нет — тогда масло и фильтр. */
  lines?: BookLine[]
  comment: string
  /** Рекомендовали, клиент отказался (ADR-055). */
  declined?: string
  sale_id: string | null
  sale_number: number | null
  /** Итог чека за вычетом возвратов по нему; у записи из тетради — null (SPEC-22). */
  sale_total_tyiyn?: number | null
}

export interface VehicleBook {
  vehicle_id: string
  plate: string
  brand: string
  model: string
  /** Машина в списке клиента; убранная клиентом в боте — false (ADR-062). */
  active?: boolean
  /** Интервал и следующая замена масла. */
  interval_km: number
  interval_days: number
  next_km: number | null
  next_date: string | null
  /** Следующая замена по каждой строке книжки (SPEC-27). */
  slots?: SlotNext[]
  records: OilRecord[]
  /** Пометки клиента из бота (SPEC-22). */
  client_notes?: ClientNote[]
}

export const km = (v: number) => `${v.toLocaleString('ru-RU').replace(/\s/g, ' ')} км`

/** Строки записи: по книжке, а у записи без строк — масло и фильтр. */
export function recordLines(r: OilRecord): BookLine[] {
  if (r.lines && r.lines.length > 0) return r.lines
  const out: BookLine[] = []
  if (r.oil_text) out.push({ slot: 'oil', text: r.oil_text })
  if (r.filter_text) out.push({ slot: 'oil_filter', text: r.filter_text })
  return out
}

/** Что сделали одной строкой: «Масло 5W-30 · Фильтр W712». */
export function recordText(r: OilRecord): string {
  return recordLines(r)
    .map((l) => l.text)
    .join(' · ')
}

export const slotLabel = (slot: BookSlot): string => BOOK_SLOT_LABELS[slot] ?? slot

/** «на 93 000 км или до 12.04.2027»; пусто — срока нет. */
export function slotNextText(n: Pick<SlotNext, 'next_km' | 'next_date'>): string {
  return [n.next_km !== null ? `на ${km(n.next_km)}` : null, n.next_date ? `до ${formatDate(n.next_date)}` : null].filter(Boolean).join(' или ')
}

/** Следующая замена масла одной строкой: «на 93 000 км или до 12.04.2027». */
export function nextText(b: VehicleBook): string {
  return slotNextText(b) || 'пока не с чего считать'
}

/** Строки книжки со сроком следующей замены: масло первым. */
export function slotsWithNext(b: VehicleBook): SlotNext[] {
  return (b.slots ?? []).filter((n) => slotNextText(n) !== '')
}

/** Самый ранний срок по строкам: просрочена ли замена хоть по одной. */
export function overdueSlots(b: VehicleBook, today: string): SlotNext[] {
  return (b.slots ?? []).filter((n) => n.next_date !== null && n.next_date < today)
}
