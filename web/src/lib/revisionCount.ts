// Пересчёт строки ревизии несколькими людьми (задача 003, Н14): поле «+ добавить» или «= итог»
// и журнал «кто сколько посчитал».

/** `add` — прибавить найденное в своём месте; `set` — записать итог строки. */
export type CountMode = 'add' | 'set'

/** Запись журнала строки с сервера: прибавил, записал итог, убрал строку. */
export interface CountEntry {
  by: string
  mode: 'add' | 'set' | 'clear'
  qty: number
}

interface CountLine {
  unit: string
  container_ml: number | null
  counted_qty: number | null
}

/**
 * Режим поля, пока человек его не выбрал сам. Строку уже кто-то считал — «+ добавить»: человек
 * вводит то, что нашёл в своём месте; не считали — «= итог». Масло в таре — всегда прибавка:
 * канистры считают сканом, вскрытую канистру вписывают литрами.
 */
export function defaultCountMode(l: CountLine): CountMode {
  if (l.unit === 'ml' && l.container_ml) return 'add'
  return (l.counted_qty ?? 0) > 0 ? 'add' : 'set'
}

/** Своя запись — сразу в журнал на экране: прибавки одного человека подряд — одной записью. */
export function appendCount(log: CountEntry[], e: CountEntry): CountEntry[] {
  const last = log[log.length - 1]
  if (last && e.mode === 'add' && last.mode === 'add' && last.by === e.by) return [...log.slice(0, -1), { ...last, qty: last.qty + e.qty }]
  return [...log, e]
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

type OwnKind = 'add' | 'set' | 'clear'

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

/** Ответ своей записи — на экран, только если он новее показанной строки. */
export function patchIfNewer<L extends VersionedLine>(line: L, version: number | undefined, patch: (l: L) => L): L {
  if (version !== undefined && line.version !== undefined && version <= line.version) return line
  const next = patch(line)
  return version === undefined ? next : { ...next, version }
}

/** Найденное на полке: целые штуки (у масла — канистры) и литры початой тары, мл. */
export interface ShelfInput {
  cans: number
  ml: number
}

/**
 * Найденное — в единицах товара, который в ту же минуту завёл другой (у него могут быть другие
 * единицы и тара): штуки — штуками, канистры — по таре этого товара, масло без тары — литрами.
 * Несовместимо (литры к штучному, штуки к маслу без тары или к весовому) — `null`: молча не
 * прибавляем, человек впишет найденное в единицах товара.
 */
export function qtyInUnits(input: ShelfInput, p: { unit: string; container_ml: number | null }): number | null {
  if (p.unit === 'piece') return input.ml > 0 ? null : input.cans
  if (p.unit === 'ml' && p.container_ml) return input.cans * p.container_ml + input.ml
  if (p.unit === 'ml') return input.cans > 0 ? null : input.ml
  return input.cans === 0 && input.ml === 0 ? 0 : null
}
