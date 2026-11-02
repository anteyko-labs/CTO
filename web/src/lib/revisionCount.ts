// Пересчёт строки ревизии несколькими людьми (задача 003, Н14): поле «+ добавить» или «= итог»
// и журнал «кто сколько посчитал».

/** `add` — прибавить найденное в своём месте; `set` — записать итог строки; `mine` — «= моё»:
 *  записать своё число строки, чужие не трогаются (задача 007). */
export type CountMode = 'add' | 'set' | 'mine'

/** Запись журнала строки с сервера: прибавил, записал итог, убрал строку. */
export interface CountEntry {
  by: string
  /** Вход, который записал (у сотрудников свои входы — задача 005, Н7): одинаковые имена не сливаются. */
  user_id?: string | null
  mode: 'add' | 'set' | 'clear'
  qty: number
}

/**
 * Режим поля, пока человек его не выбрал сам, — всегда «+ добавить»: ревизию ведут несколько
 * человек со своими входами, каждый вписывает найденное в своём месте, и итог складывается на
 * сервере («я занёс 6, Алексей 4 — в итоге 10», задача 005, Н7). «= итог» — только явным выбором;
 * 0 в непосчитанной строке — «не нашли» и в «+ добавить» (`addIsEmpty`). Выбранный режим строки
 * сам не сбрасывается.
 */
export function defaultCountMode(): CountMode {
  return 'add'
}

/**
 * «+ добавить» без числа — нечего записывать. 0 в ещё не посчитанной строке — «не нашли»: уходит
 * прибавкой 0 и строка становится посчитанной (0 + чужие прибавки), иначе пропажа молча осталась бы
 * «не посчитана» и на проведении не стала бы недостачей. В посчитанной строке +0 ничего не меняет.
 */
export function addIsEmpty(qty: number | null, counted: number | null): boolean {
  return qty === null || (qty === 0 && counted !== null)
}

/** Своя запись — сразу в журнал на экране: прибавки одного человека подряд — одной записью. */
export function appendCount(log: CountEntry[], e: CountEntry): CountEntry[] {
  const last = log[log.length - 1]
  if (last && e.mode === 'add' && last.mode === 'add' && sameWriter(last, e) && e.qty >= 0 && last.qty >= 0) return [...log.slice(0, -1), { ...last, qty: last.qty + e.qty }]
  return [...log, e]
}

/**
 * Своё число строки (задача 007: каждый видит только свой пересчёт): сумма своих прибавок после
 * последнего «= итог» или «убрал»; своих прибавок нет — `null` (строку сам не считал).
 */
export function mineOf(log: CountEntry[], userId: string): number | null {
  let from = 0
  for (let i = log.length - 1; i >= 0; i--)
    if (log[i].mode !== 'add') {
      from = i + 1
      break
    }
  let n: number | null = null
  let fixedDown = false
  for (const e of log.slice(from))
    if (e.mode === 'add' && e.user_id === userId) {
      n = (n ?? 0) + e.qty
      if (e.qty < 0) fixedDown = true
    }
  // Своё убрали крестиком («= моё 0» после прибавок) — строки у человека больше нет.
  return n === 0 && fixedDown ? null : n
}

/**
 * Кто что считает в черновике (задача 007): по каждому входу — имя и сколько товаров у него
 * посчитано (своё число строки не пусто), в порядке первой записи.
 */
export function countedBy(lines: { counted_log?: CountEntry[] }[]): { userId: string; name: string; products: number }[] {
  const out = new Map<string, { userId: string; name: string; products: number }>()
  for (const l of lines) {
    const log = l.counted_log ?? []
    for (const e of log) if (e.user_id && !out.has(e.user_id)) out.set(e.user_id, { userId: e.user_id, name: e.by, products: 0 })
    for (const p of out.values()) if (log.some((e) => e.user_id === p.userId) && mineOf(log, p.userId) !== null) p.products++
  }
  return [...out.values()]
}

/** Одна и та же запись одного входа: по входу, а у прежних ответов без входа — по имени. */
const sameWriter = (a: CountEntry, b: CountEntry): boolean => (a.user_id && b.user_id ? a.user_id === b.user_id : a.by === b.by)

/**
 * Кто сколько добавил к итогу строки: «Азиз +6, Алексей +4 = 10» (задача 005, Н7). Считается с
 * последнего «= итог» или «убрал» — записи до него итог уже не составляют; прибавки одного входа
 * складываются, в порядке первой прибавки. Последний «= итог» — первым: «Азиз = 5, Алексей +4 = 9».
 * Один «= итог» без прибавок — без повтора «= 5». `total` — итог строки с сервера.
 */
export function countSummary(log: CountEntry[], total: number | null, fmt: (qty: number) => string): string {
  let from = 0
  for (let i = log.length - 1; i >= 0; i--)
    if (log[i].mode !== 'add') {
      from = i
      break
    }
  const head = log[from]
  const parts: string[] = []
  if (head && head.mode === 'set') parts.push(`${head.by} = ${fmt(head.qty)}`)
  const adds: CountEntry[] = []
  for (const e of log.slice(head && head.mode !== 'add' ? from + 1 : 0)) {
    const same = adds.find((a) => sameWriter(a, e))
    if (same) same.qty += e.qty
    else adds.push({ ...e })
  }
  for (const a of adds) parts.push(`${a.by} +${fmt(a.qty)}`)
  if (parts.length === 0) return ''
  if (total === null || (adds.length === 0 && parts.length === 1)) return parts.join(', ')
  return `${parts.join(', ')} = ${fmt(total)}`
}

/** «Айбек +2 шт, Нурлан +1 шт, Азиз = 5 шт»; `fmt` — количество в единицах товара. */
export function countLogText(log: CountEntry[], fmt: (qty: number) => string): string {
  return log.map((e) => (e.mode === 'add' ? `${e.by} +${fmt(e.qty)}` : e.mode === 'set' ? `${e.by} = ${fmt(e.qty)}` : `${e.by} убрал`)).join(', ')
}

// ---------- Совместный пересчёт: сверка, op_id, свежесть, единицы (задача 003, Н14) ----------

/** Что человек видел в строке: посчитано столько или `null` — строки нет (не путать с 0). */
export type Seen = number | null

/** Сверка «= итог» на сервере: «строки не было» — отдельно от «посчитано 0». */
export const expectedBody = (seen: Seen): { expected_qty: number } | { expected_empty: true } =>
  seen === null ? { expected_empty: true } : { expected_qty: seen }

/** Та же сверка для очистки строки (`DELETE …?…`). */
export const expectedQuery = (seen: Seen): string => (seen === null ? 'expected_empty=true' : `expected_qty=${seen}`)

type OwnKind = 'add' | 'set' | 'clear' | 'mine'

interface OwnWrite {
  seq: number
  kind: OwnKind
  qty: number
  state: 'pending' | 'ok' | 'failed'
}

/**
 * Свои записи строк на этом устройстве. «= итог» и очистка сверяются на сервере с тем, что
 * человек видел, когда начал ввод, плюс его собственные записи, ушедшие после этого и записанные.
 * Итог из ответа своей записи не годится: в нём чужие прибавки, которых человек не видел, — чужое
 * попадает к нему только через 409 «пока вы вводили…».
 */
export class OwnWrites {
  private next = 1
  private lines = new Map<string, { writes: OwnWrite[]; mark?: { seen: Seen; from: number } }>()

  private line(id: string) {
    let l = this.lines.get(id)
    if (!l) {
      l = { writes: [] }
      this.lines.set(id, l)
    }
    return l
  }

  /** Человек начал ввод, глядя на `seen`: свои записи, ещё не дошедшие до экрана, — после. */
  see(id: string, seen: Seen): void {
    const l = this.line(id)
    const pending = l.writes.find((w) => w.state === 'pending')
    const from = pending ? pending.seq : this.next
    l.writes = l.writes.filter((w) => w.seq >= from)
    l.mark = { seen, from }
  }

  /** Своя запись ушла; номер — для `settle` и `base`. */
  start(id: string, kind: OwnKind, qty: number): number {
    const seq = this.next++
    this.line(id).writes.push({ seq, kind, qty, state: 'pending' })
    return seq
  }

  /** Ответ на свою запись: записана или нет. */
  settle(id: string, seq: number, ok: boolean): void {
    const w = this.lines.get(id)?.writes.find((x) => x.seq === seq)
    if (w) w.state = ok ? 'ok' : 'failed'
  }

  /** С чем сверять запись `seq`; ввод не начинали — `undefined`. */
  base(id: string, seq: number): Seen | undefined {
    const l = this.lines.get(id)
    if (!l?.mark) return undefined
    let v = l.mark.seen
    for (const w of l.writes) {
      if (w.seq < l.mark.from || w.seq >= seq || w.state !== 'ok') continue
      // «= моё» меняет итог на неизвестную здесь разницу — сверка «= итог» после него переспросит.
      if (w.kind === 'mine') continue
      v = w.kind === 'add' ? (v ?? 0) + w.qty : w.kind === 'set' ? w.qty : null
    }
    return v
  }

  /** С чем сверять запись `seq`; ввод не начинали — с тем, что на экране (`shown`). `null`
   *  (видели строку непосчитанной) — тоже сверка, а не «не с чем». */
  baseOr(id: string, seq: number, shown: Seen): Seen {
    const b = this.base(id, seq)
    return b === undefined ? shown : b
  }
}

/**
 * op_id ввода. Один и тот же не записанный ввод (тот же текст в поле или форме после неудачи)
 * уходит с тем же op_id, пока не придёт успешный ответ или человек не изменит ввод: сервер не
 * прибавит дважды, если первый раз дошёл, а ответ потерялся (ADR-062). Новый ввод — новый op_id.
 * `slot` — место ввода (строка, форма), `key` — сам ввод.
 */
export class InputOps {
  private m = new Map<string, { key: string; opId: string }>()
  private fresh: () => string

  constructor(fresh: () => string) {
    this.fresh = fresh
  }

  take(slot: string, key: string): string {
    const cur = this.m.get(slot)
    if (cur && cur.key === key) return cur.opId
    const opId = this.fresh()
    this.m.set(slot, { key, opId })
    return opId
  }

  /** Ввод записан: следующий такой же — новый. Если в месте уже другой ввод — его не трогаем. */
  done(slot: string, opId: string): void {
    if (this.m.get(slot)?.opId === opId) this.m.delete(slot)
  }

  /** Человек начал заново (форма сброшена): всё прежнее — уже не тот ввод. */
  clear(slot?: string): void {
    if (slot === undefined) this.m.clear()
    else this.m.delete(slot)
  }
}

/** Ключ ввода поля строки для `InputOps`: режим и число. Enter в поле и «Прибавить» из окна
 *  «пока вы вводили» с тем же числом — один ввод, один op_id. */
export const inputKey = (mode: CountMode, qty: number | null): string => `${mode}|${qty ?? ''}`

/**
 * Нажатия, на которые не пришёл ответ (обрыв связи): записались они или нет — неизвестно. Человек
 * изменил ввод и нажал снова — новый op_id, а прошлые уходят с ним (`prev_op_ids`): если одно из
 * них уже записалось, сервер ответит им, а не заведёт товар второй раз (задача 003, Н14).
 */
export class LostOps {
  private ids: string[] = []

  lost(opId: string): void {
    if (!this.ids.includes(opId)) this.ids.push(opId)
  }

  /** Прошлые нажатия без ответа — кроме этого. */
  before(opId: string): string[] {
    return this.ids.filter((x) => x !== opId)
  }

  /** Ответ пришёл: прошлые нажатия больше не нужны. */
  clear(): void {
    this.ids = []
  }
}

/**
 * «Завести» товар не из программы с найденным (задача 003, Н14). Карточку завело прошлое нажатие
 * (`earlierOf` — его товар и записанное тогда найденное), а найденное с тех пор исправили —
 * исправляем итог. Нажатие записано только после исправления: обрыв на нём — следующее «Завести»
 * уходит с тем же op_id и прошлыми нажатиями, сервер снова ответит «earlier», а не заведёт товар
 * второй раз.
 */
export async function createCounted<R>(a: {
  ops: InputOps
  lost: LostOps
  /** Ввод формы: тот же — тот же op_id. */
  key: string
  count: number
  send: (opId: string, prev: string[]) => Promise<R>
  earlierOf: (r: R) => { productId: string; counted: number } | null
  /** Записать итог `count` строки товара, сверяясь с `expected`; ошибка — пробрасывается. */
  fix: (productId: string, expected: number, opId: string) => Promise<unknown>
  noAnswer: (e: unknown) => boolean
}): Promise<R> {
  const opId = a.ops.take('new', a.key)
  let r: R
  try {
    r = await a.send(opId, a.lost.before(opId))
  } catch (e) {
    // Ответа нет — записалось ли, неизвестно: следующее нажатие несёт это с собой.
    if (a.noAnswer(e)) a.lost.lost(opId)
    throw e
  }
  const earlier = a.earlierOf(r)
  if (earlier && earlier.counted !== a.count) {
    const fix = a.ops.take('fix', `${earlier.productId}|${a.count}`)
    await a.fix(earlier.productId, earlier.counted, fix)
    a.ops.done('fix', fix)
  }
  a.ops.done('new', opId)
  a.lost.clear()
  return r
}

interface VersionedLine {
  product_id: string
  /** Номер последней записи строки на сервере; без него (старый сервер) — показываем как есть. */
  version?: number
}

/**
 * Перечитанная ревизия поверх показанной: строка берётся с сервера, если она не старее
 * показанной; строка, которой нет в ответе, остаётся, только если показана новее всего ответа
 * (`incomingVersion` — номер последней записи ревизии в ответе). Старый итог не возвращается
 * поверх свежего, в каком бы порядке ни пришли ответы.
 */
export function mergeLines<L extends VersionedLine>(shown: L[], incoming: L[], incomingVersion: number | undefined): L[] {
  const byId = new Map(shown.map((l) => [l.product_id, l]))
  const out = incoming.map((l) => {
    const s = byId.get(l.product_id)
    return s && s.version !== undefined && l.version !== undefined && s.version > l.version ? s : l
  })
  const has = new Set(incoming.map((l) => l.product_id))
  for (const s of shown) {
    if (!has.has(s.product_id) && s.version !== undefined && incomingVersion !== undefined && s.version > incomingVersion) out.push(s)
  }
  return out
}

interface ShownRevision<L extends VersionedLine> {
  head: { id: string; status: 'draft' | 'posted' | 'cancelled' }
  lines: L[]
  version?: number
}

/**
 * Перечитанная ревизия на экран. Проведённая или отменённая в черновик не возвращается: ответ
 * перечитывания черновика, пришедший позже «проведена», — старый. Черновик поверх черновика —
 * строки по версиям (`mergeLines`).
 */
export function freshRevision<L extends VersionedLine, R extends ShownRevision<L>>(cur: R | null, r: R): R {
  if (!cur || cur.head.id !== r.head.id) return r
  if (r.head.status !== 'draft') return r
  if (cur.head.status !== 'draft') return cur
  return { ...r, lines: mergeLines(cur.lines, r.lines, r.version) }
}

/** Ответ своей записи — на экран, только если он новее показанной строки. */
export function patchIfNewer<L extends VersionedLine>(line: L, version: number | undefined, patch: (l: L) => L): L {
  if (version !== undefined && line.version !== undefined && version <= line.version) return line
  const next = patch(line)
  return version === undefined ? next : { ...next, version }
}

/** Найденное на полке: целые штуки (у масла — канистры) и литры початой тары, мл; у кг и м — `milli`. */
export interface ShelfInput {
  cans: number
  ml: number
  /** Товар на вес или в длину: введено в кг (г) или м (мм) — ADR-178. */
  milli?: { unit: 'g' | 'mm'; qty: number }
}

/**
 * Найденное — в единицах товара, который в ту же минуту завёл другой (у него могут быть другие
 * единицы и тара): штуки — штуками, канистры — по таре этого товара, масло без тары — литрами.
 * кг — к кг, м — к м. Несовместимо (литры к штучному, штуки к маслу без тары или к весовому, кг к
 * метрам) — `null`: молча не прибавляем, человек впишет найденное в единицах товара. Ноль — ноль.
 */
export function qtyInUnits(input: ShelfInput, p: { unit: string; container_ml: number | null }): number | null {
  if (input.milli) {
    if (input.milli.qty === 0) return 0
    return p.unit === input.milli.unit ? input.milli.qty : null
  }
  if (p.unit === 'piece') return input.ml > 0 ? null : input.cans
  if (p.unit === 'ml' && p.container_ml) return input.cans * p.container_ml + input.ml
  if (p.unit === 'ml') return input.cans > 0 ? null : input.ml
  return input.cans === 0 && input.ml === 0 ? 0 : null
}
