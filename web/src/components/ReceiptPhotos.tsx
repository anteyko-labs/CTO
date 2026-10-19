// Фото накладной (SPEC-29 §3, ADR-080): с телефона камерой или файлом; снимок сжимается перед
// загрузкой. В приходе — до проведения копятся на экране, после — уходят на сервер; в накладной —
// просмотр, догрузка и скрытие владельцем с причиной.
import { useRef, useState, type ChangeEvent } from 'react'
import { get, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { preparePhotos, uploadPhotos, type PendingPhoto } from '../lib/photoUpload'
import type { ReceiptFile } from '../lib/types'
import { Badge, Button, ErrorBox, Field, Modal, toast } from './ui'

/** Кнопки «Сфотографировать» и «Файл»: камера телефона или выбор файла. */
export function PhotoButtons({ onPicked, busy }: { onPicked: (files: FileList) => void; busy?: boolean }) {
  const camera = useRef<HTMLInputElement>(null)
  const file = useRef<HTMLInputElement>(null)
  const pick = (e: ChangeEvent<HTMLInputElement>) => {
    if (e.target.files?.length) onPicked(e.target.files)
    e.target.value = ''
  }
  return (
    <div className="flex flex-wrap gap-2">
      <Button variant="secondary" disabled={busy} onClick={() => camera.current?.click()}>
        Сфотографировать
      </Button>
      <Button variant="secondary" disabled={busy} onClick={() => file.current?.click()}>
        Файл
      </Button>
      <input ref={camera} type="file" accept="image/*" capture="environment" className="hidden" onChange={pick} />
      <input
        ref={file}
        data-testid="receipt-photo-input"
        type="file"
        accept="image/jpeg,image/png,image/webp,application/pdf"
        multiple
        className="hidden"
        onChange={pick}
      />
    </div>
  )
}

/** Фото, ждущие проведения прихода: миниатюры и «убрать». */
export function PendingPhotos({ photos, onRemove }: { photos: PendingPhoto[]; onRemove: (key: string) => void }) {
  if (photos.length === 0) return null
  return (
    <ul className="flex flex-wrap gap-2" data-testid="pending-photos">
      {photos.map((p) => (
        <li key={p.key} className="relative">
          {p.type === 'application/pdf' ? (
            <div className="flex h-20 w-20 items-center justify-center rounded border border-slate-300 bg-slate-50 text-xs">PDF</div>
          ) : (
            <img src={p.url} alt={p.name} className="h-20 w-20 rounded border border-slate-300 object-cover" />
          )}
          <button
            type="button"
            className="absolute -right-2 -top-2 rounded-full bg-white px-1.5 text-xs text-slate-500 shadow hover:text-rose-600"
            aria-label={`Убрать ${p.name}`}
            onClick={() => onRemove(p.key)}
          >
            ✕
          </button>
        </li>
      ))}
    </ul>
  )
}

/** Фото проведённой накладной: просмотр, догрузка, скрытие владельцем. */
export function ReceiptPhotos({ receiptId, canAdd }: { receiptId: string; canAdd: boolean }) {
  const owner = useUser().role === 'owner'
  const files = useLoad(() => get<ReceiptFile[]>(`/receipts/${receiptId}/files`), [receiptId])
  const act = useAction()
  const [hiding, setHiding] = useState<{ file: ReceiptFile; reason: string } | null>(null)
  const list = files.data ?? []
  const shown = list.filter((f) => !f.hidden_at)
  const hidden = list.filter((f) => f.hidden_at)

  const add = (picked: FileList) =>
    void act.run(async () => {
      const ready = await preparePhotos(picked)
      const failed = await uploadPhotos(receiptId, ready)
      ready.forEach((p) => URL.revokeObjectURL(p.url))
      files.reload()
      if (failed.length) throw new Error(`Не загрузились: ${failed.join('; ')}`)
      toast(ready.length === 1 ? 'Фото прикреплено' : `Прикреплено фото: ${ready.length}`)
    })

  return (
    <div className="flex flex-col gap-3" data-testid="receipt-photos">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-semibold">Фото накладной</h2>
        {canAdd && <PhotoButtons busy={act.busy} onPicked={add} />}
      </div>
      <ErrorBox error={files.error ?? act.error} />
      {shown.length === 0 ? (
        <div className="text-sm text-slate-500">Фото нет{canAdd ? ' — сфотографируйте накладную или выберите файл' : ''}</div>
      ) : (
        <ul className="flex flex-wrap gap-3">
          {shown.map((f) => (
            <li key={f.id} className="flex w-32 flex-col gap-1 text-xs">
              <a href={`/api/v1/receipt-files/${f.id}`} target="_blank" rel="noreferrer" title="Открыть">
                {f.content_type === 'application/pdf' ? (
                  <div className="flex h-32 w-32 items-center justify-center rounded border border-slate-300 bg-slate-50">PDF</div>
                ) : (
                  <img src={`/api/v1/receipt-files/${f.id}`} alt={f.name || 'фото накладной'} className="h-32 w-32 rounded border border-slate-300 object-cover" />
                )}
              </a>
              <span className="truncate text-slate-600" title={f.name}>
                {f.name || 'фото'}
              </span>
              <span className="text-slate-500">
                {f.user_name}, {formatDateTime(f.created_at)}
              </span>
              <span className="flex gap-2">
                <a className="text-sky-700 underline" href={`/api/v1/receipt-files/${f.id}`} download={f.name || 'накладная'}>
                  скачать
                </a>
                {owner && (
                  <button type="button" className="text-rose-700 underline" onClick={() => setHiding({ file: f, reason: '' })}>
                    скрыть
                  </button>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
      {owner && hidden.length > 0 && (
        <details className="text-sm text-slate-600">
          <summary className="cursor-pointer">Скрытые: {hidden.length}</summary>
          <ul className="mt-1 flex flex-col gap-1">
            {hidden.map((f) => (
              <li key={f.id}>
                <Badge>скрыто</Badge>{' '}
                <a className="text-sky-700 underline" href={`/api/v1/receipt-files/${f.id}`} target="_blank" rel="noreferrer">
                  {f.name || 'фото'}
                </a>{' '}
                — {f.hidden_by}, {f.hidden_at ? formatDateTime(f.hidden_at) : ''}: {f.hidden_reason}
              </li>
            ))}
          </ul>
        </details>
      )}
      {hiding && (
        <Modal title="Скрыть фото накладной" onClose={() => setHiding(null)}>
          <div className="flex flex-col gap-3">
            <p className="text-sm text-slate-600">Файл не удаляется: он останется в базе и в журнале, но пропадёт из накладной.</p>
            <Field label="Почему скрываете" required>
              <input autoFocus value={hiding.reason} onChange={(e) => setHiding({ ...hiding, reason: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setHiding(null)}>
                Отмена
              </Button>
              <Button
                variant="danger"
                disabled={act.busy || !hiding.reason.trim()}
                onClick={() =>
                  void act.run(async () => {
                    await post(`/receipt-files/${hiding.file.id}/hide`, { reason: hiding.reason.trim() })
                    setHiding(null)
                    files.reload()
                  })
                }
              >
                Скрыть
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
