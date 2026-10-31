import { describe, expect, it } from 'vitest'
import { applyFeed, groupCount, SALES_MODES, switchEvent } from './botFeed'

describe('уведомления в бот', () => {
  it('выключенное событие добавляется один раз, включённое убирается', () => {
    expect(switchEvent([], 'shift.open', false)).toEqual(['shift.open'])
    expect(switchEvent(['shift.open'], 'shift.open', false)).toEqual(['shift.open'])
    expect(switchEvent(['receipt.post', 'shift.open'], 'receipt.post', true)).toEqual(['shift.open'])
    expect(switchEvent([], 'shift.open', true)).toEqual([])
  })

  it('считает включённые в группе', () => {
    const g = {
      key: 'shift',
      title: 'Смена',
      events: [
        { action: 'shift.open', label: 'Смена открыта', on: true },
        { action: 'shift.reopen', label: 'Смена переоткрыта', on: false },
      ],
    }
    expect(groupCount(g)).toEqual({ on: 1, all: 2 })
  })

  it('выключатели групп — по списку выключенных', () => {
    const feed = {
      off: [],
      sales: 'none' as const,
      groups: [{ key: 'shift', title: 'Смена', events: [{ action: 'shift.open', label: 'Смена открыта', on: true }] }],
    }
    const next = applyFeed(feed, ['shift.open'], 'all')
    expect(next.sales).toBe('all')
    // Прибыль в чеках — по умолчанию да, выключатель сохраняется (задача 003 Н4).
    expect(next.profit).toBe(true)
    expect(applyFeed(next, [], 'all', false).profit).toBe(false)
    expect(next.groups[0].events[0].on).toBe(false)
    expect(feed.groups[0].events[0].on).toBe(true)
  })

  it('три режима чеков, «не присылать» — последним', () => {
    expect(SALES_MODES.map((m) => m.value)).toEqual(['all', 'debt_returns', 'none'])
  })
})
