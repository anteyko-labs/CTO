// «Следующая замена» в чеке «В сервис» (SPEC-34 §2, ADR-131): по строке на каждую услугу чека со
// строкой книжки (не «прочее», не доливка на вынос). Поле заполнено заранее — пробег сейчас +
// интервал (у машины свой — он, иначе услуги); кассир меняет число или очищает (пустое — не
// напоминать по этой строке). «+ ещё строку» — только услуги «прочее» этого чека, у которых строки ещё
// нет («Замена ГУР»); товары не предлагаются никогда. Масляный, воздушный и салонный фильтры при
// строке масла двигателя своих строк не дают — они на пробег масла (ADR-117).
import { productTitle } from './battery'
import { formatLiters } from './format'
import { fromKm, type MileageUnit } from './mileage'
import { isFluid } from './oilBook'
import { BOOK_SLOT_LABELS, type BookSlot, type Product, type SaleBook, type Service } from './types'

/** Строка «Следующей замены»: строка книжки, у услуги «прочее» — `svc:<услуга>` (ADR-131). */
export type NextKey = BookSlot | `svc:${string}`

/** Ключ строки услуги «прочее». */
export const serviceKey = (id: string): NextKey => `svc:${id}`

export interface NextRow {
  /** Одна строка «Следующей замены» на строку книжки; у «прочего» — на услугу. */
  key: NextKey
  slot: BookSlot
  /** Услуга чека по этой строке — с ней строка уходит с чеком. */
  service_id: string
  /** Название услуги. */
  label: string
  /** Добавлена кнопкой «+ ещё строку» (услуга «прочее»): убирается «✕». */
  extra: boolean
  /** Что в поле: правка кассира или заполненное заранее. */
  value: string
  prefill: string
  /** Кассир правил поле (и очищенное — тоже правка: «не напоминать»). */
  edited: boolean
  interval_days: number | null
  /** У строки масла двигателя — фильтры чека, которые идут на тот же пробег: «воздушный». */
  filters?: string[]
}

/** Фильтры, которые меняют вместе с маслом двигателя (ADR-117); топливный — по своему сроку. */
const WITH_OIL: Partial<Record<BookSlot, string>> = { oil_filter: 'масляный', air_filter: 'воздушный', cabin_filter: 'салонный' }
const followsOil = (slot: BookSlot): boolean => WITH_OIL[slot] !== undefined

/** «и фильтры: воздушный, салонный на тот же пробег» — под строкой масла после «Масло двигателя —»;
 *  фильтров нет — ''. */
export function filtersNote(r: Pick<NextRow, 'filters'>): string {
  return r.filters?.length ? `и ${r.filters.length > 1 ? 'фильтры' : 'фильтр'}: ${r.filters.join(', ')} на тот же пробег` : ''
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

/** Услуга чека со строкой книжки — строка «Следующей замены» по ней; «прочее» — только «+ ещё». */
export interface NextSource {
  slot: BookSlot
  service_id: string
  /** Название услуги. */
  label: string
  /** Интервал услуги: заданный или по умолчанию строки (у машины свой — в `nextRows`). */
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

/** Тип фильтра товара → строка книжки, как в `oil_book::product_slot`. */
const FILTER_SLOT = new Map<string, BookSlot>([
  ['масляный', 'oil_filter'],
  ['воздушный', 'air_filter'],
  ['салонный', 'cabin_filter'],
  ['топливный', 'fuel_filter'],
])

/** Строка книжки услуги, по которой в чеке замена: не «прочее» и не доливка на вынос (ADR-118). */
const serviceSlot = (s: Service): BookSlot | null => (s.book_slot && s.book_slot !== 'other' && !s.takeaway ? s.book_slot : null)

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

/** Класс жидкости в названии (ADR-131) — как `oil_book::has_class_token`: слово с начала слова, после
 *  него через пробелы или дефис — число, за которым не идёт цифра. «g-force», «g 10w-40», «sg12»,
 *  «g120» — не класс. */
const ANTIFREEZE_CLASS = /(?:^|[^\p{Alphabetic}\p{N}])g[\s-]*1[123](?!\d)/u
const BRAKE_CLASS = /(?:^|[^\p{Alphabetic}\p{N}])(?:dot|дот|rosdot|росдот)[\s-]*[345](?!\d)/u

/** Строка жидкости по названию товара (ADR-130, ADR-131) — как `oil_book::slot_by_product_name`:
 *  «антифриз», «тосол», «охлажда…» и G11 / G12 / G12+ / G13 — антифриз; «тормозн…» и DOT 3 / 4 / 5
 *  (5.1, «ROSDOT 4», «ДОТ-4») — тормозная жидкость; иначе null. */
export function nameSlot(name: string): BookSlot | null {
  const n = name.toLowerCase()
  if (['антифриз', 'тосол', 'охлажда'].some((w) => n.includes(w)) || ANTIFREEZE_CLASS.test(n)) return 'antifreeze'
  if (n.includes('тормозн') || BRAKE_CLASS.test(n)) return 'brake_fluid'
  return null
}

/** Строка книжки товара чека: по категории (`productSlot`), а у категории без строки (не масла, не
 *  фильтра) — по названию товара (ADR-130). Категории нет (не загружены) — null. */
export function goodSlot(category: Pick<CategoryBook, 'book_slot'> | undefined, p: Pick<Product, 'name' | 'attrs'>): BookSlot | null {
  if (!category) return null
  if (category.book_slot === 'none') return nameSlot(p.name)
  return productSlot(category.book_slot, p.attrs?.filter_type)
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

/**
 * Услуги чека со строкой книжки по порядку строк, каждая один раз: строки книжки и «прочее» (его —
 * только «+ ещё»). Доливка на вынос — не замена (ADR-118), товары — никогда (ADR-131).
 */
export function cartSources(lines: readonly CartPosition[]): NextSource[] {
  const out: NextSource[] = []
  for (const l of lines) {
    const s = l.service
    if (!s || s.takeaway || !s.book_slot || out.some((x) => x.service_id === s.id)) continue
    out.push({ slot: s.book_slot, service_id: s.id, label: s.name, interval_km: s.interval_km ?? null, interval_days: s.interval_days ?? null })
  }
  return out
}

/** Товары чека со строкой книжки (не «прочее») по порядку строк; подарок книжка не пишет. */
export function cartGoods(lines: readonly CartPosition[], categories: readonly CategoryBook[] | null): CartGood[] {
  const out: CartGood[] = []
  for (const l of lines) {
    const p = l.product
    if (!p || l.gift) continue
    const slot = goodSlot(categories?.find((c) => c.id === p.category_id), p)
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

/** Правки кассира по строкам «Следующей замены» (`NextKey`); '' — очистил: «не напоминать». */
export type NextEdits = Partial<Record<string, string>>

export function nextRows(args: {
  /** Услуги чека со строкой книжки (`cartSources`). */
  sources: readonly NextSource[]
  /** Товары чека со строкой книжки (`cartGoods`): какие фильтры идут с маслом. */
  goods?: readonly Pick<CartGood, 'slot'>[]
  /** Услуги «прочее», добавленные «+ ещё строку». */
  extra: readonly string[]
  edits: NextEdits
  mileage: string
  unit: MileageUnit
  own: OwnIntervals
}): NextRow[] {
  const out: NextRow[] = []
  const now = args.mileage ? Number(args.mileage) : null
  const add = (key: NextKey, s: NextSource, extra: boolean, km: number | null, days: number | null) => {
    const prefill = now !== null && Number.isFinite(now) && km ? String(now + fromKm(km, args.unit)) : ''
    const edit = args.edits[key]
    out.push({ key, slot: s.slot, service_id: s.service_id, label: s.label, extra, value: edit ?? prefill, prefill, edited: edit !== undefined, interval_days: days })
  }
  // Услуги со строкой книжки — строками сами, по строке на строку книжки (первая услуга в чеке).
  for (const s of args.sources) {
    if (s.slot === 'other' || out.some((r) => r.key === s.slot)) continue
    add(s.slot, s, false, args.own[s.slot]?.km ?? s.interval_km, args.own[s.slot]?.days ?? s.interval_days)
  }
  // «Прочее» — только добавленное «+ ещё», пока услуга в чеке: пробег + интервал услуги, если его
  // задал владелец, иначе пусто (ADR-131).
  for (const id of args.extra) {
    const s = args.sources.find((x) => x.service_id === id && x.slot === 'other')
    if (s && !out.some((r) => r.key === serviceKey(id))) add(serviceKey(id), s, true, s.interval_km, s.interval_days)
  }
  // Есть строка масла двигателя — масляный, воздушный и салонный фильтры чека своих строк не дают:
  // сервер ставит им цель масла (ADR-117). Под строкой масла — какие фильтры с ней.
  const oil = out.find((r) => r.slot === 'oil')
  if (!oil) return out
  const slots = new Set<BookSlot>([...args.sources.map((s) => s.slot), ...(args.goods ?? []).map((g) => g.slot)])
  const filters = (Object.keys(WITH_OIL) as BookSlot[]).filter((s) => slots.has(s))
  if (filters.length) oil.filters = filters.map((s) => WITH_OIL[s] ?? s)
  return out.filter((r) => !followsOil(r.slot))
}

/** Строка, где следующая замена меньше пробега сейчас: «Провести» нельзя. */
export function nextTooLow(rows: readonly NextRow[], mileage: string): NextRow | null {
  if (!mileage) return null
  const now = Number(mileage)
  return rows.find((r) => r.value !== '' && Number(r.value) < now) ?? null
}

/** Следующая замена дальше пробега сейчас на столько и больше — подсказка «проверьте» (ADR-131). */
export const NEXT_FAR = 200_000

/** Строка, где следующая замена дальше пробега сейчас на `NEXT_FAR` и больше (лишний ноль?), и
 *  на сколько: подсказка, а не запрет. */
export function nextTooFar(rows: readonly NextRow[], mileage: string): { row: NextRow; by: number } | null {
  if (!mileage) return null
  const now = Number(mileage)
  for (const r of rows) {
    const by = r.value === '' ? 0 : Number(r.value) - now
    if (by >= NEXT_FAR) return { row: r, by }
  }
  return null
}

/** Следующая замена в теле чека — по услуге этого чека (ADR-131). */
export type NextChange = { service_id: string; slot: BookSlot; next_mileage: number | null }

/** Что уходит с чеком: пробег в единице чека. Поле очистил кассир — null, «не напоминать»; пусто,
 *  потому что пробег не вписан (касса без сети), или пустая строка «+ ещё» — строки нет: цели нет,
 *  книжка считает срок по интервалу. */
export function nextPayload(rows: readonly NextRow[]): NextChange[] {
  return rows.flatMap((r): NextChange[] => {
    if (r.value !== '') return [{ service_id: r.service_id, slot: r.slot, next_mileage: Number(r.value) }]
    return r.edited && !r.extra ? [{ service_id: r.service_id, slot: r.slot, next_mileage: null }] : []
  })
}

/** «+ ещё строку» (ADR-131): услуги «прочее» этого чека, у которых строки ещё нет. Товары и
 *  справочник услуг не предлагаются; предложить нечего — кнопки нет. */
export function extraChoices(sources: readonly NextSource[], rows: readonly NextRow[]): NextSource[] {
  const used = new Set<string>(rows.map((r) => r.key))
  return sources.filter((s) => s.slot === 'other' && !used.has(serviceKey(s.service_id)))
}

/**
 * Убрали услугу из чека — строка «Следующей замены» по ней («+ ещё» и правка кассира) уходит (из
 * черновика и отложенного чека тоже). Ничего не ушло — null.
 */
export function pruneNext(extra: readonly string[], edits: NextEdits, sources: readonly NextSource[]): { extra: string[]; edits: NextEdits } | null {
  const others = new Set(sources.filter((s) => s.slot === 'other').map((s) => s.service_id))
  const slots = new Set<string>(sources.filter((s) => s.slot !== 'other').map((s) => s.slot))
  const keep = (key: string) => (key.startsWith('svc:') ? others.has(key.slice(4)) : slots.has(key))
  const keptExtra = extra.filter((id) => others.has(id))
  const keptEdits: NextEdits = Object.fromEntries(Object.entries(edits).filter(([k]) => keep(k)))
  if (keptExtra.length === extra.length && Object.keys(keptEdits).length === Object.keys(edits).length) return null
  return { extra: keptExtra, edits: keptEdits }
}

/** Строки «+ ещё» из черновика или отложенного чека — услуги «прочее» (ADR-131). */
export function extraServices(raw: unknown): string[] {
  return Array.isArray(raw) ? raw.filter((x): x is string => typeof x === 'string' && x !== '') : []
}

/** Правки кассира из черновика или отложенного чека: строка → текст поля. */
export function nextEditsOf(raw: unknown): NextEdits {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {}
  return Object.fromEntries(Object.entries(raw).filter((e): e is [string, string] => typeof e[1] === 'string'))
}

/** Следующие замены исходного чека в «Исправить» (ADR-131): строки книжки — правками кассира (пусто —
 *  «не напоминать»), услуги «прочее» — строками «+ ещё» с их пробегом. */
export function nextFromDraft(list: readonly { service_id: string | null; slot: string; next_mileage: number | null }[] | undefined): { edits: NextEdits; extra: string[] } {
  const edits: NextEdits = {}
  const extra: string[] = []
  for (const n of list ?? []) {
    const value = n.next_mileage === null ? '' : String(n.next_mileage)
    if (n.slot === 'other') {
      if (n.service_id && !extra.includes(n.service_id)) {
        extra.push(n.service_id)
        edits[serviceKey(n.service_id)] = value
      }
    } else if (SLOTS.has(n.slot) && edits[n.slot] === undefined) edits[n.slot] = value
  }
  return { edits, extra }
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
