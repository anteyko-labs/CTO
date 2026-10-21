// Узкая полоса сверху в режиме кассы (SPEC-29, «Режим кассы»): смена и кассир, связь и очередь
// чеков, во весь экран и выход из режима. Меню приложения в режиме скрыто.
import { useEffect, useState } from 'react'
import { enterFullscreen, exitFullscreen, setCashierMode } from '../lib/cashierMode'
import { isOwnItem, pendingCount, watchOutbox } from '../lib/offline'
import { useUser } from '../lib/auth'
import { useServerOnline } from '../lib/hooks'
import type { Shift } from '../lib/types'

export function CashierModeBar({ shift, onOpenShift }: { shift: Shift | null | undefined; onOpenShift: () => void }) {
  // Связь — с сервером, а не только сеть устройства; не отправлено — чеки вошедшего пользователя.
  const online = useServerOnline()
  const me = useUser().id
  const [waiting, setWaiting] = useState(0)
  const [full, setFull] = useState(() => Boolean(document.fullscreenElement))
  useEffect(() => {
    const fs = () => setFull(Boolean(document.fullscreenElement))
    document.addEventListener('fullscreenchange', fs)
    const stop = watchOutbox((items) => setWaiting(pendingCount(items.filter((i) => isOwnItem(i, me)))))
    return () => {
      document.removeEventListener('fullscreenchange', fs)
      stop()
    }
  }, [me])
  const canFull = typeof document.documentElement.requestFullscreen === 'function'

  return (
    <div
      className="keep-dark no-print mt-2 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg bg-slate-900 px-3 py-1 text-white md:px-4"
      data-testid="cashier-mode-bar"
    >
      <span className="text-lg font-bold">Касса</span>
      {shift ? (
        <span className="text-base" data-testid="mode-shift">
          Смена № {shift.number} · кассир <b>{shift.cashier_name}</b>
        </span>
      ) : shift === null ? (
        <span className="flex flex-wrap items-center gap-3 text-base text-amber-300">
          Смена не открыта
          <button type="button" className="rounded-lg bg-amber-500 px-5 font-semibold text-slate-900 active:bg-amber-400" onClick={onOpenShift}>
            Открыть смену
          </button>
        </span>
      ) : null}
      <span className={`flex items-center gap-2 text-base ${online ? 'text-emerald-300' : 'text-rose-300'}`} data-testid="mode-online">
        <span className={`h-3 w-3 rounded-full ${online ? 'bg-emerald-400' : 'bg-rose-500'}`} aria-hidden="true" />
        {online ? 'В сети' : 'Нет сети — чеки уйдут позже'}
        {waiting > 0 && <span className="text-amber-300"> · не отправлено: {waiting}</span>}
      </span>
      <span className="ml-auto flex flex-wrap gap-2">
        {canFull && !full && (
          <button type="button" className="rounded-lg border border-slate-500 px-5 text-base font-medium active:bg-slate-700" onClick={enterFullscreen}>
            Во весь экран
          </button>
        )}
        <button
          type="button"
          className="rounded-lg bg-sky-600 px-5 text-base font-semibold text-white active:bg-sky-700"
          onClick={() => {
            exitFullscreen()
            setCashierMode(false)
          }}
        >
          Выйти из режима кассы
        </button>
      </span>
    </div>
  )
}
