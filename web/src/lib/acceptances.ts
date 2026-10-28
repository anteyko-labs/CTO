// Приёмка товара на планшете (SPEC-39, ADR-134): вид приёмки с сервера, отметки кассира по
// строкам, показ и ввод количеств, хранение отметок на устройстве до проведения. Количество в
// учёте — целые штуки, мл, граммы (инвариант 1); здесь только показ и ввод.
import { formatKg, formatLiters, litersInput, parseLiters } from './format'

export type AcceptanceStatus = 'waiting' | 'accepted' | 'refused' | 'cancelled'
export type Reason = 'missing' | 'damaged' | 'other'

export const REASONS: [Reason, string][] = [
  ['missing', 'не привезли'],
  ['damaged', 'брак или повреждено'],
  ['other', 'другое'],
]

export const reasonLabel = (r: string | null | undefined): string => REASONS.find(([k]) => k === r)?.[1] ?? ''

export interface AcceptanceLine {
  line_no: number
  product_id: string
  name: string
  article: string
  barcodes: string[]
  unit: 'piece' | 'ml' | 'g'
  container_ml: number | null
  invoice_qty: number
  /** Сумма строки и новая цена — только с правом «Приход товара». */
  line_cost_tyiyn: number | null
  sale_price_tyiyn: number | null
  accepted_qty: number | null
  accepted_product_id: string | null
  accepted_name: string | null
  accepted_container_ml: number | null
  reason: string | null
  comment: string
}

export interface Acceptance {
  id: string
  status: AcceptanceStatus
  supplier_id: string
  supplier_name: string
  supplier_doc: string
  comment: string
  sent_by_name: string
  sent_at: string
  from_bot: boolean
  draft_id: string | null
  photo_type: string | null
  lines_count: number
  total_tyiyn: number | null
  lines: AcceptanceLine[]
  extras: { name: string; barcode: string; qty: number }[]
  closed_at: string | null
  closed_by_name: string | null
  employee_name: string | null
  reason: string | null
  accept_comment: string
  receipt_id: string | null
  receipt_number: number | null
  receipt_state: 'reversed' | 'corrected' | null
  corrected_id: string | null
  corrected_number: number | null
  has_signature: boolean
}

export interface AcceptanceItem {
  id: string
  supplier_name: string
  supplier_doc: string
  lines_count: number
  total_tyiyn: number | null
  sent_by_name: string
  sent_at: string
  from_bot: boolean
  photo_type: string | null
}

export interface AcceptanceList {
  prices: boolean
  items: AcceptanceItem[]
}

/** Отметка кассира по строке: принято в единицах учёта, другой товар, причина, комментарий. */
export interface LineMark {
  qty: number | null
  /** Другой товар той же единицы учёта (SPEC-39 §3). */
  product?: { id: string; name: string; container_ml: number | null } | null
  reason?: Reason | null
  comment?: string
}

export interface Extra {
  name: string
  barcode: string
  qty: number
}

/** Всё, что отметил кассир, — хранится на устройстве до проведения (SPEC-39 §3). */
export interface Marks {
  lines: Record<number, LineMark>
  extras: Extra[]
  comment: string
}

export const emptyMarks = (): Marks => ({ lines: {}, extras: [], comment: '' })

type Unit = { unit: string; container_ml: number | null }

/** Масло в канистрах: учёт в мл, а накладная — целыми канистрами. */
export function inCans(l: Unit & { invoice_qty: number }): boolean {
  return l.unit === 'ml' && l.container_ml !== null && l.container_ml > 0 && l.invoice_qty % l.container_ml === 0
}

/** «20 шт», «5 кан. по 4 л», «3,5 л», «12 кг». */
export function qtyWords(l: Unit & { invoice_qty: number }, qty: number): string {
  if (l.unit === 'piece') return `${qty} шт`
  if (l.unit === 'g') return formatKg(qty)
  if (inCans(l) && l.container_ml && qty % l.container_ml === 0) return `${qty / l.container_ml} кан. по ${formatLiters(l.container_ml)}`
  return formatLiters(qty)
}

/** Короткая запись количества для «18 из 20»: штуки, канистры или литры, кг — без единицы. */
function bare(l: Unit & { invoice_qty: number }, qty: number): string {
  if (l.unit === 'piece') return String(qty)
  if (inCans(l) && l.container_ml && qty % l.container_ml === 0) return String(qty / l.container_ml)
  return litersInput(qty)
}

/** Как вводится «Принято»: целые (штуки, канистры) или до 3 знаков (литры, кг). */
export function inputKind(l: Unit & { invoice_qty: number }): 'int' | 'decimal' {
  return l.unit === 'piece' || inCans(l) ? 'int' : 'decimal'
}

/** Единица поля «Принято». */
export function inputUnit(l: Unit & { invoice_qty: number }): string {
  if (l.unit === 'piece') return 'шт'
  if (l.unit === 'g') return 'кг'
  return inCans(l) ? 'кан.' : 'л'
}

/** Набранное в поле «Принято» — в единицы учёта; пусто или не число — `null`. */
export function parseAccepted(l: Unit & { invoice_qty: number }, text: string): number | null {
  const t = text.trim()
  if (!t) return null
  if (inputKind(l) === 'int') {
    if (!/^\d{1,7}$/.test(t)) return null
    const n = Number(t)
    return l.unit === 'piece' ? n : n * (l.container_ml ?? 0)
  }
  const v = parseLiters(t)
  return v === null || v < 0 ? null : v
}

/** Единицы учёта — в текст поля «Принято». */
export function acceptedInput(l: Unit & { invoice_qty: number }, qty: number | null): string {
  if (qty === null) return ''
  if (l.unit === 'piece') return String(qty)
  if (inCans(l) && l.container_ml) return qty % l.container_ml === 0 ? String(qty / l.container_ml) : litersInput(qty)
  return litersInput(qty)
}

/** Одно касание сканером: штука или канистра; у литров и кг сканер не прибавляет. */
export function scanStep(l: Unit & { invoice_qty: number }): number | null {
  if (l.unit === 'piece') return 1
  if (l.unit === 'ml' && l.container_ml) return l.container_ml
  return null
}

export type LineState = 'none' | 'ok' | 'less' | 'more' | 'zero'

export function lineState(l: AcceptanceLine, m: LineMark | undefined): LineState {
  if (!m || m.qty === null) return 'none'
  if (m.qty === 0) return 'zero'
  if (m.qty < l.invoice_qty) return 'less'
  if (m.qty > l.invoice_qty) return 'more'
  return m.product ? 'less' : 'ok'
}

/** Подпись расхождения строки: «18 из 20», «+2 сверх накладной», «0 — не привезли». */
export function diffText(l: AcceptanceLine, m: LineMark | undefined): string {
  if (!m || m.qty === null) return ''
  if (m.qty === 0) return `0 — ${reasonLabel(m.reason) || 'не привезли'}`
  if (m.qty < l.invoice_qty) return `${bare(l, m.qty)} из ${bare(l, l.invoice_qty)}`
  if (m.qty > l.invoice_qty) return `+${bare(l, m.qty - l.invoice_qty)} сверх накладной`
  return ''
}

/** Что не даёт принять: строки без отметки, без причины расхождения, ничего не принято. */
export function acceptProblems(lines: AcceptanceLine[], marks: Marks): string[] {
  const out: string[] = []
  const unmarked = lines.filter((l) => (marks.lines[l.line_no]?.qty ?? null) === null)
  if (unmarked.length) out.push(`отметьте все строки: не отмечено ${unmarked.length}`)
  for (const l of lines) {
    const m = marks.lines[l.line_no]
    if (m && m.qty !== null && m.qty < l.invoice_qty && !m.reason) out.push(`${l.name}: принято меньше — выберите причину`)
  }
  if (unmarked.length === 0 && lines.every((l) => (marks.lines[l.line_no]?.qty ?? 0) === 0)) {
    out.push('ничего не принято — если товар не привезли, «Отказ от приёмки»')
  }
  return out
}

/** Строки, где приняли больше накладной, — для подтверждения. */
export function overLines(lines: AcceptanceLine[], marks: Marks): string[] {
  return lines
    .filter((l) => (marks.lines[l.line_no]?.qty ?? 0) > l.invoice_qty)
    .map((l) => `${l.name} — ${bare(l, marks.lines[l.line_no]?.qty ?? 0)} из ${bare(l, l.invoice_qty)}`)
}

/** Строк с расхождением: другое количество или другой товар. */
export function diffCount(lines: AcceptanceLine[], marks: Marks): number {
  return lines.filter((l) => {
    const m = marks.lines[l.line_no]
    return m && m.qty !== null && (m.qty !== l.invoice_qty || Boolean(m.product))
  }).length
}

/** Строка накладной по коду со сканера; нет — `null`. */
export function lineByCode(lines: AcceptanceLine[], code: string): AcceptanceLine | null {
  const c = code.trim()
  if (!c) return null
  return lines.find((l) => l.barcodes.includes(c) || (l.article !== '' && l.article === c)) ?? null
}

/** Тело `POST /acceptances/{id}/accept` без кассира, PIN и подписи. */
export function acceptBody(lines: AcceptanceLine[], marks: Marks) {
  return {
    lines: lines.map((l) => {
      const m = marks.lines[l.line_no]
      const qty = m?.qty ?? 0
      return {
        line_no: l.line_no,
        accepted_qty: qty,
        ...(m?.product && m.product.id !== l.product_id ? { product_id: m.product.id } : {}),
        ...(qty < l.invoice_qty && m?.reason ? { reason: m.reason } : {}),
        comment: (m?.comment ?? '').trim(),
      }
    }),
    extras: marks.extras.map((x) => ({ name: x.name.trim(), barcode: x.barcode.trim(), qty: x.qty })),
    comment: marks.comment.trim(),
  }
}

// Отметки на устройстве (SPEC-39 §3): обрыв связи и перезагрузка их не теряют.
const KEY = 'avtodom.acceptance.'

export function loadMarks(id: string): Marks {
  try {
    const raw = localStorage.getItem(KEY + id)
    if (!raw) return emptyMarks()
    const v = JSON.parse(raw) as Partial<Marks>
    return {
      lines: v.lines && typeof v.lines === 'object' ? v.lines : {},
      extras: Array.isArray(v.extras) ? v.extras : [],
      comment: typeof v.comment === 'string' ? v.comment : '',
    }
  } catch {
    return emptyMarks()
  }
}

export function saveMarks(id: string, marks: Marks): void {
  try {
    localStorage.setItem(KEY + id, JSON.stringify(marks))
  } catch {
    // Без хранилища отметки живут до перезагрузки.
  }
}

export function clearMarks(id: string): void {
  try {
    localStorage.removeItem(KEY + id)
  } catch {
    // Нечего убирать.
  }
}

/** Отметки приёмок, которых больше нет в списке ждущих, — убрать с устройства. */
export function dropStaleMarks(waiting: string[]): void {
  try {
    const keep = new Set(waiting)
    const stale: string[] = []
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i)
      if (k?.startsWith(KEY) && !keep.has(k.slice(KEY.length))) stale.push(k)
    }
    stale.forEach((k) => localStorage.removeItem(k))
  } catch {
    // Нечего убирать.
  }
}
