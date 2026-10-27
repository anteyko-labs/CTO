import { describe, expect, it } from 'vitest'
import { askOilChange, hasServiceWork, hasTopup, nextChangeProblem } from './oilChange'

describe('замена масла в кассе', () => {
  it('«Это замена масла?» — только от канистры моторного масла (ADR-118)', () => {
    const cats = [
      { id: 'motor', book_slot: 'oil' },
      { id: 'gear', book_slot: 'gear_oil' },
    ]
    // Канистра моторного — спрашиваем; 0,5–1 л (категории нет в `enough`) — доливка, без вопроса.
    expect(askOilChange(new Set(['motor']), cats)).toBe(true)
    expect(askOilChange(new Set(), cats)).toBe(false)
    // Канистра ATF — не моторное масло.
    expect(askOilChange(new Set(['gear']), cats)).toBe(false)
    // Категории не загрузились — от канистры любого масла.
    expect(askOilChange(new Set(['gear']), null)).toBe(true)
    expect(askOilChange(new Set(), null)).toBe(false)
  })

  it('доливка не требует «В сервис», мастер к ней — по желанию', () => {
    const topup = { kind: 'service', service: { takeaway: true } }
    const change = { kind: 'service', service: { takeaway: false } }
    const oil = { kind: 'container' }
    expect(hasServiceWork([topup, oil])).toBe(false)
    expect(hasServiceWork([topup, change])).toBe(true)
    expect(hasServiceWork([{ kind: 'service', service: {} }])).toBe(true)
    expect(hasTopup([topup])).toBe(true)
    expect(hasTopup([change, oil])).toBe(false)
  })

  it('интервал из чека — в пределах, мили переводятся в км', () => {
    expect(nextChangeProblem('8000', '180', 'km')).toBeNull()
    expect(nextChangeProblem('3000', '90', 'mi')).toBeNull()
    expect(nextChangeProblem('300', '90', 'mi')).toContain('миль')
    expect(nextChangeProblem('400', '90', 'km')).toContain('500')
    expect(nextChangeProblem('8000', '0', 'km')).toContain('дней')
    expect(nextChangeProblem('', '30', 'km')).toContain('числами')
  })
})
