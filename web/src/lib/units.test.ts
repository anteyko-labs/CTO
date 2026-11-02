import { describe, expect, it } from 'vitest'
import {
  asUnit,
  avgPrice,
  byThousandths,
  isMilli,
  isMilliKind,
  lineKindOf,
  parseQty,
  perWord,
  priceUnits,
  qtyInputText,
  unitShort,
  type StockUnit,
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
    expect(qtyText('mm', 1250)).toBe('1,25\u00a0м')
    expect(qtyText('mm', -500)).toBe('−0,5\u00a0м')
  })

  it('кг и м: ввод десятичным с запятой или точкой, до 3 знаков (ADR-178)', () => {
    expect(parseQty('mm', '1,25')).toBe(1250)
    expect(parseQty('mm', '1.25')).toBe(1250)
    expect(parseQty('g', '2,5')).toBe(2500)
    expect(parseQty('g', '0,001')).toBe(1)
    expect(parseQty('mm', '0')).toBe(0)
    expect(parseQty('mm', '1,2345')).toBeNull()
    expect(parseQty('mm', '-1')).toBeNull()
    expect(parseQty('mm', '')).toBeNull()
    expect(parseQty('mm', 'абв')).toBeNull()
    expect(parseQty('piece', '2')).toBe(2)
    expect(parseQty('piece', '2,5')).toBeNull()
    expect(parseQty('ml', '0,5')).toBe(500)
    expect(qtyInputText('mm', 1250)).toBe('1,25')
    expect(qtyInputText('g', 2000)).toBe('2')
    expect(qtyInputText('piece', 3)).toBe('3')
  })

  it('кг и м: подписи и единицы цены', () => {
    expect(['piece', 'ml', 'g', 'mm'].map((u) => isMilli(u as StockUnit))).toEqual([false, true, true, true])
    expect(['piece', 'ml', 'g', 'mm'].map((u) => unitShort(u as StockUnit))).toEqual(['шт', 'л', 'кг', 'м'])
    expect(perWord('mm')).toBe('за м')
    expect(perWord('g')).toBe('за кг')
    expect(perWord('ml', 4000)).toBe('за кан.')
    expect(perWord('ml')).toBe('за л')
    expect(perWord('piece')).toBe('за шт')
    expect(priceUnits('mm', null)).toBe(1000)
    expect(priceUnits('ml', 4000)).toBe(4000)
    expect(priceUnits('piece', null)).toBe(1)
    expect(lineKindOf('mm')).toBe('length')
    expect(lineKindOf('g')).toBe('weight')
    expect(isMilliKind('length')).toBe(true)
    expect(isMilliKind('piece')).toBe(false)
    expect(asUnit('mm')).toBe('mm')
    expect(asUnit('кг')).toBeNull()
    expect(avgPrice({ unit: 'mm', container_ml: null, stock_qty: 2500, avg_cost_tyiyn: 20, stock_value_tyiyn: 50_000 })).toEqual({ tyiyn: 20_000, per: 'за м' })
    expect(countText({ unit: 'mm', container_ml: null }, 1250)).toBe('1,25\u00a0м')
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
