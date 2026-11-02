// Правило баллов товара или категории (задача 002, П1; ADR-140): «как выше», «не начислять»,
// процент от оплаченного, баллы за единицу. Меняет владелец; остальным — только посмотреть.
import { perUnitWord, type PointsDraft } from '../lib/points'
import type { PointsMode, StockUnit } from '../lib/types'
import { Field } from './ui'

export function PointsRuleField({
  inheritLabel,
  unit,
  draft,
  onChange,
  disabled,
  hint,
}: {
  /** «как в категории» у товара, «по общей ставке» у категории. */
  inheritLabel: string
  /** Единица товара; у категории — нет (за штуку, литр, кг или метр — по товару). */
  unit?: StockUnit
  draft: PointsDraft
  onChange: (d: PointsDraft) => void
  disabled?: boolean
  hint?: string
}) {
  const perUnit = unit ? perUnitWord(unit) : 'за единицу (шт, л, кг или м)'
  return (
    <Field label="Баллы клиенту" hint={hint}>
      <div className="flex gap-2">
        <select
          data-testid="points-mode"
          className="min-w-0 flex-1"
          value={draft.mode}
          disabled={disabled}
          onChange={(e) => onChange({ ...draft, mode: e.target.value as PointsMode })}
        >
          <option value="inherit">{inheritLabel}</option>
          <option value="none">не начислять</option>
          <option value="percent">% от оплаченного</option>
          <option value="fixed">баллов {perUnit}</option>
        </select>
        {(draft.mode === 'percent' || draft.mode === 'fixed') && (
          <input
            data-testid="points-value"
            className="w-24"
            inputMode="decimal"
            disabled={disabled}
            placeholder={draft.mode === 'percent' ? '0,5' : '10'}
            value={draft.text}
            onChange={(e) => onChange({ ...draft, text: e.target.value })}
          />
        )}
      </div>
    </Field>
  )
}
