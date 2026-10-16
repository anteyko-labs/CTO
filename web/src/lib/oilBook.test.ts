import { describe, expect, it } from 'vitest'
import { overdueSlots, recordLines, recordText, slotNextText, slotsWithNext, type OilRecord, type VehicleBook } from './oilBook'

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
