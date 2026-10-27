// Выбор мастера или кассира в режиме кассы (SPEC-36 §16, ui-rules §9): кнопками от 56 точек вместо
// системного списка — у Chrome на Windows в нём мелкие строки. До трёх вариантов — кнопки в ряд,
// больше — одна кнопка открывает окно с крупными кнопками.
import { useState, type ReactNode } from 'react'
import { personPickLayout } from '../lib/cashierMode'
import { Modal } from './ui'

export interface PersonChoice {
  id: string
  full_name: string
}

export function PersonPick({
  id,
  label,
  required = false,
  hint,
  value,
  people,
  empty,
  onChange,
}: {
  /** id поля — как у прежнего списка: по нему находят поле проверки и подсказка «Не хватает». */
  id: string
  label: string
  required?: boolean
  hint?: ReactNode
  value: string
  people: PersonChoice[]
  /** Вариант «без выбора» (например «— без мастера —»): пустое значение тоже можно выбрать. */
  empty?: string
  onChange: (id: string) => void
}) {
  const [open, setOpen] = useState(false)
  const choices: PersonChoice[] = [...(empty ? [{ id: '', full_name: empty }] : []), ...people]
  const chosen = choices.find((c) => c.id === value)
  const btn = (on: boolean) =>
    `inline-flex min-h-[56px] min-w-[56px] items-center justify-center rounded-lg border px-3 py-1 text-center text-lg leading-tight font-medium shadow-sm active:bg-sky-100 ${
      on ? 'border-sky-600 bg-sky-50 text-sky-900 ring-2 ring-sky-600' : 'border-slate-300 bg-white text-slate-800'
    }`
  return (
    <div className="flex min-w-0 flex-col gap-1 text-sm" data-person-pick={id}>
      <span className="font-medium text-slate-700">
        {label}
        {required && (
          <span className="ml-0.5 text-rose-600" title="Обязательное поле">
            *
          </span>
        )}
      </span>
      {personPickLayout(choices.length) === 'inline' ? (
        <div id={id} data-value={value} role="radiogroup" aria-label={label} className="grid gap-2" style={{ gridTemplateColumns: `repeat(${Math.max(choices.length, 1)}, minmax(0, 1fr))` }}>
          {choices.map((c) => (
            <button key={c.id || 'none'} type="button" role="radio" aria-checked={c.id === value} className={btn(c.id === value)} onClick={() => onChange(c.id)}>
              {c.full_name}
            </button>
          ))}
        </div>
      ) : (
        <button
          id={id}
          data-value={value}
          type="button"
          aria-haspopup="dialog"
          className={`${btn(false)} w-full justify-between! text-left`}
          onClick={() => setOpen(true)}
        >
          <span className={`truncate ${chosen && chosen.id ? '' : 'text-slate-500'}`}>{chosen ? chosen.full_name : 'Выбрать'}</span>
          <span aria-hidden="true" className="ml-2 text-slate-500">
            ▾
          </span>
        </button>
      )}
      {hint && <span className="text-xs text-slate-500">{hint}</span>}
      {open && (
        <Modal title={label} onClose={() => setOpen(false)} wide>
          <div className="grid max-h-[calc(100dvh-10rem)] grid-cols-2 gap-2 overflow-y-auto overscroll-contain p-0.5 sm:grid-cols-3" data-testid="person-choices">
            {choices.map((c) => (
              <button
                key={c.id || 'none'}
                type="button"
                aria-pressed={c.id === value}
                className={`${btn(c.id === value)} min-h-[64px]!`}
                onClick={() => {
                  onChange(c.id)
                  setOpen(false)
                }}
              >
                {c.full_name}
              </button>
            ))}
          </div>
        </Modal>
      )}
    </div>
  )
}
