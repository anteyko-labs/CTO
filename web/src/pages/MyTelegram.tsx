// «Мой Телеграм» администратора (SPEC-40 §10, ADR-{h18}): сам привязывает свой чат с ботом —
// тот же код на 15 минут, ссылка и QR, что даёт владелец (ADR-135); видит, привязан ли и к какому
// Телеграм-имени, и что увидит в боте по своим правам. Владелец может выключить самостоятельную
// привязку — тогда «Привязку делает владелец».
import { useState } from 'react'
import { BotStartLink, startLink } from '../components/BotStartLink'
import { Button, Card, CardTitle, ErrorBox, Loading, PageHeader, toast } from '../components/ui'
import { del, get, newOpId, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { linkAction, linkedText, type MyTelegram as Me } from '../lib/myTelegram'

export default function MyTelegram() {
  const st = useLoad(() => get<Me>('/me/telegram'), [])
  const bot = useLoad(() => get<{ username: string | null }>('/telegram/bot'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  const act = useAction()
  const me = st.data
  if (!me) return st.error ? <ErrorBox error={st.error} /> : <Loading />
  const action = linkAction(me)

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Мой Телеграм" />
      <ErrorBox error={act.error} />
      <Card className="flex flex-col gap-3 text-sm" data-testid="my-telegram">
        <CardTitle>Бот в Телеграме</CardTitle>
        <div className="-mt-3 text-slate-700">
          В боте — накладные фото, должники, смена, остаток и расход с телефона: кнопки по вашим правам. Прибыли и себестоимости в боте
          нет.
        </div>
        <div className="font-medium" data-testid="my-telegram-state">
          {linkedText(me, me.linked_at ? formatDateTime(me.linked_at) : null)}
        </div>
        {action === 'off' ? (
          <div className="text-slate-600">Бот выключен на сервере — спросите владельца.</div>
        ) : action === 'owner' ? (
          <div className="rounded bg-amber-50 p-2 text-amber-800" data-testid="my-telegram-owner">
            Привязку делает владелец: попросите у него код в «Пользователи → Телеграм».
          </div>
        ) : code ? (
          <div className="flex flex-col gap-2">
            <div className="text-3xl font-bold tracking-widest" data-testid="my-telegram-code">
              {code.code}
            </div>
            <div className="text-slate-600">
              Откройте бота и отправьте: <code className="rounded bg-slate-100 px-1">/start {code.code}</code> — код действует до{' '}
              {formatDateTime(code.expires_at)}.
            </div>
            {bot.data?.username && <BotStartLink link={startLink(bot.data.username, code.code)} />}
            <div>
              <Button
                variant="secondary"
                onClick={() => {
                  setCode(null)
                  st.reload()
                }}
              >
                Я отправил — проверить
              </Button>
            </div>
          </div>
        ) : (
          <div>
            <Button
              disabled={act.busy}
              data-testid="my-telegram-code-btn"
              onClick={() =>
                void act.run(async () => setCode(await post<{ code: string; expires_at: string }>('/me/telegram/code', { op_id: newOpId() })))
              }
            >
              {me.linked ? 'Привязать заново' : 'Привязать Телеграм'}
            </Button>
          </div>
        )}
        {me.linked && (
          <div>
            <Button
              variant="danger"
              disabled={act.busy}
              data-testid="my-telegram-unlink"
              onClick={() =>
                void act.run(async () => {
                  await del(`/me/telegram?op_id=${newOpId()}`)
                  setCode(null)
                  st.reload()
                  toast('Телеграм отвязан')
                })
              }
            >
              Отвязать
            </Button>
          </div>
        )}
      </Card>
      <Card className="flex flex-col gap-2 text-sm">
        <CardTitle>Что вы увидите в боте</CardTitle>
        <div className="-mt-3 text-slate-500">По вашим правам («Что можно»); меняются права — меняется меню.</div>
        <ul className="flex flex-col gap-1" data-testid="my-telegram-items">
          {me.items.map((i) => (
            <li key={i.key} className={i.on ? '' : 'text-slate-400'} data-testid={`my-telegram-item-${i.key}`} data-on={i.on ? 'yes' : 'no'}>
              <span className="font-medium">
                {i.on ? '✓' : '—'} {i.button}
              </span>{' '}
              — {i.what}
              {!i.on && i.need && <span className="ml-1 text-amber-700">({i.need})</span>}
            </li>
          ))}
        </ul>
        <div className="text-slate-600" data-testid="my-telegram-events">
          {me.events.length > 0
            ? `Сообщения в бот: ${me.events.join(', ')}.`
            : 'Сообщений о событиях вам не приходит — их включает владелец.'}
        </div>
      </Card>
    </div>
  )
}
