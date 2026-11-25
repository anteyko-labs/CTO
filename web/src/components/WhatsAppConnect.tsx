// WhatsApp по QR-коду (задача 052, ADR-217): подключить номер магазина как связанное устройство —
// QR-код или код из 8 знаков, состояние, «Отключить» и переписка с клиентами. Только владелец.
import QRCode from 'qrcode'
import { useEffect, useState } from 'react'
import { Badge, Button, Card, CardTitle, Empty, ErrorBox, Field, toast } from './ui'
import { get, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import {
  codeText,
  DIRECTION_LABELS,
  pairPhoneProblem,
  polling,
  STATE_LABELS,
  type WaGateStatus,
  type WaInboxRow,
  type WaStatus,
} from '../lib/whatsappQr'

function QrImage({ value }: { value: string }) {
  const [src, setSrc] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    QRCode.toDataURL(value, { margin: 1, width: 256, errorCorrectionLevel: 'L' })
      .then((u) => alive && setSrc(u))
      .catch(() => alive && setSrc(null))
    return () => {
      alive = false
    }
  }, [value])
  return src ? <img src={src} alt="QR-код для WhatsApp" data-testid="wa-qr" className="h-64 w-64 rounded border border-slate-200" /> : null
}

export function WhatsAppConnect() {
  const st = useLoad(() => get<WaStatus>('/whatsapp'), [])
  const act = useAction()
  const [byCode, setByCode] = useState(false)
  const [phone, setPhone] = useState('')
  const [live, setLive] = useState<WaGateStatus | null>(null)
  const gate = live ?? st.data?.gate ?? null
  const state = gate?.state ?? 'off'

  // Пока ждём скан или код — спрашиваем шлюз каждые 2 с.
  useEffect(() => {
    if (!polling(state)) return
    const t = setInterval(() => {
      void get<WaStatus>('/whatsapp')
        .then((s) => setLive(s.gate))
        .catch(() => undefined)
    }, 2000)
    return () => clearInterval(t)
  }, [state])

  if (st.error) return <ErrorBox error={st.error} />
  if (!st.data) return null
  const s = st.data
  const connect = (withPhone: boolean) =>
    void act.run(async () => {
      const g = await post<WaGateStatus>('/whatsapp/connect', withPhone ? { phone } : {})
      setLive(g)
    })
  return (
    <Card className="flex flex-col gap-3" data-testid="wa-connect">
      <CardTitle>WhatsApp по QR-коду</CardTitle>
      {!s.configured ? (
        <div className="text-sm text-slate-600">
          На сервере не запущен шлюз WhatsApp (переменные WA_GATE_URL и WA_GATE_KEY). После установки здесь появится кнопка «Подключить».
        </div>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2 text-sm" data-testid="wa-state">
            <Badge tone={state === 'connected' ? 'green' : state === 'logged_out' || state === 'unreachable' ? 'rose' : 'slate'}>
              {STATE_LABELS[state] ?? state}
            </Badge>
            {state === 'connected' && gate?.phone && <span>номер {gate.phone}</span>}
            {state === 'connected' && gate?.since && <span className="text-slate-500">с {formatDateTime(gate.since)}</span>}
            <span className="text-slate-500">
              сегодня отправлено {s.sent_today} из {s.daily_limit}
              {s.pending > 0 ? `, ждут ${s.pending}` : ''}
            </span>
          </div>
          {gate?.error && <div className="text-sm text-red-700">{gate.error}</div>}
          {state === 'qr' && gate?.qr && (
            <div className="flex flex-wrap items-start gap-4">
              <QrImage value={gate.qr} />
              <ol className="list-decimal pl-5 text-sm text-slate-700">
                <li>На телефоне магазина откройте WhatsApp или WhatsApp Business.</li>
                <li>«Настройки» → «Связанные устройства» → «Привязать устройство».</li>
                <li>Наведите камеру на этот QR-код. Код обновляется сам.</li>
              </ol>
            </div>
          )}
          {state === 'code' && gate?.code && (
            <div className="flex flex-col gap-1 text-sm">
              <div className="font-mono text-3xl tracking-widest" data-testid="wa-code">
                {codeText(gate.code)}
              </div>
              <div className="text-slate-700">
                На телефоне: «Связанные устройства» → «Привязать устройство» → «Связать по номеру телефона» — и введите этот код.
              </div>
            </div>
          )}
          {state !== 'connected' && (
            <div className="flex flex-col gap-2">
              {byCode ? (
                <div className="flex flex-wrap items-end gap-2">
                  <Field label="Номер WhatsApp магазина" error={phone ? pairPhoneProblem(phone) : null}>
                    <input id="wa-pair-phone" inputMode="tel" value={phone} onChange={(e) => setPhone(e.target.value)} placeholder="0555 12 34 56" />
                  </Field>
                  <Button disabled={pairPhoneProblem(phone) !== null || act.busy} onClick={() => connect(true)}>
                    Получить код
                  </Button>
                </div>
              ) : (
                <div>
                  <Button variant={state === 'qr' ? 'secondary' : undefined} disabled={act.busy} onClick={() => connect(false)}>
                    {state === 'qr' ? 'Новый QR-код' : 'Подключить'}
                  </Button>
                </div>
              )}
              <button type="button" className="self-start text-sm text-sky-700 hover:underline" onClick={() => setByCode((v) => !v)}>
                {byCode ? 'Подключить по QR-коду' : 'Нет камеры? Код вместо QR'}
              </button>
            </div>
          )}
          {state === 'connected' && (
            <div>
              <Button
                variant="secondary"
                disabled={act.busy}
                onClick={() => {
                  if (!window.confirm('Отключить WhatsApp? Сообщения клиентам перестанут уходить в WhatsApp, пока не подключите снова.')) return
                  void act.run(async () => {
                    setLive(await post<WaGateStatus>('/whatsapp/logout', {}))
                    toast('WhatsApp отключён')
                  })
                }}
              >
                Отключить
              </Button>
            </div>
          )}
          <ErrorBox error={act.error} />
          <div className="text-xs text-slate-500">
            Подключённый номер пишет клиентам те же сообщения, что бот в Телеграме (покупка, долг, замена масла), — сразу после чека, с 09:00 до 20:00, с
            паузой между сообщениями. На входящие отвечает меню цифрами. Номер лучше отдельный: при частых рассылках WhatsApp может его ограничить.
          </div>
        </>
      )}
    </Card>
  )
}

export function WhatsAppInbox() {
  const st = useLoad(() => get<WaInboxRow[]>('/whatsapp/inbox?limit=100'), [])
  if (st.error) return <ErrorBox error={st.error} />
  if (!st.data) return null
  return (
    <Card className="flex flex-col gap-2" data-testid="wa-inbox">
      <CardTitle>Переписка в WhatsApp</CardTitle>
      {st.data.length === 0 ? (
        <Empty>Клиенты ещё не писали.</Empty>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {st.data.map((r, i) => (
            <li key={`${r.created_at}-${i}`} className="border-b border-slate-100 pb-1 last:border-0">
              <span className="text-xs text-slate-500">{formatDateTime(r.created_at)} · </span>
              <span className="font-medium">{DIRECTION_LABELS[r.direction]}</span>
              <span className="text-slate-500"> · {r.party_name ?? r.phone}</span>
              <div className="whitespace-pre-wrap break-words">{r.text}</div>
            </li>
          ))}
        </ul>
      )}
    </Card>
  )
}
