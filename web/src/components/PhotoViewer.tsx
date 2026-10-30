// Фото накладной на весь экран (задача 002, П6): снимок целиком по размеру экрана, листание
// стрелками и кнопками, Esc или ✕ — закрыть. PDF открывается отдельной вкладкой, как раньше.
import { useEffect } from 'react'

export interface ViewerPhoto {
  url: string
  name: string
}

export function PhotoViewer({
  photos,
  index,
  onIndex,
  onClose,
}: {
  photos: ViewerPhoto[]
  index: number
  onIndex: (i: number) => void
  onClose: () => void
}) {
  const n = photos.length
  const at = Math.min(Math.max(index, 0), Math.max(n - 1, 0))
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      if (e.key === 'ArrowRight' && n > 1) onIndex((at + 1) % n)
      if (e.key === 'ArrowLeft' && n > 1) onIndex((at - 1 + n) % n)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [at, n, onClose, onIndex])
  const p = photos[at]
  if (!p) return null
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Фото накладной"
      data-testid="photo-viewer"
      className="no-print fixed inset-0 z-50 flex flex-col bg-black/95"
      onMouseDown={onClose}
    >
      <div className="flex items-center justify-between gap-3 px-4 py-2 text-sm text-white" onMouseDown={(e) => e.stopPropagation()}>
        <span className="truncate">
          {p.name || 'фото накладной'}
          {n > 1 && ` · ${at + 1} из ${n}`}
        </span>
        <div className="flex items-center gap-3">
          <a className="underline" href={p.url} target="_blank" rel="noreferrer">
            открыть отдельно
          </a>
          <button type="button" aria-label="Закрыть" className="rounded px-2 py-1 text-xl leading-none hover:bg-white/10" onClick={onClose}>
            ✕
          </button>
        </div>
      </div>
      <div className="relative flex min-h-0 flex-1 items-center justify-center p-2">
        <img src={p.url} alt={p.name || 'фото накладной'} className="max-h-full max-w-full object-contain" onMouseDown={(e) => e.stopPropagation()} />
        {n > 1 && (
          <>
            <button
              type="button"
              aria-label="Предыдущее фото"
              className="absolute left-2 top-1/2 -translate-y-1/2 rounded-full bg-white/20 px-3 py-2 text-2xl text-white hover:bg-white/30"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => onIndex((at - 1 + n) % n)}
            >
              ‹
            </button>
            <button
              type="button"
              aria-label="Следующее фото"
              className="absolute right-2 top-1/2 -translate-y-1/2 rounded-full bg-white/20 px-3 py-2 text-2xl text-white hover:bg-white/30"
              onMouseDown={(e) => e.stopPropagation()}
              onClick={() => onIndex((at + 1) % n)}
            >
              ›
            </button>
          </>
        )}
      </div>
    </div>
  )
}
