// Узкая полоса сверху в режиме кассы (SPEC-29, «Режим кассы»): смена и кассир, связь и очередь
// чеков, экранная клавиатура (ADR-101), во весь экран и выход из режима. Меню приложения в режиме скрыто.
// При открытой смене — «Закрыть смену» (то же окно, что на экране «Смена») и «Чек смены» (SPEC-36 §10).
import { useEffect, useState, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from 'react'
import { enterFullscreen, exitFullscreen, setCashierMode, setKeyboardOpen, useKeyboardOpen } from '../lib/cashierMode'
import { isOwnItem, pendingCount, watchOutbox } from '../lib/offline'
import { useUser } from '../lib/auth'
import { useServerOnline } from '../lib/hooks'
import { useAction } from '../lib/hooks'
import { useShiftClose } from './ShiftClose'
import { printShift } from './shiftPrint'
import { toast } from './ui'
import type { Shift } from '../lib/types'

export function CashierModeBar({
  shift,
  onOpenShift,
  onShiftChanged,
}: {
  shift: Shift | null | undefined
  onOpenShift: () => void
  /** Смену закрыли или сдали кассу — перечитать смену на кассе. */
  onShiftChanged?: () => void
}) {
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
  const keyboard = useKeyboardOpen()
  const closer = useShiftClose(() => onShiftChanged?.())
  const printing = useAction()
  const printCheck = (id: string) =>
    void printing.run(async () => {
      await printShift(id).catch(() => toast('Чек смены не напечатался — нет связи с сервером'))
    })
  // Кнопка клавиатуры не забирает фокус у поля: ввод пойдёт в него.
  const keepFocus = (e: ReactPointerEvent | ReactMouseEvent) => e.preventDefault()

  return (
    <>
      <div
        // На широком экране — одной строкой: длинное имя кассира сокращается, а не переносит кнопки
        // на вторую строку (место по высоте — чеку, SPEC-30 §17).
        className="keep-dark no-print mt-2 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg bg-slate-900 px-3 py-1 text-white md:px-4 lg:flex-nowrap"
        data-testid="cashier-mode-bar"
      >
        <span className="text-lg font-bold">Касса</span>
        {shift ? (
          <>
            <span className="min-w-0 truncate text-base" data-testid="mode-shift">
              Смена № {shift.number} · кассир <b>{shift.cashier_name}</b>
            </span>
            <span className="flex shrink-0 gap-2">
              <button
                type="button"
                className="rounded-lg border border-slate-500 px-4 text-base font-medium active:bg-slate-700"
                disabled={printing.busy}
                onClick={() => printCheck(shift.id)}
              >
                Чек смены
              </button>
              <button
                type="button"
                className="rounded-lg bg-amber-500 px-4 text-base font-semibold text-slate-900 active:bg-amber-400"
                onClick={closer.startClosing}
              >
                Закрыть смену
              </button>
            </span>
          </>
        ) : shift === null ? (
          <span className="flex flex-wrap items-center gap-3 text-base text-amber-300">
            Смена не открыта
            <button type="button" className="rounded-lg bg-amber-500 px-5 font-semibold text-slate-900 active:bg-amber-400" onClick={onOpenShift}>
              Открыть смену
            </button>
          </span>
        ) : null}
        <span className={`flex shrink-0 items-center gap-2 text-base whitespace-nowrap ${online ? 'text-emerald-300' : 'text-rose-300'}`} data-testid="mode-online">
          <span className={`h-3 w-3 rounded-full ${online ? 'bg-emerald-400' : 'bg-rose-500'}`} aria-hidden="true" />
          {online ? 'В сети' : 'Нет сети — чеки уйдут позже'}
          {waiting > 0 && <span className="text-amber-300"> · не отправлено: {waiting}</span>}
        </span>
        <span className="ml-auto flex shrink-0 flex-wrap gap-2 lg:flex-nowrap">
          <button
            type="button"
            aria-pressed={keyboard}
            className={`rounded-lg px-5 text-base font-semibold ${keyboard ? 'border-2 border-sky-400 bg-slate-700 text-white active:bg-slate-800' : 'border border-slate-500 active:bg-slate-700'}`}
            onPointerDown={keepFocus}
            onMouseDown={keepFocus}
            onClick={() => setKeyboardOpen(!keyboard)}
          >
            ⌨ Клавиатура
          </button>
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
      {/* Окна — вне тёмной полосы: у них свои цвета. */}
      {closer.modals}
    </>
  )
}
