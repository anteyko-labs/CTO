// Скидка на кассе (задача 003 Н3, ADR-147): на весь чек — процентом или суммой, на строку — новой
// ценой или процентом. Скидка — это цена строки ниже прайса (ADR-097): раскладка скидки чека по
// строкам и всё округление — здесь, в одной функции, целыми тыйынами (I1, I2). Подарки не трогаются,
// услуги — по настройке. Цена строки — до целого сома вниз, остаток — на самую дорогую штучную
// строку или услугу (лучше с количеством 1); если точно не попасть (например, в чеке только розлив),
// показывается настоящая скидка. Право «Скидка в чеке» и предел администратора проверяет сервер.
import { divRound, pourAmount } from './money'
import { isMilliKind } from './units'

export type DiscKind = 'piece' | 'container' | 'pour' | 'weight' | 'length' | 'service'

/** Строка чека для скидки: количество — шт или канистры, у розлива — мл, у веса — граммы, у длины —
 *  мм; цены — за единицу (шт, канистру, литр, кг, м), тыйын. */
export interface DiscLine {
  key: string
  kind: DiscKind
  qty: number
  /** Цена по прайсу. */
  list: number
  /** Цена сейчас. */
  price: number
  gift?: boolean
}

/** Сумма строки: как у сервера — у розлива, веса и длины мл (г, мм) × цена за 1000 / 1000. */
export function lineSum(kind: DiscKind, qty: number, price: number): number {
  return isMilliKind(kind) ? pourAmount(qty, price) : qty * price
}

/** До целого сома вниз. */
const floorSom = (v: number): number => Math.floor(v / 100) * 100

/** list × part / whole без потери точности (произведение может выйти за 2^53). */
const scale = (list: number, part: number, whole: number): number => Number((BigInt(list) * BigInt(part)) / BigInt(whole))

/** Процент от суммы, до тыйына половиной от нуля. */
export const percentOf = (base: number, pct: number): number => divRound(base * pct, 100)

/** Строки, на которые ложится скидка на весь чек. */
const eligible = (l: DiscLine, services: boolean): boolean => !l.gift && l.list > 0 && (services || l.kind !== 'service')

/** Скидка чека сейчас: сумма по прайсу (без подарков), итог и на сколько дешевле прайса (только
 *  строки ниже прайса — поднятая цена скидку не уменьшает). */
export function checkDiscount(lines: readonly DiscLine[]): { list: number; total: number; discount: number } {
  let list = 0
  let total = 0
  let discount = 0
  for (const l of lines) {
    if (l.gift) continue
    const a = lineSum(l.kind, l.qty, l.list)
    const b = lineSum(l.kind, l.qty, l.price)
    list += a
    total += b
    if (b < a) discount += a - b
  }
  return { list, total, discount }
}

/**
 * Скидка на весь чек суммой `discount` (тыйын): новые цены строк. Скидка считается от прайса — прежняя
 * скидка строк заменяется. 0 — цены по прайсу. Ответ: новые цены (по ключу строки, только строки
 * под скидкой) и настоящая скидка — она может отличаться от заказанной на остаток округления.
 */
export function spreadDiscount(
  lines: readonly DiscLine[],
  discount: number,
  opts: { services: boolean },
): { prices: Map<string, number>; discount: number } {
  const ls = lines.filter((l) => eligible(l, opts.services) && l.qty > 0)
  const base = ls.reduce((acc, l) => acc + lineSum(l.kind, l.qty, l.list), 0)
  const prices = new Map<string, number>()
  if (base <= 0) return { prices, discount: 0 }
  const d = Math.min(Math.max(0, Math.trunc(discount)), base)
  const target = base - d
  for (const l of ls) prices.set(l.key, d === 0 ? l.list : Math.min(l.list, floorSom(scale(l.list, target, base))))
  const sum = () => ls.reduce((acc, l) => acc + lineSum(l.kind, l.qty, prices.get(l.key) ?? l.list), 0)
  let rest = target - sum()
  if (rest > 0) {
    // Остаток — на штучную строку или услугу: сначала с количеством 1 (попадаем точно), затем дороже.
    const units = ls
      .filter((l) => !isMilliKind(l.kind))
      .sort((a, b) => Number(b.qty === 1) - Number(a.qty === 1) || (prices.get(b.key) ?? 0) - (prices.get(a.key) ?? 0))
    for (const l of units) {
      if (rest <= 0) break
      const now = prices.get(l.key) ?? l.list
      const add = Math.min(Math.floor(rest / l.qty), l.list - now)
      if (add <= 0) continue
      prices.set(l.key, now + add)
      rest = target - sum()
    }
    // Только розлив, вес и длина: добавка к цене литра самой большой строки, сколько поместится.
    if (rest > 0) {
      const big = ls.filter((l) => isMilliKind(l.kind)).sort((a, b) => b.qty - a.qty)[0]
      if (big) {
        const now = prices.get(big.key) ?? big.list
        let add = Math.min(Math.floor((rest * 1000) / big.qty), big.list - now)
        while (add > 0 && sum() - lineSum(big.kind, big.qty, now) + lineSum(big.kind, big.qty, now + add) > target) add -= 1
        if (add > 0) prices.set(big.key, now + add)
      }
    }
  }
  return { prices, discount: base - sum() }
}

/** Скидка на весь чек процентом (1–99): сумма от прайса строк под скидку. */
export function percentDiscount(lines: readonly DiscLine[], pct: number, opts: { services: boolean }): number {
  const base = lines.filter((l) => eligible(l, opts.services) && l.qty > 0).reduce((acc, l) => acc + lineSum(l.kind, l.qty, l.list), 0)
  return percentOf(base, pct)
}

/** Цена строки со скидкой в процентах: от прайса, до целого сома вниз, не меньше нуля. */
export function linePercentPrice(list: number, pct: number): number {
  return Math.max(0, floorSom(list - percentOf(list, pct)))
}

/** Скидка больше предела администратора? Подпись предела («10 %», «500 с») или `null`. Сервер
 *  проверяет так же: процент — от суммы чека по прайсу (без подарков). */
export function overLimit(discount: number, listTotal: number, limit: { pct: number | null; tyiyn: number | null }): 'pct' | 'sum' | null {
  if (limit.pct !== null && discount * 100 > limit.pct * Math.max(0, listTotal)) return 'pct'
  if (limit.tyiyn !== null && discount > limit.tyiyn) return 'sum'
  return null
}
