// Обновление интерфейса через Service Worker: регистрация и применение (см. UpdateBanner).
import { registerSW } from 'virtual:pwa-register'

export let apply: ((reload?: boolean) => Promise<void>) | null = null
export const listeners = new Set<() => void>()
export let ready = false

export { cartIsEmpty } from './cartDraft'

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

/** Как применить обновление (чистая функция — проверяется тестом). */
export type ApplyPlan = 'activate-waiting' | 'fresh-from-server' | 'reload'

/**
 * «Обновить» должно срабатывать всегда (замечание заказчика 26.10.2026: «нажимаю «Обновить» — ничего
 * не происходит»). Ждущая версия есть — включаем её; нет её или она не включилась — при связи с
 * сервером сбрасываем сохранённую копию программы и грузим заново; без связи — только перезагрузка,
 * чтобы касса открылась и без сети (SPEC-09).
 */
export function applyPlan(state: { hasWaiting: boolean; waitingActivated: boolean | null; serverReachable: boolean }): ApplyPlan {
  if (state.hasWaiting && state.waitingActivated !== false) return 'activate-waiting'
  return state.serverReachable ? 'fresh-from-server' : 'reload'
}

/** Новая версия на сервере отвечает: проверяем без кэша, чтобы не сбросить копию без сети. */
async function serverReachable(): Promise<boolean> {
  try {
    const r = await fetch(`/index.html?check=${Date.now()}`, { cache: 'no-store' })
    return r.ok
  } catch {
    return false
  }
}

/**
 * Ждём, пока ждущая версия включится, не дольше `ms`. Страница, открытая мимо сохранённой копии
 * (Ctrl+F5), смены управляющей версии не получает — поэтому смотрим и на состояние самой версии.
 */
function workerActivated(w: ServiceWorker, ms: number): Promise<boolean> {
  return new Promise((resolve) => {
    const done = (ok: boolean) => {
      clearTimeout(t)
      navigator.serviceWorker.removeEventListener('controllerchange', onController)
      w.removeEventListener('statechange', onState)
      resolve(ok)
    }
    const onController = () => done(true)
    const onState = () => {
      if (w.state === 'activated') done(true)
      else if (w.state === 'redundant') done(false)
    }
    const t = setTimeout(() => done(w.state === 'activating' || w.state === 'activated'), ms)
    navigator.serviceWorker.addEventListener('controllerchange', onController)
    w.addEventListener('statechange', onState)
  })
}

/**
 * Применить новую версию интерфейса: всегда заканчивается перезагрузкой страницы. Неотправленные
 * чеки и снимок каталога лежат в IndexedDB — их не трогаем; сбрасываются только кэши оболочки.
 */
export async function applyUpdate(): Promise<void> {
  try {
    if (!('serviceWorker' in navigator)) {
      window.location.reload()
      return
    }
    const reg = await navigator.serviceWorker.getRegistration()
    const waiting = reg?.waiting ?? null
    let activated: boolean | null = null
    if (waiting) {
      const done = workerActivated(waiting, 3000)
      waiting.postMessage({ type: 'SKIP_WAITING' })
      activated = await done
    }
    const plan = applyPlan({ hasWaiting: waiting !== null, waitingActivated: activated, serverReachable: activated ? true : await serverReachable() })
    if (plan === 'fresh-from-server') {
      const regs = await navigator.serviceWorker.getRegistrations()
      await Promise.all(regs.map((r) => r.unregister().catch(() => false)))
      if ('caches' in window) {
        const keys = await caches.keys()
        await Promise.all(keys.map((k) => caches.delete(k).catch(() => false)))
      }
    }
  } catch {
    // Что бы ни случилось — страница перезагружается: кнопка не остаётся «мёртвой».
  }
  window.location.reload()
}
