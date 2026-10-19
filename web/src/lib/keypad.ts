// Экранная цифровая клавиатура режима кассы (SPEC-29, «Режим кассы»): как нажатие клавиши меняет
// текст поля. Одна функция для количества, литров, сумм и PIN — без физической клавиатуры.

/** Вид поля: целые (штуки, PIN, пробег) или с дробной частью через запятую (литры, сомы). */
export type KeypadKind = 'int' | 'decimal'

export type KeypadKey = '0' | '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' | ',' | 'back' | 'clear'

export interface KeyOptions {
  kind: KeypadKind
  /** Поле только что выбрано: первая цифра заменяет подставленное значение, а не дописывается. */
  fresh?: boolean
  /** Знаков после запятой: сомы — 2, литры и килограммы — 3. */
  decimals?: number
  /** Предел длины текста (у PIN — 4). */
  maxLength?: number
}

export const DEFAULT_MAX_LENGTH = 16

/** Новый текст поля после нажатия клавиши. Недопустимое нажатие оставляет текст как был. */
export function applyKey(value: string, key: KeypadKey, opts: KeyOptions): string {
  const maxLength = opts.maxLength && opts.maxLength > 0 ? opts.maxLength : DEFAULT_MAX_LENGTH
  if (key === 'clear') return ''
  if (key === 'back') return value.slice(0, -1)
  const base = opts.fresh ? '' : value
  if (key === ',') {
    if (opts.kind !== 'decimal') return value
    if (/[.,]/.test(base)) return value
    const next = base === '' ? '0,' : `${base},`
    return next.length > maxLength ? value : next
  }
  const sep = base.search(/[.,]/)
  if (sep >= 0 && base.length - sep - 1 >= (opts.decimals ?? 2)) return value
  const next = base === '0' ? key : base + key
  return next.length > maxLength ? value : next
}
