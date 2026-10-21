// Экранная клавиатура режима кассы (SPEC-29, «Режим кассы»; ADR-101): как нажатие клавиши меняет
// текст поля. Цифровая — количество, литры, суммы и PIN; полная — буквы для поиска, имени,
// госномера и комментария. Без физической клавиатуры.

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

// ---------- Полная клавиатура: буквы (ADR-101) ----------

/** Какую клавиатуру показать у поля: цифровую, полную или никакую. */
export type FieldPad = 'digits' | 'letters' | null

/** Поля с буквами: поиск, имя, телефон текстом, госномер, комментарий. */
const LETTER_TYPES = new Set(['', 'text', 'search', 'tel', 'email', 'url', 'password'])

/**
 * Клавиатура для поля по его тегу, типу и `data-keypad`: числа — цифровая (как в SPEC-29),
 * текст — полная; выключенные поля, флажки, даты и выбор из списка — никакой.
 */
export function fieldPad(tag: string, type: string | null, keypad: string | undefined, disabled: boolean): FieldPad {
  if (disabled) return null
  const t = tag.toLowerCase()
  if (t === 'textarea') return 'letters'
  if (t !== 'input') return null
  if (keypad !== undefined) return 'digits'
  const kind = (type ?? '').toLowerCase()
  if (kind === 'number') return 'digits'
  return LETTER_TYPES.has(kind) ? 'letters' : null
}

/** Ряд цифр полной клавиатуры (дефис — для «5W-30» и номеров). */
export const KBD_DIGITS = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-'] as const

/** Буквы по рядам: ЙЦУКЕН и латиница QWERTY. */
export const KBD_LETTERS = {
  ru: [
    ['й', 'ц', 'у', 'к', 'е', 'н', 'г', 'ш', 'щ', 'з', 'х', 'ъ'],
    ['ф', 'ы', 'в', 'а', 'п', 'р', 'о', 'л', 'д', 'ж', 'э'],
    ['я', 'ч', 'с', 'м', 'и', 'т', 'ь', 'б', 'ю'],
  ],
  en: [
    ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'],
    ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'],
    ['z', 'x', 'c', 'v', 'b', 'n', 'm'],
  ],
} as const

/** Текст поля и место курсора после нажатия. */
export interface TextEdit {
  value: string
  caret: number
}

/** Курсор поля: без выделения (или у поля его нет) — в конце текста. */
function span(value: string, start: number | null, end: number | null): [number, number] {
  const a = start === null ? value.length : Math.max(0, Math.min(start, value.length))
  const b = end === null ? a : Math.max(a, Math.min(end, value.length))
  return [a, b]
}

/** Вставить набранное на место курсора (выделенное заменяется); длина — не больше предела поля. */
export function typeText(value: string, start: number | null, end: number | null, text: string, maxLength?: number): TextEdit {
  const [a, b] = span(value, start, end)
  const next = value.slice(0, a) + text + value.slice(b)
  if (maxLength && maxLength > 0 && next.length > maxLength) return { value, caret: b }
  return { value: next, caret: a + text.length }
}

/** «Стереть»: выделенное целиком, иначе знак перед курсором. */
export function eraseBack(value: string, start: number | null, end: number | null): TextEdit {
  const [a, b] = span(value, start, end)
  if (a !== b) return { value: value.slice(0, a) + value.slice(b), caret: a }
  if (a === 0) return { value, caret: 0 }
  return { value: value.slice(0, a - 1) + value.slice(a), caret: a - 1 }
}

/**
 * Где поставить полную клавиатуру: она широкая, рядом с полем ей места нет — у нижнего края
 * экрана, а если там поле — у верхнего. Поле не закрывается; не помещается нигде — у нижнего края,
 * `scroll` — на сколько прокрутить, чтобы поле поднялось над ней.
 */
export function kbdPlace(
  field: Box,
  pad: { width: number; height: number },
  view: { width: number; height: number },
  gap = 12,
): { left: number; top: number; scroll: number } {
  const left = Math.max(gap, Math.round((view.width - pad.width) / 2))
  const bottomTop = Math.max(gap, view.height - pad.height - gap)
  if (field.top + field.height + gap <= bottomTop) return { left, top: bottomTop, scroll: 0 }
  if (field.top - gap >= gap + pad.height) return { left, top: gap, scroll: 0 }
  return { left, top: bottomTop, scroll: Math.max(0, field.top + field.height + gap - bottomTop) }
}
