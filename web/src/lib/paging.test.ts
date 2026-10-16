import { describe, expect, it } from 'vitest'
import { clampPage, pageCount, pageItems, parsePageSize, rangeText } from './paging'

describe('страницы списков', () => {
  it('число страниц и границы', () => {
    expect(pageCount(0, 50)).toBe(1)
    expect(pageCount(50, 50)).toBe(1)
    expect(pageCount(51, 50)).toBe(2)
    expect(clampPage(5, 51, 50)).toBe(1)
    expect(clampPage(-1, 51, 50)).toBe(0)
  })

  it('кнопки «1 2 3 … →»: первая, последняя и соседи', () => {
    expect(pageItems(0, 1)).toEqual([0])
    expect(pageItems(0, 3)).toEqual([0, 1, 2])
    expect(pageItems(0, 12)).toEqual([0, 1, 2, 3, 4, '…', 11])
    expect(pageItems(5, 12)).toEqual([0, '…', 4, 5, 6, '…', 11])
    expect(pageItems(11, 12)).toEqual([0, '…', 7, 8, 9, 10, 11])
    // Пропуск из одной страницы показывается номером.
    expect(pageItems(3, 7)).toEqual([0, 1, 2, 3, 4, 5, 6])
  })

  it('подпись и размер страницы', () => {
    expect(rangeText(0, 25, 120)).toBe('1–25 / 120')
    expect(rangeText(4, 25, 120)).toBe('101–120 / 120')
    expect(rangeText(0, 25, 0)).toBe('0 / 0')
    expect(parsePageSize('100')).toBe(100)
    expect(parsePageSize('7')).toBe(50)
    expect(parsePageSize(null)).toBe(50)
  })
})
