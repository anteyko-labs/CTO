// Новая версия интерфейса: касса на моноблоке открыта всегда, поэтому обновление не ждёт закрытия
// вкладок — на кассе с пустым чеком применяется само, на остальных экранах — плашка «Обновить».
import { useEffect, useState } from 'react'
import { apply, cartIsEmpty, listeners, ready } from '../lib/updates'
import { canAutoApply } from '../lib/updateRules'

export function UpdateBanner() {
  const [shown, setShown] = useState(ready)
  useEffect(() => {
    const on = () => setShown(true)
    listeners.add(on)
    return () => {
      listeners.delete(on)
    }
  }, [])
  // Раз в минуту: касса, чек пуст, окон нет — обновляемся сами, кассир ничего не потеряет.
  // Формы других экранов живут только в памяти — там ждём кнопку (ADR-059).
  useEffect(() => {
    if (!shown) return
    const t = setInterval(() => {
      const now = {
        path: window.location.pathname,
        cartEmpty: cartIsEmpty(),
        modalOpen: document.querySelector('[role="dialog"]') !== null,
        visible: document.visibilityState === 'visible',
      }
      if (canAutoApply(now)) void apply?.(true)
    }, 60_000)
    return () => clearInterval(t)
  }, [shown])
  if (!shown) return null
  return (
    <div className="no-print flex items-center justify-center gap-3 bg-sky-600 px-4 py-1.5 text-xs text-white">
      Доступна новая версия программы
      <button type="button" className="rounded bg-white/20 px-2 py-0.5 font-medium hover:bg-white/30" onClick={() => void apply?.(true)}>
        Обновить
      </button>
    </div>
  )
}
