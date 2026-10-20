import { describe, expect, it } from 'vitest'
import { nextStockSort, sortStock } from './stockSort'

const rows = [
  { name: 'Фильтр', stock_qty: 5, stock_value_tyiyn: 50_000 },
  { name: 'Антифриз', stock_qty: 12, stock_value_tyiyn: null },
  { name: 'Масло', stock_qty: 5, stock_value_tyiyn: 900_000 },
]

describe('сортировка остатков', () => {
  it('по названию, остатку и стоимости; пустая стоимость — в конце', () => {
    expect(sortStock(rows, { key: 'name', desc: false }).map((r) => r.name)).toEqual(['Антифриз', 'Масло', 'Фильтр'])
    expect(sortStock(rows, { key: 'qty', desc: true }).map((r) => r.name)).toEqual(['Антифриз', 'Масло', 'Фильтр'])
    expect(sortStock(rows, { key: 'value', desc: true }).map((r) => r.name)).toEqual(['Масло', 'Фильтр', 'Антифриз'])
    expect(sortStock(rows, { key: 'value', desc: false }).map((r) => r.name)).toEqual(['Фильтр', 'Масло', 'Антифриз'])
    expect(sortStock(rows, { key: null, desc: false })).toEqual(rows)
  })

  it('щелчок по заголовку', () => {
    expect(nextStockSort({ key: null, desc: false }, 'qty')).toEqual({ key: 'qty', desc: true })
    expect(nextStockSort({ key: 'qty', desc: true }, 'qty')).toEqual({ key: 'qty', desc: false })
    expect(nextStockSort({ key: 'qty', desc: true }, 'name')).toEqual({ key: 'name', desc: false })
  })
})
