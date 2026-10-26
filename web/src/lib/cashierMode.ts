// Режим кассы для сенсорного моноблока (SPEC-29, «Режим кассы»): касса во весь экран без меню,
// всё крупнее, цифры — экранной клавиатурой. Выбор помнит устройство.
import { useSyncExternalStore } from 'react'
import { useLocation } from 'react-router-dom'
import type { KeypadKind } from './keypad'

const KEY = 'avtodom.cashier_mode'

function read(): boolean {
  try {
    return localStorage.getItem(KEY) === '1'
  } catch {
    return false
  }
}

let on = read()
const listeners = new Set<() => void>()

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => listeners.delete(fn)
}

/** Включить или выключить режим на этом устройстве. */
export function setCashierMode(value: boolean): void {
  on = value
  try {
    if (value) localStorage.setItem(KEY, '1')
    else localStorage.removeItem(KEY)
  } catch {
    // Без хранилища режим живёт до перезагрузки.
  }
  listeners.forEach((fn) => fn())
}

/** Выбран ли режим кассы на устройстве (на любом экране). */
export function useCashierMode(): boolean {
  return useSyncExternalStore(subscribe, () => on, () => false)
}

/** Режим действует: выбран и открыт экран «Касса». На остальных экранах — обычный вид с меню. */
export function useTouchMode(): boolean {
  const mode = useCashierMode()
  const { pathname } = useLocation()
  return mode && pathname === '/'
}

// Экранная клавиатура режима кассы (ADR-101): сама не появляется — её открывают кнопкой «⌨
// Клавиатура» в полосе режима или у поля в фокусе, и она остаётся до «Скрыть».
let keyboard = false
const keyboardListeners = new Set<() => void>()

function subscribeKeyboard(fn: () => void): () => void {
  keyboardListeners.add(fn)
  return () => keyboardListeners.delete(fn)
}

/** Открыть или скрыть экранную клавиатуру. */
export function setKeyboardOpen(value: boolean): void {
  if (keyboard === value) return
  keyboard = value
  keyboardListeners.forEach((fn) => fn())
}

/** Открыта ли экранная клавиатура. */
export function useKeyboardOpen(): boolean {
  return useSyncExternalStore(subscribeKeyboard, () => keyboard, () => false)
}

// Верх полной экранной клавиатуры, точки: список найденного товара кончается над ней, иначе его
// строки уходили под клавиатуру (проверка 26.10.2026). null — клавиатуры букв на экране нет.
let keyboardTop: number | null = null
const keyboardTopListeners = new Set<() => void>()

export function setKeyboardTop(value: number | null): void {
  if (keyboardTop === value) return
  keyboardTop = value
  keyboardTopListeners.forEach((fn) => fn())
}

/** Верх клавиатуры букв на экране или null. */
export function useKeyboardTop(): number | null {
  return useSyncExternalStore(
    (fn) => {
      keyboardTopListeners.add(fn)
      return () => keyboardTopListeners.delete(fn)
    },
    () => keyboardTop,
    () => null,
  )
}

/** Во весь экран; браузер может отказать (нет жеста пользователя, запрет) — тогда просто окно. */
export function enterFullscreen(): void {
  try {
    void document.documentElement.requestFullscreen?.().catch(() => undefined)
  } catch {
    // Нет такой возможности — работаем в окне.
  }
}

export function exitFullscreen(): void {
  try {
    if (document.fullscreenElement) void document.exitFullscreen().catch(() => undefined)
  } catch {
    // Уже не во весь экран.
  }
}

/**
 * Свойства числового поля: в режиме кассы системная клавиатура не выезжает (`inputMode="none"`),
 * цифры вводят экранной клавиатурой; вне режима — обычная цифровая клавиатура телефона.
 */
export function useNumField(): (kind: KeypadKind, opts?: { decimals?: number; quick?: string[] }) => {
  inputMode: 'none' | 'numeric' | 'decimal'
  'data-keypad': KeypadKind
  'data-keypad-decimals'?: number
  'data-keypad-quick'?: string
} {
  const touch = useTouchMode()
  return (kind, opts) => ({
    inputMode: touch ? 'none' : kind === 'decimal' ? 'decimal' : 'numeric',
    'data-keypad': kind,
    'data-keypad-decimals': opts?.decimals,
    'data-keypad-quick': opts?.quick?.length ? opts.quick.join(';') : undefined,
  })
}
