// Экранная цифровая клавиатура режима кассы (SPEC-29, «Режим кассы»): как нажатие клавиши меняет
// текст поля. Одна функция для количества, литров, сумм и PIN — без физической клавиатуры.

/** Вид поля: целые (штуки, пробег), с дробной частью через запятую (литры, сомы) или набор
 *  цифр, где ноль впереди значим (PIN, ИНН). */
export type KeypadKind = 'int' | 'decimal' | 'digits'

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
  // «0» в числе заменяется первой цифрой («05» → «5»), а в PIN и кодах ноль — часть набора.
  const next = base === '0' && opts.kind !== 'digits' ? key : base + key
  return next.length > maxLength ? value : next
}

/** Прямоугольник на экране, точки (как у getBoundingClientRect). */
export interface Box {
  left: number
  top: number
  width: number
  height: number
}

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(v, Math.max(lo, hi)))

/**
 * Где поставить клавиатуру: рядом с полем и не поверх него, целиком на экране. Поле в окне —
 * сначала сбоку от окна (видны и поле, и кнопки окна); иначе справа или слева от поля, затем под
 * или над ним. Места нет нигде (узкий телефон) — у нижнего края; `scroll` — на сколько точек
 * прокрутить страницу вниз, чтобы поле поднялось над клавиатурой (0 — поле и так видно).
 */
export function padPlace(
  field: Box,
  pad: { width: number; height: number },
  view: { width: number; height: number },
  dialog?: Box | null,
  gap = 12,
): { left: number; top: number; scroll: number } {
  const { width: w, height: h } = pad
  const maxLeft = view.width - w - gap
  const maxTop = view.height - h - gap
  const sideTop = clamp(field.top + field.height / 2 - h / 2, gap, maxTop)
  const fits = (left: number, top: number) => left >= gap && left <= maxLeft && top >= gap && top <= maxTop
  const right = field.left + field.width
  const bottom = field.top + field.height
  const candidates: { left: number; top: number }[] = []
  if (dialog) {
    candidates.push({ left: dialog.left + dialog.width + gap, top: sideTop }, { left: dialog.left - gap - w, top: sideTop })
  }
  candidates.push(
    { left: right + gap, top: sideTop },
    { left: field.left - gap - w, top: sideTop },
    { left: clamp(field.left, gap, maxLeft), top: bottom + gap },
    { left: clamp(field.left, gap, maxLeft), top: field.top - gap - h },
  )
  const hit = candidates.find((c) => fits(c.left, c.top))
  if (hit) return { ...hit, scroll: 0 }
  // Нет места рядом: у нижнего края, а страницу прокрутить так, чтобы поле было над клавиатурой.
  const top = clamp(maxTop, gap, maxTop)
  return { left: clamp(view.width - w - gap, gap, maxLeft), top, scroll: Math.max(0, bottom + gap - top) }
}
