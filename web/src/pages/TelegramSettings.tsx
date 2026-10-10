// Телеграм-бот владельца: привязка чата одноразовым кодом (SPEC-18, ADR-050).
import { useState } from 'react'
import { Button, Card, CardTitle, ErrorBox, Field, Loading, PageHeader, toast } from '../components/ui'
import { del, get, post, put } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

interface Status {
  enabled: boolean
  linked: boolean
  linked_at: string | null
}

/** Процент баллов с покупки: «0,5» ↔ 50 сотых процента (SPEC-19). */
function LoyaltyRate() {
  const st = useLoad(() => get<{ rate_bp: number }>('/settings/loyalty'), [])
  const [text, setText] = useState<string | null>(null)
  const act = useAction()
  const m = text === null ? null : /^(\d{1,2})(?:[.,](\d{1,2}))?$/.exec(text.trim())
  const bp = m ? Number(m[1]) * 100 + Number((m[2] ?? '').padEnd(2, '0')) : null
  const shown = (v: number) => String(v / 100).replace('.', ',')
  if (!st.data) return <ErrorBox error={st.error} />
  return (
    <Card className="flex flex-wrap items-end gap-3 text-sm">
      <div className="min-w-0 flex-1 text-slate-700">
        <CardTitle>Баллы клиентам</CardTitle>
        Клиенты, подключившиеся к боту по номеру, получают {shown(st.data.rate_bp)} % от оплаченного деньгами баллами (1 балл = 1 сом). Списать
        баллы — на кассе «Услуги → Скидка баллами». Списанные баллы уменьшают чистую прибыль.
      </div>
      {text === null ? (
        <Button variant="secondary" onClick={() => setText(shown(st.data!.rate_bp))}>
          Изменить процент
        </Button>
      ) : (
        <>
          <Field label="Процент, %" hint="От 0 до 10">
            <input autoFocus inputMode="decimal" className="w-24" value={text} onChange={(e) => setText(e.target.value)} />
          </Field>
          <Button variant="secondary" onClick={() => setText(null)}>
            Отмена
          </Button>
          <Button
            disabled={act.busy || bp === null || bp > 1000}
            onClick={() =>
              void act.run(async () => {
                await put('/settings/loyalty', { rate_bp: bp })
                setText(null)
                st.reload()
                toast('Процент сохранён')
              })
            }
          >
            Сохранить
          </Button>
        </>
      )}
      <ErrorBox error={act.error} />
    </Card>
  )
}

export default function TelegramSettings() {
  const st = useLoad(() => get<Status>('/settings/telegram'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  const act = useAction()
  const s = st.data

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Телеграм и баллы" />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : !s.enabled ? (
        <Card className="text-sm text-slate-700">
          <CardTitle>Бот владельца</CardTitle>
          Бот выключен: на сервере не задан ключ бота. Создайте бота у @BotFather в Телеграме, ключ впишите в настройку сервера
          <code className="mx-1 rounded bg-slate-100 px-1">TELEGRAM_BOT_TOKEN</code>и перезапустите сервер.
        </Card>
      ) : s.linked ? (
        <Card className="flex flex-wrap items-center justify-between gap-3 text-sm">
          <div>
            <CardTitle>Телеграм привязан</CardTitle>
            <div className="text-slate-600">
              С {s.linked_at ? formatDateTime(s.linked_at) : '—'} сюда приходят важные события: закрытие смены и сдача кассы, недостача, возвраты,
              сторно, смена цен, долги. Кнопки внизу чата: «Сегодня», «Смена», «Долги».
            </div>
          </div>
          <Button
            variant="secondary"
            disabled={act.busy}
            onClick={() =>
              void act.run(async () => {
                await del('/settings/telegram')
                st.reload()
                toast('Телеграм отвязан')
              })
            }
          >
            Отвязать
          </Button>
        </Card>
      ) : (
        <Card className="flex flex-col gap-3 text-sm">
          <CardTitle>Привязать Телеграм</CardTitle>
          <div className="-mt-3 text-slate-700">
            Команды и уведомления — только для владельца; клиенты, поделившись номером, видят в боте свою масляную книжку. Получите код и отправьте его боту — после этого уведомления будут приходить в этот чат.
          </div>
          {code ? (
            <div className="flex flex-col gap-2">
              <div className="text-3xl font-bold tracking-widest">{code.code}</div>
              <div className="text-slate-600">
                Откройте бота и отправьте: <code className="rounded bg-slate-100 px-1">/start {code.code}</code> — код действует до{' '}
                {formatDateTime(code.expires_at)}.
              </div>
              <div>
                <Button variant="secondary" onClick={() => st.reload()}>
                  Я отправил — проверить
                </Button>
              </div>
            </div>
          ) : (
            <div>
              <Button
                disabled={act.busy}
                onClick={() =>
                  void act.run(async () => {
                    setCode(await post<{ code: string; expires_at: string }>('/settings/telegram/code', {}))
                  })
                }
              >
                Получить код
              </Button>
            </div>
          )}
        </Card>
      )}
      {s?.enabled && <LoyaltyRate />}
    </div>
  )
}
