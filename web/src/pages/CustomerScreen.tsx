// Экран покупателя на втором мониторе кассы (задача 008, ADR-179): заставка, QR банка на сумму
// крупно по центру, «Оплачено ✓», подпись клиента за долг пером планшета. Всё приходит от кассы
// этого же браузера (BroadcastChannel) — сервер и сеть не нужны.
import { useEffect, useState } from 'react'
import { SignaturePad } from '../components/SignaturePad'
import { formatSom } from '../lib/format'
import { openChannel, SCREEN_PING_MS, screenStep, screenTick, type FromScreen, type ScreenState, type ToScreen } from '../lib/customerScreen'

export default function CustomerScreen() {
  const [state, setState] = useState<ScreenState>({ kind: 'idle' })
  const [channel] = useState(openChannel)

  useEffect(() => {
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
    }
  }, [channel])

  const signed = (png: string) => {
    if (state.kind !== 'sign') return
    channel?.postMessage({ type: 'signed', id: state.id, png } satisfies FromScreen)
    setState({ kind: 'idle' })
  }

  return (
    <div className="flex h-dvh w-full cursor-default flex-col items-center justify-center bg-white p-8 text-slate-900" data-testid="customer-screen">
      {!channel && <div className="text-2xl text-rose-700">Браузер не поддерживает экран покупателя — обновите Chrome</div>}
      {state.kind === 'idle' && (
        <div className="text-center" data-testid="customer-idle">
          <div className="text-6xl font-bold tracking-tight">Avtodom</div>
          <div className="mt-4 text-2xl text-slate-500">Спасибо, что вы с нами</div>
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
