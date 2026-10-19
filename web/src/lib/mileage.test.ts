import { describe, expect, it } from 'vitest'
import { fromKm, kmToMi, mileageText, miToKm, toKm } from './mileage'

describe('пробег в км и милях', () => {
  it('переводит целыми с одним правилом округления, как сервер', () => {
    expect(miToKm(1)).toBe(2)
    expect(miToKm(100)).toBe(161)
    expect(miToKm(50_000)).toBe(80_467)
    expect(miToKm(3000)).toBe(4828)
    expect(kmToMi(8000)).toBe(4971)
    expect(kmToMi(80_467)).toBe(50_000)
    expect(kmToMi(85_295)).toBe(53_000)
    expect(toKm(1000, 'mi')).toBe(1609)
    expect(toKm(1000, 'km')).toBe(1000)
    expect(fromKm(1609, 'mi')).toBe(1000)
  })

  it('пишет пробег с единицей', () => {
    expect(mileageText(57_790, 'mi')).toBe('57 790 миль')
    expect(mileageText(93_000)).toBe('93 000 км')
  })
})
