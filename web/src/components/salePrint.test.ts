import { describe, expect, it } from 'vitest'
import { bookHtml, saleHtml } from './salePrint'
import type { Sale } from '../lib/types'

const sale = (over: Partial<Sale>): Sale =>
  ({
    id: 'x',
    number: 12,
    kind: 'sale',
    sale_type: 'service',
    cashier_name: 'Айгуль',
    master_name: 'Бакыт',
    party_name: 'Асанов',
    total_tyiyn: 300000,
    created_at: '2026-10-26T05:00:00Z',
    lines: [],
    payments: [],
    returns: [],
    ...over,
  }) as unknown as Sale

const book = {
  brand: 'Toyota',
  model: 'Camry',
  plate: '01KG123ABC',
  mileage: 217000,
  mileage_unit: 'km' as const,
  lines: [
    { label: 'Замена масла в моторе', next_mileage: 225000, unit: 'km' as const, next_date: '2026-11-26' },
    { label: 'Замена антифриза', next_mileage: 250000, unit: 'km' as const, next_date: null },
  ],
}

const plain = (s: string) => s.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

describe('печать чека «В сервис» с масляной книжкой (SPEC-34 §4)', () => {
  it('машина, пробег и следующие замены', () => {
    const t = plain(saleHtml(sale({ book }), null))
    expect(t).toContain('Масляная книжка')
    expect(t).toContain('Машина: Toyota Camry, 01KG123ABC')
    expect(t).toContain('Пробег сейчас: 217 000 км')
    expect(t).toContain('Замена масла в моторе — следующая замена на пробеге 225 000 км или до 26.11.2026')
    expect(t).toContain('Замена антифриза — следующая замена на пробеге 250 000 км')
    expect(t).not.toContain('250 000 км или до')
  })

  it('мили; без книжки, на вынос и в возврате — блока нет', () => {
    const mi = plain(bookHtml(sale({ book: { ...book, mileage: 60000, mileage_unit: 'mi', lines: [{ label: 'Замена масла', next_mileage: 65001, unit: 'mi', next_date: null }] } })))
    expect(mi).toContain('Пробег сейчас: 60 000 миль')
    expect(mi).toContain('на пробеге 65 001 миля')
    expect(bookHtml(sale({ book: null }))).toBe('')
    expect(bookHtml(sale({ book, sale_type: 'takeaway' }))).toBe('')
    expect(bookHtml(sale({ book, kind: 'return' }))).toBe('')
  })
})
