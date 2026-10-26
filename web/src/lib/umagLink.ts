// Товары учёта и строки старой базы Умаг (SPEC-35, ADR-110): коды строки, как их считает сервер,
// путь скана там, где Умаг виден («Товары», «Приход», ревизия, «Сканер товаров»), и запросы
// связывания. На кассе Умага нет: код Умага связанного товара — код самого товара.
import { ApiError, get, getPage, newOpId, post, qs, type Page } from './api'
import type { OldCatalogHit, Product, UmagAutoLink, UmagConflict, UmagLinkResult, UmagPair, UmagStats } from './types'
import { isMeasureCode, productCodes, umagCodes } from './umagCodes'

export { articleIsCode, extraIsCode, isMeasureCode, isMeasureUnit, productCodes, sameCode, umagCodes, umagPer, umagUnitWord } from './umagCodes'

/** Коды найденной строки: от сервера, у прежнего ответа — по тому же правилу на устройстве. */
export const hitCodes = (h: Pick<OldCatalogHit, 'barcode' | 'article' | 'codes'>): string[] => h.codes ?? umagCodes(h)

/** Коды найденной строки, которые перейдут товару: без кодов меры строки «литр» (ADR-113). */
export const hitProductCodes = (h: Pick<OldCatalogHit, 'barcode' | 'article' | 'codes' | 'unit'>): string[] =>
  productCodes(hitCodes(h), h.unit)

/** Коды меры найденной строки: остаются в Умаге. */
export const hitMeasureCodes = (h: Pick<OldCatalogHit, 'barcode' | 'article' | 'codes' | 'unit'>): string[] =>
  hitCodes(h).filter((c) => isMeasureCode(c, h.unit))

/** Без сети код не найден среди товаров (SPEC-35 §2): несвязанную строку Умага не узнать. */
export const offlineCodeText = (code: string): string =>
  `Без сети: код ${code} среди товаров не найден. Если он есть в базе Умаг, завести товар можно, когда будет связь`

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
  /** Товар учёта: по своему коду (`code`), артикулу (`article`) или через связанную строку Умага (`umag`). */
  | { kind: 'product'; product: Product; via: 'code' | 'article' | 'umag' }
  /** Тот же артикул у нескольких своих товаров — они в списке поиска, окна нет. */
  | { kind: 'own'; products: Product[] }
  | { kind: 'umag'; hit: OldCatalogHit }
  | { kind: 'unknown' }

export interface CodeLookup {
  byBarcode: (code: string) => Promise<Product>
  /** Свои товары по тексту — среди них ищется точный артикул. */
  searchOwn: (text: string) => Promise<Product[]>
  searchUmag: (code: string) => Promise<OldCatalogHit[]>
  productById: (id: string) => Promise<Product>
}

const serverLookup: CodeLookup = {
  byBarcode: (code) => get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`),
  searchOwn: (text) => get<Product[]>(`/products${qs({ q: text, limit: 15 })}`),
  // Список Умага — подсказка: сбой поиска в нём не мешает, код просто «не найден».
  searchUmag: (code) => searchUmag({ barcode: code }).catch(() => []),
  productById: (id) => get<Product>(`/products/${id}`),
}

/**
 * Скан или ввод кода (SPEC-35 §2): код товара (и без потерянных нулей) → товар; нет (404) — свой
 * товар с таким артикулом (артикул — не код, но это он: окно Умага «среди товаров его нет» было бы
 * неправдой); нет — точная строка Умага: связана — её товар, не связана — окно Умага; нигде —
 * «Код не найден». Ничего не записывает. Прочие ошибки — наружу: без сети вызывающий ищет в
 * снимке каталога.
 */
export async function resolveCode(code: string, api: CodeLookup = serverLookup): Promise<CodeOutcome> {
  try {
    return { kind: 'product', product: await api.byBarcode(code), via: 'code' }
  } catch (e) {
    if (!(e instanceof ApiError && e.status === 404)) throw e
  }
  const key = code.trim().toLowerCase()
  const own = (await api.searchOwn(code)).filter((p) => p.article.trim().toLowerCase() === key)
  if (own.length === 1 && own[0]) return { kind: 'product', product: own[0], via: 'article' }
  if (own.length > 1) return { kind: 'own', products: own }
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
