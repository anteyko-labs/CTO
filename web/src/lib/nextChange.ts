// «Следующая замена» в чеке «В сервис» (SPEC-34 §2): по строке на каждую услугу чека со строкой
// книжки. Поле заполнено заранее — пробег сейчас + интервал (у машины свой — он, иначе услуги);
// кассир меняет число или очищает (пустое — не напоминать по этой строке).
import { fromKm, type MileageUnit } from './mileage'
import type { BookSlot, SaleBook, Service } from './types'

export interface NextRow {
  /** Строка книжки: одна строка «Следующей замены» на строку книжки. */
  slot: BookSlot
  service_id: string
  label: string
  /** Добавлена кнопкой «+ ещё строку»: услуги в чеке нет. */
  extra: boolean
  /** Что в поле: правка кассира или заполненное заранее. */
  value: string
  prefill: string
  interval_days: number | null
}

/** Интервал строки у машины (SPEC-27): км и дни. */
export type OwnIntervals = Partial<Record<BookSlot, { km: number | null; days: number | null }>>

const bookSlot = (s: Service | null | undefined): BookSlot | null => (s?.book_slot && s.book_slot !== 'other' ? s.book_slot : null)

export function nextRows(args: {
  /** Услуги строк чека по порядку. */
  services: (Service | null | undefined)[]
  /** Услуги, добавленные «+ ещё строку». */
  extra: Service[]
  /** Правки кассира по строке книжки; '' — очистил. */
  edits: Partial<Record<BookSlot, string>>
  mileage: string
  unit: MileageUnit
  own: OwnIntervals
}): NextRow[] {
  const out: NextRow[] = []
  const add = (s: Service, extra: boolean) => {
    const slot = bookSlot(s)
    if (!slot || out.some((r) => r.slot === slot)) return
    const km = args.own[slot]?.km ?? s.interval_km ?? null
    const now = args.mileage ? Number(args.mileage) : null
    const prefill = now !== null && Number.isFinite(now) && km ? String(now + fromKm(km, args.unit)) : ''
    const edit = args.edits[slot]
    out.push({
      slot,
      service_id: s.id,
      label: s.name,
      extra,
      value: edit ?? prefill,
      prefill,
      interval_days: args.own[slot]?.days ?? s.interval_days ?? null,
    })
  }
  for (const s of args.services) if (s) add(s, false)
  for (const s of args.extra) add(s, true)
  return out
}

/** Строка, где следующая замена меньше пробега сейчас: «Провести» нельзя. */
export function nextTooLow(rows: NextRow[], mileage: string): NextRow | null {
  if (!mileage) return null
  const now = Number(mileage)
  return rows.find((r) => r.value !== '' && Number(r.value) < now) ?? null
}

/** Что уходит с чеком: пробег в единице чека; пустое поле — не напоминать. */
export function nextPayload(rows: NextRow[]): { service_id: string; slot: BookSlot; next_mileage: number | null }[] {
  return rows.map((r) => ({ service_id: r.service_id, slot: r.slot, next_mileage: r.value ? Number(r.value) : null }))
}

/** Услуги для «+ ещё строку»: со строкой книжки, которой ещё нет в блоке. */
export function extraChoices(all: Service[], rows: NextRow[]): Service[] {
  const used = new Set(rows.map((r) => r.slot))
  const seen = new Set<BookSlot>()
  return all.filter((s) => {
    const slot = bookSlot(s)
    if (!s.active || !slot || used.has(slot) || seen.has(slot)) return false
    seen.add(slot)
    return true
  })
}

const addDays = (iso: string, days: number): string => {
  const d = new Date(`${iso}T00:00:00Z`)
  d.setUTCDate(d.getUTCDate() + days)
  return d.toISOString().slice(0, 10)
}

/** Книжка для печатного чека, пока чек в очереди без сети (SPEC-34 §4): как отдаст сервер. */
export function receiptBook(
  car: { brand: string; model: string; plate: string },
  mileage: string,
  unit: MileageUnit,
  rows: NextRow[],
  today: string,
): SaleBook {
  return {
    brand: car.brand,
    model: car.model,
    plate: car.plate,
    mileage: mileage ? Number(mileage) : null,
    mileage_unit: unit,
    lines: rows
      .filter((r) => r.value !== '')
      .map((r) => ({
        label: r.label,
        next_mileage: Number(r.value),
        unit,
        next_date: r.interval_days ? addDays(today, r.interval_days) : null,
      })),
  }
}
