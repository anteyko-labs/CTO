// Позиция списка старой программы → поля новой карточки товара (SPEC-20, SPEC-29 §1): масло —
// объём тары из названия, цена и закупка за литр пересчитываются за канистру; строки «кг» и «м» —
// товар в килограммах и метрах, цена и закупка за кг (м) как есть (ADR-174, ADR-178).
import { litersInput, somInput } from './format'
import { divRound } from './money'
import type { Category, OldCatalogHit } from './types'
import { isMeasureCode, isMeasureUnit, measureOf, productCodes, productUnitOf, umagCodes } from './umagCodes'
import { containerFromName } from './xlsx'

export interface CardPreset {
  name: string
  barcode: string
  article: string
  categoryId: string
  /** Объём тары, литры строкой; пусто — не масло или не нашли в названии. */
  liters: string
  /** Цена продажи и закупка в сомах строкой — за штуку или канистру. */
  price: string
  cost: string
  /** Строка Умага «кг» или «м» — товар в килограммах (метрах), цена и закупка за кг (м) (ADR-174, ADR-178). */
  unit?: 'g' | 'mm'
}

/** Единица «килограмм» в старой программе: «кг», «килограмм». */
export const isKgUnit = (unit: string): boolean => measureOf(unit) === 'kg'

/** Единица «литр» в старой программе: «л», «литр», «Литр». */
export const isLiterUnit = (unit: string): boolean => /литр|^л\.?$|л$/i.test(unit.trim())

/**
 * Код новой карточки: скан, иначе штрихкод строки. У строки меры («литр», «кг», «метр») её коды
 * Умага (276…, PLU) продавали литр — карточка канистры берёт заводской код (EAN), а без него код
 * выдаст программа: иначе касса по коду литра добавляла бы канистру (ADR-113).
 */
function cardBarcode(h: OldCatalogHit, scanned: string): string {
  if (!isMeasureUnit(h.unit)) return scanned || h.barcode
  if (scanned && !isMeasureCode(scanned, h.unit)) return scanned
  return productCodes(h.codes ?? umagCodes(h), h.unit)[0] ?? ''
}

/** Единица карточки — сервера (`product_unit`), у прежнего ответа — по колонке единицы; только кг и м. */
function kgOrMeters(h: OldCatalogHit): { unit?: 'g' | 'mm' } {
  const u = h.product_unit ?? productUnitOf(h.unit)
  return u === 'g' || u === 'mm' ? { unit: u } : {}
}

/**
 * Поля карточки из позиции старого списка. Категория — по названию из старой программы (у масла —
 * если это категория масел), иначе у масла — моторные (`book_slot` «oil»), иначе запомненная `sticky`. Код скана важнее кода из списка.
 */
export function presetFromHit(h: OldCatalogHit, categories: Category[], sticky = '', scanned = ''): CardPreset {
  const liter = isLiterUnit(h.unit)
  const ml = liter ? containerFromName(h.name) : null
  // Масло без своей категории в Умаге — в моторные (строка книжки «масло»), а не в первую по
  // алфавиту категорию масел: «АНТИФРИЗЫ» видом «масло» забирали моторные масла (задача 007).
  const oils = categories.filter((c) => c.kind === 'oil')
  const oilCat = (oils.find((c) => c.book_slot === 'oil') ?? oils[0])?.id ?? ''
  const byNameCat = categories.find((c) => h.category && c.name.toLowerCase() === h.category.trim().toLowerCase())
  const byName = byNameCat?.id
  const known = categories.some((c) => c.id === sticky) ? sticky : ''
  // Литры — категория строки Умага, если это масла; иначе моторные.
  const categoryId = liter ? (byNameCat?.kind === 'oil' ? byNameCat.id : '') || oilCat || known : byName || known
  const per = (v: number) => (ml ? (divRound(v * ml, 1000) ?? 0) : v)
  // Масло за литр без объёма в названии: цена литра — не цена канистры; поля пустые, пока не
  // вписан объём (проверка 20.10.2026).
  const unknownTare = liter && !ml
  return {
    name: h.name,
    barcode: cardBarcode(h, scanned),
    article: h.article,
    categoryId,
    liters: ml ? litersInput(ml) : '',
    price: h.price_tyiyn && !unknownTare ? somInput(per(h.price_tyiyn)) : '',
    cost: h.cost_tyiyn && !unknownTare ? somInput(per(h.cost_tyiyn)) : '',
    ...kgOrMeters(h),
  }
}
