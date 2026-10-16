import { describe, expect, it } from 'vitest'
import { canisterPourPrice, divRound, markupPermille, pourAmount, pourPricePerL, saleFromMarkup } from './money'

describe('округление как на сервере', () => {
  it('те же примеры, что в server/src/domain/money.rs', () => {
    expect(divRound(5, 2)).toBe(3)
    expect(divRound(-5, 2)).toBe(-3)
    expect(divRound(7, 3)).toBe(2)
    expect(divRound(1, 3)).toBe(0)
    expect(divRound(2, 3)).toBe(1)
    expect(pourAmount(1500, 33_333)).toBe(50_000)
  })
})

describe('одна цена масла (ADR-074)', () => {
  it('розлив за литр — от цены канистры, как на сервере', () => {
    expect(canisterPourPrice(200_000, 4000)).toBe(50_000)
    expect(pourAmount(4000, 50_000)).toBe(200_000)
    expect(pourAmount(2500, 50_000)).toBe(125_000)
    expect(canisterPourPrice(199_999, 4000)).toBe(50_000)
    expect(canisterPourPrice(199_998, 4000)).toBe(50_000)
    expect(canisterPourPrice(199_996, 4000)).toBe(49_999)
    expect(canisterPourPrice(12_000_000, 200_000)).toBe(60_000)
    expect(canisterPourPrice(1, 0)).toBeNull()
    expect(pourPricePerL({ unit: 'ml', container_ml: 4000, sale_price_tyiyn: 200_000 })).toBe(50_000)
    expect(pourPricePerL({ unit: 'piece', container_ml: null, sale_price_tyiyn: 50_000 })).toBeNull()
  })
})

describe('наценка и цена продажи в приходе (SPEC-28 §1)', () => {
  it('те же случаи, что markup_and_sale_price_are_linked на сервере', () => {
    expect(markupPermille(160_000, 200_000)).toBe(250)
    expect(saleFromMarkup(160_000, 250)).toBe(200_000)
    expect(markupPermille(30_000, 35_000)).toBe(167)
    expect(saleFromMarkup(30_000, 167)).toBe(35_010)
    expect(markupPermille(10_000, 9_000)).toBe(-100)
    expect(saleFromMarkup(10_000, -100)).toBe(9_000)
    expect(saleFromMarkup(33_333, 105)).toBe(36_833)
    expect(markupPermille(200_000, 200_100)).toBe(1)
    expect(markupPermille(0, 100)).toBeNull()
    expect(saleFromMarkup(100, -1001)).toBeNull()
    expect(saleFromMarkup(100, -1000)).toBe(0)
  })
})
