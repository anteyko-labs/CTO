// Касса → экран покупателя (задача 008, ADR-179): что показать и подпись клиента за долг.
import { useCallback, useEffect, useRef, useState } from 'react'
import { randomId } from '../lib/api'
import { openChannel, SCREEN_GONE_MS, type FromScreen, type ToScreen } from '../lib/customerScreen'

export function useCustomerScreen() {
  const [channel] = useState(openChannel)
  const [seen, setSeen] = useState(0)
  const [now, setNow] = useState(() => Date.now())
  const [signed, setSigned] = useState<{ id: string; png: string } | null>(null)
  // Что на экране без подписи (QR, чек кассы, заставка) и открытая просьба подписи.
  const base = useRef<ToScreen | null>(null)
  const signing = useRef<Extract<ToScreen, { type: 'sign' }> | null>(null)

  useEffect(() => {
    if (!channel) return
    channel.onmessage = (e: MessageEvent<FromScreen>) => {
      const m = e.data
      if (m.type === 'here') {
        setSeen(Date.now())
        // Экран открыли или перезагрузили — повторить, что на нём должно быть.
        if (base.current) channel.postMessage(base.current)
        if (signing.current) channel.postMessage(signing.current)
      } else if (m.type === 'signed' && m.id === signing.current?.id) {
        signing.current = null
        setSigned({ id: m.id, png: m.png })
        if (base.current) channel.postMessage(base.current)
      }
    }
    const t = setInterval(() => setNow(Date.now()), 2000)
    return () => {
      clearInterval(t)
      channel.close()
    }
  }, [channel])

  /** Показать на экране; то же самое повторно не шлётся. Пока ждём подпись — только запомнить. */
  const show = useCallback(
    (m: ToScreen) => {
      if (!channel || JSON.stringify(m) === JSON.stringify(base.current)) return
      base.current = m
      if (!signing.current) channel.postMessage(m)
    },
    [channel],
  )

  /** Попросить подпись: номер просьбы; ответ — в `signed`. */
  const askSign = useCallback(
    (title: string, lines: string[]) => {
      const id = randomId()
      signing.current = { type: 'sign', id, title, lines }
      setSigned(null)
      channel?.postMessage(signing.current)
      return id
    },
    [channel],
  )

  /** Подпись больше не нужна — экран возвращается к чеку кассы. */
  const cancelSign = useCallback(() => {
    const s = signing.current
    signing.current = null
    if (!channel || !s) return
    channel.postMessage({ type: 'sign-cancel', id: s.id } satisfies ToScreen)
    if (base.current) channel.postMessage(base.current)
  }, [channel])

  return { present: Boolean(channel) && now - seen < SCREEN_GONE_MS, show, askSign, cancelSign, signed }
}
