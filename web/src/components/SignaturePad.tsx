// Подпись пальцем (SPEC-39 §3): белое поле, чёрная линия, «Очистить», «Готово». Без библиотек:
// `canvas` → PNG (ADR-134). Пустую подпись экран не принимает.
import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { Button } from './ui'

/** Поле подписи в точках; PNG — в этом размере (до 200 КБ с запасом). */
const WIDTH = 720
const HEIGHT = 300

export function SignaturePad({ busy, onDone, doneLabel = 'Готово' }: { busy?: boolean; onDone: (png: string) => void; doneLabel?: string }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const last = useRef<{ x: number; y: number } | null>(null)
  // Длина линии в точках: точка или короткий штрих — не подпись.
  const [ink, setInk] = useState(0)

  const ctx = () => {
    const c = canvas.current?.getContext('2d') ?? null
    if (c) {
      c.lineWidth = 3
      c.lineCap = 'round'
      c.lineJoin = 'round'
      c.strokeStyle = '#000'
    }
    return c
  }
  // Белый фон — в PNG, иначе подпись на прозрачном фоне не видна в тёмной теме.
  const paintWhite = () => {
    const c = ctx()
    if (c) {
      c.fillStyle = '#fff'
      c.fillRect(0, 0, WIDTH, HEIGHT)
    }
  }
  const clear = () => {
    paintWhite()
    setInk(0)
  }
  useEffect(paintWhite, [])

  const point = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect()
    return { x: ((e.clientX - r.left) * WIDTH) / r.width, y: ((e.clientY - r.top) * HEIGHT) / r.height }
  }
  const down = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    last.current = point(e)
  }
  const move = (e: ReactPointerEvent<HTMLCanvasElement>) => {
    const from = last.current
    if (!from) return
    const to = point(e)
    const c = ctx()
    if (!c) return
    c.beginPath()
    c.moveTo(from.x, from.y)
    c.lineTo(to.x, to.y)
    c.stroke()
    last.current = to
    setInk((n) => n + Math.hypot(to.x - from.x, to.y - from.y))
  }
  const up = () => {
    last.current = null
  }
  const signed = ink >= 40

  return (
    <div className="flex flex-col gap-3" data-testid="signature-pad">
      <canvas
        ref={canvas}
        width={WIDTH}
        height={HEIGHT}
        aria-label="Поле подписи"
        className="w-full touch-none rounded-lg border-2 border-slate-400 bg-white"
        style={{ aspectRatio: `${WIDTH} / ${HEIGHT}` }}
        onPointerDown={down}
        onPointerMove={move}
        onPointerUp={up}
        onPointerCancel={up}
        onPointerLeave={up}
      />
      {!signed && <div className="text-sm text-slate-500">Распишитесь пальцем в белом поле</div>}
      <div className="flex justify-between gap-3">
        <Button variant="secondary" className="px-6 text-lg" onClick={clear} disabled={busy}>
          Очистить
        </Button>
        <Button
          className="px-8 text-lg"
          data-testid="signature-done"
          disabled={busy || !signed}
          onClick={() => {
            const url = canvas.current?.toDataURL('image/png')
            if (url) onDone(url)
          }}
        >
          {doneLabel}
        </Button>
      </div>
    </div>
  )
}
