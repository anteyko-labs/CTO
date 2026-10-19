import { describe, expect, it } from 'vitest'
import { applyKey, type KeypadKey } from './keypad'

/** Нажать клавиши подряд, как кассир пальцем. */
function press(start: string, keys: KeypadKey[], opts: Parameters<typeof applyKey>[2]): string {
  let v = start
  let fresh = opts.fresh ?? false
  for (const k of keys) {
    v = applyKey(v, k, { ...opts, fresh })
    fresh = false
  }
  return v
}

describe('экранная клавиатура', () => {
  it('цифры дописываются, ведущий ноль заменяется', () => {
    expect(press('', ['1', '2', '0'], { kind: 'int' })).toBe('120')
    expect(press('0', ['5'], { kind: 'int' })).toBe('5')
    expect(press('0', ['0'], { kind: 'int' })).toBe('0')
  })

  it('первая цифра в только что выбранном поле заменяет подставленное', () => {
    expect(press('1', ['3'], { kind: 'int', fresh: true })).toBe('3')
    expect(press('1', ['3', '2'], { kind: 'int', fresh: true })).toBe('32')
    expect(press('1500', [','], { kind: 'decimal', fresh: true })).toBe('0,')
    // Стереть в выбранном поле — убрать последний знак, а не всё.
    expect(applyKey('1500', 'back', { kind: 'decimal', fresh: true })).toBe('150')
  })

  it('запятая — только у дробных и только одна', () => {
    expect(press('', [','], { kind: 'int' })).toBe('')
    expect(press('', [',', '5'], { kind: 'decimal' })).toBe('0,5')
    expect(press('2', [',', '5', ','], { kind: 'decimal' })).toBe('2,5')
    expect(press('2.5', [','], { kind: 'decimal' })).toBe('2.5')
  })

  it('знаков после запятой не больше заданного', () => {
    expect(press('', ['1', ',', '2', '5', '9'], { kind: 'decimal' })).toBe('1,25')
    expect(press('', ['1', ',', '2', '5', '9', '1'], { kind: 'decimal', decimals: 3 })).toBe('1,259')
  })

  it('стереть и очистить', () => {
    expect(applyKey('125', 'back', { kind: 'int' })).toBe('12')
    expect(applyKey('', 'back', { kind: 'int' })).toBe('')
    expect(applyKey('1,25', 'clear', { kind: 'decimal' })).toBe('')
  })

  it('длина не больше предела поля: PIN — 4 цифры', () => {
    expect(press('', ['1', '2', '3', '4', '5'], { kind: 'int', maxLength: 4 })).toBe('1234')
    expect(press('', Array<KeypadKey>(20).fill('9'), { kind: 'int' })).toBe('9'.repeat(16))
  })
})
