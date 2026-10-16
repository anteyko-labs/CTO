// Выбор периода отчёта: «С», «По» и быстрые кнопки (SPEC-27 §6). Период не длиннее года.
import { Card, Field } from './ui'
import { shiftDate } from '../lib/format'
import { normalizeRange, type Range } from '../lib/income'

const QUICK_DAYS = [
  [0, 'Сегодня'],
  [6, '7 дней'],
  [29, 'Месяц'],
] as const

export function PeriodPicker({ value, today, onChange }: { value: Range; today: string; onChange: (r: Range) => void }) {
  return (
    <Card className="no-print mb-4 flex flex-wrap items-end gap-3">
      <Field label="С">
        <input
          type="date"
          className="w-40"
          value={value.from}
          max={today}
          onChange={(e) => e.target.value && onChange(normalizeRange(e.target.value, value.to))}
        />
      </Field>
      <Field label="По">
        <input
          type="date"
          className="w-40"
          value={value.to}
          max={today}
          onChange={(e) => e.target.value && onChange(normalizeRange(value.from, e.target.value))}
        />
      </Field>
      <div className="flex flex-wrap gap-2">
        {QUICK_DAYS.map(([n, label]) => {
          const on = value.to === today && value.from === shiftDate(today, -n)
          return (
            <button
              key={label}
              type="button"
              aria-pressed={on}
              className={`min-h-[38px] rounded-md border px-3 py-2 text-sm ${on ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 hover:bg-slate-50'}`}
              onClick={() => onChange({ from: shiftDate(today, -n), to: today })}
            >
              {label}
            </button>
          )
        })}
      </div>
    </Card>
  )
}
