// Клавиатурный сканер на кассе (ui-rules §4, SPEC-30 §13). Сканер — та же клавиатура: печатает
// код туда, где фокус, и жмёт Enter. Касание кнопки оставляет фокус на ней, и Enter сканера
// нажимал её ещё раз: плитка или «+» у строки добавляли свой товар вместо отсканированного
// (26.10.2026: «в чеке постоянно добавляет воздушный фильтр»), «Точно очистить?» стирал чек, а код
// в поле количества или «Получено» становился количеством или суммой — и Enter проводил чек.
// Здесь решается, куда идёт клавиша; `useScanToPicker` вешает это на экран.
import { useEffect } from 'react'

/** Наибольший промежуток между знаками одного скана и до его Enter, мс: сканер шлёт знаки через
 *  1–10 мс, человек так быстро не печатает. */
export const SCAN_GAP_MS = 50
/** Код сканера — не короче 6 знаков (как и поиск по коду в поле товара). */
export const SCAN_MIN_LENGTH = 6
/** Событие поля поиска товара: «найди этот код» — код скана, пойманный в другом поле. */
export const SCAN_EVENT = 'avtodom:scan'

/** Где фокус: поиск товара; строка ввода (количество, цена, «Получено», пробег, комментарий,
 *  поиск клиента); другое поле (список, многострочное, дата); не поле — кнопка, ссылка, флажок или
 *  сама страница. */
export type FocusKind = 'picker' | 'input' | 'text' | 'control'

/** Открыто ли окно и где фокус: в окне или за ним. */
export type DialogPlace = 'none' | 'inside' | 'outside'

export interface KeyFacts {
  key: string
  /** Настоящая клавиша или сканер; экранная клавиатура шлёт Enter сама — его не трогаем. */
  trusted: boolean
  /** Ctrl, Alt или ⌘: сочетание (Ctrl+Enter проводит чек), а не набор. */
  modified: boolean
  /** Набор через систему ввода ещё не закончен. */
  composing: boolean
  /** Автоповтор зажатой клавиши — это человек, не сканер. */
  repeat: boolean
  focus: FocusKind
  dialog: DialogPlace
  /** У экрана (или у открытого окна) есть поле поиска товара. */
  picker: boolean
}

export type KeyRoute =
  /** Не трогать. */
  | 'pass'
  /** Фокус — в поиск товара до того, как знак напечатается: и код, и Enter сканера уйдут туда. */
  | 'picker'
  /** Снять фокус с кнопки окна: Enter сканера её не нажмёт. */
  | 'blur'
  /** Погасить: Enter или пробел не должен нажать кнопку за открытым окном. */
  | 'block'
  /** Строка ввода: знак может быть началом скана — придержать (`holdStep`). */
  | 'hold'

/** Печатный знак, а не пробел (пробел нажимает кнопку, как Enter) и не служебная клавиша. */
export const isTyped = (key: string): boolean => key.length === 1 && key !== ' '

/**
 * Куда идёт нажатие. Знак, набранный на кнопке, — всегда начало скана: у кнопки набора нет, а
 * Enter после него нажал бы её ещё раз. Одиночный Enter или пробел на кнопке — клавиатура, его не
 * трогаем. Окно открыто, а фокус за ним — Enter и пробел гасятся. В строке ввода экрана (и за
 * открытым окном: окно подарка встаёт, пока фокус в количестве) знак придерживается: скан ли это,
 * ясно только по скорости и Enter в конце.
 */
export function routeKey(f: KeyFacts): KeyRoute {
  if (!f.trusted || f.modified || f.composing || f.focus === 'picker') return 'pass'
  const typed = isTyped(f.key)
  if (f.focus === 'input' && f.dialog !== 'inside' && typed && !f.repeat) return 'hold'
  if (f.dialog === 'outside') return f.key === 'Enter' || f.key === ' ' ? 'block' : 'pass'
  if (!typed) return 'pass'
  if (f.focus === 'control') return f.picker ? 'picker' : 'blur'
  return 'pass'
}

/** Придержанные знаки строки ввода и время последнего (мс, время события). */
export interface Held {
  chars: string
  last: number
}

export type HoldStep =
  /** Знак придержан; `release` — прежние знаки, которые уже не скан: их — в поле, как набраны. */
  | { kind: 'hold'; held: Held; release: string }
  /** Скан целиком: код — в поиск товара, поле не меняется, Enter гасится. */
  | { kind: 'scan'; code: string }
  /** Не скан: придержанное — в поле, клавиша идёт как обычно. */
  | { kind: 'pass'; release: string }

/**
 * Следующее нажатие при придержанных знаках строки ввода: быстрые знаки копятся; Enter сразу за
 * ними и не меньше 6 знаков — скан; иначе придержанное отдаётся полю.
 */
export function holdStep(held: Held | null, key: string, now: number, holdable: boolean): HoldStep {
  const recent = held !== null && now - held.last <= SCAN_GAP_MS
  if (key === 'Enter' && held !== null && recent && held.chars.length >= SCAN_MIN_LENGTH) return { kind: 'scan', code: held.chars }
  if (holdable && isTyped(key)) {
    if (held !== null && recent) return { kind: 'hold', held: { chars: held.chars + key, last: now }, release: '' }
    return { kind: 'hold', held: { chars: key, last: now }, release: held?.chars ?? '' }
  }
  return { kind: 'pass', release: held?.chars ?? '' }
}

// ---------- Страница ----------

type Field = HTMLInputElement | HTMLTextAreaElement

/** Типы input, в которых не набирают текст. */
const NOT_TEXT = new Set(['button', 'submit', 'reset', 'checkbox', 'radio', 'range', 'color', 'file', 'image'])
/** Строки ввода, куда знак вписывается на место курсора (у даты, числа и пароля — свой ввод). */
const LINE_TYPES = new Set(['', 'text', 'search', 'tel', 'email', 'url'])
/** Клавиши-модификаторы сами по себе: сканер жмёт Shift перед заглавной буквой кода. */
const MODIFIER_KEYS = new Set(['Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'AltGraph'])

function focusKind(el: Element | null): FocusKind {
  if (!(el instanceof HTMLElement) || el === document.body) return 'control'
  if (el.matches('[data-picker]')) return 'picker'
  if (el.isContentEditable || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement) return 'text'
  if (el instanceof HTMLInputElement) {
    if (NOT_TEXT.has(el.type) || el.readOnly) return 'control'
    return LINE_TYPES.has(el.type) ? 'input' : 'text'
  }
  return 'control'
}

/** Верхнее открытое окно. */
function topDialog(): Element | null {
  const all = document.querySelectorAll('[role="dialog"]')
  return all.length > 0 ? all[all.length - 1] : null
}

/** Поле поиска товара: в открытом окне — его (возврат без чека, плитки), иначе — экрана. */
function scopePicker(dialog: Element | null): HTMLInputElement | null {
  return (dialog ?? document).querySelector<HTMLInputElement>('input[data-picker]')
}

/** Перевести фокус в поле поиска товара экрана: следующий скан уйдёт туда. */
export function focusScanField(): void {
  scopePicker(null)?.focus()
}

/** Вписать придержанные знаки на место курсора, как набрала бы клавиатура (с пределом длины поля). */
function insertText(el: Field, text: string): void {
  let start: number | null = null
  let end: number | null = null
  try {
    start = el.selectionStart
    end = el.selectionEnd
  } catch {
    // У поля нет курсора — дописываем в конец.
  }
  const value = el.value
  const a = Math.min(start ?? value.length, value.length)
  const b = Math.max(a, Math.min(end ?? a, value.length))
  const room = el.maxLength > 0 ? Math.max(0, el.maxLength - (value.length - (b - a))) : text.length
  const add = text.slice(0, room)
  if (!add) return
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  const next = value.slice(0, a) + add + value.slice(b)
  if (setter) setter.call(el, next)
  else el.value = next
  el.dispatchEvent(new Event('input', { bubbles: true }))
  try {
    el.setSelectionRange(a + add.length, a + add.length)
  } catch {
    // Курсор не ставится — не страшно.
  }
}

/**
 * Сканер на экране: пока экран открыт, знак, набранный на кнопке или на пустом месте, переводит
 * фокус в поиск товара; Enter за открытым окном не нажимает кнопку под ним; код, отсканированный
 * в поле ввода (количество, «Получено», комментарий), уходит в поиск, а поле остаётся как было.
 * `touch` — режим кассы: клавиатуры нет, набор в списке выбора (мастер) — только от сканера.
 */
export function useScanToPicker(touch = false): void {
  useEffect(() => {
    let held: (Held & { field: Field }) | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    const release = (chars?: string) => {
      clearTimeout(timer)
      const h = held
      held = null
      const text = chars ?? h?.chars ?? ''
      if (h && text && h.field.isConnected) insertText(h.field, text)
    }
    const onKey = (e: KeyboardEvent) => {
      if (MODIFIER_KEYS.has(e.key)) return
      const active = document.activeElement
      // Придержанное — только для поля, в котором набирали; сочетание клавиш (Ctrl+Enter) — не скан.
      if (held && (held.field !== active || !e.isTrusted || e.ctrlKey || e.metaKey || e.altKey)) release()
      const dialog = topDialog()
      const route = routeKey({
        key: e.key,
        trusted: e.isTrusted,
        modified: e.ctrlKey || e.metaKey || e.altKey,
        composing: e.isComposing,
        repeat: e.repeat,
        focus: touch && active instanceof HTMLSelectElement ? 'control' : focusKind(active),
        dialog: dialog === null ? 'none' : dialog.contains(active) ? 'inside' : 'outside',
        picker: scopePicker(dialog) !== null,
      })
      if (held || route === 'hold') {
        const field = active as Field
        const step = holdStep(held, e.key, e.timeStamp, route === 'hold')
        if (step.kind === 'scan') {
          e.preventDefault()
          e.stopPropagation()
          clearTimeout(timer)
          held = null
          const picker = scopePicker(null)
          picker?.focus()
          picker?.dispatchEvent(new CustomEvent(SCAN_EVENT, { detail: step.code }))
          return
        }
        if (step.kind === 'hold') {
          if (step.release) release(step.release)
          e.preventDefault()
          clearTimeout(timer)
          held = { ...step.held, field }
          timer = setTimeout(() => release(), SCAN_GAP_MS + 10)
          return
        }
        // Не скан: придержанное — в поле, клавиша — как обычно (за окном Enter гасится ниже).
        release(step.release)
      }
      if (route === 'picker') scopePicker(dialog)?.focus()
      else if (route === 'blur' && active instanceof HTMLElement) active.blur()
      else if (route === 'block') e.preventDefault()
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      release()
    }
  }, [touch])
}
