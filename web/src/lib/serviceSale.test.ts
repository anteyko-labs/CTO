import { describe, expect, it } from 'vitest'
import { clientNamed, isDiscount, oilAmountText, serviceClientMissing, servicePrice, servicePriceText } from './serviceSale'
import type { Party } from './types'

const money = (v: number) => `${v / 100} с`

describe('цена вида замены (ADR-096)', () => {
  const motor = { price_tyiyn: 20_000, free_with_oil: true }
  const own = { price_tyiyn: 40_000, free_with_oil: false }
  it('с нашим маслом — бесплатно, без масла — цена клиенту', () => {
    expect(servicePrice(motor, true)).toBe(0)
    expect(servicePrice(motor, false)).toBe(20_000)
    expect(servicePrice(own, true)).toBe(40_000)
    expect(servicePrice(undefined, true)).toBe(0)
  })
  it('подпись на кнопке', () => {
    expect(servicePriceText(motor, true, money)).toBe('бесплатно — масло наше')
    expect(servicePriceText(motor, false, money)).toBe('200 с')
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
