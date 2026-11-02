import { describe, expect, it } from 'vitest'
import { checkDiscount, linePercentPrice, lineSum, overLimit, percentDiscount, spreadDiscount, type DiscLine } from './discount'

const total = (lines: DiscLine[], prices: Map<string, number>) =>
  lines.reduce((acc, l) => acc + lineSum(l.kind, l.qty, l.gift ? 0 : (prices.get(l.key) ?? l.price)), 0)

describe('скидка на кассе (задача 003 Н3)', () => {
  // Масло канистрой 2 шт по 2 500 с, фильтр 1 шт 650 с, розлив 1,5 л по 610 с, услуга 300 с, подарок.
  const mixed: DiscLine[] = [
    { key: 'oil', kind: 'container', qty: 2, list: 250_000, price: 250_000 },
    { key: 'filter', kind: 'piece', qty: 1, list: 65_000, price: 65_000 },
    { key: 'pour', kind: 'pour', qty: 1_500, list: 61_000, price: 61_000 },
    { key: 'work', kind: 'service', qty: 1, list: 30_000, price: 30_000 },
    { key: 'gift', kind: 'piece', qty: 1, list: 20_000, price: 0, gift: true },
  ]

  it('скидка на чек суммой — ровно на смешанном чеке, подарок не тронут', () => {
    const before = total(mixed, new Map())
    expect(before).toBe(500_000 + 65_000 + 91_500 + 30_000)
    const r = spreadDiscount(mixed, 50_000, { services: true })
    expect(r.discount).toBe(50_000)
    expect(total(mixed, r.prices)).toBe(before - 50_000)
    expect(r.prices.has('gift')).toBe(false)
    // Цены строк — целыми сомами, кроме той, что берёт остаток.
    const notWhole = [...r.prices.entries()].filter(([, p]) => p % 100 !== 0)
    expect(notWhole.length).toBeLessThanOrEqual(1)
    for (const l of mixed) if (!l.gift) expect(r.prices.get(l.key)).toBeLessThanOrEqual(l.list)
  })

  it('процент — от прайса строк под скидку; услуги — по настройке', () => {
    expect(percentDiscount(mixed, 10, { services: true })).toBe(68_650)
    const r = spreadDiscount(mixed, percentDiscount(mixed, 10, { services: false }), { services: false })
    expect(r.prices.has('work')).toBe(false)
    expect(r.discount).toBe(65_650)
  })

  it('ноль — цены по прайсу (убрать скидку)', () => {
    const cut = mixed.map((l) => ({ ...l, price: l.gift ? 0 : l.list - 100 }))
    const r = spreadDiscount(cut, 0, { services: true })
    expect(r.discount).toBe(0)
    expect(r.prices.get('oil')).toBe(250_000)
    expect(total(cut, r.prices)).toBe(total(mixed, new Map()))
  })

  it('только розлив — точно не попасть: показывается настоящая скидка', () => {
    const pour: DiscLine[] = [{ key: 'p', kind: 'pour', qty: 1_300, list: 61_000, price: 61_000 }]
    const r = spreadDiscount(pour, 7_777, { services: true })
    expect(r.discount).toBeGreaterThanOrEqual(7_777)
    expect(r.discount).toBeLessThan(7_777 + 100)
    expect(total(pour, r.prices)).toBe(79_300 - r.discount)
  })

  it('скидка не больше суммы чека', () => {
    const r = spreadDiscount(mixed, 10_000_000, { services: true })
    expect(r.discount).toBe(total(mixed, new Map()))
  })

  it('скидка на строку процентом — до целого сома вниз', () => {
    expect(linePercentPrice(125_000, 10)).toBe(112_500)
    expect(linePercentPrice(125_500, 10)).toBe(112_900)
    expect(linePercentPrice(100, 100)).toBe(0)
  })

  it('скидка чека сейчас и предел администратора', () => {
    const cut = mixed.map((l) => (l.key === 'filter' ? { ...l, price: 55_000 } : l.key === 'oil' ? { ...l, price: 260_000 } : l))
    const d = checkDiscount(cut)
    // Поднятая цена масла скидку не уменьшает.
    expect(d.discount).toBe(10_000)
    expect(d.list).toBe(686_500)
    expect(overLimit(d.discount, d.list, { pct: null, tyiyn: null })).toBeNull()
    expect(overLimit(d.discount, d.list, { pct: 1, tyiyn: null })).toBe('pct')
    expect(overLimit(d.discount, d.list, { pct: 2, tyiyn: null })).toBeNull()
    expect(overLimit(d.discount, d.list, { pct: null, tyiyn: 9_999 })).toBe('sum')
  })
})

describe('скидка: строка в метрах — как на вес (ADR-175)', () => {
  it('сумма — цена за м × мм / 1000, скидка чека раскладывается без потерь', () => {
    expect(lineSum('length', 1500, 20_000)).toBe(30_000)
    expect(lineSum('weight', 2500, 10_000)).toBe(25_000)
    const lines: DiscLine[] = [
      { key: 'hose', kind: 'length', qty: 1500, list: 20_000, price: 20_000 },
      { key: 'filter', kind: 'piece', qty: 1, list: 65_000, price: 65_000 },
    ]
    const r = spreadDiscount(lines, 5_000, { services: true })
    expect(r.discount).toBe(5_000)
    expect(total(lines, r.prices)).toBe(95_000 - 5_000)
  })
})
