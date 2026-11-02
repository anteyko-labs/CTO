// Накладная по фото (SPEC-30 §9, ADR-098): черновик прихода из фото в боте или с экрана,
// распознанный сервером. Здесь — вид черновика и перевод его строк в строки «Нового прихода».
// Только целые тыйыны и тысячные доли: литры масла в канистры — делением без остатка.
import { formatLiters, formatSom, litersInput, parseLiters } from './format'
import { divRound } from './money'
import type { Product } from './types'

/** Черновик в списке «Накладные по фото». */
export interface DraftItem {
  id: string
  source: 'bot' | 'screen'
  status: 'new' | 'used' | 'removed'
  /** off — распознавание не настроено, limit — лимит часа, pending — идёт, done, failed. */
  recognition: 'off' | 'limit' | 'pending' | 'done' | 'failed'
  created_at: string
  user_name: string
  name: string
  content_type: string
  supplier_name: string
  doc_number: string
  lines: number
  total_tyiyn: number | null
  /** Фото в накладной: первое и дописанные «Ещё фото». */
  photos: number
}

export interface DraftList {
  /** На сервере есть ключ сервиса: «Заполнить по фото» работает. */
  enabled: boolean
  items: DraftItem[]
}

/** Строка накладной и подсказка для неё. */
export interface DraftLine {
  no: number
  name: string
  code: string
  barcode: string
  quantity: string
  unit: string
  price: string
  sum: string
  /** Количество в тысячных: «2,5» → 2500. */
  qty_milli: number | null
  /** Единица — литры. */
  liters: boolean
  /** Цена за единицу накладной и сумма строки. */
  price_tyiyn: number | null
  sum_tyiyn: number | null
  /** exact — штрихкод, артикул или запомненный выбор; similar — похожее название (проверьте);
   *  several — по коду несколько товаров (выберите из `candidates`); none — не найден. */
  match: 'exact' | 'similar' | 'several' | 'none'
  product: Product | null
  /** Товар — по запомненному «название у поставщика → товар» (задача 002, П3). */
  remembered: boolean
  /** Другие товары на выбор: похожие (до 3) или все точные совпадения по коду. */
  candidates: Product[]
  /** С какого фото строка: 1 — первое, дальше — «Ещё фото». */
  photo: number
  /** Для найденного товара: штук или канистр и цена за штуку или канистру. */
  count: number | null
  each_tyiyn: number | null
  note: string | null
}

export interface Draft {
  id: string
  source: 'bot' | 'screen'
  status: 'new' | 'used' | 'removed'
  recognition: DraftItem['recognition']
  recognized: boolean
  created_at: string
  user_name: string
  name: string
  content_type: string
  receipt_id: string | null
  receipt_number: number | null
  invoice: {
    readable: boolean
    supplier_name: string
    supplier_inn: string
    doc_number: string
    doc_date: string
    total: string
  } | null
  supplier: { id: string; name: string; by: 'inn' | 'name' } | null
  lines: DraftLine[]
  total_tyiyn: number | null
  lines_sum_tyiyn: number | null
  message: string | null
  /** Фото по порядку: первое и дописанные «Ещё фото» (задача 002, П2). */
  photos: DraftPhoto[]
  /** Фото не подходит к первому — другой поставщик или № накладной: предупреждение. */
  warnings: string[]
  /** Ответ на «Ещё фото»: номер добавленного фото — его строки дописываются в приход. */
  added_photo: number | null
}

export interface DraftPhoto {
  id: string
  no: number
  name: string
  content_type: string
  recognition: DraftItem['recognition']
  supplier_name: string
  doc_number: string
  lines: number
}

/** Строки, которые добавляет ответ: после «Ещё фото» — только нового фото, иначе — все. */
export function newLines(d: Pick<Draft, 'lines' | 'added_photo'>): DraftLine[] {
  return d.added_photo === null ? d.lines : d.lines.filter((l) => l.photo === d.added_photo)
}

/** Строка прихода с тем же товаром, что и новая, — «уже есть в приходе — сложить?». */
export function duplicateOf<L extends { key: string; product: Pick<Product, 'id'> }>(lines: L[], productId: string, key?: string): L | null {
  return lines.find((l) => l.product.id === productId && l.key !== key) ?? null
}

/**
 * Сложить количество двух строк одного товара. Штуки и канистры — целые, литры, кг и метры — до
 * тысячных (как вводятся). `null` — не сложить: разные единицы строки или неверный ввод.
 */
export function addCounts(a: string, b: string, measure: 'whole' | 'milli'): string | null {
  if (measure === 'whole') {
    const [x, y] = [a, b].map((s) => (/^\d+$/.test(s.trim()) ? Number(s.trim()) : null))
    if (x === null || y === null) return null
    const sum = x + y
    return Number.isSafeInteger(sum) ? String(sum) : null
  }
  const x = parseLiters(a)
  const y = parseLiters(b)
  if (x === null || y === null) return null
  const sum = x + y
  return Number.isSafeInteger(sum) ? litersInput(sum) : null
}

export const draftFileUrl = (id: string): string => `/api/v1/receipt-drafts/${id}/file`

/** Количество и цена строки для товара, выбранного вручную; то же правило, что на сервере. */
export function suggestFor(
  line: Pick<DraftLine, 'qty_milli' | 'liters' | 'price_tyiyn' | 'sum_tyiyn' | 'quantity' | 'unit'>,
  product: Pick<Product, 'unit' | 'container_ml'>,
): { count: number | null; each: number | null; note: string | null; measure?: 'l' | 'kg' | 'm' } {
  const notes: string[] = []
  const q = line.qty_milli
  let count: number | null = null
  let each: number | null = line.price_tyiyn
  const c = product.unit === 'ml' ? product.container_ml : null
  if (c && line.liters && q !== null) {
    // Канистра масла, а в накладной литры: 20 л по 4 л → 5 канистр, цена литра × 4.
    if (q % c === 0) {
      count = q / c
      each = line.price_tyiyn === null ? null : divRound(line.price_tyiyn * c, 1000)
      notes.push(`${formatLiters(q)} = ${count} кан. по ${formatLiters(c)}`)
    } else {
      // Не делится на канистры — строка в литрах с ценой литра из накладной (SPEC-36 §13).
      if (each === null && line.sum_tyiyn !== null) each = divRound(line.sum_tyiyn * 1000, q)
      notes.push(`${formatLiters(q)} не делится на канистры по ${formatLiters(c)} — строка в литрах, цена за литр`)
      return { count: q, each, note: notes.join('; '), measure: 'l' }
    }
  } else if ((product.unit === 'g' || product.unit === 'mm') && q !== null) {
    // На вес и в длину: количество — кг (м) до 3 знаков, цена — за кг (м), из суммы — одним округлением
    // (SPEC-36 §13, ADR-175). Единица накладной не «кг»/«м» («500 г», «150 см») — проверить.
    if (each === null && line.sum_tyiyn !== null && q > 0) each = divRound(line.sum_tyiyn * 1000, q)
    const kg = product.unit === 'g'
    const same = kg ? /^(кг|килограмм[а-я]*)\.?$/i.test(line.unit.trim()) : /^(м|метр[а-я]*|пог\.?\s*м)\.?$/i.test(line.unit.trim())
    const note = same ? null : `в накладной «${line.quantity} ${line.unit}» — проверьте единицу: строка в ${kg ? 'кг' : 'метрах'}`
    return { count: q, each, note, measure: kg ? 'kg' : 'm' }
  } else if (q !== null && q % 1000 === 0) {
    count = q / 1000
  } else if (q !== null) {
    notes.push(`количество ${line.quantity} ${line.unit} — укажите целое`)
  } else {
    notes.push('количество не прочитано')
  }
  if (each === null && count !== null && line.sum_tyiyn !== null) {
    each = divRound(line.sum_tyiyn, count)
    notes.push('цена — по сумме строки')
  }
  return { count, each, note: notes.length ? notes.join('; ') : null }
}

/** Строки, которые сразу становятся строками прихода, и строки без товара. */
export function splitLines(d: Pick<Draft, 'lines'>): { found: DraftLine[]; missing: DraftLine[] } {
  return {
    found: d.lines.filter((l) => l.product !== null && (l.match === 'exact' || l.match === 'similar')),
    missing: d.lines.filter((l) => l.product === null || l.match === 'none' || l.match === 'several'),
  }
}

/** Вариант товара одной строкой: артикул, остаток, последняя цена прихода. */
export function candidateText(p: Pick<Product, 'article' | 'last_purchase_price_tyiyn'>, stock: string): string {
  return [p.article, `остаток ${stock}`, p.last_purchase_price_tyiyn !== null ? `прошлый приход ${formatSom(p.last_purchase_price_tyiyn)}` : null]
    .filter(Boolean)
    .join(' · ')
}

/** Сверка итога накладной с суммой строк прихода; `null` — сходится или сверять нечего. */
export function totalMismatch(invoiceTotal: number | null, receiptTotal: number | null): string | null {
  if (invoiceTotal === null || receiptTotal === null || invoiceTotal === receiptTotal) return null
  return `Итог накладной ${formatSom(invoiceTotal)}, сумма строк ${formatSom(receiptTotal)} — расходится на ${formatSom(Math.abs(invoiceTotal - receiptTotal))}`
}

/** «Номер документа поставщика»: номер и дата из накладной. */
export function docText(inv: Draft['invoice']): string {
  if (!inv) return ''
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(inv.doc_date)
  const date = m ? `${m[3]}.${m[2]}.${m[1]}` : ''
  return [inv.doc_number, date && `от ${date}`].filter(Boolean).join(' ')
}

/** Что с распознаванием — для строки списка. */
export function recognitionText(d: Pick<DraftItem, 'recognition' | 'lines'>): string | null {
  switch (d.recognition) {
    case 'off':
      return 'не распознана: распознавание не настроено'
    case 'limit':
      return 'не распознана: был исчерпан лимит часа'
    case 'pending':
      return 'распознаётся…'
    case 'failed':
      return 'не удалось распознать'
    default:
      return d.lines === 0 ? 'не удалось распознать' : null
  }
}
