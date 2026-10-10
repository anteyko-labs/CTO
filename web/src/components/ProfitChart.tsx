// График прибыли: за день — по часам (выручка и валовая), за период — по дням (валовая и чистая).
export interface Bar {
  label: string
  title: string
  values: { value: number; tone: 'soft' | 'gross' | 'net' }[]
}

const TONE = {
  soft: 'fill-sky-200',
  gross: 'fill-sky-600',
  net: 'fill-emerald-600',
}

/** Сокращённая сумма для оси: 12 500 → «12,5 тыс.». */
function short(t: number): string {
  const som = t / 100
  if (Math.abs(som) >= 1000) return `${(som / 1000).toFixed(Math.abs(som) >= 10000 ? 0 : 1).replace('.', ',')} тыс.`
  return `${Math.round(som)}`
}

export function ProfitChart({ bars, legend }: { bars: Bar[]; legend: { tone: keyof typeof TONE; label: string }[] }) {
  const all = bars.flatMap((b) => b.values.map((v) => v.value))
  const max = Math.max(0, ...all)
  const min = Math.min(0, ...all)
  const span = max - min || 1
  const W = 720
  const H = 240
  const left = 64
  const top = 12
  const bottom = 28
  const plotH = H - top - bottom
  const y = (v: number) => top + ((max - v) / span) * plotH
  const slot = (W - left - 8) / Math.max(bars.length, 1)
  const group = Math.min(slot * 0.8, 48)
  const each = group / Math.max(bars[0]?.values.length ?? 1, 1)
  // Подписи по оси X — не чаще, чем помещаются.
  const every = Math.ceil(bars.length / 12)
  const ticks = [max, (max + min) / 2, min].filter((v, i, a) => a.indexOf(v) === i)

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap gap-4 text-xs text-slate-600">
        {legend.map((l) => (
          <span key={l.label} className="flex items-center gap-1.5">
            <svg width="10" height="10" aria-hidden="true">
              <rect width="10" height="10" rx="2" className={TONE[l.tone]} />
            </svg>
            {l.label}
          </span>
        ))}
      </div>
      {bars.length === 0 ? (
        <div className="py-8 text-center text-sm text-slate-500">Продаж за этот период нет</div>
      ) : (
        <div className="overflow-x-auto">
          <svg viewBox={`0 0 ${W} ${H}`} className="h-auto w-full min-w-[480px]" role="img" aria-label="График прибыли">
            {ticks.map((t) => (
              <g key={t}>
                <line x1={left} x2={W - 4} y1={y(t)} y2={y(t)} className="stroke-slate-200" strokeWidth="1" />
                <text x={left - 6} y={y(t) + 4} textAnchor="end" className="fill-slate-500 text-[11px]">
                  {short(t)}
                </text>
              </g>
            ))}
            <line x1={left} x2={W - 4} y1={y(0)} y2={y(0)} className="stroke-slate-400" strokeWidth="1" />
            {bars.map((b, i) => {
              const x0 = left + i * slot + (slot - group) / 2
              return (
                <g key={b.label + i}>
                  <title>{b.title}</title>
                  {b.values.map((v, j) => {
                    const h = Math.abs(y(v.value) - y(0))
                    return (
                      <rect
                        key={j}
                        x={x0 + j * each + 1}
                        y={v.value >= 0 ? y(v.value) : y(0)}
                        width={Math.max(each - 2, 1)}
                        height={Math.max(h, v.value === 0 ? 0 : 1)}
                        rx="2"
                        className={v.tone === 'net' && v.value < 0 ? 'fill-rose-500' : TONE[v.tone]}
                      />
                    )
                  })}
                  {i % every === 0 && (
                    <text x={x0 + group / 2} y={H - 8} textAnchor="middle" className="fill-slate-500 text-[11px]">
                      {b.label}
                    </text>
                  )}
                </g>
              )
            })}
          </svg>
        </div>
      )}
    </div>
  )
}
