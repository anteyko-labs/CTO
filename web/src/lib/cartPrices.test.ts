import { describe, expect, it } from 'vitest'
import { priceChangeText, repriceLines, type PricedLine } from './cartPrices'
import type { Product, Service } from './types'

const prod = (id: string, price: number) => ({ id, name: `Товар ${id}`, sale_price_tyiyn: price }) as Product
const serv = (id: string, price: number) => ({ id, name: `Услуга ${id}`, price_tyiyn: price }) as Service
const listOf = (l: PricedLine) => l.product?.sale_price_tyiyn ?? l.service?.price_tyiyn ?? 0
const money = (v: number) => `${v / 100} с`

describe('цена в чеке по свежему каталогу', () => {
  it('прайсовая цена становится новой, ручная остаётся, подарок не трогается', () => {
    const lines: PricedLine[] = [
      { key: 'a', product: prod('a', 40_000), priceText: '400' },
      { key: 'b', product: prod('b', 10_000), priceText: '90' },
      { key: 'c', product: prod('c', 5_000), priceText: '0', gift: true },
      { key: 's', service: serv('s', 20_000), priceText: '200' },
    ]
    const fresh = {
      products: new Map([
        ['a', prod('a', 35_000)],
        ['b', prod('b', 12_000)],
        ['c', prod('c', 6_000)],
      ]),
      services: new Map([['s', serv('s', 20_000)]]),
    }
    const r = repriceLines(lines, fresh, listOf)
    expect(r.lines.map((l) => l.priceText)).toEqual(['350', '90', '0', '200'])
    expect(r.lines[1].product?.sale_price_tyiyn).toBe(12_000)
    expect(r.lines[2].product?.sale_price_tyiyn).toBe(5_000)
    expect(r.changes).toEqual([
      { key: 'a', name: 'Товар a', before: 40_000, after: 35_000, kept: false },
      { key: 'b', name: 'Товар b', before: 10_000, after: 12_000, kept: true },
    ])
    expect(priceChangeText(r.changes[0], money)).toBe('Товар a — было 400 с, стало 350 с')
  })

  it('без свежих данных и без смены цены — изменений нет', () => {
    const lines: PricedLine[] = [{ key: 'a', product: prod('a', 40_000), priceText: '400' }]
    expect(repriceLines(lines, { products: new Map(), services: new Map() }, listOf).changes).toEqual([])
    expect(repriceLines(lines, { products: new Map([['a', prod('a', 40_000)]]), services: new Map() }, listOf).changes).toEqual([])
  })
})
