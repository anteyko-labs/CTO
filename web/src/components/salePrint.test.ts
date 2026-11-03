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

const MOTOR = 'Замена масла в моторе'
const book = {
  brand: 'Toyota',
  model: 'Camry',
  plate: '01KG123ABC',
  mileage: 130000,
  mileage_unit: 'km' as const,
  lines: [
    { label: 'Mobil 1 5W-30 · 4 л', slot: 'oil' as const, next_mileage: 138000, unit: 'km' as const, next_date: '2026-12-01', group: MOTOR },
    { label: 'W712 (масляный) · 1 шт', slot: 'oil_filter' as const, next_mileage: 138000, unit: 'km' as const, next_date: null, group: MOTOR },
    { label: 'C2201 (воздушный) · 1 шт', slot: 'air_filter' as const, next_mileage: 138000, unit: 'km' as const, next_date: null, group: MOTOR },
    { label: 'Mobil ATF 3309 · 1 л', slot: 'gear_oil' as const, next_mileage: 170000, unit: 'km' as const, next_date: null, group: null },
    { label: 'Фильтр топливный WK 820 · 1 шт', slot: 'fuel_filter' as const, next_mileage: 160000, unit: 'km' as const, next_date: null, group: 'Замена топливного фильтра' },
    { label: 'жидкость клиента (антифриз)', slot: 'antifreeze' as const, next_mileage: 190000, unit: 'km' as const, next_date: null, group: 'Замена антифриза' },
  ],
  filled: [
    { slot: 'oil' as const, label: 'Mobil 1 5W-30 · 4 л', product: 'Mobil 1 5W-30 · 4 л', group: MOTOR },
    { slot: 'oil_filter' as const, label: 'W712 (масляный) · 1 шт', product: 'W712 · 1 шт', group: MOTOR },
    { slot: 'air_filter' as const, label: 'C2201 (воздушный) · 1 шт', product: 'C2201 · 1 шт', group: MOTOR },
    { slot: 'gear_oil' as const, label: 'Mobil ATF 3309 · 1 л', product: 'Mobil ATF 3309 · 1 л', group: null },
    { slot: 'fuel_filter' as const, label: 'Фильтр топливный WK 820 · 1 шт', product: 'Фильтр топливный WK 820 · 1 шт', group: 'Замена топливного фильтра' },
    { slot: 'antifreeze' as const, label: 'жидкость клиента (антифриз)', product: 'жидкость клиента', group: 'Замена антифриза' },
  ],
}

const plain = (s: string) => s.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

describe('печать чека «В сервис» с масляной книжкой (SPEC-34 §4, ADR-115, ADR-116, задача 003 Н5)', () => {
  it('машина, пробег и что поменяли по видам замены — товаром, со следующей заменой только по пробегу', () => {
    const t = plain(saleHtml(sale({ book }), null))
    expect(t).toContain('Масляная книжка')
    expect(t).toContain('Машина: Toyota Camry, 01KG123ABC')
    expect(t).toContain('Пробег сейчас: 130 000 км')
    // Масло и фильтры с ним — под заменой масла, следующая — одной строкой.
    expect(t).toContain(`${MOTOR} Mobil 1 5W-30 · 4 л W712 (масляный) · 1 шт C2201 (воздушный) · 1 шт Следующая замена — на 138 000 км`)
    // Масло без услуги — само, с товаром; топливный фильтр — свой срок под своей услугой.
    expect(t).toContain('Mobil ATF 3309 · 1 л Следующая замена — на 170 000 км')
    expect(t).toContain('Замена топливного фильтра Фильтр топливный WK 820 · 1 шт Следующая замена — на 160 000 км')
    expect(t).toContain('Замена антифриза жидкость клиента (антифриз) Следующая замена — на 190 000 км')
    for (const no of ['Масло двигателя', 'Масло в коробке', 'Фильтр масляный', 'Антифриз:', 'или до', '01.12.2026']) expect(t).not.toContain(no)
    // Настройка «Блок книжки в товарном чеке» выключена (задача 003 Н5) — блока нет.
    expect(plain(saleHtml(sale({ book }), null, { bookBlock: false }))).not.toContain('Масляная книжка')
  })

  it('мили; без книжки, на вынос и в возврате — блока нет', () => {
    const mi = plain(bookHtml(sale({ book: { ...book, mileage: 60000, mileage_unit: 'mi', filled: undefined, lines: [{ label: 'Mobil 1 5W-30 · 4 л', next_mileage: 65001, unit: 'mi', next_date: null }] } })))
    expect(mi).toContain('Пробег сейчас: 60 000 миль')
    expect(mi).toContain('Mobil 1 5W-30 · 4 л Следующая замена — на 65 001 миля')
    expect(bookHtml(sale({ book: null }))).toBe('')
    expect(bookHtml(sale({ book, sale_type: 'takeaway' }))).toBe('')
    expect(bookHtml(sale({ book, kind: 'return' }))).toBe('')
    // Печатать нечего — пустого блока нет.
    expect(bookHtml(sale({ book: { ...book, lines: [], filled: [] } }))).toBe('')
  })

  it('у строк вида замены разный срок — у каждой свой', () => {
    const lines = book.lines.map((l) => (l.slot === 'air_filter' ? { ...l, next_mileage: 145000 } : l))
    const t = plain(bookHtml(sale({ book: { ...book, lines } })))
    expect(t).toContain(`${MOTOR} Mobil 1 5W-30 · 4 л — на 138 000 км W712 (масляный) · 1 шт — на 138 000 км C2201 (воздушный) · 1 шт — на 145 000 км`)
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

  it('строка в метрах — «1,5 м × …/м», на вес — «/кг» (ADR-178)', () => {
    const lines = [
      l({ name: 'Шланг', kind: 'length', qty: 1500, unit_price_tyiyn: 20000, amount_tyiyn: 30000 }),
      l({ name: 'Солидол', kind: 'weight', qty: 2500, unit_price_tyiyn: 10000, amount_tyiyn: 25000 }),
    ]
    const t = plain(saleHtml(sale({ lines } as unknown as Partial<Sale>), null))
    const som = (x: number) => plain(formatSomExact(x))
    expect(t).toContain(`Шланг 1,5 м × ${som(20000)}/м ${som(30000)}`)
    expect(t).toContain(`Солидол 2,5 кг × ${som(10000)}/кг ${som(25000)}`)
  })
})

describe('строка «Скидка» в печатном чеке (задача 003 Н3)', () => {
  const l = (over: Record<string, unknown>) => ({ name: '', kind: 'piece', qty: 1, gift: false, unit_price_tyiyn: 0, list_price_tyiyn: 0, amount_tyiyn: 0, container_ml: null, withheld_for: null, ...over })
  const lines = [
    l({ name: 'Фильтр', qty: 2, unit_price_tyiyn: 60000, list_price_tyiyn: 65000, amount_tyiyn: 120000 }),
    l({ name: 'Розлив', kind: 'pour', qty: 1500, unit_price_tyiyn: 55000, list_price_tyiyn: 61000, amount_tyiyn: 82500 }),
    l({ name: 'Подарок', gift: true, list_price_tyiyn: 20000 }),
    l({ name: 'Дороже прайса', unit_price_tyiyn: 31000, list_price_tyiyn: 30000, amount_tyiyn: 31000 }),
  ]
  it('по настройке — на сколько строки дешевле прайса; по умолчанию строки нет', () => {
    const s = sale({ lines } as unknown as Partial<Sale>)
    expect(plain(saleHtml(s, null, { discount: true }))).toContain(`Скидка −${plain(formatSomExact(19000))}`)
    expect(plain(saleHtml(s, null))).not.toContain('Скидка')
  })
})

describe('ширина товарного чека по ленте принтера', () => {
  it('без жёстких 80 мм: на ленте 58 мм суммы справа не обрезаются, длинное название переносится', () => {
    const html = saleHtml(sale({}), null)
    expect(html).not.toMatch(/size:\s*80mm/)
    expect(html).not.toMatch(/[^-]width:\s*72mm/)
    expect(html).toContain('overflow-wrap: anywhere')
  })
})
