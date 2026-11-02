// Экран покупателя на втором мониторе кассы (задача 008, ADR-179): заставка, QR банка на сумму
// крупно по центру, «Оплачено ✓», подпись клиента за долг пером планшета. Всё приходит от кассы
// этого же браузера (BroadcastChannel) — сервер и сеть не нужны.
import { useEffect, useRef, useState } from 'react'
import { SignaturePad } from '../components/SignaturePad'
import { formatSom } from '../lib/format'
import { openChannel, SCREEN_PING_MS, screenStep, screenTick, type FromScreen, type ScreenState, type ToScreen } from '../lib/customerScreen'

export default function CustomerScreen() {
  const [state, setState] = useState<ScreenState>({ kind: 'idle' })
  const ref = useRef<BroadcastChannel | null>(null)
  const supported = typeof BroadcastChannel !== 'undefined'

  useEffect(() => {
    const channel = openChannel()
    ref.current = channel
    if (!channel) return
    const send = (m: FromScreen) => channel.postMessage(m)
    channel.onmessage = (e: MessageEvent<ToScreen>) => setState((s) => screenStep(s, e.data, Date.now()))
    send({ type: 'here' })
    const ping = setInterval(() => {
      send({ type: 'here' })
      setState((s) => screenTick(s, Date.now()))
    }, SCREEN_PING_MS)
    return () => {
      clearInterval(ping)
      channel.close()
      ref.current = null
    }
  }, [])

  const signed = (png: string) => {
    if (state.kind !== 'sign') return
    ref.current?.postMessage({ type: 'signed', id: state.id, png } satisfies FromScreen)
    setState({ kind: 'idle' })
  }

  return (
    <div className="flex h-dvh w-full cursor-default flex-col items-center justify-center bg-white p-8 text-slate-900" data-testid="customer-screen">
      {!supported && <div className="text-2xl text-rose-700">Браузер не поддерживает экран покупателя — обновите Chrome</div>}
      {state.kind === 'idle' && (
        <div className="text-center" data-testid="customer-idle">
          <div className="text-6xl font-bold tracking-tight">Avtodom</div>
          <div className="mt-4 text-2xl text-slate-500">Спасибо, что вы с нами</div>
        </div>
      )}
      {state.kind === 'cart' && (
        <div className="flex h-full w-full max-w-5xl flex-col gap-4" data-testid="customer-cart">
          <div className="text-3xl font-semibold">Ваша покупка</div>
          <div className="min-h-0 flex-1 overflow-hidden">
            {/* Новые строки — внизу; не влезли — видны последние. */}
            <div className="flex h-full flex-col justify-end divide-y divide-slate-200 text-2xl">
              {state.lines.slice(-12).map((l, i) => (
                <div key={i} className="flex items-baseline gap-4 py-2">
                  <span className="min-w-0 flex-1 truncate">{l.name}</span>
                  <span className="shrink-0 text-slate-500 tabular-nums">{l.qty}</span>
                  <span className="w-48 shrink-0 text-right font-semibold tabular-nums">{formatSom(l.amount_tyiyn)}</span>
                </div>
              ))}
            </div>
          </div>
          <div className="flex items-baseline justify-between border-t-4 border-slate-900 pt-4">
            <span className="text-4xl font-semibold">Итого</span>
            <span className="text-6xl font-bold tabular-nums" data-testid="customer-cart-total">
              {formatSom(state.total_tyiyn)}
            </span>
          </div>
        </div>
      )}
      {state.kind === 'qr' && (
        <div className="flex h-full w-full flex-col items-center justify-center gap-6" data-testid="customer-qr">
          <div className="text-4xl font-semibold">К оплате</div>
          <div className="text-7xl font-bold tabular-nums" data-testid="customer-qr-amount">
            {formatSom(state.amount_tyiyn)}
          </div>
          <img
            src={`data:image/png;base64,${state.qr_base64}`}
            alt={`QR на ${formatSom(state.amount_tyiyn)}`}
            className="min-h-0 w-auto max-w-full flex-1 object-contain [image-rendering:pixelated]"
          />
          <div className="text-2xl text-slate-600">Отсканируйте QR в приложении своего банка</div>
        </div>
      )}
      {state.kind === 'paid' && (
        <div className="text-center" data-testid="customer-paid">
          <div className="text-[10rem] leading-none text-emerald-600">✓</div>
          <div className="mt-6 text-6xl font-bold text-emerald-700">Оплачено</div>
          <div className="mt-4 text-4xl tabular-nums">{formatSom(state.amount_tyiyn)}</div>
        </div>
      )}
      {state.kind === 'sign' && (
        <div className="flex w-full max-w-5xl flex-col gap-4" data-testid="customer-sign">
          <div className="text-4xl font-semibold">{state.title}</div>
          {state.lines.map((l) => (
            <div key={l} className="text-2xl text-slate-700">
              {l}
            </div>
          ))}
          <SignaturePad hint="Распишитесь пером на планшете в белом поле" doneLabel="Подписать" onDone={signed} />
        </div>
      )}
    </div>
  )
}
