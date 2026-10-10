// Тепловая карта продаж: дни недели × часы, чем темнее клетка — тем больше выручка.
// Одна шкала одного цвета (sky): в тёмной теме ступени палитры переопределены и остаются по порядку.
import { useState } from 'react'
import { formatSom } from '../lib/format'
import type { HeatCell } from '../lib/types'

const DAYS = ['Пн', 'Вт', 'Ср', 'Чт', 'Пт', 'Сб', 'Вс']

/** Ступени яркости: 0 — продаж нет, дальше по долям от самой сильной клетки. */
const LEVELS = ['bg-slate-100', 'bg-sky-100', 'bg-sky-200', 'bg-sky-300', 'bg-sky-500', 'bg-sky-700']

const level = (v: number, max: number) => (v <= 0 || max <= 0 ? 0 : Math.min(5, Math.ceil((v * 5) / max)))

export function Heatmap({ cells, days = 28 }: { cells: HeatCell[]; days?: number }) {
  const [hover, setHover] = useState<HeatCell | null>(null)
  // Рабочие часы 8–21; если продавали раньше или позже — сетка расширяется.
  const from = Math.min(8, ...cells.filter((c) => c.sales_count > 0).map((c) => c.hour))
  const to = Math.max(21, ...cells.filter((c) => c.sales_count > 0).map((c) => c.hour))
  const hours = Array.from({ length: to - from + 1 }, (_, k) => from + k)
  const byKey = new Map(cells.map((c) => [`${c.dow}:${c.hour}`, c]))
  const cell = (dow: number, hour: number): HeatCell => byKey.get(`${dow}:${hour}`) ?? { dow, hour, revenue_tyiyn: 0, sales_count: 0 }
  const max = Math.max(0, ...cells.map((c) => c.revenue_tyiyn))
  const best = cells.reduce<HeatCell | null>((a, c) => (c.revenue_tyiyn > (a?.revenue_tyiyn ?? 0) ? c : a), null)
  const text = (c: HeatCell) =>
    `${DAYS[c.dow - 1]}, ${c.hour}:00–${c.hour + 1}:00 · ${c.sales_count ? `${formatSom(Math.max(0, c.revenue_tyiyn))}, чеков ${c.sales_count}` : 'продаж нет'}`

  if (max <= 0) return <div className="rounded-md bg-slate-50 py-8 text-center text-sm text-slate-500">За последние {days} дней продаж нет</div>

  return (
    <div className="flex flex-col gap-2">
      <div
        className="grid gap-0.5 text-[11px] text-slate-500"
        style={{ gridTemplateColumns: `1.75rem repeat(${hours.length}, minmax(0, 1fr))` }}
        onMouseLeave={() => setHover(null)}
        role="img"
        aria-label="Продажи по дням недели и часам"
      >
        <span />
        {hours.map((h) => (
          <span key={h} className="text-center tabular-nums">
            {h}
          </span>
        ))}
        {DAYS.map((name, di) => (
          <div key={name} className="contents">
            <span className="flex items-center">{name}</span>
            {hours.map((h) => {
              const c = cell(di + 1, h)
              const on = hover?.dow === c.dow && hover.hour === c.hour
              return (
                <button
                  key={h}
                  type="button"
                  title={text(c)}
                  aria-label={text(c)}
                  className={`h-6 rounded-sm ${LEVELS[level(c.revenue_tyiyn, max)]} ${on ? 'ring-2 ring-slate-700' : ''}`}
                  onMouseEnter={() => setHover(c)}
                  onFocus={() => setHover(c)}
                  onClick={() => setHover(c)}
                />
              )
            })}
          </div>
        ))}
      </div>
      <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 text-xs text-slate-500">
        <span className="min-h-4">
          {hover ? (
            <span className="font-medium text-slate-800">{text(hover)}</span>
          ) : best ? (
            <>
              Сильнее всего: <span className="font-medium text-slate-800">{text(best)}</span>
            </>
          ) : null}
        </span>
        <span className="flex items-center gap-1">
          меньше
          {LEVELS.map((l) => (
            <span key={l} className={`h-3 w-3 rounded-sm ${l}`} />
          ))}
          больше
        </span>
      </div>
    </div>
  )
}
