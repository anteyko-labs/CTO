// Коды строки старой базы Умаг — то же правило, что на сервере (`umag_codes`, миграция 0067,
// SPEC-35 §1, ADR-113): штрихкод; артикул, если похож на код; из «доп. кода» — только куски,
// похожие на код. «Доп. код» в выгрузке — свободный текст («SHELLHELIX CITY 5W30 1L», «0888602305  U»),
// а слово или обрывок, ставший кодом товара, на кассе находил не тот товар.

import type { StockUnit } from './units'

const CODE_CHARS = /^[0-9A-Za-z-]+$/
const DIGITS = /^\d+$/

/** Артикул — код: латиница, цифры и дефис, от четырёх знаков, есть цифра («BKR6E»). */
export function articleIsCode(t: string): boolean {
  return CODE_CHARS.test(t) && t.length >= 4 && t.length <= 64 && /\d/.test(t)
}

/**
 * Кусок «доп. кода» — код: латиница, цифры и дефис, не меньше четырёх цифр; с буквами — от шести
 * знаков; не длиннее 64; вязкость («10W-40») — не код.
 */
export function extraIsCode(t: string): boolean {
  if (!CODE_CHARS.test(t) || t.length > 64) return false
  if (t.replace(/\D/g, '').length < 4) return false
  if (!DIGITS.test(t) && t.length < 6) return false
  return !/^\d{1,2}w-?\d{2}$/i.test(t)
}

/** Коды строки Умага: штрихкод, артикул-код и куски «доп. кода»-коды; пустые и повторы — прочь. */
export function umagCodes(row: { barcode: string; article: string; extra_code?: string }): string[] {
  const out: string[] = []
  const add = (c: string) => {
    if (c && !out.includes(c)) out.push(c)
  }
  add(row.barcode.trim())
  const article = row.article.trim()
  if (articleIsCode(article)) add(article)
  for (const c of (row.extra_code ?? '').trim().split(/[;,\s]+/)) if (extraIsCode(c)) add(c)
  return out
}

/** Строку Умага продавали мерой: литр, кг, метр (как `umag_by_measure` на сервере). */
export function isMeasureUnit(unit: string): boolean {
  return /^(л|лт|литр[а-я]*|кг|килограмм[а-я]*|г|гр|грамм[а-я]*|м|метр[а-я]*)\.?$/.test(unit.trim().toLowerCase())
}

/** Числовой код без потерянных Excel нулей впереди; остальные — как есть (`umag_bare` на сервере). */
function bare(code: string): string {
  const c = code.trim()
  const stripped = c.replace(/^0+/, '')
  return DIGITS.test(c) && stripped !== '' ? stripped : c
}

/**
 * Код меры у строки «литр», «кг», «метр» — не заводской код: короче восьми цифр (276…, PLU) или
 * EAN-13 магазина с префиксом 2 — весовая наклейка «1 литр» (код строки 2760275, количество 01000
 * и контрольная — 2760275010002) и коды магазина Умага 211…. В Умаге он продавал литр или
 * килограмм; товару учёта (канистра, ведро) не переходит — на кассе добавил бы всю тару (ADR-113;
 * как `umag_measure_code` на сервере).
 */
export const isMeasureCode = (code: string, unit: string): boolean => isMeasureUnit(unit) && /^(\d{1,7}|2\d{12})$/.test(bare(code))

/** Коды строки, которые переходят товару (`umag_product_codes` на сервере). */
export const productCodes = (codes: readonly string[], unit: string): string[] => codes.filter((c) => !isMeasureCode(c, unit))

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

/**
 * Единица меры строки Умага: литр, кг, грамм, метр; штучная — `null`. «г/гр/грамм» — грамм: цена
 * у Умага за грамм, а не за кг (задача 006, К8).
 */
export function measureOf(unit: string): 'l' | 'kg' | 'g' | 'm' | null {
  if (!isMeasureUnit(unit)) return null
  const u = unit.trim().toLowerCase()
  if (u.startsWith('л')) return 'l'
  if (u.startsWith('м')) return 'm'
  if (u.startsWith('к')) return 'kg'
  return 'g'
}

/**
 * Единица карточки учёта по колонке единицы Умага — как `old_catalog::product_unit` на сервере:
 * кг → граммы, м → миллиметры, л → мл; «г» и прочее — штуки (ADR-178).
 */
export function productUnitOf(unit: string): StockUnit {
  const m = measureOf(unit)
  return m === 'kg' ? 'g' : m === 'm' ? 'mm' : m === 'l' ? 'ml' : 'piece'
}

/** Цена строки меры в Умаге — за единицу меры: « за л», « за кг», « за г», « за м»; у штучной — пусто. */
export function umagPer(unit: string): string {
  const m = measureOf(unit)
  return m === 'l' ? ' за л' : m === 'kg' ? ' за кг' : m === 'g' ? ' за г' : m === 'm' ? ' за м' : ''
}

/** Что продавал код меры: «литр», «килограмм», «грамм», «метр». */
export function umagUnitWord(unit: string): string {
  const m = measureOf(unit)
  return m === 'l' ? 'литр' : m === 'm' ? 'метр' : m === 'kg' ? 'килограмм' : m === 'g' ? 'грамм' : 'штуку'
}
