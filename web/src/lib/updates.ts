// Обновление интерфейса через Service Worker: регистрация и применение (см. UpdateBanner).
import { registerSW } from 'virtual:pwa-register'

export let apply: ((reload?: boolean) => Promise<void>) | null = null
export const listeners = new Set<() => void>()
export let ready = false

/** Пустой ли чек на кассе: черновик хранится в sessionStorage (см. Cashier.tsx). */
export function cartIsEmpty(): boolean {
  try {
    const raw = sessionStorage.getItem('avtodom.cart')
    if (!raw) return true
    const d = JSON.parse(raw) as { lines?: unknown[] }
    return !d.lines || d.lines.length === 0
  } catch {
    return true
  }
}

export function startUpdates(): void {
  if (apply || !('serviceWorker' in navigator)) return
  apply = registerSW({
    onNeedRefresh() {
      ready = true
      listeners.forEach((l) => l())
    },
    onRegisteredSW(_url, reg) {
      // Долго открытая касса сама спрашивает о новой версии раз в 30 минут.
      if (reg) setInterval(() => void reg.update().catch(() => undefined), 30 * 60_000)
    },
  })
}
