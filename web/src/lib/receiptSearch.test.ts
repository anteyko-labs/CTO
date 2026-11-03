import { describe, expect, it } from 'vitest'
import { batteryText, isBattery, receiptHref, receiptKindText, receiptsQuery } from './receiptSearch'
import type { ReceiptListItem } from './types'
import { qs } from './api'

const base = { from: '2026-09-30', to: '2026-10-30', kind: 'all', q: '', photo: false }

describe('поиск приходов', () => {
  it('без поиска — период', () => {
    expect(qs(receiptsQuery(base))).toBe('?from=2026-09-30&to=2026-10-30&kind=all')
  })

  it('с поиском — за всё время', () => {
    expect(receiptsQuery({ ...base, q: '  НК-777 ' })).toEqual({ q: 'НК-777', kind: 'all', photo: undefined })
  })

  it('отбор «с фото»', () => {
    expect(qs(receiptsQuery({ ...base, photo: true }))).toContain('photo=true')
    expect(qs(receiptsQuery(base))).not.toContain('photo')
  })
})

/** Неразрывные пробелы показа — обычными, чтобы сравнивать текст. */
const plain = (t: string) => t.replace(/\u00a0/g, ' ')

const row = (over: Partial<ReceiptListItem>): ReceiptListItem => ({
  id: 'x1',
  number: 7,
  supplier_name: null,
  supplier_doc: '',
  total_tyiyn: 125_000,
  reversal_of: null,
  reversed: false,
  user_name: 'Админ',
  created_at: '2026-11-03T05:00:00Z',
  files: 0,
  ...over,
})

describe('приём аккумуляторов в «Приходе» (задача 025)', () => {
  it('отбор «Приём аккумуляторов» уходит на сервер', () => {
    expect(qs(receiptsQuery({ ...base, kind: 'battery' }))).toContain('kind=battery')
  })

  it('приём за наличные: вес, цена за кг, окно на месте', () => {
    const r = row({ kind: 'battery_intake', grams: 12_500, price_per_kg_tyiyn: 10_000 })
    expect(isBattery(r)).toBe(true)
    expect(plain(batteryText(r))).toBe('12,5 кг по 100 с/кг')
    expect(receiptHref(r)).toBeNull()
    expect(receiptKindText(r)).toBe('Приём аккумуляторов')
  })

  it('аккумулятор в оплату чека и возврат чека — ссылка на чек', () => {
    const r = row({ kind: 'battery_tradein', grams: 3_000, total_tyiyn: 30_000 })
    expect(plain(batteryText(r))).toBe('чек № 7 · 3 кг')
    expect(receiptHref(r)).toBe('/sales/x1')
    expect(receiptKindText(r)).toBe('Аккумулятор в оплату чека')
    const back = row({ kind: 'battery_tradein', grams: -3_000, total_tyiyn: -30_000, number: 8 })
    expect(plain(batteryText(back))).toBe('возврат чека № 8 · −3 кг')
    expect(receiptKindText(back)).toBe('Аккумулятор: возврат чека')
  })

  it('накладная и оприходование — как раньше', () => {
    expect(isBattery(row({ kind: 'receipt' }))).toBe(false)
    expect(receiptHref(row({ kind: 'receipt' }))).toBe('/receipts/x1')
    expect(receiptHref(row({}))).toBe('/receipts/x1')
    expect(receiptHref(row({ kind: 'stock_in' }))).toBe('/stock-ins/x1')
    expect(receiptKindText(row({ kind: 'stock_in' }))).toBe('Оприходование')
  })
})
