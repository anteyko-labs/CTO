import { describe, expect, it } from 'vitest'
import { type AdminBot, applyAdminFeed, enabledCount, helperLimit, helperStatus, switchAdminEvent } from './adminBot'

const bot: AdminBot = {
  linked: true,
  linked_at: null,
  items: [],
  on: [],
  helper: { on: false, daily_limit: 50, common_on: true, configured: true, today: 0 },
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

describe('помощник администратора в боте', () => {
  const h = { on: true, daily_limit: 50, common_on: true, configured: true, today: 3 }

  it('лимит — целое от 1 до 500', () => {
    expect(helperLimit('50')).toBe(50)
    expect(helperLimit(' 1 ')).toBe(1)
    expect(helperLimit('500')).toBe(500)
    expect(helperLimit('0')).toBeNull()
    expect(helperLimit('501')).toBeNull()
    expect(helperLimit('5,5')).toBeNull()
    expect(helperLimit('')).toBeNull()
  })

  it('состояние словами: выключен, не настроен, выключен всем, работает', () => {
    expect(helperStatus({ ...h, on: false })).toContain('Выключен')
    expect(helperStatus({ ...h, configured: false })).toContain('не настроен')
    expect(helperStatus({ ...h, common_on: false })).toContain('выключен всем')
    expect(helperStatus(h)).toBe('Работает: сегодня 3 из 50 вопросов.')
  })
})
