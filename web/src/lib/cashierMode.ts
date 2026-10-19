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
