import { describe, expect, it } from 'vitest'
import { extraChoices, nextPayload, nextRows, nextTooLow, receiptBook } from './nextChange'
import type { Service } from './types'

const svc = (id: string, name: string, slot: Service['book_slot'], km: number | null, days: number | null = 365): Service => ({
  id,
  name,
  price_tyiyn: 0,
  master_fee_tyiyn: 3000,
  active: true,
  book_slot: slot,
  interval_km: km,
  interval_days: km === null ? null : days,
})

const oil = svc('s1', 'Замена масла в моторе', 'oil', 8000, 31)
const anti = svc('s2', 'Замена антифриза', 'antifreeze', 60000, 730)
const wash = svc('s3', 'Мойка', 'other', null)
const gear = svc('s4', 'Замена масла в коробке', 'gear_oil', 40000, 730)

describe('следующая замена в чеке (SPEC-34)', () => {
  it('по строке на услугу со строкой книжки: пробег сейчас + интервал, у машины свой — он', () => {
    const rows = nextRows({ services: [oil, wash, anti, oil], extra: [], edits: {}, mileage: '217000', unit: 'km', own: { antifreeze: { km: 33000, days: 700 } } })
    expect(rows.map((r) => [r.label, r.value, r.interval_days])).toEqual([
      ['Замена масла в моторе', '225000', 31],
      ['Замена антифриза', '250000', 700],
    ])
    // Без пробега — поле пустое, пока кассир не впишет.
    expect(nextRows({ services: [oil], extra: [], edits: {}, mileage: '', unit: 'km', own: {} })[0].value).toBe('')
  })

  it('правка кассира и пустое поле — уходят с чеком; меньше пробега — нельзя', () => {
    const rows = nextRows({ services: [oil, anti], extra: [gear], edits: { oil: '226000', antifreeze: '' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextPayload(rows)).toEqual([
      { service_id: 's1', slot: 'oil', next_mileage: 226000 },
      { service_id: 's2', slot: 'antifreeze', next_mileage: null },
      { service_id: 's4', slot: 'gear_oil', next_mileage: 257000 },
    ])
    expect(rows[2].extra).toBe(true)
    expect(nextTooLow(rows, '217000')).toBeNull()
    const low = nextRows({ services: [oil], extra: [], edits: { oil: '216000' }, mileage: '217000', unit: 'km', own: {} })
    expect(nextTooLow(low, '217000')?.slot).toBe('oil')
  })

  it('мили: интервал в км переводится в мили машины', () => {
    const rows = nextRows({ services: [oil], extra: [], edits: {}, mileage: '60000', unit: 'mi', own: {} })
    // 8 000 км ≈ 4 971 миля.
    expect(rows[0].value).toBe('64971')
  })

  it('«+ ещё строку» — услуги со строкой книжки, которой ещё нет', () => {
    const rows = nextRows({ services: [oil], extra: [], edits: {}, mileage: '1000', unit: 'km', own: {} })
    const second = svc('s5', 'Замена масла (масло клиента)', 'oil', 8000)
    expect(extraChoices([oil, second, wash, anti, gear, { ...anti, id: 's6' }], rows).map((s) => s.id)).toEqual(['s2', 's4'])
  })

  it('книжка для чека из очереди — без пустых строк, дата по дням', () => {
    const rows = nextRows({ services: [oil, anti], extra: [], edits: { antifreeze: '' }, mileage: '217000', unit: 'km', own: {} })
    const b = receiptBook({ brand: 'Toyota', model: 'Camry', plate: '01KG123ABC' }, '217000', 'km', rows, '2026-10-26')
    expect(b.lines).toEqual([{ label: 'Замена масла в моторе', next_mileage: 225000, unit: 'km', next_date: '2026-11-26' }])
    expect(b.mileage).toBe(217000)
  })
})
