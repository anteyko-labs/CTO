// Товары учёта и строки старой базы Умаг (SPEC-35, ADR-110): коды строки, как их считает сервер,
// путь скана там, где Умаг виден («Товары», «Приход», ревизия, «Сканер товаров»), и запросы
// связывания. На кассе Умага нет: код Умага связанного товара — код самого товара.
import { ApiError, get, getPage, newOpId, post, qs, type Page } from './api'
import type { OldCatalogHit, Product, UmagAutoLink, UmagConflict, UmagLinkResult, UmagPair, UmagStats } from './types'

/** Артикул — код, если в нём только цифры, латиница и дефис и знаков не меньше четырёх. */
const CODE_LIKE = /^[0-9A-Za-z-]+$/

/**
 * Коды строки Умага, как на сервере: штрихкод, артикул (если похож на код) и каждый «доп. код»
 * (через «;», «,» или пробел). Пустые и повторы выкидываются, порядок — как в строке.
 */
export function umagCodes(row: { barcode: string; article: string; extra_code?: string }): string[] {
  const out: string[] = []
  const add = (c: string) => {
    const t = c.trim()
    if (t && !out.includes(t)) out.push(t)
  }
  add(row.barcode)
  const article = row.article.trim()
  if (article.length >= 4 && CODE_LIKE.test(article)) add(article)
  for (const c of (row.extra_code ?? '').split(/[;,\s]+/)) add(c)
  return out
}

/** Коды найденной строки: от сервера, у прежнего ответа — штрихкод и артикул-код. */
export const hitCodes = (h: Pick<OldCatalogHit, 'barcode' | 'article' | 'codes'>): string[] => h.codes ?? umagCodes(h)

const DIGITS = /^\d+$/

/**
 * Тот же код: совпал как есть или, у чисто числовых, без нулей впереди — Excel их теряет
 * (как `GET /products/by-barcode` на сервере, SPEC-35 §1).
 */
export function sameCode(a: string, b: string): boolean {
  const x = a.trim()
  const y = b.trim()
  if (x === '' || y === '') return false
  if (x === y) return true
  if (!DIGITS.test(x) || !DIGITS.test(y)) return false
  const nx = x.replace(/^0+/, '')
  return nx !== '' && nx === y.replace(/^0+/, '')
}

/** Коды строки коротко: первые три и сколько ещё. */
export function codesShort(h: Pick<OldCatalogHit, 'barcode' | 'article' | 'codes'>): string {
  const all = hitCodes(h)
  if (all.length === 0) return ''
  const head = all.slice(0, 3).join(', ')
  return `${all.length > 1 ? 'коды' : 'код'} ${head}${all.length > 3 ? ` и ещё ${all.length - 3}` : ''}`
}

/** Текст без права «Товары и штрихкоды» (SPEC-35 §2). */
export const UMAG_NO_RIGHT = 'Товар есть в базе Умаг, но не заведён: попросите владельца или администратора завести его'

/** Строку ещё не завели: она не связана с товаром и ни один её код не у товара. */
export const unlinkedHit = (h: OldCatalogHit): boolean => !h.product_id && !h.linked_product_id

/** Что делать с кодом, которого нет у товаров, по ответу поиска Умага. */
export type UmagStep =
  /** Строка связана — берём её товар. */
  | { kind: 'linked'; productId: string; hit: OldCatalogHit }
  /** Строка не связана — окно «Товар из старой базы Умаг». */
  | { kind: 'umag'; hit: OldCatalogHit }
  /** Нет и в Умаге — прежнее «Код не найден». */
  | { kind: 'none' }

/** Точное совпадение кода: связанная строка важнее несвязанной (одинаковый код у двух строк). */
export function umagStep(hits: readonly OldCatalogHit[]): UmagStep {
  const exact = hits.filter((h) => h.exact)
  const linked = exact.find((h) => h.product_id)
  if (linked?.product_id) return { kind: 'linked', productId: linked.product_id, hit: linked }
  const first = exact[0]
  return first ? { kind: 'umag', hit: first } : { kind: 'none' }
}

/** Чем кончился поиск кода там, где виден Умаг. */
export type CodeOutcome =
  /** Товар учёта: по своему коду (`code`) или через связанную строку Умага (`umag`). */
  | { kind: 'product'; product: Product; via: 'code' | 'umag' }
  | { kind: 'umag'; hit: OldCatalogHit }
  | { kind: 'unknown' }

export interface CodeLookup {
  byBarcode: (code: string) => Promise<Product>
  searchUmag: (code: string) => Promise<OldCatalogHit[]>
  productById: (id: string) => Promise<Product>
}

const serverLookup: CodeLookup = {
  byBarcode: (code) => get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`),
  // Список Умага — подсказка: сбой поиска в нём не мешает, код просто «не найден».
  searchUmag: (code) => searchUmag({ barcode: code }).catch(() => []),
  productById: (id) => get<Product>(`/products/${id}`),
}

/**
 * Скан или ввод кода (SPEC-35 §2): код товара (и без потерянных нулей) → товар; нет (404) — точная
 * строка Умага: связана — её товар, не связана — окно Умага; нигде — «Код не найден». Ничего не
 * записывает. Прочие ошибки — наружу: без сети вызывающий ищет в снимке каталога.
 */
export async function resolveCode(code: string, api: CodeLookup = serverLookup): Promise<CodeOutcome> {
  try {
    return { kind: 'product', product: await api.byBarcode(code), via: 'code' }
  } catch (e) {
    if (!(e instanceof ApiError && e.status === 404)) throw e
  }
  const step = umagStep(await api.searchUmag(code))
  if (step.kind === 'linked') return { kind: 'product', product: await api.productById(step.productId), via: 'umag' }
  if (step.kind === 'umag') return { kind: 'umag', hit: step.hit }
  return { kind: 'unknown' }
}

/** Итог связывания словами: что перенесено и какие коды остались у других товаров. */
export function linkSummary(r: Pick<UmagLinkResult, 'copied' | 'conflicts'>): string {
  const parts = [r.copied.length > 0 ? `коды перенесены: ${r.copied.join(', ')}` : 'новых кодов нет']
  if (r.conflicts.length > 0) parts.push(`не перенесены — ${conflictText(r.conflicts)}`)
  return parts.join('; ')
}

/** «код 123 — у «Масло Х»» для отметки конфликта. */
export const conflictText = (list: readonly UmagConflict[]): string =>
  list.map((c) => `код ${c.code} — у «${c.product_name}»`).join(', ')

/** Сходство названий для показа: 0,734 → «73 %». */
export const similarityText = (s: number): string => `${Math.round(Math.min(1, Math.max(0, s)) * 100)} %`

/**
 * Номер операции на попытку: повтор того же действия после обрыва уходит с тем же `op_id`,
 * и сервер вернёт прежний ответ (инвариант 4). После ответа ключ забывается.
 */
export function opIds() {
  const map = new Map<string, string>()
  return {
    get(key: string): string {
      const known = map.get(key)
      if (known) return known
      const id = newOpId()
      map.set(key, id)
      return id
    },
    done(key: string): void {
      map.delete(key)
    },
  }
}

// ---------- Запросы ----------

export const searchUmag = (p: { barcode?: string; q?: string }) => get<OldCatalogHit[]>(`/old-catalog/search${qs(p)}`)

export const umagStats = () => get<UmagStats>('/old-catalog')

export const linkUmag = (rowId: string, productId: string, opId: string) =>
  post<UmagLinkResult>(`/old-catalog/${rowId}/link`, { op_id: opId, product_id: productId })

export const unlinkUmag = (rowId: string, opId: string) => post<{ ok: boolean }>(`/old-catalog/${rowId}/unlink`, { op_id: opId })

export const autoLinkUmag = (opId: string) => post<UmagAutoLink>('/old-catalog/auto-link', { op_id: opId })

export const rejectUmagPair = (rowId: string, productId: string, opId: string) =>
  post<{ ok: boolean }>(`/old-catalog/${rowId}/reject`, { op_id: opId, product_id: productId })

/** Похожие пары страницей: номер страницы с нуля; число пар — из `X-Total-Count`. */
export const umagPairs = (q: string, page: number, size: number): Promise<Page<UmagPair>> =>
  getPage<UmagPair>(`/old-catalog/pairs${qs({ q: q.trim(), page, size })}`, { limit: size, offset: page * size })
