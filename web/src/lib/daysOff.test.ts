import { describe, expect, it } from 'vitest'
import { belowMinimum, canTakeOff, dayOf, onDuty, type DaysOffEmployee } from './daysOff'

const staff: DaysOffEmployee[] = [
  { id: 'a', full_name: 'Азат', days_off: ['2026-12-02'] },
  { id: 'b', full_name: 'Бакыт', days_off: [] },
  { id: 'c', full_name: 'Чынгыз', days_off: ['2026-12-02', '2026-12-03'] },
]

describe('график выходных', () => {
  it('считает работающих по дню', () => {
    expect(dayOf('2026-12', 2)).toBe('2026-12-02')
    expect(onDuty(staff, '2026-12-01')).toBe(3)
    expect(onDuty(staff, '2026-12-02')).toBe(1)
    expect(onDuty(staff, '2026-12-03')).toBe(2)
  })

  it('минимум: 0 — без ограничений, иначе не ниже', () => {
    expect(canTakeOff(staff, '2026-12-02', 0)).toBe(true)
    expect(belowMinimum(staff, '2026-12-02', 0)).toBe(false)
    expect(canTakeOff(staff, '2026-12-03', 2)).toBe(false)
    expect(canTakeOff(staff, '2026-12-01', 2)).toBe(true)
    expect(belowMinimum(staff, '2026-12-02', 2)).toBe(true)
  })
})
