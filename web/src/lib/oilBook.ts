// Масляная книжка: типы ответа и тексты строк книжки (SPEC-16, SPEC-27).
import type { ClientNote } from './clientNotes'
import { formatDate } from './format'
import { mileageText, type MileageUnit } from './mileage'
import { BOOK_SLOT_LABELS, type BookSlot } from './types'

/** Строка замены по книжке: масло, фильтр по виду, прочие работы. */
export interface BookLine {
  slot: BookSlot
  text: string
  /** Услуга чека по строке (SPEC-34); у «прочего» — своя строка у каждой услуги (ADR-131). */
  service?: string | null
  service_id?: string | null
  /** Следующую замену задал кассир: пробег в единице машины и дата; пустая — не напоминать. */
  target_set?: boolean
  next_mileage?: number | null
  next_unit?: MileageUnit | null
  next_km?: number | null
  next_date?: string | null
  /** Подпись — что залито или поставлено (сервер, `oil_book::title_of`, задача 003 Н5). */
  title?: string
  /** Вид замены: услуга строки, у фильтров с маслом — услуга масла (`oil_book::line_group`). */
  group?: string | null
}

/** Следующая замена по строке книжки; у «прочего» — по каждой услуге, с её названием (ADR-131). */
export interface SlotNext {
  slot: BookSlot
  /** Услуга «прочее», чья это следующая замена; у остальных строк — нет. */
  service_id?: string | null
  /** Название строки книжки («Масло в коробке»), у «прочего» — услуги: для интервала и выбора строки. */
  label: string
  /** Что залито или поставлено в последнюю замену строки: «Mobil ATF 3309 · 1 л» (задача 003 Н5) —
   *  под этой подписью следующая замена; у старого ответа нет — `label`. */
  title?: string
  /** Товары последней замены без количества. */
  products?: string[]
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

/** Своё у клиента: строка жидкости по услуге без товара. */
export const ownFluid = (slot: BookSlot): string => (slot === 'oil' || slot === 'gear_oil' ? 'масло клиента' : 'жидкость клиента')

/** Пометка строки книжки в скобках — когда без неё непонятно: «масло клиента (коробка)». */
const SLOT_NOTE: Partial<Record<BookSlot, string>> = {
  oil: 'двигатель',
  gear_oil: 'коробка',
  oil_filter: 'масляный',
  air_filter: 'воздушный',
  cabin_filter: 'салонный',
  fuel_filter: 'топливный',
  antifreeze: 'антифриз',
  brake_fluid: 'тормозная',
}

/** Вид фильтра в названии: «Фильтр топливный WK 820», «Замена топливного фильтра». */
const FILTER_STEM: Partial<Record<BookSlot, string>> = { oil_filter: 'масл', air_filter: 'воздуш', cabin_filter: 'салон', fuel_filter: 'топлив' }

/**
 * Подпись строки книжки — что залито или поставлено (задача 003 Н5), как `oil_book::title_of` на
 * сервере: товар с количеством («Mobil ATF 3309 · 1 л»); фильтр без вида в названии — с видом в
 * скобках («W712 (масляный) · 1 шт»); жидкость по услуге без товара — «масло клиента (коробка)»;
 * работа без товара — название услуги. Название строки книжки вместо товара не пишется.
 */
export function titleOf(slot: BookSlot, text: string, service?: string | null): string {
  const note = SLOT_NOTE[slot] ?? ''
  if (isFluid(slot) && service != null && service === text) return `${ownFluid(slot)} (${note})`
  const stem = FILTER_STEM[slot]
  if (stem && !text.toLowerCase().includes(stem)) {
    const at = text.lastIndexOf(' · ')
    return at >= 0 ? `${text.slice(0, at)} (${note}) · ${text.slice(at + 3)}` : `${text} (${note})`
  }
  return text
}

/** Подпись строки замены: от сервера, у старого ответа — по тому же правилу. */
export const lineTitle = (l: BookLine): string => l.title || titleOf(l.slot, l.text, l.service)

/** Подпись следующей замены строки книжки: что залили в последний раз (задача 003 Н5). */
export const slotTitle = (n: Pick<SlotNext, 'title' | 'label'>): string => n.title || n.label

/** Фильтры, которые меняют с маслом двигателя (ADR-117): в чеке книжки — под заменой масла. */
export const followsOil = (slot: BookSlot): boolean => slot === 'oil_filter' || slot === 'air_filter' || slot === 'cabin_filter'

/** Вид замены строки: от сервера; у старого ответа — услуга строки, у фильтра с маслом — услуга масла. */
export function lineGroup(l: BookLine, all: readonly BookLine[]): string | null {
  if (l.group !== undefined) return l.group
  if (l.service) return l.service
  return followsOil(l.slot) ? (all.find((o) => o.slot === 'oil' && o.service)?.service ?? null) : null
}

/** Строка чека книжки: подпись, вид замены и следующая замена («на 138 000 км»; '' — без срока). */
export interface BookItem {
  title: string
  group: string | null
  next: string
}

/** Вид замены в чеке книжки: услуга, её строки и общая следующая замена (если у всех строк одна). */
export interface BookGroup {
  head: string | null
  items: BookItem[]
  next: string | null
}

/**
 * Чек книжки по видам замены (задача 003 Н5): строки одной услуги — вместе под её названием
 * («Замена масла в моторе»: масло и фильтры с ним), следующая замена — одной строкой, если у всех
 * она одна, иначе у каждой; строка без вида — сама по себе.
 */
export function bookGroups(items: readonly BookItem[]): BookGroup[] {
  const out: BookGroup[] = []
  for (const it of items) {
    const g = it.group ? out.find((x) => x.head === it.group) : undefined
    if (g) g.items.push(it)
    else out.push({ head: it.group, items: [it], next: null })
  }
  for (const g of out) {
    const first = g.items[0]?.next ?? ''
    if (first && g.items.every((i) => i.next === first)) g.next = first
  }
  return out
}

const escHtml = (s: string) =>
  s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c] ?? c)

/** Виды замены в HTML для печати (чек книжки и блок книжки товарного чека): `.bk` — вид, `.grp` —
 *  его название, `.it` — что залито или поставлено, `.nx` — следующая замена. Строка, которая
 *  повторяет название вида (работа без товара), отдельно не пишется. */
export function bookGroupsHtml(groups: readonly BookGroup[]): string {
  return groups
    .map((g) => {
      const head = g.head ? `<div class="grp">${escHtml(g.head)}</div>` : ''
      const items = g.items
        .filter((i) => i.title !== g.head)
        .map((i) => `<div class="it">${escHtml(i.title)}${!g.next && i.next ? ` — ${escHtml(i.next)}` : ''}</div>`)
        .join('')
      const next = g.next ? `<div class="nx">Следующая замена — ${escHtml(g.next)}</div>` : ''
      return `<div class="bk">${head}${items}${next}</div>`
    })
    .join('')
}

/** Что сделали одной строкой, подписью строк (`lineTitle`, задача 003 Н5), как в боте: «Mobil 1 ·
 *  4 л; W712 (масляный) · 1 шт» — «Прошлая замена» книжки и история в печати книжки. */
export function recordText(r: OilRecord): string {
  return recordLines(r).map(lineTitle).join('; ')
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

/** Строка следующей замены: строка книжки, у услуги «прочее» — `other:<услуга>` (ADR-131). */
export const slotKey = (n: { slot: BookSlot; service_id?: string | null }): string => (n.slot === 'other' && n.service_id ? `other:${n.service_id}` : n.slot)

/** Строки книжки, которые меняли в чеке `saleId` (по его записи); записи нет — ни одной. */
function saleSlots(b: VehicleBook, saleId: string): Set<string> {
  const r = b.records.find((x) => x.sale_id === saleId)
  return new Set(r ? recordLines(r).map(slotKey) : [])
}

/**
 * Следующая замена по строкам этого чека — итог чека и книжка, напечатанная после него (ADR-111).
 * Строка, заменённая в прошлый раз (воздушный фильтр из прошлого чека), сюда не входит: иначе
 * «Следующая · Фильтр воздушный» после каждого чека машины читалась как фильтр в этом чеке.
 */
export function slotsOfSale(b: VehicleBook, saleId: string): SlotNext[] {
  const own = saleSlots(b, saleId)
  return slotsWithNext(b).filter((n) => own.has(slotKey(n)))
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
export function categorySlotText(c: { book_slot: BookSlot | 'none'; attributes: { key: string; enabled?: boolean }[] }): { main: string; note: string | null } {
  if (c.book_slot === 'none') return { main: '—', note: null }
  const label = slotLabel(c.book_slot)
  // «Тип» выключен у категории (задача 002 П10) — строка от категории.
  if (c.book_slot.endsWith('_filter') && c.attributes.some((a) => a.key === 'filter_type' && a.enabled !== false)) {
    return { main: 'По типу фильтра', note: `без типа — ${label.toLowerCase()}` }
  }
  return { main: label, note: null }
}
