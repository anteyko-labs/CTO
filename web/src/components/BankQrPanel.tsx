// QR на сумму чека (SPEC-31): кассир просит у сервера QR банка Optima Business на сумму к оплате
// картой и показывает его покупателю; сервер опрашивает банк, касса спрашивает у сервера раз в 2 с.
// В режиме кассы QR — во всю правую колонку, кнопки не ниже 56 точек (ui-rules §9).
// Сервер молчит три опроса подряд — «Нет связи с сервером», чек не ждёт QR (ADR-130).
import { useCallback, useEffect, useState } from 'react'
import { get, newOpId, post } from '../lib/api'
import { formatSom } from '../lib/format'
import { errorText } from '../lib/hooks'
import { countdown, OPEN_QR_EVERY_MS, QR_LOST_AFTER, QR_STATUS_TEXT, secondsLeft, type BankQr, type QrView } from '../lib/optima'
import { Button } from './ui'

/** QR кассы: создать, следить за оплатой, отменить. */
export function useBankQr() {
  const [qr, setQr] = useState<QrView | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [now, setNow] = useState(() => Date.now())
  // Неудачных опросов сервера подряд.
  const [failures, setFailures] = useState(0)
  const lost = failures >= QR_LOST_AFTER
  const waiting = qr?.status === 'waiting'
  // Неоплаченный в срок QR сервер ещё 30 минут проверяет: поздняя оплата (тест банка 23.10.2026)
  // появится на кассе сама, пока окно QR открыто.
  const watching = waiting || qr?.status === 'unpaid'
  const id = qr?.id

  const take = useCallback((r: BankQr) => setQr((q) => (q && q.id !== r.id ? q : { ...r, qr_base64: r.qr_base64 ?? q?.qr_base64, received: Date.now() })), [])

  useEffect(() => {
    if (!watching || !id) return
    const tick = setInterval(() => setNow(Date.now()), 1000)
    const poll = setInterval(() => {
      void get<BankQr>(`/bank-qr/${id}`)
        .then((r) => {
          setFailures(0)
          take(r)
        })
        .catch(() => setFailures((n) => n + 1))
    }, 2000)
    return () => {
      clearInterval(tick)
      clearInterval(poll)
    }
  }, [watching, id, take])

  const create = async (amount: number) => {
    setBusy(true)
    setError(null)
    try {
      const r = await post<BankQr>('/bank-qr', { op_id: newOpId(), amount_tyiyn: amount })
      setNow(Date.now())
      setFailures(0)
      setQr({ ...r, received: Date.now() })
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  /** QR этой кассы из списка сервера (оплаченный без чека или ждущий) — на экран. */
  const adopt = (r: BankQr) => {
    setNow(Date.now())
    setFailures(0)
    setError(null)
    setQr({ ...r, received: Date.now() })
  }

  /** «Отменить»: ждущий QR больше не ждём; оплаченный сервер не отменит — он и останется.
   *  Без связи с сервером окно просто закрывается: оплата, если была, придёт в список QR кассы. */
  const cancel = async () => {
    if (!qr) return
    if (qr.status !== 'waiting') {
      setQr(null)
      return
    }
    if (lost) {
      void post(`/bank-qr/${qr.id}/cancel`).catch(() => undefined)
      setQr(null)
      setFailures(0)
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
    setFailures(0)
  }

  return { qr, error, busy, now, lost, create, adopt, cancel, clear, setError }
}

/** Ждущие и оплаченные без чека QR этой кассы (ADR-130): при открытии кассы и раз в 5 с. */
export function useOpenQrs(enabled: boolean) {
  const [list, setList] = useState<BankQr[]>([])
  const refresh = useCallback(() => {
    if (!enabled) return
    void get<BankQr[]>('/bank-qr/open')
      .then(setList)
      .catch(() => undefined)
  }, [enabled])
  useEffect(() => {
    if (!enabled) return
    refresh()
    const t = setInterval(refresh, OPEN_QR_EVERY_MS)
    return () => clearInterval(t)
  }, [enabled, refresh])
  /** QR ушёл в чек — из списка сразу, не дожидаясь сервера. */
  const forget = useCallback((id: string | undefined) => {
    if (id) setList((l) => l.filter((q) => q.id !== id))
  }, [])
  return { list: enabled ? list : [], refresh, forget }
}

/** QR на экране: картинка, сумма, обратный отсчёт; не оплачен или отклонён — повторить. */
export function BankQrPanel({
  qr,
  now,
  touch,
  busy,
  lost = false,
  onCancel,
  onRetry,
}: {
  qr: QrView
  now: number
  touch: boolean
  busy: boolean
  /** Сервер не отвечает три опроса подряд. */
  lost?: boolean
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
          {qr.status === 'waiting' && lost ? (
            <b className="text-rose-700" data-testid="bank-qr-lost">
              Нет связи с сервером — отметьте оплату вручную
            </b>
          ) : qr.status === 'waiting' ? (
            left > 0 ? (
              <>
                Ждём оплату: покупатель сканирует QR в приложении банка · осталось <b className="tabular-nums">{countdown(left)}</b>
              </>
            ) : (
              'Срок QR вышел — ждём ответ банка…'
            )
          ) : qr.status === 'unpaid' ? (
            'Если покупатель всё же оплатил — касса заметит это сама в ближайшие 30 минут. Иначе создайте новый QR или отметьте оплату вручную.'
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
            {qr.status === 'waiting' && !lost ? 'Отменить' : 'Закрыть'}
          </Button>
        </div>
      </div>
    </div>
  )
}
