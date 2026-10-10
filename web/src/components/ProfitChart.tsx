// График прибыли линиями: плавная кривая с заливкой, точки, последняя сумма подписана,
// при наведении — вертикальная линия и карточка со всеми суммами этой точки.
import { useState } from 'react'
import { formatSom } from '../lib/format'

export interface Series {
  name: string
  /** Цвет линии и заливки: класс stroke/fill задаётся через currentColor. */
  color: 'sky' | 'emerald' | 'slate'
  values: number[]
}

export interface Point {
  label: string
  /** Заголовок подсказки: «12:00–13:00», «Пн, 06.10». */
  title: string
  /** Дополнительные строки подсказки сверх линий. */
  extra?: [string, number][]
}

const COLOR: Record<Series['color'], { text: string; dot: string }> = {
  sky: { text: 'text-sky-600', dot: 'bg-sky-600' },
  emerald: { text: 'text-emerald-600', dot: 'bg-emerald-600' },
  slate: { text: 'text-slate-400', dot: 'bg-slate-400' },
}

/** Сумма для оси: 12 500 с → «12,5 тыс.». */
function short(t: number): string {
  const som = t / 100
  const abs = Math.abs(som)
  if (abs >= 1_000_000) return `${(som / 1_000_000).toFixed(1).replace('.', ',')} млн`
  if (abs >= 1000) return `${(som / 1000).toFixed(abs >= 10_000 ? 0 : 1).replace('.', ',')} тыс.`
  return `${Math.round(som)}`
}

/** «Круглый» шаг сетки: 1, 2, 5 × 10ⁿ. */
function niceStep(span: number, ticks: number): number {
  const raw = span / ticks
  const pow = 10 ** Math.floor(Math.log10(raw || 1))
  const n = raw / pow
  return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10) * pow
}

/** Плавная кривая через точки (монотонная, без «перелётов» выше максимума). */
function smoothPath(pts: [number, number][]): string {
  if (pts.length === 0) return ''
  if (pts.length === 1) return `M${pts[0][0]},${pts[0][1]}`
  const d = [`M${pts[0][0]},${pts[0][1]}`]
  for (let i = 0; i < pts.length - 1; i++) {
    const [x0, y0] = pts[i]
    const [x1, y1] = pts[i + 1]
    const dx = (x1 - x0) / 3
    d.push(`C${x0 + dx},${y0} ${x1 - dx},${y1} ${x1},${y1}`)
  }
  return d.join(' ')
}

export function ProfitChart({ points, series, empty = 'Продаж за этот период нет' }: { points: Point[]; series: Series[]; empty?: string }) {
  const [hover, setHover] = useState<number | null>(null)
  const all = series.flatMap((s) => s.values)
  const hasData = all.some((v) => v !== 0)

  const W = 760
  const H = 280
  const left = 58
  const right = 16
  const top = 20
  const bottom = 30
  const plotW = W - left - right
  const plotH = H - top - bottom

  const rawMax = Math.max(0, ...all)
  const rawMin = Math.min(0, ...all)
  const step = niceStep(rawMax - rawMin || 100, 4)
  const max = Math.ceil(rawMax / step) * step || step
  const min = Math.floor(rawMin / step) * step
  const ticks: number[] = []
  for (let v = min; v <= max + step / 2; v += step) ticks.push(v)

  const x = (i: number) => left + (points.length <= 1 ? plotW / 2 : (i / (points.length - 1)) * plotW)
  const y = (v: number) => top + ((max - v) / (max - min || 1)) * plotH
  const every = Math.ceil(points.length / 10)

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap gap-4 text-xs text-slate-600">
        {series.map((s) => (
          <span key={s.name} className="flex items-center gap-1.5">
            <span className={`h-2.5 w-2.5 rounded-full ${COLOR[s.color].dot}`} />
            {s.name}
          </span>
        ))}
      </div>
      {!hasData ? (
        <div className="rounded-md bg-slate-50 py-10 text-center text-sm text-slate-500">{empty}</div>
      ) : (
        <div className="relative overflow-x-auto">
          <svg
            viewBox={`0 0 ${W} ${H}`}
            className="h-auto w-full min-w-[480px] select-none"
            role="img"
            aria-label="График прибыли"
            onMouseLeave={() => setHover(null)}
            onMouseMove={(e) => {
              const box = e.currentTarget.getBoundingClientRect()
              const px = ((e.clientX - box.left) / box.width) * W
              const i = points.length <= 1 ? 0 : Math.round(((px - left) / plotW) * (points.length - 1))
              setHover(Math.max(0, Math.min(points.length - 1, i)))
            }}
          >
            <defs>
              {series.map((s, si) => (
                <linearGradient key={s.name} id={`area-${si}`} x1="0" x2="0" y1="0" y2="1">
                  <stop offset="0%" stopColor="currentColor" stopOpacity="0.28" className={COLOR[s.color].text} />
                  <stop offset="100%" stopColor="currentColor" stopOpacity="0" className={COLOR[s.color].text} />
                </linearGradient>
              ))}
            </defs>

            {ticks.map((t) => (
              <g key={t}>
                <line x1={left} x2={W - right} y1={y(t)} y2={y(t)} className={t === 0 ? 'stroke-slate-300' : 'stroke-slate-100'} strokeWidth="1" />
                <text x={left - 8} y={y(t) + 4} textAnchor="end" className="fill-slate-400 text-[11px]">
                  {short(t)}
                </text>
              </g>
            ))}
            {/* Ниже нуля — убыток: лёгкая красная подложка. */}
            {min < 0 && <rect x={left} y={y(0)} width={plotW} height={y(min) - y(0)} className="fill-rose-50" />}

            {points.map(
              (p, i) =>
                i % every === 0 && (
                  <text key={p.label + i} x={x(i)} y={H - 8} textAnchor="middle" className="fill-slate-400 text-[11px]">
                    {p.label}
                  </text>
                ),
            )}

            {series.map((s, si) => {
              const pts = s.values.map((v, i) => [x(i), y(v)] as [number, number])
              const line = smoothPath(pts)
              const area = `${line} L${x(s.values.length - 1)},${y(0)} L${x(0)},${y(0)} Z`
              return (
                <g key={s.name} className={COLOR[s.color].text}>
                  <path d={area} fill={`url(#area-${si})`} />
                  <path d={line} fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />
                  {pts.map(([px, py], i) => (
                    <circle
                      key={i}
                      cx={px}
                      cy={py}
                      r={hover === i ? 5 : points.length <= 16 ? 3 : 0}
                      className={s.values[i] < 0 ? 'fill-rose-500' : 'fill-current'}
                      stroke="white"
                      strokeWidth="1.5"
                    />
                  ))}
                </g>
              )
            })}

            {hover !== null && <line x1={x(hover)} x2={x(hover)} y1={top} y2={top + plotH} className="stroke-slate-300" strokeDasharray="4 4" />}
          </svg>

          {hover !== null && (
            <div
              className="pointer-events-none absolute top-2 z-10 min-w-52 whitespace-nowrap rounded-md border border-slate-200 bg-white px-3 py-2 text-xs shadow-lg"
              style={{
                left: `${(x(hover) / W) * 100}%`,
                transform: x(hover) > W * 0.6 ? 'translateX(calc(-100% - 12px))' : 'translateX(12px)',
              }}
            >
              <div className="mb-1 font-medium text-slate-800">{points[hover].title}</div>
              {series.map((s) => (
                <div key={s.name} className="flex items-center justify-between gap-4">
                  <span className="flex items-center gap-1.5 text-slate-600">
                    <span className={`h-2 w-2 rounded-full ${COLOR[s.color].dot}`} />
                    {s.name}
                  </span>
                  <span className={`font-medium ${s.values[hover] < 0 ? 'text-rose-600' : 'text-slate-900'}`}>{formatSom(s.values[hover])}</span>
                </div>
              ))}
              {(points[hover].extra ?? []).map(([label, v]) => (
                <div key={label} className="flex justify-between gap-4 text-slate-500">
                  <span>{label}</span>
                  <span>{formatSom(v)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
