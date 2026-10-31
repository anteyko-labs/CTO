// Телеграм администратора (ADR-135, задача 003 Н6): код на 15 минут, ссылка и QR; «Отвязать».
// Его чат принимает фото накладных; кнопки «📷 Накладная» и «📄 Мои накладные». Открывается из
// «Пользователей» и из «Телеграм и баллы» → «Накладные в бот присылают». Только владелец.
import { useState } from 'react'
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
    <Modal title={`Телеграм: ${user.full_name}`} onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm" data-testid="user-telegram-modal">
        <div className="text-slate-600">
          Администратор с правом «Приход товара» присылает в бот фото накладных — они становятся черновиками прихода. В его чате кнопки «📷 Накладная» (как
          снять) и «📄 Мои накладные».
        </div>
        {noReceiptsRight(user) && (
          <div className="rounded bg-amber-50 p-2 text-amber-800">Нет права «Приход товара» — фото не примутся. Выдайте его в «Что можно».</div>
        )}
        {user.telegram_linked_at && <div>Привязан {formatDateTime(user.telegram_linked_at)}</div>}
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
