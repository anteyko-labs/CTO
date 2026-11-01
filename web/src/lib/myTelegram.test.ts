import { describe, expect, it } from 'vitest'
import { hintKey, linkAction, linkedText, type MyTelegram, showBotHint } from './myTelegram'

const me: MyTelegram = {
  enabled: true,
  self_link: true,
  linked: false,
  linked_at: null,
  tg_name: null,
  items: [],
  events: [],
}

describe('Мой Телеграм', () => {
  it('кнопка: код сам, привязку делает владелец, бот выключен', () => {
    expect(linkAction(me)).toBe('code')
    expect(linkAction({ ...me, self_link: false })).toBe('owner')
    expect(linkAction({ ...me, enabled: false })).toBe('off')
  })

  it('состояние: с какого времени и к какому Телеграм-имени', () => {
    expect(linkedText(me, null)).toBe('Телеграм не привязан')
    expect(linkedText({ ...me, linked: true, tg_name: '@erbol' }, '01.11.2026, 10:00')).toBe('Привязан с 01.11.2026, 10:00 к @erbol')
    expect(linkedText({ ...me, linked: true }, '01.11.2026, 10:00')).toBe('Привязан с 01.11.2026, 10:00')
  })

  it('подсказка «Подключите бот» — один раз и только без привязки', () => {
    expect(showBotHint(me, false)).toBe(true)
    expect(showBotHint(me, true)).toBe(false)
    expect(showBotHint({ ...me, linked: true }, false)).toBe(false)
    expect(showBotHint({ ...me, self_link: false }, false)).toBe(false)
    expect(showBotHint({ ...me, enabled: false }, false)).toBe(false)
    expect(showBotHint(null, false)).toBe(false)
    expect(hintKey('u1')).not.toBe(hintKey('u2'))
  })
})
