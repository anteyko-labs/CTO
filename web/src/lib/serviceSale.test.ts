import { describe, expect, it } from 'vitest'
import { clientNamed, isDiscount, looksLikePlate, oilAmountText, oilEnough, returnWithholds, serviceClientMissing, servicePrice, servicePriceText, withholdAmount } from './serviceSale'
import type { Party } from './types'

const money = (v: number) => `${v / 100} с`

describe('цена вида замены по маслу того же вида (ADR-096, ADR-104)', () => {
  const motor = { price_tyiyn: 20_000, free_with_oil: true, oil_category_id: 'motor' }
  const bare = { price_tyiyn: 30_000, free_with_oil: true, oil_category_id: null }
  const own = { price_tyiyn: 40_000, free_with_oil: false, oil_category_id: null }
  const can = (categoryId: string, units: number, containerMl = 4000, gift = false) => ({ categoryId, categoryKind: 'oil', units, containerMl, gift })
  it('канистра своей категории — бесплатно; долив, чужая категория, подарок — цена клиенту', () => {
    expect(servicePrice(motor, oilEnough([can('motor', 4000)]))).toBe(0)
    expect(servicePrice(motor, oilEnough([can('motor', 500)]))).toBe(20_000)
    expect(servicePrice(motor, oilEnough([can('gear', 4000)]))).toBe(20_000)
    expect(servicePrice(motor, oilEnough([can('motor', 4000, 4000, true)]))).toBe(20_000)
    // Розлив в мл вместе с канистрами; канистра — наименьшая среди строк.
    expect(servicePrice(motor, oilEnough([can('motor', 1000, 1000), can('motor', 500, 4000)]))).toBe(0)
    expect(servicePrice(motor, oilEnough([can('motor', 3500, 4000)]))).toBe(20_000)
    expect(servicePrice(bare, oilEnough([can('motor', 4000)]))).toBe(30_000)
    expect(servicePrice(own, oilEnough([can('motor', 4000)]))).toBe(40_000)
    expect(servicePrice(undefined, new Set())).toBe(0)
  })
  it('подпись на кнопке', () => {
    expect(servicePriceText(motor, new Set(['motor']), money)).toBe('бесплатно — масло наше')
    expect(servicePriceText(motor, new Set(), money)).toBe('200 с')
  })
  it('возврат масла ниже канистры удерживает цену оставшейся замены', () => {
    const lines = [
      { line_no: 1, kind: 'container', gift: false, product_id: 'p', container_ml: 4000, qty: 1, units: 4000, oil_category_id: 'motor' },
      { line_no: 2, kind: 'pour', gift: false, product_id: 'p', container_ml: 4000, qty: 1000, units: 1000, oil_category_id: 'motor' },
      { line_no: 3, kind: 'service', gift: false, product_id: null, container_ml: null, qty: 1, units: 0, oil_category_id: 'motor', withhold_price_tyiyn: 20_000 },
    ]
    const map = (m: Record<number, number>) => (n: number) => m[n] ?? 0
    // Вернули литр — осталась канистра: не удерживается.
    expect(returnWithholds(lines, map({}), map({ 2: 1000 }), 50_000)).toEqual([])
    // Вернули канистру — остался 1 л: удерживается 200 с.
    expect(returnWithholds(lines, map({}), map({ 1: 1 }), 200_000)).toEqual([{ line_no: 3, amount: 20_000 }])
    // Замену вернули вместе с маслом — нет.
    expect(returnWithholds(lines, map({}), map({ 1: 1, 3: 1 }), 200_000)).toEqual([])
    // Не больше суммы возврата.
    expect(returnWithholds(lines, map({ 1: 1 }), map({ 2: 1000 }), 0)).toEqual([])
    expect(withholdAmount(4000, 0, 4000, 2, 20_000)).toBe(40_000)
    expect(withholdAmount(3000, 0, 4000, 1, 20_000)).toBe(0)
  })
})

const person = (extra: Partial<Party>): Pick<Party, 'kind' | 'name' | 'first_name' | 'last_name' | 'phone'> => ({
  kind: 'person',
  name: 'Иванов Иван',
  first_name: 'Иван',
  last_name: 'Иванов',
  phone: '0555123456',
  ...extra,
})

describe('данные клиента для «В сервис» (ADR-100)', () => {
  it('человек — имя и фамилия, фирма — название, прежняя карточка — два слова', () => {
    expect(clientNamed(person({}))).toBe(true)
    expect(clientNamed(person({ last_name: '' }))).toBe(false)
    expect(clientNamed(person({ first_name: '', last_name: '', name: 'Азамат' }))).toBe(false)
    expect(clientNamed(person({ first_name: '', last_name: '', name: 'Азамат Уулу' }))).toBe(true)
    expect(clientNamed({ kind: 'company', name: 'ОсОО Транс', first_name: '', last_name: '' })).toBe(true)
  })
  it('недостающее — в порядке сервера', () => {
    expect(serviceClientMissing({ party: null, contactChosen: false, vehicleChosen: false, mileage: '' })).toEqual(['client', 'phone', 'vehicle', 'mileage'])
    expect(serviceClientMissing({ party: person({}), contactChosen: false, vehicleChosen: true, mileage: '120000' })).toEqual([])
    expect(serviceClientMissing({ party: person({ phone: ' ' }), contactChosen: false, vehicleChosen: true, mileage: '0' })).toEqual(['phone', 'mileage'])
    // Телефон фирмы может быть у того, кто приехал: проверит сервер.
    expect(serviceClientMissing({ party: { kind: 'company', name: 'Транс', phone: '' }, contactChosen: true, vehicleChosen: false, mileage: '5' })).toEqual(['vehicle'])
  })
})

describe('скидка (ADR-097)', () => {
  it('ниже каталога — скидка; подарок, та же и выше — нет', () => {
    expect(isDiscount(1_900, 2_000)).toBe(true)
    expect(isDiscount(2_000, 2_000)).toBe(false)
    expect(isDiscount(2_500, 2_000)).toBe(false)
    expect(isDiscount(0, 2_000, true)).toBe(false)
    expect(isDiscount(null, 2_000)).toBe(false)
  })
})

describe('масло в заменах', () => {
  it('канистры, розлив и всего', () => {
    expect(oilAmountText({ canisters: 2, pour_ml: 1_500, total_ml: 9_500, container_ml: 4_000 })).toBe('2 кан. по 4 л + 1,5 л розливом = 9,5 л')
    expect(oilAmountText({ canisters: 0, pour_ml: 3_000, total_ml: 3_000, container_ml: 4_000 })).toBe('3 л розливом = 3 л')
  })
})

describe('looksLikePlate', () => {
  it('госномер — да; имя, телефон и ПИН — нет', () => {
    expect(looksLikePlate('01KG123ABC')).toBe(true)
    expect(looksLikePlate('B 1234 AB')).toBe(true)
    expect(looksLikePlate('123')).toBe(true)
    expect(looksLikePlate('Нурлан')).toBe(false)
    expect(looksLikePlate('0555 123 456')).toBe(false)
    expect(looksLikePlate('+996555123456')).toBe(false)
    expect(looksLikePlate('22501199000123')).toBe(false)
  })
})
