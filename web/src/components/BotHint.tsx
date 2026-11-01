// Подсказка администратору без привязки (SPEC-40 §10, ADR-{h18}): «Подключите бот» на главной.
// Показывается один раз на устройстве (отметка — при показе), закрывается крестиком; бот выключен,
// чат уже привязан или привязку делает владелец — не показывается.
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { get } from '../lib/api'
import { hintKey, type MyTelegram, showBotHint } from '../lib/myTelegram'

function seenBefore(key: string): boolean {
  try {
    return localStorage.getItem(key) === '1'
  } catch {
    return true
  }
}

export function BotHint({ userId }: { userId: string }) {
  const key = hintKey(userId)
  const [show, setShow] = useState(false)
  useEffect(() => {
    if (seenBefore(key)) return
    let alive = true
    get<MyTelegram>('/me/telegram')
      .then((t) => {
        if (!alive || !showBotHint(t, seenBefore(key))) return
        setShow(true)
        try {
          localStorage.setItem(key, '1')
        } catch {
          // Без хранилища подсказка покажется и в следующий раз.
        }
      })
      .catch(() => undefined)
    return () => {
      alive = false
    }
  }, [key])
  if (!show) return null
  return (
    <div className="no-print mx-4 mt-3 flex items-start gap-3 rounded-md border border-sky-200 bg-sky-50 p-3 text-sm text-sky-900" data-testid="bot-hint">
      <div className="flex-1">
        Подключите бот — накладные фото, должники, смена с телефона.{' '}
        <Link to="/my-telegram" className="font-medium text-sky-700 underline" onClick={() => setShow(false)}>
          Мой Телеграм
        </Link>
      </div>
      <button type="button" aria-label="Закрыть подсказку" className="text-sky-700 hover:text-sky-900" onClick={() => setShow(false)}>
        ✕
      </button>
    </div>
  )
}
