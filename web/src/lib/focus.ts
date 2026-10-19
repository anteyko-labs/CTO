// Перевод фокуса после отрисовки: поле, которое появится в этой же отрисовке (новая строка,
// поле после закрытия окна), получает фокус сразу, как только оно есть на странице, а не по
// таймеру. Таймер срабатывал позже и перехватывал ввод: набранная цена уходила в количество.
// Отложенное выделение числового поля при фокусе — тоже только пока фокус в нём (main.tsx).
import { useCallback, useLayoutEffect, useRef, useState } from 'react'

interface Focusable {
  focus: () => void
  select?: () => void
}

/**
 * Отдать фокус ожидающему полю: `find` ищет его по селектору. Запрос выполняется один раз —
 * нашли поле или нет, — чтобы старый запрос не перехватил фокус позже. Возвращает, удалось ли.
 */
export function applyFocus(target: string | null, find: (selector: string) => Focusable | null): boolean {
  if (target === null) return false
  const el = find(target)
  if (!el) return false
  el.focus()
  // Выделяем текст, чтобы ввод сразу заменил подставленное значение.
  el.select?.()
  return true
}

/**
 * Выделить ли числовое поле после получения фокуса (выделение откладывается до конца события).
 * Только если фокус всё ещё в нём (иначе select() вернёт фокус обратно) и значение не меняли
 * (иначе выделение съест уже набранное).
 */
export function shouldSelectOnFocus(stillFocused: boolean, valueAtFocus: string, valueNow: string): boolean {
  return stillFocused && valueAtFocus === valueNow
}

const findInDocument = (selector: string): Focusable | null => {
  const el = document.querySelector<HTMLElement>(selector)
  if (!el) return null
  return el instanceof HTMLInputElement ? el : { focus: () => el.focus() }
}

/**
 * Возвращает `focusAfterRender(selector)`: фокус переходит в поле в той отрисовке, что вызвана
 * этим же обработчиком, — до того, как пользователь успеет что-то набрать.
 */
export function useFocusAfterRender(): (selector: string) => void {
  const pending = useRef<string | null>(null)
  const [tick, setTick] = useState(0)
  useLayoutEffect(() => {
    const target = pending.current
    pending.current = null
    applyFocus(target, findInDocument)
  }, [tick])
  return useCallback((selector: string) => {
    pending.current = selector
    setTick((t) => t + 1)
  }, [])
}
