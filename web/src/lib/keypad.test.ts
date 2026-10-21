import { describe, expect, it } from 'vitest'
import { applyKey, eraseBack, fieldPad, kbdPlace, padPlace, typeText, type Box, type KeypadKey } from './keypad'

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

  it('PIN и коды: ноль впереди остаётся', () => {
    expect(press('', ['0', '1', '2', '3'], { kind: 'digits', maxLength: 4 })).toBe('0123')
    expect(press('', ['0', '0', '7'], { kind: 'digits' })).toBe('007')
    expect(press('012', [','], { kind: 'digits' })).toBe('012')
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

describe('где стоит экранная клавиатура', () => {
  const pad = { width: 342, height: 520 }
  const overlaps = (f: Box, p: { left: number; top: number }) =>
    p.left < f.left + f.width && f.left < p.left + pad.width && p.top < f.top + f.height && f.top < p.top + pad.height
  const inside = (p: { left: number; top: number }, v: { width: number; height: number }) =>
    p.left >= 0 && p.top >= 0 && p.left + pad.width <= v.width && p.top + pad.height <= v.height

  it('моноблок 1920: рядом с полем «Получено», а не в углу', () => {
    const view = { width: 1920, height: 1080 }
    const field: Box = { left: 1500, top: 700, width: 300, height: 60 }
    const p = padPlace(field, pad, view)
    expect(overlaps(field, p)).toBe(false)
    expect(inside(p, view)).toBe(true)
    expect(p.left + pad.width).toBeLessThanOrEqual(field.left)
    expect(p.scroll).toBe(0)
  })

  it('поле у левого края — справа от поля; в окне — сбоку от окна', () => {
    const view = { width: 1366, height: 768 }
    const field: Box = { left: 40, top: 300, width: 200, height: 56 }
    const p = padPlace(field, pad, view)
    expect(p.left).toBe(252)
    expect(overlaps(field, p)).toBe(false)
    const dialog: Box = { left: 400, top: 100, width: 500, height: 400 }
    const inDialog: Box = { left: 420, top: 300, width: 460, height: 56 }
    expect(padPlace(inDialog, pad, view, dialog).left).toBe(912)
  })

  it('телефон: места рядом нет — у нижнего края, страница прокручивается над клавиатурой', () => {
    const view = { width: 390, height: 844 }
    const top: Box = { left: 16, top: 100, width: 358, height: 56 }
    const a = padPlace(top, pad, view)
    expect(overlaps(top, a)).toBe(false)
    expect(inside(a, view)).toBe(true)
    const mid: Box = { left: 16, top: 400, width: 358, height: 56 }
    const b = padPlace(mid, pad, view)
    expect(inside(b, view)).toBe(true)
    // Прокрутили на `scroll` — поле над клавиатурой.
    expect(overlaps({ ...mid, top: mid.top - b.scroll }, b)).toBe(false)
  })
})

describe('полная клавиатура с буквами (ADR-101)', () => {
  it('какая клавиатура у какого поля', () => {
    expect(fieldPad('INPUT', null, undefined, false)).toBe('letters')
    expect(fieldPad('INPUT', 'search', undefined, false)).toBe('letters')
    expect(fieldPad('INPUT', 'tel', undefined, false)).toBe('letters')
    expect(fieldPad('TEXTAREA', null, undefined, false)).toBe('letters')
    // Числа и PIN — цифровая, как раньше.
    expect(fieldPad('INPUT', 'text', 'decimal', false)).toBe('digits')
    expect(fieldPad('INPUT', 'password', 'digits', false)).toBe('digits')
    expect(fieldPad('INPUT', 'checkbox', undefined, false)).toBeNull()
    expect(fieldPad('INPUT', 'date', undefined, false)).toBeNull()
    expect(fieldPad('SELECT', null, undefined, false)).toBeNull()
    expect(fieldPad('INPUT', 'text', undefined, true)).toBeNull()
  })

  it('буквы встают на место курсора, выделенное заменяется', () => {
    expect(typeText('', null, null, 'м')).toEqual({ value: 'м', caret: 1 })
    expect(typeText('масо', 3, 3, 'л')).toEqual({ value: 'масло', caret: 4 })
    expect(typeText('масло 5w30', 6, 10, '10w40')).toEqual({ value: 'масло 10w40', caret: 11 })
    // Предел длины поля: лишнее не вводится.
    expect(typeText('01KG', null, null, 'A', 4)).toEqual({ value: '01KG', caret: 4 })
  })

  it('стереть: знак перед курсором или выделенное', () => {
    expect(eraseBack('масло', null, null)).toEqual({ value: 'масл', caret: 4 })
    expect(eraseBack('масло', 2, 2)).toEqual({ value: 'мсло', caret: 1 })
    expect(eraseBack('масло', 0, 5)).toEqual({ value: '', caret: 0 })
    expect(eraseBack('масло', 0, 0)).toEqual({ value: 'масло', caret: 0 })
  })

  it('стоит у нижнего края, а поле внизу — у верхнего; поле не закрыто', () => {
    const view = { width: 1366, height: 768 }
    const kbd = { width: 1200, height: 380 }
    const covers = (f: Box, p: { top: number }) => p.top < f.top + f.height && f.top < p.top + kbd.height
    // Поиск товара вверху — клавиатура внизу.
    const search: Box = { left: 24, top: 60, width: 800, height: 64 }
    const a = kbdPlace(search, kbd, view)
    expect(a.top).toBe(768 - 380 - 12)
    expect(a.left).toBe(83)
    expect(covers(search, a)).toBe(false)
    // Комментарий внизу правой колонки — клавиатура вверху.
    const comment: Box = { left: 900, top: 600, width: 400, height: 56 }
    const b = kbdPlace(comment, kbd, view)
    expect(b.top).toBe(12)
    expect(covers(comment, b)).toBe(false)
    // Узкий телефон: места нет — внизу, страница прокручивается.
    const phone = kbdPlace({ left: 16, top: 300, width: 358, height: 56 }, { width: 366, height: 380 }, { width: 390, height: 600 })
    expect(phone.scroll).toBeGreaterThan(0)
  })
})
