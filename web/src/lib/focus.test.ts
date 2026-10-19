import { describe, expect, it } from 'vitest'
import { applyFocus, shouldSelectOnFocus } from './focus'

describe('перевод фокуса после отрисовки', () => {
  it('фокус и выделение текста в найденном поле', () => {
    const calls: string[] = []
    const ok = applyFocus('#line-1-count', (s) => (s === '#line-1-count' ? { focus: () => calls.push('focus'), select: () => calls.push('select') } : null))
    expect(ok).toBe(true)
    expect(calls).toEqual(['focus', 'select'])
  })

  it('нет поля или нет запроса — ничего не делаем', () => {
    expect(applyFocus('#line-2-count', () => null)).toBe(false)
    expect(applyFocus(null, () => ({ focus: () => undefined }))).toBe(false)
  })
})

describe('выделение числового поля при фокусе', () => {
  it('выделяем, пока фокус в поле и ничего не набрали', () => {
    expect(shouldSelectOnFocus(true, '10', '10')).toBe(true)
  })

  it('фокус ушёл — не выделяем: select() вернул бы фокус в прежнее поле', () => {
    expect(shouldSelectOnFocus(false, '10', '10')).toBe(false)
  })

  it('уже набрали — не выделяем, иначе следующий знак заменит набранное', () => {
    expect(shouldSelectOnFocus(true, '', '3')).toBe(false)
  })
})
