// Накладные по фото (SPEC-30 §9, ADR-098): список черновиков прихода из бота и с экрана — дата,
// поставщик из накладной, строк, итог, фото; «Открыть» заполняет «Новый приход», «Убрать» убирает
// черновик из списка. «Заполнить по фото» — снимок камерой или файл, распознаёт сервер.
import { useRef, type ChangeEvent } from 'react'
import { newOpId, post, qs, upload } from '../lib/api'
import { formatDateTime, formatSom, plural } from '../lib/format'
import { useAction } from '../lib/hooks'
import { prepareFile } from '../lib/photo'
import { draftFileUrl, recognitionText, type Draft, type DraftItem } from '../lib/receiptDrafts'
import { Badge, Button, Card, CardTitle, ErrorBox } from './ui'

/** Миниатюра фото накладной; PDF — значком. По нажатию фото открывается целиком. */
export function DraftThumb({ id, contentType, size = 'h-16 w-16' }: { id: string; contentType: string; size?: string }) {
  return (
    <a href={draftFileUrl(id)} target="_blank" rel="noreferrer" title="Открыть фото накладной" className="shrink-0">
      {contentType === 'application/pdf' ? (
        <div className={`flex ${size} items-center justify-center rounded border border-slate-300 bg-slate-50 text-xs`}>PDF</div>
      ) : (
        <img src={draftFileUrl(id)} alt="фото накладной" className={`${size} rounded border border-slate-300 object-cover`} />
      )}
    </a>
  )
}

/** Блок «Накладные по фото: N». Пустой список — блока нет. */
export function ReceiptDraftsBlock({
  items,
  onOpen,
  onChanged,
}: {
  items: DraftItem[]
  onOpen: (id: string) => void
  onChanged: () => void
}) {
  const act = useAction()
  if (items.length === 0) return null
  const remove = (d: DraftItem) => {
    if (!window.confirm(`Убрать накладную по фото${d.supplier_name ? ` «${d.supplier_name}»` : ''}? Фото останется в журнале.`)) return
    void act.run(async () => {
      await post(`/receipt-drafts/${d.id}/remove`)
      onChanged()
    })
  }
  return (
    <Card>
      <div data-testid="receipt-drafts">
        <CardTitle>Накладные по фото: {items.length}</CardTitle>
        <ErrorBox error={act.error} />
        <ul className="divide-y divide-slate-100">
          {items.map((d) => {
            const state = recognitionText(d)
            return (
              <li key={d.id} className="flex flex-wrap items-center gap-3 py-2" data-testid="receipt-draft">
                <DraftThumb id={d.id} contentType={d.content_type} />
                <div className="min-w-0 flex-1 text-sm">
                  <div className="font-medium">
                    {d.supplier_name || 'Поставщик не указан'}
                    {d.doc_number && <span className="ml-2 text-slate-500">№ {d.doc_number}</span>}
                  </div>
                  <div className="text-xs text-slate-500">
                    {formatDateTime(d.created_at)} · <Badge tone={d.source === 'bot' ? 'sky' : 'slate'}>{d.source === 'bot' ? 'из бота' : 'с экрана'}</Badge> · {d.user_name}
                  </div>
                  <div className="text-xs text-slate-600">
                    {state ?? (
                      <>
                        {d.lines} {plural(d.lines, ['строка', 'строки', 'строк'])}
                        {d.total_tyiyn !== null && <> · итого {formatSom(d.total_tyiyn)}</>}
                      </>
                    )}
                  </div>
                </div>
                <div className="flex gap-2">
                  <Button onClick={() => onOpen(d.id)}>Открыть</Button>
                  <Button variant="secondary" disabled={act.busy} onClick={() => remove(d)}>
                    Убрать
                  </Button>
                </div>
              </li>
            )
          })}
        </ul>
      </div>
    </Card>
  )
}

/** «Заполнить по фото»: камера телефона или файл; снимок сжимается, сервер сохраняет и распознаёт. */
export function FillFromPhoto({ onDraft, disabled }: { onDraft: (d: Draft) => void; disabled?: boolean }) {
  const camera = useRef<HTMLInputElement>(null)
  const file = useRef<HTMLInputElement>(null)
  const act = useAction()
  const pick = (e: ChangeEvent<HTMLInputElement>) => {
    const f = e.target.files?.[0]
    e.target.value = ''
    if (!f) return
    void act.run(async () => {
      const p = await prepareFile(f)
      // Распознавание — обычно до минуты, с повторами сервиса — до четырёх: ждём пять.
      const d = await upload<Draft>(`/receipt-drafts${qs({ op_id: newOpId(), name: p.name })}`, p.blob, p.type, 300_000)
      onDraft(d)
    })
  }
  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="secondary" disabled={disabled || act.busy} onClick={() => camera.current?.click()}>
          {act.busy ? 'Распознаю…' : 'Заполнить по фото'}
        </Button>
        <Button variant="ghost" disabled={disabled || act.busy} onClick={() => file.current?.click()}>
          из файла
        </Button>
        {act.busy && <span className="text-xs text-slate-500">распознавание обычно занимает до минуты</span>}
      </div>
      <input ref={camera} type="file" accept="image/*" capture="environment" className="hidden" onChange={pick} />
      <input
        ref={file}
        data-testid="fill-photo-input"
        type="file"
        accept="image/jpeg,image/png,image/webp,application/pdf"
        className="hidden"
        onChange={pick}
      />
      <ErrorBox error={act.error} />
    </div>
  )
}
