// Ссылка на бота с кодом привязки и её QR (ADR-121): владелец и администратор (ADR-135).
import QRCode from 'qrcode'
import { useEffect, useState } from 'react'

/** Ссылка, открывающая бота сразу с кодом привязки (SPEC-36 §7). */
export function startLink(username: string, code: string): string {
  return `https://t.me/${encodeURIComponent(username)}?start=${encodeURIComponent(code)}`
}

/** Кнопка «Открыть бота» и QR той же ссылки — навести телефон владельца. */
export function BotStartLink({ link }: { link: string }) {
  const [qr, setQr] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    QRCode.toDataURL(link, { margin: 1, width: 192, errorCorrectionLevel: 'M' })
      .then((url) => alive && setQr(url))
      .catch(() => alive && setQr(null))
    return () => {
      alive = false
    }
  }, [link])
  return (
    <div className="flex flex-wrap items-center gap-4">
      {qr && <img src={qr} alt="QR-код ссылки на бота" data-testid="bot-qr" className="h-48 w-48 rounded border border-slate-200" />}
      <div className="flex flex-col gap-2">
        <a
          href={link}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex min-h-[42px] items-center justify-center rounded-md bg-sky-600 px-4 py-2 text-sm font-medium text-white hover:bg-sky-700"
        >
          Открыть бота
        </a>
        <div className="text-slate-600">На телефоне — наведите камеру на QR, бот откроется с кодом: нажмите «Запустить».</div>
      </div>
    </div>
  )
}
