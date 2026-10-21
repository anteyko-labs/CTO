// Поиск товара (ProductPicker): относятся ли найденные позиции к набранному тексту и что делает Enter.

/**
 * Результаты по запросу `foundFor` показываются, пока набранный текст совпадает с ним или
 * продолжает его (идёт набор, новый запрос ещё не отправлен). Пустое поле после выбора товара —
 * список скрыт, но не стёрт: тот же запрос, набранный снова, сразу показывает найденное.
 */
export function resultsMatch(text: string, foundFor: string): boolean {
  const t = text.trim()
  return t.length > 0 && t.startsWith(foundFor)
}

/** Что делает Enter в поле поиска товара. */
export type EnterPlan =
  /** Код целиком (сканер или набранный штрихкод) — точный поиск по коду. */
  | 'code'
  /** Найдено по всему набранному тексту — берём выделенную строку. */
  | 'choose'
  /** Показано найденное только по началу текста (или ещё ничего) — сначала спросить сервер о
   *  полном тексте и взять первое из ответа (SPEC-30 §13). */
  | 'search'

/**
 * Enter выбирает только из найденного по всему набранному тексту. Иначе товар из списка по
 * первым буквам попадал в чек (21.10.2026: набрали моторное масло — встал воздушный фильтр).
 */
export function planEnter(text: string, foundFor: string, found: number): EnterPlan {
  const t = text.trim()
  if (t.length < 2 || /^\d{6,}$/.test(t)) return 'code'
  if (foundFor === t) return found > 0 ? 'choose' : 'code'
  return 'search'
}

/** Ступень найденного, как на сервере: точный код, название с начала, остальное. */
export function searchRank(p: { name: string; article: string; barcodes: string[] }, query: string): number {
  const q = query.trim().toLowerCase()
  if (p.barcodes.includes(query.trim()) || (p.article !== '' && p.article.toLowerCase() === q)) return 0
  if (p.name.toLowerCase().startsWith(q)) return 1
  return 2
}

/** Чем кончился Enter: товар из списка, поиск по коду или ничего (текст поменяли, пока ждали). */
export type EnterOutcome<T> = { kind: 'pick'; item: T; fresh?: T[] } | { kind: 'code'; fresh?: T[] } | { kind: 'stale' }

/**
 * Enter в поле поиска. Найдено по всему тексту — выделенная строка; показано найденное по
 * началу текста — сразу (без задержки набора) спрашиваем полный текст и берём первое из ответа.
 * Строку, выбранную стрелками в том же тексте, кассир выбрал сам — её и берём.
 * `current` — что в поле сейчас: пока ждали ответа, текст могли поменять.
 */
export async function resolveEnter<T>(
  s: { text: string; foundFor: string; found: T[]; active: number; navigated: boolean },
  searchFull: (q: string) => Promise<T[]>,
  current: () => string,
): Promise<EnterOutcome<T>> {
  const q = s.text.trim()
  const plan = planEnter(s.text, s.foundFor, s.found.length)
  if (plan === 'choose' || (plan === 'search' && s.navigated && s.found.length > 0 && resultsMatch(q, s.foundFor))) {
    return { kind: 'pick', item: s.found[s.active] ?? s.found[0] }
  }
  if (plan === 'code') return { kind: 'code' }
  const fresh = await searchFull(q)
  if (current().trim() !== q) return { kind: 'stale' }
  const first = fresh[0]
  return first === undefined ? { kind: 'code', fresh } : { kind: 'pick', item: first, fresh }
}
