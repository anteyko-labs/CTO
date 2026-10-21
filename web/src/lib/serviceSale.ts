// Чек «В сервис» на кассе (SPEC-30 §2–4): цена вида замены, данные клиента и скидка — те же
// правила, что на сервере (ADR-096, ADR-100, ADR-097). Сервер решает окончательно; здесь —
// чтобы касса показала цену и недостающее до «Провести».
import { formatLiters } from './format'
import type { Party, Service, ServiceOil } from './types'

/** Цена услуги по каталогу: «бесплатно с нашим маслом» и в чеке есть наше масло — 0, иначе —
 *  цена клиенту из справочника. */
export function servicePrice(s: Pick<Service, 'price_tyiyn' | 'free_with_oil'> | undefined, ourOil: boolean): number {
  if (!s) return 0
  return s.free_with_oil && ourOil ? 0 : s.price_tyiyn
}

/** Подпись цены на кнопке выбора вида замены. */
export function servicePriceText(s: Pick<Service, 'price_tyiyn' | 'free_with_oil'>, ourOil: boolean, money: (v: number) => string): string {
  if (s.free_with_oil && ourOil) return 'бесплатно — масло наше'
  return money(s.price_tyiyn)
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
