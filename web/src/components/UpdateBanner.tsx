// Новая версия интерфейса: касса на моноблоке открыта всегда, поэтому обновление не ждёт закрытия
// вкладок — при пустом чеке применяется само, иначе — плашка «Обновить».
import { useEffect, useState } from 'react'
import { apply, cartIsEmpty, listeners, ready } from '../lib/updates'

export function UpdateBanner() {
  const [shown, setShown] = useState(ready)
  useEffect(() => {
    const on = () => setShown(true)
    listeners.add(on)
    return () => {
      listeners.delete(on)
    }
  }, [])
  // Чек пуст минуту подряд — обновляемся сами: кассир ничего не потеряет.
  useEffect(() => {
    if (!shown) return
    const t = setInterval(() => {
      if (cartIsEmpty() && document.visibilityState === 'visible') void apply?.(true)
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
