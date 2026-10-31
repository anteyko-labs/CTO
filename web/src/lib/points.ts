// Баллы по товару, категории и общей ставке (задача 002, П1; ADR-140). Считает сервер; здесь —
// ввод и показ правила. Процент — сотые доли процента (50 = 0,5 %), «за единицу» — тыйыны за
// штуку, литр или кг. Без float.
import { formatPercentBp, formatPoints, parsePercentBp, parseSom } from './format'
import type { PointsMode } from './types'

export type { PointsMode }

export interface PointsRule {
  mode: PointsMode
  value: number
}

/** Подпись «за единицу» по единице товара. */
export function perUnitWord(unit: 'piece' | 'ml' | 'g' | undefined): string {
  return unit === 'ml' ? 'за литр' : unit === 'g' ? 'за кг' : 'за штуку'
}

/** Значение для поля ввода: процент «0,5», баллы «10». */
export function pointsInput(rule: PointsRule): string {
  if (rule.mode === 'percent') return formatPercentBp(rule.value).replace(/\s*%$/, '').replace(/,00$/, '').replace(/(,\d)0$/, '$1')
  if (rule.mode === 'fixed') return formatPoints(rule.value).replace(/ /g, '')
  return ''
}

/** Правило из выбора и введённого числа; неверное число — сообщение. */
export function parsePointsRule(mode: PointsMode, text: string): PointsRule | string {
  if (mode === 'inherit' || mode === 'none') return { mode, value: 0 }
  if (mode === 'percent') {
    const bp = parsePercentBp(text)
    if (bp === null || bp <= 0 || bp > 10_000) return 'Процент баллов — от 0,01 до 100'
    return { mode, value: bp }
  }
  const v = parseSom(text)
  if (v === null || v <= 0 || v > 100_000_000) return 'Баллы за единицу — число больше нуля'
  return { mode, value: v }
}

/** Правило словами: «0,5 % от оплаченного», «10 баллов за литр», «не начислять». */
export function pointsRuleText(rule: PointsRule, unit?: 'piece' | 'ml' | 'g'): string {
  switch (rule.mode) {
    case 'none':
      return 'не начислять'
    case 'percent':
      return `${pointsInput(rule)} % от оплаченного`
    case 'fixed':
      return `${formatPoints(rule.value)} ${perUnitWord(unit)}`
    default:
      return ''
  }
}

/** Какое правило действует у товара: товар → категория → общая ставка. */
export function effectivePoints(
  product: PointsRule | null,
  category: PointsRule | null,
  commonBp: number,
): { rule: PointsRule; from: 'product' | 'category' | 'common' } {
  if (product && product.mode !== 'inherit') return { rule: product, from: 'product' }
  if (category && category.mode !== 'inherit') return { rule: category, from: 'category' }
  return { rule: { mode: 'percent', value: commonBp }, from: 'common' }
}

export const POINTS_FROM: Record<'product' | 'category' | 'common', string> = {
  product: 'задано у товара',
  category: 'как в категории',
  common: 'общая ставка',
}

/** Правило в форме: выбор и введённое число. */
export interface PointsDraft {
  mode: PointsMode
  text: string
}

export function pointsDraft(mode: PointsMode | undefined, value: number | undefined): PointsDraft {
  const rule = { mode: mode ?? 'inherit', value: value ?? 0 }
  return { mode: rule.mode, text: pointsInput(rule) }
}

/** Правило поменялось против сохранённого (числа сравниваются после разбора). */
export function pointsChanged(draft: PointsDraft, mode: PointsMode | undefined, value: number | undefined): boolean {
  const parsed = parsePointsRule(draft.mode, draft.text)
  if (typeof parsed === 'string') return true
  const saved = parsed.mode === 'percent' || parsed.mode === 'fixed' ? (value ?? 0) : 0
  return parsed.mode !== (mode ?? 'inherit') || parsed.value !== saved
}
