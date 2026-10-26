import { describe, expect, it } from 'vitest'
import { categorySlotText, overdueSlots, recordLines, recordText, slotNextText, slotsBeforeSale, slotsOfSale, slotsWithNext, type OilRecord, type SlotNext, type VehicleBook } from './oilBook'
import { oilBookHtml } from './oilBookPrint'

const rec = (over: Partial<OilRecord>): OilRecord => ({
  id: '1',
  change_date: '2026-10-01',
  mileage_km: 85_000,
  oil_text: 'Масло 5W-30',
  filter_text: 'Фильтр W712',
  comment: '',
  sale_id: null,
  sale_number: null,
  ...over,
})

describe('масляная книжка по строкам', () => {
  it('старая запись без строк — масло и масляный фильтр', () => {
    expect(recordLines(rec({})).map((l) => l.slot)).toEqual(['oil', 'oil_filter'])
    expect(recordText(rec({ lines: [{ slot: 'air_filter', text: 'A1' }] }))).toBe('A1')
  })

  it('следующая замена по каждой строке', () => {
    const b: VehicleBook = {
      vehicle_id: 'v',
      plate: '01KG',
      brand: '',
      model: '',
      interval_km: 8000,
      interval_days: 31,
      next_km: 93_000,
      next_date: '2026-11-01',
      records: [],
      slots: [
        { slot: 'oil', label: 'Масло двигателя', interval_km: 8000, interval_days: 31, own_interval: false, last_date: '2026-10-01', last_km: 85_000, last_text: 'М', next_km: 93_000, next_date: '2026-11-01' },
        { slot: 'other', label: 'Прочие работы', interval_km: null, interval_days: null, own_interval: false, last_date: '2026-10-01', last_km: 85_000, last_text: 'Антифриз', next_km: null, next_date: null },
        { slot: 'air_filter', label: 'Фильтр воздушный', interval_km: 15000, interval_days: 3, own_interval: true, last_date: '2026-09-01', last_km: 80_000, last_text: 'A1', next_km: 95_000, next_date: '2026-09-04' },
      ],
    }
    expect(slotNextText(b.slots![0])).toBe('на 93 000 км или до 01.11.2026')
    expect(slotsWithNext(b).map((n) => n.slot)).toEqual(['oil', 'air_filter'])
    expect(overdueSlots(b, '2026-10-16').map((n) => n.slot)).toEqual(['air_filter'])
  })
})

describe('пробег в милях в книжке (ADR-084)', () => {
  it('запись и следующая замена — в единице машины', async () => {
    const { recordMileage } = await import('./oilBook')
    expect(recordMileage(rec({ mileage: 50_000, mileage_unit: 'mi', mileage_km: 80_467 }))).toBe('50 000 миль')
    expect(recordMileage(rec({ mileage_km: 85_000 }))).toBe('85 000 км')
    expect(recordMileage(rec({ mileage_km: null }))).toBeNull()
    expect(slotNextText({ next_km: 85_295, next_mileage: 53_000, unit: 'mi', next_date: null })).toBe('на 53 000 миль')
  })
})

describe('строка книжки категории', () => {
  it('фильтры — по типу фильтра товара, без типа — строка категории', () => {
    const filters = [{ key: 'filter_type' }, { key: 'cross' }]
    expect(categorySlotText({ book_slot: 'oil_filter', attributes: filters })).toEqual({ main: 'По типу фильтра', note: 'без типа — фильтр масляный' })
    expect(categorySlotText({ book_slot: 'air_filter', attributes: filters })).toEqual({ main: 'По типу фильтра', note: 'без типа — фильтр воздушный' })
    expect(categorySlotText({ book_slot: 'oil_filter', attributes: [] })).toEqual({ main: 'Фильтр масляный', note: null })
    expect(categorySlotText({ book_slot: 'oil', attributes: [] })).toEqual({ main: 'Масло двигателя', note: null })
    expect(categorySlotText({ book_slot: 'none', attributes: filters })).toEqual({ main: '—', note: null })
  })
})

describe('следующая замена после чека — по строкам этого чека (ADR-111)', () => {
  // 09KG999ZXZ: в чеке № 1 (17.10) меняли масло и воздушный фильтр, в чеке № 8 — масло и антифриз.
  // Итог чека № 8 и книжка после него писали «Следующая · Фильтр воздушный» — «в чеке постоянно
  // добавляет воздушный фильтр» (26.10.2026).
  const slot = (s: SlotNext['slot'], label: string, last: string, nextKm: number, nextDate: string | null): SlotNext => ({
    slot: s,
    label,
    interval_km: null,
    interval_days: null,
    own_interval: false,
    last_date: last,
    last_km: null,
    last_text: null,
    next_km: nextKm,
    next_date: nextDate,
  })
  const book: VehicleBook = {
    vehicle_id: 'v',
    plate: '09KG999ZXZ',
    brand: 'Toyota',
    model: 'Camry',
    interval_km: 8000,
    interval_days: 31,
    next_km: 228_000,
    next_date: '2026-11-26',
    records: [
      rec({ id: 'r8', change_date: '2026-10-26', mileage_km: 220_000, sale_id: 's8', sale_number: 8, lines: [{ slot: 'oil', text: 'Масло 5W-30' }, { slot: 'antifreeze', text: 'Замена антифриза' }] }),
      rec({ id: 'r1', change_date: '2026-10-17', mileage_km: 216_000, sale_id: 's1', sale_number: 1, lines: [{ slot: 'oil', text: 'Масло 5W-30' }, { slot: 'air_filter', text: 'Фильтр A1' }] }),
    ],
    slots: [
      slot('oil', 'Масло двигателя', '2026-10-26', 228_000, '2026-11-26'),
      slot('air_filter', 'Фильтр воздушный', '2026-10-17', 231_000, '2027-10-17'),
      slot('antifreeze', 'Антифриз', '2026-10-26', 280_000, null),
    ],
  }
  const plain = (html: string) => html.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

  it('строки этого чека и строки прошлых замен — порознь', () => {
    expect(slotsOfSale(book, 's8').map((n) => n.slot)).toEqual(['oil', 'antifreeze'])
    expect(slotsBeforeSale(book, 's8').map((n) => n.slot)).toEqual(['air_filter'])
    // Записи чека в книжке нет — строк чека тоже нет.
    expect(slotsOfSale(book, 'другой')).toEqual([])
  })

  it('книжка после чека: «Следующая замена» без воздушного фильтра, он — по прошлым заменам с датой', async () => {
    const text = plain(await oilBookHtml(book, { link: null, saleId: 's8' }))
    const next = text.split('Следующая замена — что наступит раньше')[1]?.split('По прошлым заменам')[0] ?? ''
    expect(next).toContain('Масло двигателя')
    expect(next).toContain('Антифриз')
    expect(next).not.toContain('Фильтр воздушный')
    expect(text).toMatch(/По прошлым заменам.*Фильтр воздушный \(замена 17\.10\.2026\) на 231 000 км или до 17\.10\.2027/)
    // Печать из карточки клиента — вся книжка, как прежде.
    const full = plain(await oilBookHtml(book, { link: null }))
    expect(full.split('Следующая замена — что наступит раньше')[1]).toContain('Фильтр воздушный')
  })
})
