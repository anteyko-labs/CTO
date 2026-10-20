import { describe, expect, it } from 'vitest'
import { formatLiters, formatOilStock, formatPercentBp, formatSom, formatSomExact, litersInput, parseLiters, parsePercentBp, parseSom, plural, shiftDate, somInput } from './format'

const nb = (s: string) => s.replace(/\u00a0/g, ' ')

describe('формат', () => {
  it('сомы', () => {
    expect(nb(formatSom(123450))).toBe('1 234,50 с')
    expect(nb(formatSom(5))).toBe('0,05 с')
    expect(nb(formatSom(-1500))).toBe('−15 с')
    expect(nb(formatSom(3285000))).toBe('32 850 с')
    expect(nb(formatSomExact(3285000))).toBe('32 850,00 с')
    expect(somInput(123450)).toBe('1234,50')
    expect(somInput(40000)).toBe('400')
  })

  it('литры и масло', () => {
    expect(nb(formatLiters(2500))).toBe('2,5 л')
    expect(nb(formatLiters(4000))).toBe('4 л')
    expect(nb(formatLiters(125))).toBe('0,125 л')
    expect(nb(formatOilStock(14500, 4000))).toBe('14,5 л = 3 кан. по 4 л + 2,5 л')
    expect(nb(formatOilStock(8000, 4000))).toBe('8 л = 2 кан. по 4 л')
    expect(nb(formatOilStock(4000, 4000))).toBe('4 л')
    expect(nb(formatOilStock(2500, 4000))).toBe('2,5 л')
    expect(nb(formatOilStock(0, 4000))).toBe('0 л')
  })

  it('литры и канистры сходятся до миллилитра', () => {
    // Остаток в 1 мл сверх канистры не теряется и не округляется.
    expect(nb(formatOilStock(4001, 4000))).toBe('4,001 л = 1 кан. по 4 л + 0,001 л')
    expect(nb(formatOilStock(3999, 4000))).toBe('3,999 л')
    // Тара 4,5 л.
    expect(nb(formatOilStock(9000, 4500))).toBe('9 л = 2 кан. по 4,5 л')
    expect(nb(formatOilStock(9100, 4500))).toBe('9,1 л = 2 кан. по 4,5 л + 0,1 л')
    // Минус — просто литры со знаком, без канистр.
    expect(nb(formatOilStock(-500, 4000))).toBe('−0,5 л')
    expect(nb(formatLiters(-2500))).toBe('−2,5 л')
    expect(nb(formatLiters(1))).toBe('0,001 л')
    // 0,1 л — ровно 100 мл, ввод и показ туда и обратно.
    expect(parseLiters('0,1')).toBe(100)
    expect(parseLiters('0,001')).toBe(1)
    expect(parseLiters('4,5')).toBe(4500)
    expect(parseLiters('0,0001')).toBeNull()
    for (const ml of [1, 100, 999, 4001, 14500, 199999]) {
      expect(parseLiters(litersInput(ml))).toBe(ml)
    }
    expect(litersInput(12500)).toBe('12,5')
    expect(litersInput(-500)).toBe('-0,5')
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

  it('ставка в процентах хранится целыми базисными пунктами', () => {
    expect(parsePercentBp('0,5')).toBe(50)
    expect(parsePercentBp('2')).toBe(200)
    expect(parsePercentBp('1,25 %')).toBe(125)
    expect(parsePercentBp('0,125')).toBeNull()
    expect(parsePercentBp('-1')).toBeNull()
    expect(nb(formatPercentBp(50))).toBe('0,50 %')
    expect(nb(formatPercentBp(200))).toBe('2,00 %')
  })
})

describe('слово при числе', () => {
  it('позиция, позиции, позиций', () => {
    const f = ['позиция', 'позиции', 'позиций'] as const
    expect([1, 3, 5, 11, 12, 21, 22, 25, 111, 1003].map((n) => `${n} ${plural(n, f)}`)).toEqual([
      '1 позиция',
      '3 позиции',
      '5 позиций',
      '11 позиций',
      '12 позиций',
      '21 позиция',
      '22 позиции',
      '25 позиций',
      '111 позиций',
      '1003 позиции',
    ])
  })
})
