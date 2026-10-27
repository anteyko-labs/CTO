// «Следующая замена» в чеке «В сервис» (SPEC-34 §2): по строке на каждую услугу чека со строкой
// книжки. Поле заполнено заранее — пробег сейчас + интервал (у машины свой — он, иначе услуги);
// кассир меняет число или очищает (пустое — не напоминать по этой строке). «+ ещё строку» — только
// позиции этого чека (уточнение заказчика 26.10.2026): товары со строкой книжки, а не справочник услуг.
// Масляный, воздушный и салонный фильтры при строке масла двигателя своих строк не дают — они на
// пробег масла (ADR-117).
import { productTitle } from './battery'
import { formatLiters } from './format'
import { fromKm, type MileageUnit } from './mileage'
import { isFluid } from './oilBook'
import { BOOK_SLOT_LABELS, type BookSlot, type Product, type SaleBook, type Service } from './types'

export interface NextRow {
  /** Строка книжки: одна строка «Следующей замены» на строку книжки. */
  slot: BookSlot
  /** Услуга чека по этой строке; у строки по товару — нет: уходит строкой книжки. */
  service_id: string | null
  label: string
  /** Добавлена кнопкой «+ ещё строку» по товару чека: убирается «✕». */
  extra: boolean
  /** Что в поле: правка кассира или заполненное заранее. */
  value: string
  prefill: string
  interval_days: number | null
  /** У строки масла двигателя — фильтры чека, которые идут на тот же пробег: «воздушный». */
  filters?: string[]
}

/** Фильтры, которые меняют вместе с маслом двигателя (ADR-117); топливный — по своему сроку. */
const WITH_OIL: Partial<Record<BookSlot, string>> = { oil_filter: 'масляный', air_filter: 'воздушный', cabin_filter: 'салонный' }
const followsOil = (slot: BookSlot): boolean => WITH_OIL[slot] !== undefined

/** «и фильтры: воздушный, салонный — на тот же пробег» под строкой масла; фильтров нет — ''. */
export function filtersNote(r: Pick<NextRow, 'filters'>): string {
  return r.filters?.length ? `и ${r.filters.length > 1 ? 'фильтры' : 'фильтр'}: ${r.filters.join(', ')} — на тот же пробег` : ''
}

/** Интервал строки у машины (SPEC-27): км и дни. */
export type OwnIntervals = Partial<Record<BookSlot, { km: number | null; days: number | null }>>

/** Категория товара для книжки: строка и интервал, заданный владельцем (`GET /categories`, снимок
 *  кассы); строку проверяет `productSlot`. */
export interface CategoryBook {
  id: string
  book_slot: string
  book_interval_km: number | null
  book_interval_days: number | null
}

/** Позиция чека со строкой книжки: услуга или товар. */
export interface NextSource {
  slot: BookSlot
  /** Услуга — строка «Следующей замены» по ней встаёт сама; у товара — null. */
  service_id: string | null
  /** Услуга — её названием; товар — строкой книжки и товаром: «Фильтр воздушный (MANN C-110)». */
  label: string
  /** Интервал: у услуги или категории товара, иначе по умолчанию строки (у машины свой — в `nextRows`). */
  interval_km: number | null
  interval_days: number | null
}

/** Строка чека: товар или услуга; подарок в книжку не идёт (как на сервере). */
export interface CartPosition {
  product?: Product
  service?: Service
  gift?: boolean
  /** Вид строки и количество, как набрал кассир, — для «что залито» (ADR-116). */
  kind?: string
  qtyText?: string
}

/** Товар чека со строкой книжки: что и сколько залили. */
export interface CartGood {
  slot: BookSlot
  name: string
  kind: string
  qty: string
  containerMl: number | null
}

const SLOTS = new Set<string>(Object.keys(BOOK_SLOT_LABELS))

/** Интервал строки по умолчанию, км и дни, — как `oil_book::slot_default` на сервере. */
const SLOT_DEFAULT: Partial<Record<BookSlot, [number, number]>> = {
  oil: [8000, 31],
  oil_filter: [8000, 31],
  gear_oil: [40_000, 730],
  air_filter: [15_000, 365],
  cabin_filter: [15_000, 365],
  fuel_filter: [30_000, 730],
  antifreeze: [60_000, 730],
  brake_fluid: [40_000, 730],
}

/** Тип фильтра товара → строка книжки, как в `oil_book::product_slot`. */
const FILTER_SLOT = new Map<string, BookSlot>([
  ['масляный', 'oil_filter'],
  ['воздушный', 'air_filter'],
  ['салонный', 'cabin_filter'],
  ['топливный', 'fuel_filter'],
])

const serviceSlot = (s: Service): BookSlot | null => (s.book_slot && s.book_slot !== 'other' ? s.book_slot : null)

/**
 * Строка книжки товара — как на сервере (`oil_book::product_slot`): по категории, а у фильтров —
 * по «Типу фильтра» товара (одна категория «Фильтры» на все виды). Категория не в книжку — null.
 */
export function productSlot(categorySlot: string | null | undefined, filterType: string | null | undefined): BookSlot | null {
  if (!categorySlot || !SLOTS.has(categorySlot)) return null
  const slot = categorySlot as BookSlot
  if (slot.endsWith('_filter')) return FILTER_SLOT.get(filterType?.trim() ?? '') ?? slot
  return slot
}

/** Категории из снимка кассы — без связи. У снимка до 26.10.2026 строк книжки нет: null, как будто
 *  категории не загружены (товары не предлагаются, черновик не трогается). */
export function snapshotCategories(
  list: readonly { id: string; book_slot?: string; book_interval_km?: number | null; book_interval_days?: number | null }[] | undefined,
): CategoryBook[] | null {
  if (!list) return null
  const out: CategoryBook[] = []
  for (const c of list) {
    if (c.book_slot === undefined) return null
    out.push({ id: c.id, book_slot: c.book_slot, book_interval_km: c.book_interval_km ?? null, book_interval_days: c.book_interval_days ?? null })
  }
  return out
}

/** Интервал категории для строки книжки, км и дни, — как на сервере (`oil_book::category_interval`):
 *  категории товара, если её строка та же; иначе любой категории с этой строкой. У фильтра строку
 *  решает тип: интервал общей категории «Фильтры» (строка — масляный фильтр) воздушному не подходит. */
function categoryInterval(categories: readonly CategoryBook[], slot: BookSlot, own: CategoryBook | undefined): [number, number] | null {
  const of = (c: CategoryBook | undefined): [number, number] | null =>
    c && c.book_slot === slot && c.book_interval_km != null && c.book_interval_days != null ? [c.book_interval_km, c.book_interval_days] : null
  return of(own) ?? categories.map(of).find((i) => i !== null) ?? null
}

/**
 * Позиции чека со строкой книжки (не «прочее») по порядку строк: услуги и товары. Подарок книжка не
 * пишет; пока категории не загружены (`null`), товары не считаются.
 */
export function cartSources(lines: readonly CartPosition[], categories: readonly CategoryBook[] | null): NextSource[] {
  const out: NextSource[] = []
  for (const l of lines) {
    if (l.service) {
      const slot = serviceSlot(l.service)
      if (slot) out.push({ slot, service_id: l.service.id, label: l.service.name, interval_km: l.service.interval_km ?? null, interval_days: l.service.interval_days ?? null })
      continue
    }
    const p = l.product
    if (!p || l.gift) continue
    const cat = categories?.find((c) => c.id === p.category_id)
    const slot = productSlot(cat?.book_slot, p.attrs?.filter_type)
    if (!cat || !slot || slot === 'other') continue
    // Интервал категории этой строки, иначе строки по умолчанию; дни — те же, что возьмёт сервер.
    const [km, days] = categoryInterval(categories ?? [], slot, cat) ?? SLOT_DEFAULT[slot] ?? [null, null]
    out.push({ slot, service_id: null, label: `${BOOK_SLOT_LABELS[slot]} (${productTitle(p)})`, interval_km: km, interval_days: days })
  }
  return out
}

/** Товары чека со строкой книжки (не «прочее») по порядку строк; подарок книжка не пишет. */
export function cartGoods(lines: readonly CartPosition[], categories: readonly CategoryBook[] | null): CartGood[] {
  const out: CartGood[] = []
  for (const l of lines) {
    const p = l.product
    if (!p || l.gift) continue
    const slot = productSlot(categories?.find((c) => c.id === p.category_id)?.book_slot, p.attrs?.filter_type)
    if (!slot || slot === 'other') continue
    out.push({ slot, name: productTitle(p), kind: l.kind ?? 'piece', qty: (l.qtyText ?? '1').trim(), containerMl: p.container_ml ?? null })
  }
  return out
}

/** Товар на кассе под строкой «Следующей замены»: «Sintec G12 × 2», «Totachi × 1 кан.», «ATF × 1,5 л». */
export function goodShort(g: CartGood): string {
  const qty = g.kind === 'container' ? `${g.qty} кан.` : g.kind === 'pour' ? `${g.qty} л` : g.qty
  return `${g.name} × ${qty}`
}

/** Товар в чеке книжки — как запись книжки на сервере: «Sintec G12 · 2 шт», «Totachi · 1 кан. × 4 л». */
export function goodBook(g: CartGood): string {
  const liters = (ml: number) => formatLiters(ml).replace(/ /g, ' ')
  const qty =
    g.kind === 'container' && g.containerMl ? `${g.qty} кан. × ${liters(g.containerMl)}` : g.kind === 'pour' ? `${g.qty} л` : `${g.qty} шт`
  return `${g.name} · ${qty}`
}

/**
 * «Это замена?» (ADR-117): в чеке «В сервис» моторного масла не меньше канистры его категории
 * (`enough` — категории масла с канистрой), а услуги масла двигателя нет — без неё это доливка.
 */
export function oilChangeHint(lines: readonly CartPosition[], categories: readonly CategoryBook[] | null, enough: ReadonlySet<string>): boolean {
  if (lines.some((l) => l.service && serviceSlot(l.service) === 'oil')) return false
  return [...enough].some((id) => categories?.find((c) => c.id === id)?.book_slot === 'oil')
}

export function nextRows(args: {
  /** Позиции чека со строкой книжки (`cartSources`). */
  sources: readonly NextSource[]
  /** Строки книжки, добавленные «+ ещё строку». */
  extra: readonly BookSlot[]
  /** Правки кассира по строке книжки; '' — очистил. */
  edits: Partial<Record<BookSlot, string>>
  mileage: string
  unit: MileageUnit
  own: OwnIntervals
}): NextRow[] {
  const out: NextRow[] = []
  const add = (s: NextSource, extra: boolean) => {
    if (out.some((r) => r.slot === s.slot)) return
    const km = args.own[s.slot]?.km ?? s.interval_km
    const now = args.mileage ? Number(args.mileage) : null
    const prefill = now !== null && Number.isFinite(now) && km ? String(now + fromKm(km, args.unit)) : ''
    const edit = args.edits[s.slot]
    out.push({
      slot: s.slot,
      service_id: s.service_id,
      label: s.label,
      extra,
      value: edit ?? prefill,
      prefill,
      interval_days: args.own[s.slot]?.days ?? s.interval_days,
    })
  }
  // Услуги чека — строками сами; товары — только добавленные «+ ещё строку», пока они в чеке.
  for (const s of args.sources) if (s.service_id) add(s, false)
  for (const slot of args.extra) {
    const s = args.sources.find((x) => x.slot === slot)
    if (s) add(s, true)
  }
  // Есть строка масла двигателя — масляный, воздушный и салонный фильтры чека своих строк не дают:
  // сервер ставит им цель масла (ADR-117). Под строкой масла — какие фильтры с ней.
  const oil = out.find((r) => r.slot === 'oil')
  if (!oil) return out
  const filters = [...new Set(args.sources.map((s) => s.slot).filter(followsOil))]
  if (filters.length) oil.filters = filters.map((s) => WITH_OIL[s] ?? s)
  return out.filter((r) => !followsOil(r.slot))
}

/** Строка, где следующая замена меньше пробега сейчас: «Провести» нельзя. */
export function nextTooLow(rows: NextRow[], mileage: string): NextRow | null {
  if (!mileage) return null
  const now = Number(mileage)
  return rows.find((r) => r.value !== '' && Number(r.value) < now) ?? null
}

/** Следующая замена в теле чека: по услуге или строкой книжки товара чека (без услуги). */
export type NextChange = { service_id?: string; slot: BookSlot; next_mileage: number | null }

/** Что уходит с чеком: пробег в единице чека; пустое поле — не напоминать. Строка по товару —
 *  без услуги: сервер ставит цель на строку замены этого товара. */
export function nextPayload(rows: NextRow[]): NextChange[] {
  return rows.map((r) => {
    const next_mileage = r.value ? Number(r.value) : null
    return r.service_id ? { service_id: r.service_id, slot: r.slot, next_mileage } : { slot: r.slot, next_mileage }
  })
}

/**
 * «+ ещё строку»: позиции этого чека, чьей строки книжки ещё нет в блоке, — по одной на строку
 * (первая в чеке: на неё сервер и поставит цель). Справочник услуг не предлагается.
 */
export function extraChoices(sources: readonly NextSource[], rows: readonly NextRow[]): NextSource[] {
  const used = new Set<BookSlot>(rows.map((r) => r.slot))
  // Фильтры, которые идут с маслом, при строке масла не предлагаются (ADR-117).
  const withOil = used.has('oil')
  return sources.filter((s) => {
    if (used.has(s.slot) || (withOil && followsOil(s.slot))) return false
    used.add(s.slot)
    return true
  })
}

/**
 * Убрали товар или услугу из чека — строка «Следующей замены», добавленная по ним, и правка кассира
 * по их строке книжки уходят (из черновика и отложенного чека тоже). Ничего не ушло — null.
 */
export function pruneNext(
  extra: readonly BookSlot[],
  edits: Partial<Record<BookSlot, string>>,
  sources: readonly NextSource[],
): { extra: BookSlot[]; edits: Partial<Record<BookSlot, string>> } | null {
  const have = new Set<string>(sources.map((s) => s.slot))
  const keptExtra = extra.filter((s) => have.has(s))
  const keptEdits = Object.fromEntries(Object.entries(edits).filter(([s]) => have.has(s))) as Partial<Record<BookSlot, string>>
  if (keptExtra.length === extra.length && Object.keys(keptEdits).length === Object.keys(edits).length) return null
  return { extra: keptExtra, edits: keptEdits }
}

/** Строки «+ ещё» из черновика или отложенного чека; прежние (услуги из справочника) не берутся. */
export function extraSlots(raw: unknown): BookSlot[] {
  return Array.isArray(raw) ? raw.filter((x): x is BookSlot => typeof x === 'string' && SLOTS.has(x)) : []
}

const addDays = (iso: string, days: number): string => {
  const d = new Date(`${iso}T00:00:00Z`)
  d.setUTCDate(d.getUTCDate() + days)
  return d.toISOString().slice(0, 10)
}

/** Книжка для печатного чека, пока чек в очереди без сети (SPEC-34 §4): как отдаст сервер — только
 *  жидкости строк «Следующей замены» (ADR-116): что залито — товары чека этой строки, без товара —
 *  своё у клиента; следующая замена — непустые строки. */
export function receiptBook(
  car: { brand: string; model: string; plate: string },
  mileage: string,
  unit: MileageUnit,
  rows: NextRow[],
  today: string,
  goods: readonly CartGood[] = [],
): SaleBook {
  const fluids = rows.filter((r) => isFluid(r.slot))
  return {
    brand: car.brand,
    model: car.model,
    plate: car.plate,
    mileage: mileage ? Number(mileage) : null,
    mileage_unit: unit,
    lines: fluids
      .filter((r) => r.value !== '')
      .map((r) => ({
        label: BOOK_SLOT_LABELS[r.slot],
        next_mileage: Number(r.value),
        unit,
        next_date: r.interval_days ? addDays(today, r.interval_days) : null,
        slot: r.slot,
      })),
    filled: fluids.flatMap((r) => {
      const label = BOOK_SLOT_LABELS[r.slot]
      const own = goods.filter((g) => g.slot === r.slot)
      if (own.length) return own.map((g) => ({ slot: r.slot, label, product: goodBook(g) }))
      return [{ slot: r.slot, label, product: r.slot === 'oil' || r.slot === 'gear_oil' ? 'масло клиента' : 'жидкость клиента' }]
    }),
  }
}
