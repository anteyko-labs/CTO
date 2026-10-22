// QR на сумму чека (SPEC-31): кассир просит у сервера QR банка Optima Business на сумму к оплате
// картой и показывает его покупателю; сервер опрашивает банк, касса спрашивает у сервера раз в 2 с.
// В режиме кассы QR — во всю правую колонку, кнопки не ниже 56 точек (ui-rules §9).
import { useCallback, useEffect, useState } from 'react'
import { get, newOpId, post } from '../lib/api'
import { formatSom } from '../lib/format'
import { errorText } from '../lib/hooks'
import { countdown, QR_STATUS_TEXT, secondsLeft, type BankQr, type QrView } from '../lib/optima'
import { Button } from './ui'

/** QR кассы: создать, следить за оплатой, отменить. */
export function useBankQr() {
  const [qr, setQr] = useState<QrView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  const waiting = qr?.status === 'waiting'
  const id = qr?.id

  const take = useCallback((r: BankQr) => setQr((q) => (q && q.id !== r.id ? q : { ...r, qr_base64: r.qr_base64 ?? q?.qr_base64, received: Date.now() })), [])

  useEffect(() => {
    if (!waiting || !id) return
    const tick = setInterval(() => setNow(Date.now()), 1000)
    const poll = setInterval(() => {
      void get<BankQr>(`/bank-qr/${id}`)
        .then(take)
        .catch(() => undefined)
    }, 2000)
    return () => {
      clearInterval(tick)
      clearInterval(poll)
    }
  }, [waiting, id, take])

  const create = async (amount: number) => {
    setBusy(true)
    setError(null)
    try {
      const r = await post<BankQr>('/bank-qr', { op_id: newOpId(), amount_tyiyn: amount })
      setNow(Date.now())
      setQr({ ...r, received: Date.now() })
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  /** «Отменить»: ждущий QR больше не ждём; оплаченный сервер не отменит — он и останется. */
  const cancel = async () => {
    if (!qr) return
    if (qr.status !== 'waiting') {
      setQr(null)
      return
    }
    setBusy(true)
    try {
      take(await post<BankQr>(`/bank-qr/${qr.id}/cancel`))
      setQr((q) => (q && q.status === 'cancelled' ? null : q))
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const clear = () => {
    setQr(null)
    setError(null)
  }

  return { qr, error, busy, now, create, cancel, clear, setError }
}

/** QR на экране: картинка, сумма, обратный отсчёт; не оплачен или отклонён — повторить. */
export function BankQrPanel({
  qr,
  now,
  touch,
  busy,
  onCancel,
  onRetry,
}: {
  qr: QrView
  now: number
  touch: boolean
  busy: boolean
  onCancel: () => void
  onRetry: () => void
}) {
  const left = secondsLeft(qr, now)
  const failed = qr.status === 'unpaid' || qr.status === 'error' || qr.status === 'cancelled'
  return (
    <div
      data-testid="bank-qr"
      className={
        touch
          ? 'absolute inset-0 z-20 flex flex-col items-center gap-1.5 rounded-lg bg-white p-2.5'
          : 'flex flex-col items-center gap-2 rounded-md border border-sky-200 bg-sky-50/40 p-3'
      }
    >
      <div className="flex w-full items-baseline justify-between gap-2">
        <span className={touch ? 'text-xl font-semibold' : 'text-base font-semibold'}>QR банка на сумму</span>
        <span className={`tabular-nums font-bold ${touch ? 'text-3xl' : 'text-2xl'}`} data-testid="bank-qr-amount">
          {formatSom(qr.amount_tyiyn)}
        </span>
      </div>
      {qr.status === 'waiting' && qr.qr_base64 ? (
        <img
          src={`data:image/png;base64,${qr.qr_base64}`}
          alt={`QR на ${formatSom(qr.amount_tyiyn)}`}
          className={touch ? 'min-h-0 w-full flex-1 object-contain [image-rendering:pixelated]' : 'h-60 w-60 object-contain [image-rendering:pixelated]'}
        />
      ) : (
        <div className={`flex w-full items-center justify-center rounded-md ${touch ? 'min-h-0 flex-1' : 'h-24'} ${failed ? 'bg-rose-50' : 'bg-slate-50'}`}>
          <span className={`text-center font-semibold ${touch ? 'text-2xl' : 'text-lg'} ${failed ? 'text-rose-700' : 'text-slate-600'}`} role="status">
            {QR_STATUS_TEXT[qr.status]}
          </span>
        </div>
      )}
      <div className={`flex w-full items-center gap-2 ${touch ? '' : 'flex-col'}`}>
        <div className={`min-w-0 flex-1 text-slate-600 ${touch ? 'text-base leading-snug' : 'text-center text-sm'}`} role="status" data-testid="bank-qr-wait">
          {qr.status === 'waiting' ? (
            left > 0 ? (
              <>
                Ждём оплату: покупатель сканирует QR в приложении банка · осталось <b className="tabular-nums">{countdown(left)}</b>
              </>
            ) : (
              'Срок QR вышел — ждём ответ банка…'
            )
          ) : qr.status !== 'cancelled' ? (
            'Создайте новый QR или отметьте оплату картой вручную.'
          ) : null}
        </div>
        <div className={`flex shrink-0 gap-2 ${touch ? '' : 'w-full'}`}>
          {failed && (
            <Button className={`flex-1 ${touch ? 'min-h-[56px]! px-5! text-lg!' : ''}`} disabled={busy} onClick={onRetry}>
              Новый QR
            </Button>
          )}
          <Button variant="secondary" className={`flex-1 ${touch ? 'min-h-[56px]! px-6! text-lg!' : ''}`} disabled={busy} onClick={onCancel}>
            {qr.status === 'waiting' ? 'Отменить' : 'Закрыть'}
          </Button>
        </div>
      </div>
    </div>
  )
}
