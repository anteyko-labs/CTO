import { describe, expect, it } from 'vitest'
import { formatLiters, formatOilStock, formatSom, parseLiters, parseSom, shiftDate, somInput } from './format'

const nb = (s: string) => s.replace(/\u00a0/g, ' ')

describe('формат', () => {
  it('сомы', () => {
    expect(nb(formatSom(123450))).toBe('1 234,50 с')
    expect(nb(formatSom(5))).toBe('0,05 с')
    expect(nb(formatSom(-1500))).toBe('−15,00 с')
    expect(somInput(123450)).toBe('1234,50')
  })

  it('литры и масло', () => {
    expect(nb(formatLiters(2500))).toBe('2,5 л')
    expect(nb(formatLiters(4000))).toBe('4 л')
    expect(nb(formatLiters(125))).toBe('0,125 л')
    expect(nb(formatOilStock(14500, 4000))).toBe('14,5 л · 3 кан. по 4 л + 2,5 л')
    expect(nb(formatOilStock(8000, 4000))).toBe('8 л · 2 кан. по 4 л')
    expect(nb(formatOilStock(4000, 4000))).toBe('4 л')
    expect(nb(formatOilStock(2500, 4000))).toBe('2,5 л')
    expect(nb(formatOilStock(0, 4000))).toBe('0 л')
  })

  it('разбор ввода без дробных чисел', () => {
    expect(parseSom('1 234,5')).toBe(123450)
    expect(parseSom('0.1')).toBe(10)
    expect(parseSom('333,33')).toBe(33333)
    expect(parseSom('1,234')).toBeNull()
    expect(parseSom('-5')).toBeNull()
    expect(parseSom('abc')).toBeNull()
    expect(parseLiters('1,5')).toBe(1500)
    expect(parseLiters('0,1')).toBe(100)
  })

  it('сдвиг даты не зависит от часового пояса устройства', () => {
    expect(shiftDate('2026-03-01', -1)).toBe('2026-02-28')
    expect(shiftDate('2026-10-08', -7)).toBe('2026-10-01')
    expect(shiftDate('2026-12-31', 1)).toBe('2027-01-01')
  })
})
