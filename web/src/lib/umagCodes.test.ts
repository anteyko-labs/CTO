import { describe, expect, it } from 'vitest'
import { isMeasureUnit, measureOf, productUnitOf, umagPer, umagUnitWord } from './umagCodes'

describe('единица строки Умага (задача 006, К8)', () => {
  it('кг → граммы, м → миллиметры, л → мл, «г» и прочее — штуки', () => {
    expect(productUnitOf('кг')).toBe('g')
    expect(productUnitOf('Килограмм')).toBe('g')
    expect(productUnitOf('м')).toBe('mm')
    expect(productUnitOf('метр')).toBe('mm')
    expect(productUnitOf('л')).toBe('ml')
    expect(productUnitOf('Литр')).toBe('ml')
    expect(productUnitOf('г')).toBe('piece')
    expect(productUnitOf('грамм')).toBe('piece')
    expect(productUnitOf('шт')).toBe('piece')
    expect(productUnitOf('мл')).toBe('piece')
  })

  it('цена «г» — за грамм, а не за кг; коды меры у «г» — как раньше', () => {
    expect(measureOf('гр')).toBe('g')
    expect(umagPer('г')).toBe(' за г')
    expect(umagUnitWord('грамм')).toBe('грамм')
    expect(umagPer('кг')).toBe(' за кг')
    expect(umagPer('м')).toBe(' за м')
    expect(umagPer('шт')).toBe('')
    expect(isMeasureUnit('г')).toBe(true)
  })
})
