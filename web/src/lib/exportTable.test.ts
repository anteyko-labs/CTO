import { describe, expect, it } from 'vitest'
import { ApiError, type Page } from './api'
import { cappedWarning, EXPORT_MAX_ROWS, fetchAllPages, fileName, periodText, tableSheet, unitName, xl } from './exportTable'

/** Подставной список на сервере: `n` строк, страницы по limit/offset, как paging.rs. */
function fakeServer(n: number, failAt?: number) {
  const calls: { limit: number; offset: number }[] = []
  const load = (_path: string, page: { limit: number; offset: number }): Promise<Page<number>> => {
    calls.push(page)
    if (failAt !== undefined && calls.length === failAt) return Promise.reject(new ApiError(0, 'network', 'Нет связи с сервером'))
    const total = Math.min(n, 10_000)
    const items = Array.from({ length: Math.max(0, Math.min(page.limit, total - page.offset)) }, (_, i) => page.offset + i)
    return Promise.resolve({ items, total: n })
  }
  return { load, calls }
}

describe('выгрузка таблиц', () => {
  it('все страницы: 1234 строки за 3 запроса', async () => {
    const s = fakeServer(1234)
    const all = await fetchAllPages('/receipts', s.load)
    expect(all.items.length).toBe(1234)
    expect(all.items[1233]).toBe(1233)
    expect(all.capped).toBe(false)
    expect(s.calls).toEqual([
      { limit: 500, offset: 0 },
      { limit: 500, offset: 500 },
      { limit: 500, offset: 1000 },
    ])
    expect(cappedWarning(all)).toBeUndefined()
  })

  it('потолок 10 000 строк — не больше 20 запросов', async () => {
    const s = fakeServer(25_000)
    const all = await fetchAllPages('/receipts', s.load)
    expect(all.items.length).toBe(EXPORT_MAX_ROWS)
    expect(all.capped).toBe(true)
    expect(s.calls.length).toBe(20)
    expect(cappedWarning(all)).toContain('сузьте период')
  })

  it('пустой список — один запрос', async () => {
    const s = fakeServer(0)
    const all = await fetchAllPages('/receipts', s.load)
    expect(all.items).toEqual([])
    expect(s.calls.length).toBe(1)
  })

  it('обрыв связи посередине — ошибка, а не неполный файл', async () => {
    const s = fakeServer(1234, 2)
    await expect(fetchAllPages('/receipts', s.load)).rejects.toThrow('Нет связи')
  })

  it('ячейки из целых', () => {
    expect(xl.money(1)).toEqual({ money: 1 })
    expect(xl.units(4500, 'ml')).toEqual({ qty: 4500, scale: 3 })
    expect(xl.units(3, 'piece')).toEqual({ int: 3 })
    expect(xl.units(1250, 'mm')).toEqual({ qty: 1250, scale: 3 })
    expect(xl.units(2500, 'g')).toEqual({ qty: 2500, scale: 3 })
    expect(['piece', 'ml', 'g', 'mm'].map((u) => unitName(u as 'piece'))).toEqual(['шт', 'л', 'кг', 'м'])
    expect(xl.liters(946)).toEqual({ qty: 946, scale: 3 })
    expect(xl.date('2026-10-31T05:00:00Z')).toEqual({ date: '2026-10-31' })
    expect(xl.date(null)).toBeNull()
  })

  it('лист из столбцов и строк с итогом', () => {
    const sheet = tableSheet(
      'Расходы',
      [
        { label: 'Статья', value: (r: { a: string; s: number }) => r.a },
        { label: 'Сумма', value: (r) => xl.money(r.s) },
      ],
      [
        { a: 'Хозтовары', s: 1200 },
        { a: 'Налоги', s: 500 },
      ],
      { total: ['Итого', xl.money(1700)] },
    )
    expect(sheet.columns.map((c) => c.label)).toEqual(['Статья', 'Сумма'])
    expect(sheet.rows).toEqual([
      ['Хозтовары', { money: 1200 }],
      ['Налоги', { money: 500 }],
      { cells: ['Итого', { money: 1700 }], bold: true },
    ])
  })

  it('имя файла с периодом', () => {
    expect(fileName('Прибыль и убытки', periodText('2026-10-01', '2026-10-31'))).toBe('Прибыль и убытки 01.10.2026–31.10.2026.xlsx')
    expect(fileName('Приход', periodText('2026-10-31', '2026-10-31'))).toBe('Приход 31.10.2026.xlsx')
    expect(fileName('Касса: смена №1/2')).toBe('Касса смена №1 2.xlsx')
    expect(fileName('')).toBe('Таблица.xlsx')
  })
})
