// Телеграм-бот владельца: привязка чата одноразовым кодом (SPEC-18, ADR-050).
import { useState } from 'react'
import { Button, Card, ErrorBox, Loading, PageHeader, toast } from '../components/ui'
import { del, get, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'

interface Status {
  enabled: boolean
  linked: boolean
  linked_at: string | null
}

export default function TelegramSettings() {
  const st = useLoad(() => get<Status>('/settings/telegram'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  const act = useAction()
  const s = st.data

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Телеграм" />
      <ErrorBox error={st.error ?? act.error} />
      {!s ? (
        <Loading />
      ) : !s.enabled ? (
        <Card className="text-sm text-slate-700">
          Бот выключен: на сервере не задан ключ бота. Создайте бота у @BotFather в Телеграме, ключ впишите в настройку сервера
          <code className="mx-1 rounded bg-slate-100 px-1">TELEGRAM_BOT_TOKEN</code>и перезапустите сервер.
        </Card>
      ) : s.linked ? (
        <Card className="flex flex-wrap items-center justify-between gap-3 text-sm">
          <div>
            <div className="font-medium">Телеграм привязан</div>
            <div className="text-slate-600">
              С {s.linked_at ? formatDateTime(s.linked_at) : '—'} сюда приходят важные события: закрытие смены и сдача кассы, недостача, возвраты,
              сторно, смена цен, долги. Команды бота: /сегодня, /смена, /долги.
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
          <div className="text-slate-700">
            Бот работает только для владельца. Получите код и отправьте его боту — после этого уведомления будут приходить в этот чат.
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
    </div>
  )
}
