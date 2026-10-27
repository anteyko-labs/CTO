// Чек «В сервис» на кассе (SPEC-30 §2–4): цена вида замены, данные клиента и скидка — те же
// правила, что на сервере (ADR-096, ADR-100, ADR-097). Сервер решает окончательно; здесь —
// чтобы касса показала цену и недостающее до «Провести».
import { formatLiters } from './format'
import type { Party, Service, ServiceOil } from './types'

/** Категории масла, которого в чеке не меньше канистры (ADR-104): строки нашего масла (не подарок)
 *  по категориям, канистра — наименьший объём канистры среди строк категории, розлив — в мл. */
export type OilEnough = ReadonlySet<string>

export interface OilLine {
  categoryId?: string | null
  categoryKind?: string | null
  /** Сколько мл в строке: канистры × объём или розлив. */
  units: number
  containerMl?: number | null
  gift?: boolean
}

/** Наименьшая канистра среди строк; нет объёма — null. */
export function canisterMl(lines: readonly OilLine[]): number | null {
  const sizes = lines.map((l) => l.containerMl ?? 0).filter((c) => c > 0)
  return sizes.length ? Math.min(...sizes) : null
}

export function oilEnough(lines: readonly OilLine[]): OilEnough {
  const by = new Map<string, OilLine[]>()
  for (const l of lines) {
    if (l.gift || l.categoryKind !== 'oil' || !l.categoryId) continue
    by.set(l.categoryId, [...(by.get(l.categoryId) ?? []), l])
  }
  const out = new Set<string>()
  for (const [cat, list] of by) {
    const can = canisterMl(list)
    if (can !== null && list.reduce((acc, l) => acc + l.units, 0) >= can) out.add(cat)
  }
  return out
}

/**
 * Окно «Вид замены» (SPEC-36 §5): без доливок (они на вынос, ADR-118); сначала замены — услуги со
 * строкой книжки, потом остальные; внутри — «бесплатно с нашим маслом» выше, дальше по порядку списка.
 */
export function kindChoices<T extends Pick<Service, 'book_slot' | 'free_with_oil' | 'takeaway'>>(list: readonly T[]): T[] {
  const change = (s: T) => Number(Boolean(s.book_slot && s.book_slot !== 'other'))
  return list.filter((s) => !s.takeaway).sort((a, b) => change(b) - change(a) || Number(Boolean(b.free_with_oil)) - Number(Boolean(a.free_with_oil)))
}

type PricedService = Pick<Service, 'price_tyiyn' | 'free_with_oil' | 'oil_category_id'>

/** Услуга бесплатна: «бесплатно с нашим маслом», категория масла выбрана и такого масла в чеке
 *  не меньше канистры. Без категории флажок не действует. */
export function serviceFree(s: PricedService | undefined, enough: OilEnough): boolean {
  return Boolean(s?.free_with_oil && s.oil_category_id && enough.has(s.oil_category_id))
}

/** Цена услуги по каталогу: бесплатна по маслу — 0, иначе — цена клиенту из справочника. */
export function servicePrice(s: PricedService | undefined, enough: OilEnough): number {
  if (!s) return 0
  return serviceFree(s, enough) ? 0 : s.price_tyiyn
}

/** Подпись цены на кнопке выбора вида замены. */
export function servicePriceText(s: PricedService, enough: OilEnough, money: (v: number) => string): string {
  if (serviceFree(s, enough)) return 'бесплатно — масло наше'
  return money(s.price_tyiyn)
}

/** Удержание за замену при возврате масла (ADR-104) — то же правило, что на сервере: масла
 *  категории было не меньше канистры, после возврата меньше, а услуга остаётся. */
export function withholdAmount(beforeMl: number, afterMl: number, canister: number, servicesLeft: number, price: number): number {
  if (canister <= 0 || servicesLeft <= 0 || price <= 0) return 0
  return beforeMl >= canister && afterMl < canister ? price * servicesLeft : 0
}

/** Строка исходного чека для расчёта удержания. */
export interface ReturnableLine {
  line_no: number
  kind: string
  gift: boolean
  product_id: string | null
  container_ml: number | null
  qty: number
  units: number
  oil_category_id?: string | null
  withhold_price_tyiyn?: number | null
}

/** Что удержит возврат: по каждой бесплатной замене, ставшей платной. `done` — сколько по строке
 *  уже вернули, `now` — сколько возвращают сейчас (в единицах строки). Итог не больше `refund`. */
export function returnWithholds(
  lines: readonly ReturnableLine[],
  done: (lineNo: number) => number,
  now: (lineNo: number) => number,
  refund: number,
): { line_no: number; amount: number }[] {
  const unitsOf = (l: ReturnableLine, qty: number) => (l.qty ? Math.round((l.units * qty) / l.qty) : 0)
  const out: { line_no: number; amount: number }[] = []
  let room = refund
  for (const s of lines) {
    const price = s.withhold_price_tyiyn ?? 0
    if (s.kind !== 'service' || !price || !s.oil_category_id) continue
    const oil = lines.filter((l) => !l.gift && l.product_id && l.oil_category_id === s.oil_category_id)
    const can = canisterMl(oil.map((l) => ({ units: l.units, containerMl: l.container_ml })))
    if (can === null) continue
    const before = oil.reduce((acc, l) => acc + l.units - unitsOf(l, done(l.line_no)), 0)
    const back = oil.reduce((acc, l) => acc + unitsOf(l, now(l.line_no)), 0)
    if (back <= 0) continue
    const amount = Math.min(withholdAmount(before, before - back, can, s.qty - done(s.line_no) - now(s.line_no), price), room)
    if (amount > 0) {
      out.push({ line_no: s.line_no, amount })
      room -= amount
    }
  }
  return out
}

/** Клиент назван: фирма — названием, человек — именем и фамилией; у прежних карточек без
 *  отдельных имени и фамилии хватает полного имени из двух слов (как на сервере). */
export function clientNamed(p: Pick<Party, 'kind' | 'name' | 'first_name' | 'last_name'>): boolean {
  if (p.kind === 'company') return p.name.trim() !== ''
  const first = (p.first_name ?? '').trim()
  const last = (p.last_name ?? '').trim()
  if (first || last) return Boolean(first && last)
  return p.name.trim().split(/\s+/).filter(Boolean).length >= 2
}

/** Что из данных клиента нужно чеку «в сервис», как оно называется в подсказке и куда поставить
 *  курсор. Порядок — как в ответе сервера. */
export type ClientNeed = 'client' | 'phone' | 'vehicle' | 'mileage'

export const CLIENT_NEED_LABELS: Record<ClientNeed, string> = {
  client: 'клиент (имя и фамилия или фирма)',
  phone: 'телефон клиента',
  vehicle: 'машина с госномером',
  mileage: 'пробег',
}

/** То же коротко — для строки «Не хватает: …» в режиме кассы. */
export const CLIENT_NEED_SHORT: Record<ClientNeed, string> = {
  client: 'имя клиента',
  phone: 'телефон',
  vehicle: 'машина',
  mileage: 'пробег',
}

/** Чего не хватает чеку «в сервис» (ADR-100). Телефон фирмы может быть у того, кто приехал:
 *  касса его не знает — проверит сервер. */
export function serviceClientMissing(input: {
  party: Pick<Party, 'kind' | 'name' | 'first_name' | 'last_name' | 'phone'> | null
  contactChosen: boolean
  vehicleChosen: boolean
  mileage: string
}): ClientNeed[] {
  const { party } = input
  const out: ClientNeed[] = []
  if (!party || !clientNamed(party)) out.push('client')
  if (!party || (!party.phone.trim() && !(party.kind === 'company' && input.contactChosen))) out.push('phone')
  if (!party || !input.vehicleChosen) out.push('vehicle')
  if (!(Number(input.mileage) > 0)) out.push('mileage')
  return out
}

/** Скидка — цена строки ниже цены по каталогу (ADR-097). Подарок и цена выше — не скидка;
 *  баллы — оплата, а не цена строки. */
export function isDiscount(price: number | null, list: number, gift = false): boolean {
  return !gift && price !== null && price < list
}

/** Сколько масла залито в чеках вида замены: «2 кан. по 4 л + 1,5 л розливом = 9,5 л». */
export function oilAmountText(o: Pick<ServiceOil, 'canisters' | 'pour_ml' | 'total_ml' | 'container_ml'>): string {
  const parts: string[] = []
  if (o.canisters !== 0) parts.push(`${o.canisters} кан.${o.container_ml ? ` по ${formatLiters(o.container_ml)}` : ''}`)
  if (o.pour_ml !== 0) parts.push(`${formatLiters(o.pour_ml)} розливом`)
  const total = formatLiters(o.total_ml)
  return parts.length ? `${parts.join(' + ')} = ${total}` : total
}

/** Поиск клиента похож на госномер: есть цифры, не телефон и не ПИН/ИНН (те — одни цифры, от 9).
 *  Только тогда машину клиента ищут по набранному; иначе ставится его единственная машина. */
export function looksLikePlate(text: string): boolean {
  const t = text.trim()
  if (!/\d/.test(t) || t.length > 12) return false
  const onlyDigits = /^[\d\s+()-]+$/.test(t)
  return !(onlyDigits && t.replace(/\D/g, '').length >= 9)
}
