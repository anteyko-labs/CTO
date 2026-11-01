// Телеграм администратора (ADR-135, задача 003 Н6, SPEC-40): код на 15 минут, ссылка и QR;
// «Отвязать»; «Что видит в боте» по его правам и события, которые ему приходят. Открывается из
// «Пользователей» и из «Телеграм и баллы» → «Накладные в бот присылают». Только владелец.
import { useState } from 'react'
import { AdminBotPanel } from './AdminBotPanel'
import { BotStartLink, startLink } from './BotStartLink'
import { Button, ErrorBox, Modal, toast } from './ui'
import { del, get, post } from '../lib/api'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { UserRow } from '../lib/types'

/** У администратора нет права «Приход товара» — фото накладных бот от него не примет. */
export function noReceiptsRight(u: Pick<UserRow, 'role' | 'permissions'>): boolean {
  return u.role === 'admin' && !(u.permissions ?? []).includes('receipts')
}

export function AdminTelegramModal({ user, onClose, onChanged }: { user: UserRow; onClose: () => void; onChanged: () => void }) {
  const act = useAction()
  const bot = useLoad(() => get<{ username: string | null }>('/telegram/bot'), [])
  const [code, setCode] = useState<{ code: string; expires_at: string } | null>(null)
  return (
    <Modal title={`Телеграм: ${user.full_name}`} onClose={onClose} wide>
      <div className="flex flex-col gap-3 text-sm" data-testid="user-telegram-modal">
        <div className="text-slate-600">
          В чате администратора — кнопки по его правам: с правом «Приход товара» он присылает фото накладных («📷 Накладная», «📄 Мои накладные»), смотрит
          смену, остаток, должников, книжку машины, проводит расход из кассы смены. Прибыли и себестоимости бот ему не показывает. Если включить «Помощник в
          боте», он спрашивает своими словами — в тех же пределах.
        </div>
        {noReceiptsRight(user) && (
          <div className="rounded bg-amber-50 p-2 text-amber-800">Нет права «Приход товара» — фото не примутся. Выдайте его в «Что можно».</div>
        )}
        <div data-testid="user-telegram-state">
          {user.telegram_linked_at
            ? `Привязан ${formatDateTime(user.telegram_linked_at)}`
            : 'Телеграм не привязан — дайте код ниже или администратор возьмёт его сам в «Мой Телеграм»'}
        </div>
        {code ? (
          <div className="flex flex-col gap-2">
            <div className="text-3xl font-bold tracking-widest" data-testid="user-telegram-code">
              {code.code}
            </div>
            <div className="text-slate-600">
              Администратор открывает бота и отправляет: <code className="rounded bg-slate-100 px-1">/start {code.code}</code> — код действует до{' '}
              {formatDateTime(code.expires_at)}.
            </div>
            {bot.data?.username && <BotStartLink link={startLink(bot.data.username, code.code)} />}
          </div>
        ) : (
          <Button
            disabled={act.busy}
            data-testid="user-telegram-code-btn"
            onClick={() => void act.run(async () => setCode(await post<{ code: string; expires_at: string }>(`/users/${user.id}/telegram/code`, {})))}
          >
            {user.telegram_linked_at ? 'Привязать заново: код' : 'Код привязки'}
          </Button>
        )}
        <ErrorBox error={act.error} />
        <AdminBotPanel userId={user.id} />
        <div className="flex justify-between gap-2">
          {user.telegram_linked_at ? (
            <Button
              variant="danger"
              disabled={act.busy}
              onClick={() =>
                void act.run(async () => {
                  await del(`/users/${user.id}/telegram`)
                  toast('Телеграм отвязан')
                  onChanged()
                })
              }
            >
              Отвязать
            </Button>
          ) : (
            <span />
          )}
          <Button variant="secondary" onClick={onClose}>
            Закрыть
          </Button>
        </div>
      </div>
    </Modal>
  )
}
