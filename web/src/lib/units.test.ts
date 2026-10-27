import { describe, expect, it } from 'vitest'
import {
  avgPrice,
  byThousandths,
  containerLabel,
  countText,
  kgInput,
  oilCountText,
  oilFields,
  parseKg,
  parseOil,
  perCan,
  perLiter,
  qtyText,
  splitOil,
  viscosityFromName,
  viscosityKey,
} from './units'

describe('единицы учёта', () => {
  it('количество: литры, килограммы, штуки', () => {
    expect(qtyText('ml', 3500)).toBe('3,5 л')
    expect(qtyText('g', 12_000)).toBe('12 кг')
    expect(qtyText('piece', 3)).toBe('3 шт')
    expect(qtyText(undefined, 2)).toBe('2 шт')
  })

  it('средняя цена весового товара — за кг из стоимости остатка', () => {
    // 12 кг лома на 840 с: 70 с за кг, а не «0,07 за шт».
    expect(avgPrice({ unit: 'g', container_ml: null, stock_qty: 12_000, avg_cost_tyiyn: 7, stock_value_tyiyn: 84_000 })).toEqual({ tyiyn: 7000, per: 'за кг' })
    expect(avgPrice({ unit: 'g', container_ml: null, stock_qty: 0, avg_cost_tyiyn: 8, stock_value_tyiyn: 0 })).toEqual({ tyiyn: 8000, per: 'за кг' })
  })

  it('масло в таре — за канистру, штучный — за шт, без средней — null', () => {
    expect(avgPrice({ unit: 'ml', container_ml: 4000, stock_qty: 4000, avg_cost_tyiyn: 200_000, stock_value_tyiyn: 200_000 })).toEqual({ tyiyn: 200_000, per: 'за 4 л' })
    expect(avgPrice({ unit: 'ml', container_ml: null, stock_qty: 2000, avg_cost_tyiyn: 300, stock_value_tyiyn: 600_000 })).toEqual({ tyiyn: 300_000, per: 'за л' })
    expect(avgPrice({ unit: 'piece', container_ml: null, stock_qty: 7, avg_cost_tyiyn: 40_000, stock_value_tyiyn: 280_000 })).toEqual({ tyiyn: 40_000, per: 'за шт' })
    expect(avgPrice({ unit: 'piece', container_ml: null, stock_qty: 7 })).toBeNull()
  })
})

const nb = (s: string) => s.replace(/ /g, ' ')

describe('канистры и литры, килограммы (SPEC-36 §13)', () => {
  it('«2 кан. + 1,5 л» = 9 500 мл; пустые поля — 0, оба пустые — не считали', () => {
    expect(parseOil('2', '1,5', 4000)).toBe(9500)
    expect(parseOil('8', '', 4000)).toBe(32_000)
    expect(parseOil('', '2,5', 4000)).toBe(2500)
    expect(parseOil('0', '0', 4000)).toBe(0)
    expect(parseOil(' ', '', 4000)).toBe('empty')
    expect(parseOil('1,5', '', 4000)).toBeNull()
    expect(parseOil('1', '0,0005', 4000)).toBeNull()
    expect(parseOil('-1', '', 4000)).toBeNull()
  })

  it('обратно — целые канистры и остаток, как в formatOilStock', () => {
    expect(splitOil(34_500, 4000)).toEqual({ cans: 8, restMl: 2500 })
    expect(oilFields(34_500, 4000)).toEqual({ cans: '8', liters: '2,5' })
    expect(oilFields(8000, 4000)).toEqual({ cans: '2', liters: '' })
    expect(oilFields(0, 4000)).toEqual({ cans: '0', liters: '' })
  })

  it('показ: «8 кан. + 2,5 л», без канистр — литры, минус — со знаком', () => {
    expect(nb(oilCountText(34_500, 4000))).toBe('8 кан. + 2,5 л')
    expect(nb(oilCountText(8000, 4000))).toBe('2 кан.')
    expect(nb(oilCountText(2500, 4000))).toBe('2,5 л')
    expect(nb(oilCountText(0, 4000))).toBe('0 л')
    expect(nb(oilCountText(-9500, 4000))).toBe('−2 кан. + 1,5 л')
    expect(nb(countText({ unit: 'ml', container_ml: 4000 }, 9500))).toBe('2 кан. + 1,5 л')
    expect(nb(countText({ unit: 'g', container_ml: null }, 129_300))).toBe('129,3 кг')
    expect(countText({ unit: 'piece', container_ml: null }, 3)).toBe('3 шт')
  })

  it('подпись тары: канистра или бочка', () => {
    expect(nb(containerLabel(4000))).toBe('кан. по 4 л')
    expect(nb(containerLabel(208_000))).toBe('тара по 208 л')
  })

  it('вес — кг до 3 знаков: показанное число не умножается на 1000', () => {
    expect(parseKg('129,3')).toBe(129_300)
    expect(parseKg(kgInput(129_300))).toBe(129_300)
    expect(kgInput(129_300)).toBe('129,3')
    expect(parseKg('1,2345')).toBeNull()
  })

  it('цена литра и канистры, сумма по тысячным — одно округление', () => {
    expect(perLiter(180_000, 4000)).toBe(45_000)
    expect(perCan(45_000, 4000)).toBe(180_000)
    expect(byThousandths(45_000, 18_000)).toBe(810_000)
    expect(byThousandths(10_000, 129_300)).toBe(1_293_000)
  })

  it('вязкость из названия', () => {
    expect(viscosityFromName('QUAKER STATE 5W30 4L')).toBe('5W-30')
    expect(viscosityFromName('Shell Helix 10W-40 1 л')).toBe('10W-40')
    expect(viscosityFromName('Lukoil Luxe 5/40')).toBe('5W-40')
    expect(viscosityFromName('Castrol Transmax 75W-90 1 л')).toBe('75W-90')
    expect(viscosityFromName('Фильтр масляный W 712/75')).toBeNull()
    expect(viscosityFromName('Totachi ATF Dexron III 4 л')).toBeNull()
    expect(viscosityKey('5w30')).toBe(viscosityKey('5W-30'))
  })
})
