// Новая версия интерфейса: касса на моноблоке открыта всегда, поэтому обновление не ждёт закрытия
// вкладок — на кассе с пустым чеком применяется само, на остальных экранах — плашка «Обновить».
import { useEffect, useState, useSyncExternalStore } from 'react'
import { applyUpdate, cartIsEmpty, listeners, ready } from '../lib/updates'
import { canAutoApply } from '../lib/updateRules'

const subscribe = (on: () => void) => {
  listeners.add(on)
  return () => {
    listeners.delete(on)
  }
}

export function UpdateBanner() {
  // Весть о новой версии приходит через десятки миллисекунд после F5 — между отрисовкой и
  // подпиской; useSyncExternalStore перечитывает `ready` после подписки, плашка не теряется.
  const shown = useSyncExternalStore(subscribe, () => ready)
  const [busy, setBusy] = useState(false)
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
      if (canAutoApply(now)) void applyUpdate()
    }, 60_000)
    return () => clearInterval(t)
  }, [shown])
  if (!shown) return null
  return (
    <div className="no-print flex items-center justify-center gap-3 bg-sky-600 px-4 py-1.5 text-xs text-white">
      Доступна новая версия программы
      <button
        type="button"
        className="rounded bg-white/20 px-2 py-0.5 font-medium hover:bg-white/30 disabled:opacity-70"
        disabled={busy}
        onClick={() => {
          setBusy(true)
          void applyUpdate()
        }}
      >
        {busy ? 'Обновляем…' : 'Обновить'}
      </button>
    </div>
  )
}
