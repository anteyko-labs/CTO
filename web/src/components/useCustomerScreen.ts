// Касса → экран покупателя (задача 008, ADR-179): что показать и подпись клиента за долг.
import { useCallback, useEffect, useRef, useState } from 'react'
import { randomId } from '../lib/api'
import { openChannel, SCREEN_GONE_MS, type FromScreen, type ToScreen } from '../lib/customerScreen'

export function useCustomerScreen() {
  const [channel] = useState(openChannel)
  const [seen, setSeen] = useState(0)
  const [now, setNow] = useState(() => Date.now())
  const [signed, setSigned] = useState<{ id: string; png: string } | null>(null)
  const last = useRef('')
  const signing = useRef<string | null>(null)

  useEffect(() => {
    if (!channel) return
    channel.onmessage = (e: MessageEvent<FromScreen>) => {
      const m = e.data
      if (m.type === 'here') {
        setSeen(Date.now())
        // Экран открыли или перезагрузили — повторить, что на нём должно быть.
        if (last.current) channel.postMessage(JSON.parse(last.current) as ToScreen)
      } else if (m.type === 'signed' && m.id === signing.current) {
        signing.current = null
        setSigned({ id: m.id, png: m.png })
      }
    }
    const t = setInterval(() => setNow(Date.now()), 2000)
    return () => {
      clearInterval(t)
      channel.close()
    }
  }, [channel])

  /** Показать на экране; то же самое повторно не шлётся. */
  const show = useCallback(
    (m: ToScreen) => {
      const text = JSON.stringify(m)
      if (!channel || text === last.current) return
      last.current = text
      channel.postMessage(m)
    },
    [channel],
  )

  /** Попросить подпись: номер просьбы; ответ — в `signed`. */
  const askSign = useCallback(
    (title: string, lines: string[]) => {
      const id = randomId()
      signing.current = id
      setSigned(null)
      show({ type: 'sign', id, title, lines })
      return id
    },
    [show],
  )

  const cancelSign = useCallback(() => {
    const id = signing.current
    signing.current = null
    if (id) show({ type: 'sign-cancel', id })
  }, [show])

  return { present: Boolean(channel) && now - seen < SCREEN_GONE_MS, show, askSign, cancelSign, signed }
}
