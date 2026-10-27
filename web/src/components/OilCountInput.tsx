// Пересчёт масла: «кан. по 4 л» и «+ л», рядом итог «= 34,5 л» (SPEC-36 §13, ADR-123). В учёт идёт
// одно число мл — переводит `parseOil`; здесь только поля.
import type { FocusEvent, KeyboardEvent } from 'react'
import { formatLiters } from '../lib/format'
import { containerLabel, parseOil } from '../lib/units'

export interface OilCount {
  cans: string
  liters: string
}

export function OilCountInput({
  containerMl,
  value,
  onChange,
  onCommit,
  onEnter,
  idPrefix,
  dataCount,
  autoFocus,
}: {
  containerMl: number
  value: OilCount
  onChange: (v: OilCount) => void
  /** Фокус ушёл из обоих полей — записать. */
  onCommit?: () => void
  onEnter?: () => void
  /** Поля получают id `${idPrefix}` (канистры) и `${idPrefix}-l` (литры). */
  idPrefix?: string
  /** Строка ревизии: `data-count` у поля канистр, `data-count-l` у поля литров. */
  dataCount?: string
  autoFocus?: boolean
}) {
  const ml = parseOil(value.cans, value.liters, containerMl)
  const label = containerLabel(containerMl)
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'Enter') return
    e.preventDefault()
    if (onEnter) onEnter()
    else e.currentTarget.blur()
  }
  const onBlur = (e: FocusEvent<HTMLDivElement>) => {
    // Переход между своими полями — ещё не конец ввода.
    if (onCommit && !e.currentTarget.contains(e.relatedTarget as Node | null)) onCommit()
  }
  return (
    <div className="flex flex-wrap items-center gap-1 text-sm" onBlur={onBlur} data-testid="oil-count">
      <input
        id={idPrefix}
        data-count={dataCount}
        aria-label={label}
        className={`w-16 ${ml === null ? 'border-rose-400' : ''}`}
        inputMode="numeric"
        autoFocus={autoFocus}
        value={value.cans}
        onChange={(e) => onChange({ ...value, cans: e.target.value })}
        onKeyDown={onKeyDown}
      />
      <span className="text-xs text-slate-500">{label}</span>
      <span className="text-slate-400">+</span>
      <input
        id={idPrefix ? `${idPrefix}-l` : undefined}
        data-count-l={dataCount}
        aria-label="+ л"
        className={`w-16 ${ml === null ? 'border-rose-400' : ''}`}
        inputMode="decimal"
        value={value.liters}
        onChange={(e) => onChange({ ...value, liters: e.target.value })}
        onKeyDown={onKeyDown}
      />
      <span className="text-xs text-slate-500">л</span>
      {typeof ml === 'number' && (
        <span className="whitespace-nowrap text-xs font-medium text-slate-700" data-testid="oil-count-total">
          = {formatLiters(ml)}
        </span>
      )}
    </div>
  )
}
