// Скан штрихкода камерой телефона (задача 007): встроенный в браузер BarcodeDetector (Chrome на
// Android), без библиотек. Нет поддержки — сообщение, сканер и поиск работают как раньше.
import { useEffect, useRef, useState } from 'react'
import { Button, Modal } from './ui'

interface Detector {
  detect(source: HTMLVideoElement): Promise<{ rawValue: string }[]>
}
type DetectorCtor = new (opts?: { formats?: string[] }) => Detector

const FORMATS = ['ean_13', 'ean_8', 'upc_a', 'upc_e', 'code_128', 'code_39', 'code_93', 'itf', 'codabar', 'qr_code']

/** Камера умеет читать коды в этом браузере. */
const cameraScanSupported = (): boolean =>
  typeof window !== 'undefined' && 'BarcodeDetector' in window && Boolean(navigator.mediaDevices?.getUserMedia)

export function CameraScanButton({ onCode, label = 'Сканировать камерой' }: { onCode: (code: string) => void; label?: string }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <Button variant="secondary" type="button" aria-label={label} title={label} data-testid="camera-scan" onClick={() => setOpen(true)}>
        📷
      </Button>
      {open && (
        <CameraScanModal
          onClose={() => setOpen(false)}
          onCode={(c) => {
            setOpen(false)
            onCode(c)
          }}
        />
      )}
    </>
  )
}

function CameraScanModal({ onClose, onCode }: { onClose: () => void; onCode: (code: string) => void }) {
  const video = useRef<HTMLVideoElement>(null)
  const [error, setError] = useState<string | null>(cameraScanSupported() ? null : 'Этот браузер не читает коды камерой — откройте программу в Chrome на Android или сканируйте сканером.')
  const done = useRef(onCode)
  useEffect(() => {
    done.current = onCode
  })

  useEffect(() => {
    if (!cameraScanSupported()) return
    let stream: MediaStream | null = null
    let alive = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const Ctor = (window as unknown as { BarcodeDetector: DetectorCtor }).BarcodeDetector
    const detector = new Ctor({ formats: FORMATS })
    const tick = async () => {
      const v = video.current
      if (!alive || !v) return
      try {
        if (v.readyState >= 2) {
          const found = await detector.detect(v)
          const code = found.find((f) => f.rawValue.trim())?.rawValue.trim()
          if (code && alive) {
            alive = false
            navigator.vibrate?.(80)
            done.current(code)
            return
          }
        }
      } catch {
        // кадр не разобрался — следующий
      }
      timer = setTimeout(() => void tick(), 200)
    }
    navigator.mediaDevices
      .getUserMedia({ video: { facingMode: { ideal: 'environment' } }, audio: false })
      .then((s) => {
        if (!alive) return s.getTracks().forEach((t) => t.stop())
        stream = s
        if (video.current) {
          video.current.srcObject = s
          void video.current.play()
        }
        void tick()
      })
      .catch(() => setError('Нет доступа к камере — разрешите камеру для сайта в настройках браузера.'))
    return () => {
      alive = false
      if (timer) clearTimeout(timer)
      stream?.getTracks().forEach((t) => t.stop())
    }
  }, [])

  return (
    <Modal title="Наведите камеру на штрихкод" onClose={onClose}>
      <div className="flex flex-col gap-3">
        {error ? (
          <p className="text-sm text-rose-700">{error}</p>
        ) : (
          <video ref={video} className="w-full rounded bg-black" playsInline muted />
        )}
        <div className="flex justify-end">
          <Button variant="secondary" onClick={onClose}>
            Закрыть
          </Button>
        </div>
      </div>
    </Modal>
  )
}
