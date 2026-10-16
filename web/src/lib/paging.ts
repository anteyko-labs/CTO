// Страницы длинных списков (SPEC-28 §2): номера страниц с пропусками, размер страницы, границы.
// Одна логика для всех списков — и тех, что режет сервер, и тех, что пришли целиком.

/** Размеры страницы на выбор. */
export const PAGE_SIZES = [25, 50, 100] as const
export type PageSize = (typeof PAGE_SIZES)[number]
export const DEFAULT_PAGE_SIZE: PageSize = 50

/** Сколько страниц у `total` строк по `size` (хотя бы одна). */
export const pageCount = (total: number, size: number): number => Math.max(1, Math.ceil(Math.max(0, total) / Math.max(1, size)))

/** Номер страницы (с нуля) в допустимых границах. */
export const clampPage = (page: number, total: number, size: number): number => Math.min(Math.max(0, page), pageCount(total, size) - 1)

/**
 * Кнопки страниц: первая, последняя и соседи текущей, пропуски — `'…'`. Номера с нуля.
 * 12 страниц, текущая 5 → [0, '…', 4, 5, 6, '…', 11].
 */
export function pageItems(current: number, pages: number, around = 1): (number | '…')[] {
  if (pages <= 1) return [0]
  const out: (number | '…')[] = []
  let prev = -1
  for (let i = 0; i < pages; i++) {
    const near = Math.abs(i - current) <= around
    // Пропуск из одной страницы не прячем: «1 2 3», а не «1 … 3».
    const edge = i === 0 || i === pages - 1 || (current <= around + 2 && i <= around * 2 + 2) || (current >= pages - around - 3 && i >= pages - around * 2 - 3)
    if (near || edge) {
      if (prev >= 0 && i - prev > 1) out.push(i - prev === 2 ? prev + 1 : '…')
      out.push(i)
      prev = i
    }
  }
  return out
}

/** Строки «с–по / всего» для полосы под таблицей, как в старой программе: «1–25 / 312». */
export function rangeText(page: number, size: number, total: number): string {
  if (total <= 0) return '0 / 0'
  const from = page * size + 1
  const to = Math.min(total, from + size - 1)
  return `${from}–${to} / ${total}`
}

/** Размер страницы из сохранённого значения: только из списка, иначе по умолчанию. */
export function parsePageSize(v: unknown): PageSize {
  const n = Number(v)
  return (PAGE_SIZES as readonly number[]).includes(n) ? (n as PageSize) : DEFAULT_PAGE_SIZE
}
