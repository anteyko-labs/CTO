// Форма исправления из формы создания (SPEC-29 §24, ADR-085): исходный документ загружается здесь,
// форма получает его готовым — поля заполняются сразу, без повторной загрузки в самой форме.
import type { ReactNode } from 'react'
import type { Correction, CorrectionKind } from '../lib/corrections'
import { CorrectionBanner, useCorrection } from './Correction'
import { Button, ErrorBox, Loading } from './ui'

/** Что форма получает в режиме исправления: исходный документ, комментарий и проведение. */
export interface FixMode<T> {
  doc: T
  label: string
  number: number | null
  comment: string
  setComment: (v: string) => void
  busy: boolean
  error: string | null
  save(doc: T): Promise<Correction | undefined>
}

/** Загружает исходный документ и показывает форму, только когда он есть. */
export function FixDraft<T>({ kind, id, onCancel, children }: { kind: CorrectionKind; id: string; onCancel: () => void; children: (fix: FixMode<T>) => ReactNode }) {
  const c = useCorrection<T>(kind, id)
  const d = c.draft.data
  if (!d) {
    return c.draft.error ? (
      <div className="flex flex-col gap-2">
        <ErrorBox error={c.draft.error} />
        <div className="flex justify-end">
          <Button variant="secondary" onClick={onCancel}>
            Закрыть
          </Button>
        </div>
      </div>
    ) : (
      <Loading />
    )
  }
  return children({
    doc: d.doc,
    label: d.kind_label,
    number: d.number,
    comment: c.comment,
    setComment: c.setComment,
    busy: c.busy,
    error: c.error,
    save: (doc) => c.save(doc),
  })
}

/** Плашка «Исправление документа № N» с обязательным комментарием. */
export function FixBanner<T>({ fix }: { fix: FixMode<T> }) {
  return <CorrectionBanner label={fix.label} number={fix.number} comment={fix.comment} onComment={fix.setComment} />
}

/** Подпись главной кнопки формы исправления. */
export const FIX_SUBMIT = 'Сохранить исправление'
