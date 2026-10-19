// «Исправить» у документов владельца (SPEC-29 §24, ADR-085): кнопка, плашка формы исправления,
// связи «исправлен → № N» / «исправление № M» и сравнение «было — стало» — одни для всех документов.
import { useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import { newOpId } from '../lib/api'
import { useAuth } from '../lib/auth'
import { loadCorrectionDraft, loadCorrectionLinks, numberText, postCorrection, type Correction, type CorrectionKind, type CorrectionMark, type DiffItem } from '../lib/corrections'
import { formatDateTime } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { Badge, Button, Field, Notice } from './ui'

/** Владелец видит «Исправить»; администратору кнопки нет (сервер тоже ответит 403). */
export function FixButton({ onClick, className = '', small = false }: { onClick: () => void; className?: string; small?: boolean }) {
  const { user } = useAuth()
  if (user?.role !== 'owner') return null
  return (
    <Button variant="secondary" className={`${small ? 'min-h-0 px-2 py-1 text-xs' : ''} ${className}`} onClick={onClick} data-testid="fix-button">
      Исправить
    </Button>
  )
}

/** Что поменялось: таблица «что — было — стало». */
export function DiffSummary({ diff }: { diff: DiffItem[] }) {
  if (diff.length === 0) return <div className="text-xs text-slate-500">Без изменений</div>
  return (
    <table className="w-full text-xs" data-testid="diff-summary">
      <thead>
        <tr className="text-left text-slate-500">
          <th className="py-1 pr-2 font-medium">Что</th>
          <th className="py-1 pr-2 font-medium">Было</th>
          <th className="py-1 font-medium">Стало</th>
        </tr>
      </thead>
      <tbody>
        {diff.map((d, i) => (
          <tr key={i} className="border-t border-slate-100 align-top">
            <td className="py-1 pr-2">{d.what}</td>
            <td className="py-1 pr-2 text-slate-500 line-through decoration-slate-300">{d.old}</td>
            <td className="py-1 font-medium">{d.new}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

function Ref({ id, number, hrefOf, fallback }: { id: string; number: number | null; hrefOf?: (id: string) => string; fallback: string }) {
  const text = numberText(number) || fallback
  return hrefOf ? (
    <Link className="font-medium text-sky-700 hover:underline" to={hrefOf(id)}>
      {text}
    </Link>
  ) : (
    <b>{text}</b>
  )
}

/** Связь исправления для истории документа. `hrefOf` — адрес документа того же вида. */
export function CorrectionNote({ c, side, hrefOf }: { c: Correction; side: 'original' | 'new'; hrefOf?: (id: string) => string }) {
  return (
    <Notice tone={side === 'original' ? 'amber' : 'sky'}>
      <div className="flex flex-col gap-2" data-testid={side === 'original' ? 'corrected-by' : 'correction-of'}>
        <div>
          {side === 'original' ? (
            <>
              Исправлен → <Ref id={c.new_id} number={c.new_number} hrefOf={hrefOf} fallback="новый документ" />
            </>
          ) : (
            <>
              Исправление <Ref id={c.original_id} number={c.original_number} hrefOf={hrefOf} fallback="документа" />
            </>
          )}
          <span className="text-slate-600">
            {' '}
            · {c.user_name}, {formatDateTime(c.created_at)} · {c.comment}
          </span>
        </div>
        {c.diff.length > 0 && <DiffSummary diff={c.diff} />}
      </div>
    </Notice>
  )
}

/** Связи документа: исправлен ли он и не исправление ли он сам. */
export function CorrectionLinks({ id, hrefOf }: { id: string; hrefOf?: (id: string) => string }) {
  const links = useLoad(() => loadCorrectionLinks(id), [id])
  if (!links.data) return null
  return (
    <>
      {links.data.correction_of && <CorrectionNote c={links.data.correction_of} side="new" hrefOf={hrefOf} />}
      {links.data.corrected_by && <CorrectionNote c={links.data.corrected_by} side="original" hrefOf={hrefOf} />}
    </>
  )
}

/** Отметка строки списка: «исправлен → № 15» / «исправление № 12». */
export function CorrectionBadges({ mark }: { mark: CorrectionMark | undefined }) {
  if (!mark) return null
  return (
    <>
      {mark.correctionOf && <Badge tone="sky">исправление {numberText(mark.correctionOf.original_number)}</Badge>}
      {mark.correctedTo && <Badge tone="amber">исправлен → {numberText(mark.correctedTo.new_number) || 'новый'}</Badge>}
    </>
  )
}

/** Плашка формы исправления и обязательный комментарий «что и почему». */
export function CorrectionBanner({
  label,
  number,
  comment,
  onComment,
}: {
  label: string
  number: number | null
  comment: string
  onComment: (v: string) => void
}) {
  return (
    <div className="flex flex-col gap-2 rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900" data-testid="correction-banner">
      <div className="font-semibold">
        Исправление документа {numberText(number) || `«${label}»`}
      </div>
      <div className="text-xs">
        Исходный документ сторнируется, исправленный проводится — одним действием. В истории останутся оба со ссылками друг на
        друга.
      </div>
      <Field label="Что и почему исправляете" required>
        <input id="correction-comment" value={comment} maxLength={500} onChange={(e) => onComment(e.target.value)} placeholder="например: кассир ошибся в количестве" />
      </Field>
    </div>
  )
}

/**
 * Исправление документа из формы: исходный документ для заполнения, комментарий и проведение.
 * Попытка — один `op_id`: повтор того же «Сохранить» не проводит второй раз.
 */
export function useCorrection<T = Record<string, unknown>>(kind: CorrectionKind, id: string | null) {
  const draft = useLoad(() => (id ? loadCorrectionDraft<T>(kind, id) : Promise.resolve(null)), [kind, id])
  const [comment, setComment] = useState('')
  const attempt = useRef<{ key: string; opId: string } | null>(null)
  const act = useAction()
  const save = (doc: unknown) =>
    act.run(async () => {
      if (!id) throw new Error('нет документа')
      if (!comment.trim()) throw new Error('Укажите, что и почему исправляете')
      const key = JSON.stringify([doc, comment.trim()])
      if (attempt.current?.key !== key) attempt.current = { key, opId: newOpId() }
      return postCorrection({ op_id: attempt.current.opId, kind, original_id: id, comment: comment.trim(), doc })
    })
  return { draft, comment, setComment, save, busy: act.busy, error: act.error || draft.error }
}
