// График прибыли линиями: плавная кривая с заливкой, точки, последняя сумма подписана,
// при наведении — вертикальная линия и карточка со всеми суммами этой точки.
import { useLayoutEffect, useRef, useState } from 'react'
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
  // «5 тыс.», а не «5,0 тыс.»: лишний ноль только удлиняет подпись оси.
  const trim = (v: string) => v.replace(/\.0$/, '').replace('.', ',')
  if (abs >= 1_000_000) return `${trim((som / 1_000_000).toFixed(1))} млн`
  if (abs >= 1000) return `${trim((som / 1000).toFixed(abs >= 10_000 ? 0 : 1))} тыс.`
  return `${Math.round(som)}`
}

/** «Круглый» шаг сетки: 1, 2, 5 × 10ⁿ. */
function niceStep(span: number, ticks: number): number {
  const raw = span / ticks
  const pow = 10 ** Math.floor(Math.log10(raw || 1))
  const n = raw / pow
  return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10) * pow
}

/**
 * Плавная кривая через точки — монотонная кубическая (как monotoneX): между двумя точками
 * линия не уходит выше большей и ниже меньшей, поэтому не рисует продаж, которых не было.
 */
function smoothPath(pts: [number, number][]): string {
  const n = pts.length
  if (n === 0) return ''
  if (n < 3) return pts.map(([px, py], i) => `${i ? 'L' : 'M'}${px},${py}`).join(' ')
  const dx: number[] = []
  const m: number[] = []
  for (let i = 0; i < n - 1; i++) {
    dx.push(pts[i + 1][0] - pts[i][0])
    m.push(dx[i] === 0 ? 0 : (pts[i + 1][1] - pts[i][1]) / dx[i])
  }
  const t: number[] = [m[0]]
  for (let i = 1; i < n - 1; i++) {
    const a = m[i - 1]
    const b = m[i]
    // Перелом (вершина или впадина) — касательная горизонтальна, иначе гармоническое среднее наклонов.
    t.push(a * b <= 0 ? 0 : (3 * (dx[i - 1] + dx[i])) / ((2 * dx[i] + dx[i - 1]) / a + (dx[i] + 2 * dx[i - 1]) / b))
  }
  t.push(m[n - 2])
  const d = [`M${pts[0][0]},${pts[0][1]}`]
  for (let i = 0; i < n - 1; i++) {
    const [x0, y0] = pts[i]
    const [x1, y1] = pts[i + 1]
    const h = dx[i] / 3
    d.push(`C${x0 + h},${y0 + t[i] * h} ${x1 - h},${y1 - t[i + 1] * h} ${x1},${y1}`)
  }
  return d.join(' ')
}

/** Ширина блока в пикселях: график рисуется в натуральную величину, подписи не мельчают на телефоне. */
function useWidth(fallback: number) {
  const ref = useRef<HTMLDivElement>(null)
  const [width, setWidth] = useState(fallback)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const update = () => el.clientWidth > 0 && setWidth(el.clientWidth)
    update()
    const ro = new ResizeObserver(update)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return { ref, width }
}

export function ProfitChart({ points, series, empty = 'Продаж за этот период нет' }: { points: Point[]; series: Series[]; empty?: string }) {
  const [hover, setHover] = useState<number | null>(null)
  const { ref, width } = useWidth(760)
  const all = series.flatMap((s) => s.values)
  const hasData = all.some((v) => v !== 0)

  // Одна единица рисунка — один пиксель экрана: шрифт 12px остаётся 12px и на телефоне.
  const W = Math.max(280, Math.round(width))
  const phone = W < 560
  const H = phone ? 220 : 280
  const left = phone ? 60 : 64
  // Справа запас под подпись последней точки, чтобы её не обрезало.
  const right = phone ? 20 : 24
  const top = 16
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
  // На телефоне не больше 5 подписей по оси X, на большом экране — до 10.
  // Последняя точка подписана всегда, соседняя с ней подпись убирается, чтобы не налезали.
  const every = Math.max(1, Math.ceil((points.length - 1) / (phone ? 4 : 9)))
  const labeled = (i: number) => i === points.length - 1 || (i % every === 0 && points.length - 1 - i >= every * 0.6)
  const anchor = (i: number) => (points.length > 1 && i === points.length - 1 ? 'end' : points.length > 1 && i === 0 ? 'start' : 'middle')

  return (
    <div ref={ref} className="flex flex-col gap-3">
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
        <div className="relative">
          <svg
            viewBox={`0 0 ${W} ${H}`}
            width={W}
            height={H}
            className="block h-auto w-full touch-pan-y select-none"
            role="img"
            aria-label="График прибыли"
            onMouseLeave={() => setHover(null)}
            onPointerMove={(e) => {
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
                <text x={left - 8} y={y(t) + 4} textAnchor="end" className="fill-slate-500 text-[12px] tabular-nums">
                  {short(t)}
                </text>
              </g>
            ))}
            {/* Ниже нуля — убыток: лёгкая красная подложка. */}
            {min < 0 && <rect x={left} y={y(0)} width={plotW} height={y(min) - y(0)} className="fill-rose-50" />}

            {points.map(
              (p, i) =>
                labeled(i) && (
                  <text key={p.label + i} x={x(i)} y={H - 8} textAnchor={anchor(i)} className="fill-slate-500 text-[12px] tabular-nums">
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
                      r={hover === i ? 5 : points.length <= (phone ? 10 : 16) ? 3 : 0}
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
              className="pointer-events-none absolute top-2 z-10 min-w-48 whitespace-nowrap rounded-md border border-slate-200 bg-white px-3 py-2 text-xs shadow-lg"
              style={
                // На телефоне карточка прижимается к краю напротив точки, чтобы не вылезать за экран.
                phone
                  ? x(hover) > W / 2
                    ? { left: 4 }
                    : { right: 4 }
                  : {
                      left: `${(x(hover) / W) * 100}%`,
                      transform: x(hover) > W * 0.6 ? 'translateX(calc(-100% - 12px))' : 'translateX(12px)',
                    }
              }
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
