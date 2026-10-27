import { describe, expect, it } from 'vitest'
import { bookHtml, saleHtml } from './salePrint'
import type { Sale } from '../lib/types'
import { formatSomExact } from '../lib/format'

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
    { label: 'Масло двигателя', slot: 'oil' as const, next_mileage: 225000, unit: 'km' as const, next_date: '2026-11-26' },
    { label: 'Антифриз', slot: 'antifreeze' as const, next_mileage: 250000, unit: 'km' as const, next_date: null },
  ],
  filled: [
    { slot: 'oil' as const, label: 'Масло двигателя', product: 'масло клиента' },
    { slot: 'antifreeze' as const, label: 'Антифриз', product: 'Sintec G12 красный 5 кг · 2 шт' },
  ],
}

const plain = (s: string) => s.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

describe('печать чека «В сервис» с масляной книжкой (SPEC-34 §4, ADR-115, ADR-116)', () => {
  it('машина, пробег, что залито и следующая замена — только пробег', () => {
    const t = plain(saleHtml(sale({ book }), null))
    expect(t).toContain('Масляная книжка')
    expect(t).toContain('Машина: Toyota Camry, 01KG123ABC')
    expect(t).toContain('Пробег сейчас: 217 000 км')
    expect(t).toContain('Залито: Масло двигателя: масло клиента Антифриз: Sintec G12 красный 5 кг · 2 шт')
    expect(t).toContain('Следующая замена: Масло двигателя: на 225 000 км Антифриз: на 250 000 км')
    expect(t).not.toContain('или до')
    expect(t).not.toContain('26.11.2026')
  })

  it('мили; без книжки, на вынос и в возврате — блока нет', () => {
    const mi = plain(bookHtml(sale({ book: { ...book, mileage: 60000, mileage_unit: 'mi', filled: undefined, lines: [{ label: 'Масло двигателя', next_mileage: 65001, unit: 'mi', next_date: null }] } })))
    expect(mi).toContain('Пробег сейчас: 60 000 миль')
    expect(mi).toContain('Масло двигателя: на 65 001 миля')
    expect(mi).not.toContain('Залито')
    expect(bookHtml(sale({ book: null }))).toBe('')
    expect(bookHtml(sale({ book, sale_type: 'takeaway' }))).toBe('')
    expect(bookHtml(sale({ book, kind: 'return' }))).toBe('')
  })
})

describe('строки товарного чека (ADR-130)', () => {
  const l = (over: Record<string, unknown>) => ({ name: '', kind: 'piece', qty: 1, unit_price_tyiyn: 0, amount_tyiyn: 0, container_ml: null, withheld_for: null, ...over })
  it('услуга — название и сумма одной строкой, «× цена» только при количестве больше одного; товар — как был', () => {
    const lines = [
      l({ name: 'Замена масла', kind: 'service', qty: 1, unit_price_tyiyn: 50000, amount_tyiyn: 50000 }),
      l({ name: 'Мойка', kind: 'service', qty: 2, unit_price_tyiyn: 30000, amount_tyiyn: 60000 }),
      l({ name: 'Фильтр W712', kind: 'piece', qty: 1, unit_price_tyiyn: 70000, amount_tyiyn: 70000 }),
    ]
    const html = saleHtml(sale({ lines } as unknown as Partial<Sale>), null)
    expect(html).toContain(`<tr><td>Замена масла</td><td class="r">${formatSomExact(50000)}</td></tr>`)
    expect(html).toContain(`<tr><td>Мойка</td><td class="r">${formatSomExact(60000)}</td></tr>`)
    const som = (t: number) => plain(formatSomExact(t))
    const t = plain(html)
    expect(t).toContain(`2 × ${som(30000)}`)
    expect(t).not.toContain(`1 × ${som(50000)}`)
    expect(t).not.toContain('усл.')
    expect(t).toContain(`Фильтр W712 1 шт × ${som(70000)} ${som(70000)}`)
  })
})
