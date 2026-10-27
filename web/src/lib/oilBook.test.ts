import { describe, expect, it } from 'vitest'
import { categorySlotText, fluidText, lineNextText, overdueSlots, recordLines, recordText, slotNextText, slotsOfSale, slotsWithNext, type OilRecord, type SlotNext, type VehicleBook } from './oilBook'
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
    expect(slotNextText(b.slots![0])).toBe('на 93 000 км')
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

describe('следующая замена — только пробег (ADR-115)', () => {
  it('строка с пробегом — без даты; без пробега — «по сроку до»', () => {
    expect(slotNextText({ next_km: 258_000, next_mileage: 258_000, unit: 'km', next_date: '2026-11-26' })).toBe('на 258 000 км')
    expect(slotNextText({ next_km: null, next_date: '2027-04-12' })).toBe('по сроку до 12.04.2027')
    expect(slotNextText({ next_km: null, next_date: null })).toBe('')
    const line = { slot: 'oil' as const, text: 'Масло', target_set: true, next_mileage: 225_000, next_unit: 'km' as const, next_date: '2027-01-05' }
    expect(lineNextText(line)).toBe('следующая замена на 225 000 км')
    expect(lineNextText({ ...line, next_unit: 'mi' })).toBe('следующая замена на 225 000 миль')
    expect(lineNextText({ ...line, next_mileage: null })).toBe('следующая замена по сроку до 05.01.2027')
    expect(lineNextText({ ...line, next_mileage: null, next_date: null })).toBe('')
  })
})

describe('чек книжки после чека — только этот чек и только жидкости (ADR-116)', () => {
  // 09KG999ZXZ: в чеке № 1 (17.10) меняли масло и воздушный фильтр, в чеке № 8 — масло клиента,
  // антифриз и фильтры. «Прошлые замены с чека убрать», «кроме масел ничего не показывать».
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
    next_km: 258_000,
    next_date: '2026-11-26',
    records: [
      rec({
        id: 'r8',
        change_date: '2026-10-26',
        mileage_km: 250_000,
        sale_id: 's8',
        sale_number: 8,
        declined: 'салонный фильтр',
        lines: [
          { slot: 'oil', text: 'Замена масла в моторе', service: 'Замена масла в моторе' },
          { slot: 'oil_filter', text: 'Фильтр W712 · 1 шт' },
          { slot: 'antifreeze', text: 'Sintec G12 красный 5 кг · 2 шт' },
        ],
      }),
      rec({ id: 'r1', change_date: '2026-10-17', mileage_km: 216_000, sale_id: 's1', sale_number: 1, lines: [{ slot: 'oil', text: 'Масло 5W-30' }, { slot: 'air_filter', text: 'Фильтр A1' }] }),
    ],
    slots: [
      slot('oil', 'Масло двигателя', '2026-10-26', 258_000, '2026-11-26'),
      slot('oil_filter', 'Фильтр масляный', '2026-10-26', 258_000, '2026-11-26'),
      slot('air_filter', 'Фильтр воздушный', '2026-10-17', 231_000, '2027-10-17'),
      slot('antifreeze', 'Антифриз', '2026-10-26', 280_000, '2028-10-26'),
    ],
  }
  const plain = (html: string) => html.replace(/<style[\s\S]*?<\/style>/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ')

  it('строки этого чека', () => {
    expect(slotsOfSale(book, 's8').map((n) => n.slot)).toEqual(['oil', 'oil_filter', 'antifreeze'])
    // Записи чека в книжке нет — строк чека тоже нет.
    expect(slotsOfSale(book, 'другой')).toEqual([])
  })

  it('залито и следующая замена — по жидкостям, с товаром; без фильтров, мастера, рекомендаций и истории', async () => {
    const text = plain(await oilBookHtml(book, { link: null, saleId: 's8', master: 'Бакыт', client: 'Асанов' }))
    expect(text).toContain('09KG999ZXZ · Toyota Camry')
    expect(text).toContain('Дата 26.10.2026')
    expect(text).toContain('Пробег 250 000 км')
    expect(text).toContain('Залито: Масло двигателя масло клиента Антифриз Sintec G12 красный 5 кг · 2 шт')
    expect(text).toContain('Следующая замена: Масло двигателя на 258 000 км Антифриз на 280 000 км')
    for (const no of ['Фильтр', 'Бакыт', 'Мастер', 'Рекомендовано', 'салонный', 'По прошлым заменам', 'Прошлые замены', 'наступит раньше', 'или до', '17.10.2026']) {
      expect(text).not.toContain(no)
    }
  })

  it('печать из карточки клиента — прежняя выписка с историей, только пробег', async () => {
    const full = plain(await oilBookHtml(book, { link: null, master: 'Бакыт' }))
    expect(full).toContain('Мастер Бакыт')
    expect(full).toContain('Рекомендовано салонный фильтр')
    expect(full).toContain('Фильтр воздушный на 231 000 км')
    expect(full).toMatch(/Прошлые замены 17\.10\.2026/)
    expect(full).not.toContain('наступит раньше')
    expect(full).not.toContain('или до')
    expect(fluidText({ slot: 'gear_oil', text: 'Замена масла АКПП', service: 'Замена масла АКПП' })).toBe('масло клиента')
    expect(fluidText({ slot: 'brake_fluid', text: 'Тормозная жидкость' })).toBe('жидкость клиента')
  })
})
