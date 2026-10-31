// Выгрузка таблиц экрана в Excel (задача 003 Н13, ADR-159): описание столбцов, все строки
// отбора (а не одна страница), имя файла и скачивание. Файл собирается из того, что сервер уже
// отдал этому пользователю (I12, I13): столбцы выгрузки повторяют условия столбцов экрана.
import { get, getPage, type Page } from './api'
import { formatDate, formatDateTime } from './format'
import { writeXlsx, type Cell, type Sheet } from './xlsxWrite'

/** Столбец выгрузки: подпись и значение строки. */
export interface ExportCol<T> {
  label: string
  value: (row: T) => Cell
  width?: number
}

/** Ячейки из целых — те же типы, что у `writeXlsx`; перевод в сомы/литры — в нём, без float. */
export const xl = {
  money: (tyiyn: number): Cell => ({ money: tyiyn }),
  int: (n: number): Cell => ({ int: n }),
  /** Миллилитры → литры с тремя знаками. */
  liters: (ml: number): Cell => ({ qty: ml, scale: 3 }),
  /** Количество в единице товара: мл → литры, граммы → кг (три знака), остальное — целым. */
  units: (units: number, unit: string | undefined): Cell => (unit === 'ml' || unit === 'g' ? { qty: units, scale: 3 } : { int: units }),
  date: (iso: string | null | undefined): Cell => (iso ? { date: iso.slice(0, 10) } : null),
  /** Дата и время текстом, как на экране: «31.10.2026 14:05». */
  dateTime: (iso: string | null | undefined): Cell => (iso ? formatDateTime(iso) : ''),
  text: (s: string | null | undefined): Cell => s ?? '',
  /** Деньги, если сервер их отдал (закупка — только владельцу, I13); нет — пустая ячейка. */
  maybeMoney: (tyiyn: number | null | undefined): Cell => (tyiyn == null ? null : { money: tyiyn }),
}

/** Единица количества в файле: «л», «кг», «шт» — числа в столбце количества идут в ней. */
export function unitName(unit: string | undefined): string {
  return unit === 'ml' ? 'л' : unit === 'g' ? 'кг' : 'шт'
}

/** Подпись над таблицей: название, отбор (непустые части), когда выгружено. */
export function exportTitle(name: string, filter: (string | false | null | undefined)[] = [], now = new Date()): string[] {
  const parts = filter.filter((x): x is string => Boolean(x))
  return [name, ...(parts.length ? [`Отбор: ${parts.join('; ')}`] : []), `Выгружено: ${formatDateTime(now.toISOString())}`]
}

/** Один лист с подписью отбора — частый случай кнопки «В Excel». */
export function listSheet<T>(
  name: string,
  cols: ExportCol<T>[],
  rows: T[],
  opts: { filter?: (string | false | null | undefined)[]; total?: Cell[]; warning?: string; sheet?: string } = {},
): Sheet {
  return tableSheet(opts.sheet ?? name, cols, rows, { title: exportTitle(name, opts.filter), total: opts.total, warning: opts.warning })
}

/** Строка «Итого» по подписям столбцов: остальные ячейки пустые (столбцы бывают по условию). */
export function totalBy<T>(cols: ExportCol<T>[], cells: Record<string, Cell>): Cell[] {
  return cols.map((c) => cells[c.label] ?? null)
}

/** Столбцы, которые есть только при условии (как столбцы экрана: `owner && …`). */
export function colsIf<T>(on: boolean, cols: ExportCol<T>[]): ExportCol<T>[] {
  return on ? cols : []
}

/** Лист из строк и столбцов; `total` — строка «Итого» (жирная). */
export function tableSheet<T>(
  name: string,
  cols: ExportCol<T>[],
  rows: T[],
  opts: { title?: string[]; footer?: string[]; total?: Cell[]; warning?: string } = {},
): Sheet {
  return {
    name,
    columns: cols.map((c) => ({ label: c.label, width: c.width })),
    rows: [...rows.map((r) => cols.map((c) => c.value(r))), ...(opts.total ? [{ cells: opts.total, bold: true }] : [])],
    title: opts.title,
    footer: opts.footer,
    warning: opts.warning,
  }
}

/** Сколько строк выгружается из длинного списка: как `MAX_ROWS` сервера (paging.rs). */
export const EXPORT_MAX_ROWS = 10_000
/** Страница запроса при выгрузке — как `MAX_PAGE` сервера. */
export const EXPORT_PAGE = 500

export interface AllPages<T> {
  items: T[]
  /** Сколько строк во всём отборе (по заголовку `X-Total-Count`). */
  total: number
  /** Упёрлись в потолок `EXPORT_MAX_ROWS`: выгружены не все строки. */
  capped: boolean
}

/**
 * Все строки списка, который режет сервер: страницы по 500, пока не наберётся `total`, но не
 * больше 10 000 строк (не больше 20 запросов). Обрыв связи посередине — ошибка, файла нет.
 */
export async function fetchAllPages<T>(
  path: string,
  load: (path: string, page: { limit: number; offset: number }) => Promise<Page<T>> = getPage,
  max = EXPORT_MAX_ROWS,
): Promise<AllPages<T>> {
  const items: T[] = []
  let total = 0
  for (let offset = 0; offset < max; ) {
    const limit = Math.min(EXPORT_PAGE, max - offset)
    const page = await load(path, { limit, offset })
    total = page.total
    items.push(...page.items)
    offset += page.items.length
    if (page.items.length === 0 || offset >= total) break
  }
  return { items: items.slice(0, max), total, capped: total > max }
}

/**
 * Все товары отбора `/products/list` (ответ — `{ items, totals.count }`, страница до 2000): по 2000,
 * пока не наберётся `count`, не больше 10 000.
 */
export async function fetchAllProducts<T, R extends { items: T[]; totals: { count: number } }>(
  path: string,
  load: (path: string) => Promise<R> = get,
  max = EXPORT_MAX_ROWS,
): Promise<AllPages<T> & { first: R | null }> {
  const items: T[] = []
  let total = 0
  let first: R | null = null
  const sep = path.includes('?') ? '&' : '?'
  for (let offset = 0; offset < max; ) {
    const limit = Math.min(2000, max - offset)
    const page = await load(`${path}${sep}limit=${limit}&offset=${offset}`)
    first ??= page
    total = page.totals.count
    items.push(...page.items)
    offset += page.items.length
    if (page.items.length === 0 || offset >= total) break
  }
  return { items: items.slice(0, max), total, capped: total > max, first }
}

/** Предупреждение, если выгружены не все строки. */
export function cappedWarning(all: { total: number; capped: boolean }): string | undefined {
  return all.capped
    ? `Выгружены первые ${EXPORT_MAX_ROWS.toLocaleString('ru-RU')} строк из ${all.total.toLocaleString('ru-RU')} — сузьте период или поиск`
    : undefined
}

/** Подпись периода: «01.10.2026–31.10.2026» или один день. */
export function periodText(from: string, to: string): string {
  return from === to ? formatDate(from) : `${formatDate(from)}–${formatDate(to)}`
}

/** Имя файла: «Прибыль и убытки 01.10.2026–31.10.2026.xlsx»; знаки, недопустимые в именах файлов, убираются. */
export function fileName(base: string, period?: string): string {
  const name = [base, period]
    .filter(Boolean)
    .join(' ')
    // eslint-disable-next-line no-control-regex
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  return `${name || 'Таблица'}.xlsx`
}

/** Скачать файл: ссылка с `download` и временный адрес. */
export function downloadBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.rel = 'noopener'
  document.body.appendChild(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 10_000)
}

/** Собрать книгу и скачать её. */
export function downloadXlsx(name: string, sheets: Sheet[]): void {
  downloadBlob(writeXlsx(sheets), name)
}
