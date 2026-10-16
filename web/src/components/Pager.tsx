// Страницы длинных списков (SPEC-28 §2): «← 1 2 3 … 12 →» и размер страницы 25/50/100.
// `usePager` — список пришёл целиком и режется на экране; `usePagedLoad` — страницу режет сервер
// (`limit`/`offset`, число строк — в заголовке `X-Total-Count`). Размер страницы запоминается
// на устройстве для каждого списка.
import { useMemo, useState } from 'react'
import { getPage, type Page } from '../lib/api'
import { useLoad } from '../lib/hooks'
import { clampPage, DEFAULT_PAGE_SIZE, PAGE_SIZES, pageCount, pageItems, parsePageSize, rangeText, type PageSize } from '../lib/paging'

const SIZE_KEY = 'avtodom.page_size.'

function readSize(key: string): PageSize {
  try {
    return parsePageSize(localStorage.getItem(SIZE_KEY + key))
  } catch {
    return DEFAULT_PAGE_SIZE
  }
}

function saveSize(key: string, size: PageSize) {
  try {
    localStorage.setItem(SIZE_KEY + key, String(size))
  } catch {
    // Без хранилища размер просто не запомнится.
  }
}

/**
 * Состояние страниц: номер (с нуля) и размер. Смена `scope` (отбор, сортировка, поиск) сама
 * возвращает на первую страницу.
 */
export function usePageState(key: string, scope = '') {
  const [at, setAt] = useState({ scope, page: 0 })
  const [size, setSizeState] = useState<PageSize>(() => readSize(key))
  const page = at.scope === scope ? at.page : 0
  const setPage = (p: number) => setAt({ scope, page: p })
  const setSize = (s: PageSize) => {
    saveSize(key, s)
    setSizeState(s)
    setAt({ scope, page: 0 })
  }
  return { page, size, setPage, setSize }
}

export type PageState = ReturnType<typeof usePageState>

/** Список пришёл целиком: страница из него. Смена длины списка возвращает в границы. */
export function usePager<T>(rows: readonly T[], key: string, scope = '') {
  const st = usePageState(key, scope)
  const current = clampPage(st.page, rows.length, st.size)
  const shown = useMemo(() => rows.slice(current * st.size, current * st.size + st.size), [rows, current, st.size])
  return { ...st, page: current, shown, total: rows.length }
}

/**
 * Страницу режет сервер: `path` — адрес без `limit`/`offset`. При смене `path` (отбор, сортировка)
 * — снова первая страница.
 */
export function usePagedLoad<T>(path: string, key: string) {
  const st = usePageState(key, path)
  const load = useLoad<Page<T>>(() => getPage<T>(path, { limit: st.size, offset: st.page * st.size }), [path, st.page, st.size])
  return { ...st, ...load, items: load.data?.items ?? [], total: load.data?.total ?? 0 }
}

/**
 * Полоса страниц под таблицей, как в старой программе: «⏮ ◀ 1 2 3 … 13 ▶ ⏭  1–25 / 312  по 25».
 * Одна страница и строк не больше меньшего размера — ничего не показывает. `label` — для чтения
 * с экрана и тестов («Страницы приходов»).
 */
export function Pager({
  page,
  size,
  total,
  onPage,
  onSize,
  label = 'Страницы',
}: {
  page: number
  size: PageSize
  total: number
  onPage: (p: number) => void
  onSize?: (s: PageSize) => void
  label?: string
}) {
  const pages = pageCount(total, size)
  if (pages <= 1 && total <= PAGE_SIZES[0]) return null
  const btn =
    'inline-flex min-h-9 min-w-9 items-center justify-center rounded-md border border-slate-300 bg-white px-2 text-sm tabular-nums hover:bg-slate-50 disabled:cursor-not-allowed disabled:text-slate-300 disabled:hover:bg-white'
  const last = pages - 1
  return (
    <nav
      aria-label={label}
      className="no-print mt-3 flex flex-wrap items-center justify-between gap-2 rounded-md border border-slate-200 bg-slate-50 px-2 py-1.5 text-sm"
    >
      <div className="flex flex-wrap items-center gap-1">
        <button type="button" className={btn} disabled={page === 0} aria-label="Первая страница" title="Первая страница" onClick={() => onPage(0)}>
          ⏮
        </button>
        <button type="button" className={btn} disabled={page === 0} aria-label="Предыдущая страница" title="Предыдущая страница" onClick={() => onPage(page - 1)}>
          ◀
        </button>
        {pageItems(page, pages).map((it, i) =>
          it === '…' ? (
            <span key={`gap-${i}`} className="px-1 text-slate-400">
              …
            </span>
          ) : (
            <button
              key={it}
              type="button"
              aria-current={it === page ? 'page' : undefined}
              className={`${btn} ${it === page ? '!border-sky-600 !bg-sky-600 font-semibold text-white' : ''}`}
              onClick={() => onPage(it)}
            >
              {it + 1}
            </button>
          ),
        )}
        <button type="button" className={btn} disabled={page >= last} aria-label="Следующая страница" title="Следующая страница" onClick={() => onPage(page + 1)}>
          ▶
        </button>
        <button type="button" className={btn} disabled={page >= last} aria-label="Последняя страница" title="Последняя страница" onClick={() => onPage(last)}>
          ⏭
        </button>
      </div>
      <div className="flex items-center gap-3">
        <span className="font-medium tabular-nums text-slate-700" data-testid="pager-range">
          {rangeText(page, size, total)}
        </span>
        {onSize && (
          <label className="flex items-center gap-1 text-slate-600">
            по
            <select aria-label="Строк на странице" className="py-1" value={size} onChange={(e) => onSize(parsePageSize(e.target.value))}>
              {PAGE_SIZES.map((s) => (
                <option key={s} value={s}>
                  {s}
                </option>
              ))}
            </select>
          </label>
        )}
      </div>
    </nav>
  )
}
