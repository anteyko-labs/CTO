// Позиция списка старой программы → поля новой карточки товара (SPEC-20, SPEC-29 §1): масло —
// объём тары из названия, цена и закупка за литр пересчитываются за канистру.
import { litersInput, somInput } from './format'
import { divRound } from './money'
import type { Category, OldCatalogHit } from './types'
import { isMeasureCode, isMeasureUnit, productCodes, umagCodes } from './umagCodes'
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
}

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

/**
 * Поля карточки из позиции старого списка. Категория — по названию из старой программы, у масла —
 * первая категория масел, иначе запомненная `sticky`. Код скана важнее кода из списка.
 */
export function presetFromHit(h: OldCatalogHit, categories: Category[], sticky = '', scanned = ''): CardPreset {
  const liter = isLiterUnit(h.unit)
  const ml = liter ? containerFromName(h.name) : null
  const oilCat = categories.find((c) => c.kind === 'oil')?.id ?? ''
  const byName = categories.find((c) => h.category && c.name.toLowerCase() === h.category.trim().toLowerCase())?.id
  const known = categories.some((c) => c.id === sticky) ? sticky : ''
  const categoryId = liter ? oilCat || known : byName || known
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
  }
}
