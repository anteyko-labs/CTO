import { describe, expect, it } from 'vitest'
import { SCAN_GAP_MS, SCAN_MIN_LENGTH, holdStep, routeKey, type Held, type KeyFacts } from './scanner'

const facts = (over: Partial<KeyFacts>): KeyFacts => ({
  key: '4',
  trusted: true,
  modified: false,
  composing: false,
  repeat: false,
  focus: 'control',
  dialog: 'none',
  picker: true,
  ...over,
})

describe('куда идёт клавиша на кассе', () => {
  it('знак на кнопке (плитка, «+», «Очистить») или на пустом месте — фокус в поиск товара', () => {
    // 26.10.2026: после касания плитки Enter сканера нажимал её снова — в чек вставал фильтр.
    expect(routeKey(facts({ key: '4' }))).toBe('picker')
    expect(routeKey(facts({ key: 'A' }))).toBe('picker')
  })

  it('одиночный Enter или пробел на кнопке — клавиатура, нажатие не трогаем', () => {
    expect(routeKey(facts({ key: 'Enter' }))).toBe('pass')
    expect(routeKey(facts({ key: ' ' }))).toBe('pass')
    expect(routeKey(facts({ key: 'Tab' }))).toBe('pass')
  })

  it('поиск товара, текстовое поле, сочетание клавиш и экранная клавиатура — не трогаем', () => {
    expect(routeKey(facts({ focus: 'picker' }))).toBe('pass')
    expect(routeKey(facts({ focus: 'picker', key: 'Enter', dialog: 'outside' }))).toBe('pass')
    expect(routeKey(facts({ focus: 'text' }))).toBe('pass')
    expect(routeKey(facts({ modified: true }))).toBe('pass')
    expect(routeKey(facts({ key: 'Enter', modified: true }))).toBe('pass')
    expect(routeKey(facts({ trusted: false, key: 'Enter' }))).toBe('pass')
    expect(routeKey(facts({ composing: true }))).toBe('pass')
  })

  it('числовое поле (количество, «Получено») — знак придерживается до ясности, скан ли это', () => {
    expect(routeKey(facts({ focus: 'number' }))).toBe('hold')
    expect(routeKey(facts({ focus: 'number', key: 'Enter' }))).toBe('pass')
    // Зажатая клавиша — человек; числовое поле в окне (PIN, сумма) — окно само.
    expect(routeKey(facts({ focus: 'number', repeat: true }))).toBe('pass')
    expect(routeKey(facts({ focus: 'number', dialog: 'inside' }))).toBe('pass')
  })

  it('окно открыто, фокус за ним: Enter и пробел гасятся — кнопка под окном не нажимается', () => {
    expect(routeKey(facts({ dialog: 'outside', key: 'Enter' }))).toBe('block')
    expect(routeKey(facts({ dialog: 'outside', key: ' ' }))).toBe('block')
    expect(routeKey(facts({ dialog: 'outside', key: '4' }))).toBe('pass')
    expect(routeKey(facts({ dialog: 'outside', focus: 'number', key: '4' }))).toBe('pass')
  })

  it('кнопка в окне: есть поиск товара в окне — туда, нет — фокус снимается, Enter её не нажмёт', () => {
    expect(routeKey(facts({ dialog: 'inside', picker: true }))).toBe('picker')
    expect(routeKey(facts({ dialog: 'inside', picker: false }))).toBe('blur')
    expect(routeKey(facts({ dialog: 'inside', key: 'Enter' }))).toBe('pass')
  })
})

describe('скан в числовое поле', () => {
  /** Знаки кода через `gap` мс с момента `t0`: шаги по очереди. */
  function type(code: string, gap: number, t0 = 1000): { held: Held | null; steps: ReturnType<typeof holdStep>[]; t: number } {
    let held: Held | null = null
    const steps: ReturnType<typeof holdStep>[] = []
    let t = t0
    for (const ch of code) {
      const s = holdStep(held, ch, t, true)
      steps.push(s)
      held = s.kind === 'hold' ? s.held : null
      t += gap
    }
    return { held, steps, t: t - gap }
  }

  it('сканер: 13 знаков через 5 мс и Enter — код в поиск, поле не меняется', () => {
    const { held, steps, t } = type('4608122000035', 5)
    expect(steps.every((s) => s.kind === 'hold' && s.release === '')).toBe(true)
    expect(holdStep(held, 'Enter', t + 5, false)).toEqual({ kind: 'scan', code: '4608122000035' })
  })

  it('человек: знаки с паузой — каждый уходит в поле, Enter остаётся Enter', () => {
    const { held, steps } = type('500', 150)
    expect(steps[1]).toEqual({ kind: 'hold', held: { chars: '0', last: 1150 }, release: '5' })
    expect(steps[2]).toEqual({ kind: 'hold', held: { chars: '0', last: 1300 }, release: '0' })
    expect(holdStep(held, 'Enter', 1400, false)).toEqual({ kind: 'pass', release: '0' })
  })

  it('быстро, но коротко («12» и Enter) — не скан: цифры в поле, Enter — полю', () => {
    const { held, t } = type('12', 5)
    expect(holdStep(held, 'Enter', t + 5, false)).toEqual({ kind: 'pass', release: '12' })
    expect(SCAN_MIN_LENGTH).toBe(6)
  })

  it('Enter после паузы — не скан, даже если знаков много', () => {
    const { held, t } = type('4608122000035', 5)
    expect(holdStep(held, 'Enter', t + SCAN_GAP_MS + 1, false)).toEqual({ kind: 'pass', release: '4608122000035' })
  })

  it('другая клавиша (стереть, стрелка, Tab) — придержанное в поле, клавиша идёт как обычно', () => {
    const { held, t } = type('46', 5)
    expect(holdStep(held, 'Backspace', t + 5, false)).toEqual({ kind: 'pass', release: '46' })
    expect(holdStep(null, 'Tab', t + 5, false)).toEqual({ kind: 'pass', release: '' })
  })
})
