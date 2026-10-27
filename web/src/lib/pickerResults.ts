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
  /** Найдено по всему набранному тексту — выбор из показанного. */
  | 'choose'
  /** Показано найденное только по началу текста (или ещё ничего) — сначала спросить сервер о
   *  полном тексте (SPEC-30 §13). */
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

/** Короткий код из цифр (доп. код Умага на ценнике, «1024»): ищется только как код. */
export const isShortCode = (text: string): boolean => /^\d{1,5}$/.test(text.trim())

/** Чем кончился Enter: товар из списка, поиск по коду, найдено несколько — строку выбирает
 *  кассир, или ничего (текст поменяли, пока ждали). */
export type EnterOutcome<T> =
  | { kind: 'pick'; item: T; fresh?: T[] }
  | { kind: 'code'; fresh?: T[] }
  | { kind: 'ambiguous'; fresh?: T[] }
  | { kind: 'stale' }

/**
 * Что взять из найденного по всему тексту. Только однозначное (исправлено 26.10.2026: «в чеке
 * постоянно добавляет воздушный фильтр» — Enter клал первую из найденных строк, а первым по
 * названию почти всегда шёл воздушный фильтр):
 * - ровно одна строка точно совпала по коду или артикулу — она;
 * - строка из цифр — только точный код: нет его среди найденного — искать как код, а не брать
 *   товар, у которого эти цифры в названии («1024» → «E1024L01 ВОЗДУШКА»);
 * - найдена одна строка — она; ничего — искать как код; несколько — выбирает кассир.
 */
function decide<T>(list: T[], q: string, exact: (item: T, q: string) => boolean, fresh: T[] | undefined): EnterOutcome<T> {
  const withFresh = <O extends EnterOutcome<T>>(o: O): O => (fresh === undefined ? o : { ...o, fresh })
  const exacts = list.filter((x) => exact(x, q))
  if (exacts.length === 1) return withFresh({ kind: 'pick', item: exacts[0] })
  if (/^\d+$/.test(q)) return withFresh({ kind: 'code' })
  if (exacts.length === 0 && list.length === 1) return withFresh({ kind: 'pick', item: list[0] })
  if (list.length === 0) return withFresh({ kind: 'code' })
  return withFresh({ kind: 'ambiguous' })
}

/**
 * Enter в поле поиска. Строку, выбранную стрелками в том же тексте, кассир выбрал сам — её и
 * берём. Иначе — из найденного по всему тексту и только однозначное (`decide`); показано
 * найденное по началу текста — сразу (без задержки набора) спрашиваем полный текст.
 * `current` — что в поле сейчас: пока ждали ответа, текст могли поменять. `exact` — строка
 * точно совпала по коду или артикулу.
 */
export async function resolveEnter<T>(
  s: { text: string; foundFor: string; found: T[]; active: number; navigated: boolean },
  searchFull: (q: string) => Promise<T[]>,
  current: () => string,
  exact: (item: T, q: string) => boolean = () => false,
): Promise<EnterOutcome<T>> {
  const q = s.text.trim()
  const plan = planEnter(s.text, s.foundFor, s.found.length)
  if (plan !== 'code' && s.navigated && s.found.length > 0 && resultsMatch(q, s.foundFor)) {
    return { kind: 'pick', item: s.found[s.active] ?? s.found[0] }
  }
  if (plan === 'code') return { kind: 'code' }
  if (plan === 'choose') return decide(s.found, q, exact, undefined)
  const fresh = await searchFull(q)
  if (current().trim() !== q) return { kind: 'stale' }
  return decide(fresh, q, exact, fresh)
}
