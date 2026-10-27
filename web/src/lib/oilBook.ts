// Масляная книжка: типы ответа и тексты строк книжки (SPEC-16, SPEC-27).
import type { ClientNote } from './clientNotes'
import { formatDate } from './format'
import { mileageText, type MileageUnit } from './mileage'
import { BOOK_SLOT_LABELS, type BookSlot } from './types'

/** Строка замены по книжке: масло, фильтр по виду, прочие работы. */
export interface BookLine {
  slot: BookSlot
  text: string
  /** Услуга чека по строке (SPEC-34). */
  service?: string | null
  /** Следующую замену задал кассир: пробег в единице машины и дата; пустая — не напоминать. */
  target_set?: boolean
  next_mileage?: number | null
  next_unit?: MileageUnit | null
  next_km?: number | null
  next_date?: string | null
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
  /** Единица пробега машины и пробег последней и следующей замены в ней (ADR-084). */
  unit?: MileageUnit
  last_mileage?: number | null
  next_mileage?: number | null
}

export interface OilRecord {
  id: string
  change_date: string
  mileage_km: number | null
  /** Пробег, как его вписали, и единица (ADR-084). */
  mileage?: number | null
  mileage_unit?: MileageUnit
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
  /** Пометка точки к машине. */
  comment?: string
  /** Машина в списке клиента; убранная клиентом в боте — false (ADR-062). */
  active?: boolean
  /** Пробег машины — в км или милях (ADR-084). */
  mileage_unit?: MileageUnit
  /** Следующая замена масла в единице машины. */
  next_mileage?: number | null
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

/** «следующая замена на 225 000 км» — цель кассира у строки, только пробег (ADR-115); у цели без
 *  пробега — «по сроку до 05.01.2027»; без цели — ''. */
export function lineNextText(l: BookLine): string {
  if (l.next_mileage != null) return `следующая замена на ${mileageText(l.next_mileage, l.next_unit ?? 'km')}`
  return l.next_date ? `следующая замена по сроку до ${formatDate(l.next_date)}` : ''
}

/** Жидкости книжки (ADR-116, ADR-117): масло двигателя, масло в коробке, антифриз, тормозная. */
export const isFluid = (slot: BookSlot): boolean => slot === 'oil' || slot === 'gear_oil' || slot === 'antifreeze' || slot === 'brake_fluid'

/** Чем залили строку книжки: товар с количеством; строка по услуге без товара — своё у клиента. */
export function fluidText(l: BookLine): string {
  const own = l.text === (l.service ?? '') || l.text === slotLabel(l.slot)
  if (!own) return l.text
  return l.slot === 'oil' || l.slot === 'gear_oil' ? 'масло клиента' : 'жидкость клиента'
}

/** Что сделали одной строкой: «Масло 5W-30 · Фильтр W712». */
export function recordText(r: OilRecord): string {
  return recordLines(r)
    .map((l) => l.text)
    .join(' · ')
}

export const slotLabel = (slot: BookSlot): string => BOOK_SLOT_LABELS[slot] ?? slot

/** Пробег записи, как его вписали: «57 790 миль»; без пробега — null. */
export function recordMileage(r: Pick<OilRecord, 'mileage' | 'mileage_unit' | 'mileage_km'>): string | null {
  if (r.mileage !== undefined && r.mileage !== null) return mileageText(r.mileage, r.mileage_unit)
  return r.mileage_km !== null ? mileageText(r.mileage_km, 'km') : null
}

/** «на 93 000 км» (в милях — у машины в милях) — только пробег (ADR-115); у строки без пробега —
 *  «по сроку до 12.04.2027»; пусто — срока нет. */
export function slotNextText(n: Pick<SlotNext, 'next_km' | 'next_date' | 'next_mileage' | 'unit'>): string {
  const at = n.next_mileage !== undefined && n.next_mileage !== null ? mileageText(n.next_mileage, n.unit) : n.next_km !== null ? km(n.next_km) : null
  if (at !== null) return `на ${at}`
  return n.next_date ? `по сроку до ${formatDate(n.next_date)}` : ''
}

/** Следующая замена масла одной строкой: «на 93 000 км». */
export function nextText(b: VehicleBook): string {
  return slotNextText({ ...b, unit: b.mileage_unit }) || 'пока не с чего считать'
}

/** Строки книжки со сроком следующей замены: масло первым. */
export function slotsWithNext(b: VehicleBook): SlotNext[] {
  return (b.slots ?? []).filter((n) => slotNextText(n) !== '')
}

/** Строки книжки, которые меняли в чеке `saleId` (по его записи); записи нет — ни одной. */
function saleSlots(b: VehicleBook, saleId: string): Set<string> {
  const r = b.records.find((x) => x.sale_id === saleId)
  return new Set(r ? recordLines(r).map((l) => l.slot) : [])
}

/**
 * Следующая замена по строкам этого чека — итог чека и книжка, напечатанная после него (ADR-111).
 * Строка, заменённая в прошлый раз (воздушный фильтр из прошлого чека), сюда не входит: иначе
 * «Следующая · Фильтр воздушный» после каждого чека машины читалась как фильтр в этом чеке.
 */
export function slotsOfSale(b: VehicleBook, saleId: string): SlotNext[] {
  const own = saleSlots(b, saleId)
  return slotsWithNext(b).filter((n) => own.has(n.slot))
}

/** Самый ранний срок по строкам: просрочена ли замена хоть по одной. */
export function overdueSlots(b: VehicleBook, today: string): SlotNext[] {
  return (b.slots ?? []).filter((n) => n.next_date !== null && n.next_date < today)
}

/**
 * Строка книжки категории в списке категорий. У фильтров товар пишется по своему «Типу фильтра»
 * (как на сервере, `product_slot`); строка категории — только для товаров без типа. Поэтому
 * «Воздушные фильтры» со строкой по умолчанию не показываются как «Фильтр масляный» (проверка 20.10.2026).
 */
export function categorySlotText(c: { book_slot: BookSlot | 'none'; attributes: { key: string }[] }): { main: string; note: string | null } {
  if (c.book_slot === 'none') return { main: '—', note: null }
  const label = slotLabel(c.book_slot)
  if (c.book_slot.endsWith('_filter') && c.attributes.some((a) => a.key === 'filter_type')) {
    return { main: 'По типу фильтра', note: `без типа — ${label.toLowerCase()}` }
  }
  return { main: label, note: null }
}
