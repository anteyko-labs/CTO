// Выгрузка товаров старой программы (Excel): строки по заголовкам столбцов — для списка старой
// программы (SPEC-20) и для услуг на новой точке (SPEC-02, проверка 20.10.2026).
import { somToTyiyn } from './xlsx'

export interface UmagRow {
  name: string
  barcode: string
  article: string
  extra_code: string
  cost_tyiyn: number
  price_tyiyn: number
  unit: string
  supplier: string
  category: string
}

/** Строки выгрузки → строки для сервера; столбцы ищем по заголовкам первой строки. */
export function mapUmagRows(rows: string[][]): UmagRow[] {
  const head = rows[0]?.map((h) => h.toLowerCase()) ?? []
  const col = (...names: string[]) => head.findIndex((h) => names.some((n) => h.includes(n)))
  const c = {
    name: col('название', 'наименование'),
    barcode: col('штрихкод', 'штрих-код'),
    article: col('артикул'),
    extra: col('доп. код', 'доп код'),
    cost: col('закуп'),
    price: col('прод. цена', 'цена прод', 'розн'),
    unit: col('ед. изм', 'ед изм', 'единица'),
    supplier: col('поставщик'),
    category: col('категория'),
  }
  if (c.name < 0) throw new Error('Не нашёл столбец «Название товара» в первой строке файла')
  const at = (r: string[], i: number) => (i >= 0 ? (r[i] ?? '') : '')
  return rows.slice(1).map((r) => ({
    name: at(r, c.name),
    barcode: at(r, c.barcode),
    article: at(r, c.article),
    extra_code: at(r, c.extra),
    cost_tyiyn: somToTyiyn(at(r, c.cost)),
    price_tyiyn: somToTyiyn(at(r, c.price)),
    unit: at(r, c.unit),
    supplier: at(r, c.supplier),
    category: at(r, c.category),
  }))
}

/** Строка выгрузки как кандидат в услуги. */
export interface ServiceCandidate {
  name: string
  price_tyiyn: number
  category: string
  unit: string
  /** Похоже на услугу: категория «Незаданные» или нет закупочной цены, единица — штуки. */
  likely: boolean
  /** Такая услуга уже есть (без учёта регистра) — не загружается. */
  exists: boolean
}

const key = (name: string) => name.trim().toLowerCase()

/** Штуки: «шт», «шт.», «штука», пусто. Литры и килограммы — товар, не услуга. */
const isPieces = (unit: string) => {
  const u = unit.trim().toLowerCase()
  return u === '' || u.startsWith('шт')
}

/**
 * Кандидаты в услуги из выгрузки: без пустых названий и повторов. Отмечены заранее те, что похожи
 * на услугу и которых ещё нет; похожие — сверху.
 */
export function serviceCandidates(rows: UmagRow[], existing: readonly string[]): ServiceCandidate[] {
  const have = new Set(existing.map(key))
  const seen = new Set<string>()
  const out: ServiceCandidate[] = []
  for (const r of rows) {
    const name = r.name.trim()
    if (!name || seen.has(key(name))) continue
    seen.add(key(name))
    const undefinedCategory = r.category.trim() === '' || r.category.trim().toLowerCase().startsWith('незадан')
    out.push({
      name,
      price_tyiyn: r.price_tyiyn,
      category: r.category.trim(),
      unit: r.unit.trim(),
      likely: (undefinedCategory || r.cost_tyiyn === 0) && isPieces(r.unit),
      exists: have.has(key(name)),
    })
  }
  const rank = (c: ServiceCandidate) => (c.exists ? 2 : c.likely ? 0 : 1)
  return out.map((c, i) => [c, i] as const).sort((a, b) => rank(a[0]) - rank(b[0]) || a[1] - b[1]).map(([c]) => c)
}

/** Отмеченные заранее: похожие на услугу и которых ещё нет. */
export const preselected = (list: readonly ServiceCandidate[]): Set<string> => new Set(list.filter((c) => c.likely && !c.exists).map((c) => c.name))
