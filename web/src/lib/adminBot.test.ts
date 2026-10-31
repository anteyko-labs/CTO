import { describe, expect, it } from 'vitest'
import { type AdminBot, applyAdminFeed, enabledCount, switchAdminEvent } from './adminBot'

const bot: AdminBot = {
  linked: true,
  linked_at: null,
  items: [],
  on: [],
  groups: [
    {
      key: 'shift',
      title: 'Смена',
      events: [
        { action: 'shift.open', label: 'Смена открыта', on: false, allowed: true },
        { action: 'shift.close', label: 'Смена закрыта', on: false, allowed: true },
      ],
    },
    { key: 'stock', title: 'Склад', events: [{ action: 'receipt.post', label: 'Приход', on: false, allowed: false, need: 'Нет права: Приход' }] },
  ],
}

describe('лента администратора в боте', () => {
  it('включённое добавляется один раз, выключенное убирается', () => {
    expect(switchAdminEvent([], 'shift.open', true)).toEqual(['shift.open'])
    expect(switchAdminEvent(['shift.open'], 'shift.open', true)).toEqual(['shift.open'])
    expect(switchAdminEvent(['receipt.post', 'shift.open'], 'receipt.post', false)).toEqual(['shift.open'])
    expect(switchAdminEvent([], 'shift.open', false)).toEqual([])
  })

  it('отметки и счётчик — по списку включённого', () => {
    const next = applyAdminFeed(bot, ['shift.close'])
    expect(next.groups[0].events.map((e) => e.on)).toEqual([false, true])
    expect(next.groups[1].events[0].on).toBe(false)
    expect(enabledCount(next)).toBe('1 из 3')
    expect(enabledCount(bot)).toBe('0 из 3')
  })
})
