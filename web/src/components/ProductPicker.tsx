import { useEffect, useRef, useState } from 'react'
import { ApiError, get, qs } from '../lib/api'
import { formatOilStock, formatSom } from '../lib/format'
import { useDebounced } from '../lib/hooks'
import type { Product } from '../lib/types'

/**
 * Поле для сканера и поиска. Enter со строкой из цифр — точный поиск по штрихкоду,
 * иначе — выбор из списка найденных. Набранный или вставленный код ищется и без Enter:
 * сервер ищет его среди кодов товара наравне с названием и артикулом.
 * Незнакомый код передаётся в `onUnknownCode`.
 */
export function ProductPicker({
  onPick,
  onUnknownCode,
  autoFocus = true,
  placeholder = 'Сканируйте штрихкод или введите название',
}: {
  onPick: (p: Product) => void
  onUnknownCode?: (code: string) => void
  autoFocus?: boolean
  placeholder?: string
}) {
  const [text, setText] = useState('')
  const [results, setResults] = useState<Product[]>([])
  const [active, setActive] = useState(0)
  const [message, setMessage] = useState<string | null>(null)
  const input = useRef<HTMLInputElement>(null)
  const query = useDebounced(text.trim(), 200)

  useEffect(() => {
    if (query.length < 2) {
      setResults([])
      return
    }
    let alive = true
    get<Product[]>(`/products${qs({ q: query, limit: 15 })}`)
      .then((r) => {
        if (alive) {
          setResults(r)
          setActive(0)
        }
      })
      .catch(() => alive && setResults([]))
    return () => {
      alive = false
    }
  }, [query])

  const pick = (p: Product) => {
    onPick(p)
    setText('')
    setResults([])
    setMessage(null)
    input.current?.focus()
  }

  const submit = async () => {
    const code = text.trim()
    if (!code) return
    if (results.length > 0 && !/^\d{6,}$/.test(code)) {
      pick(results[active] ?? results[0])
      return
    }
    try {
      pick(await get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`))
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        setText('')
        if (onUnknownCode) onUnknownCode(code)
        else setMessage(`Код ${code} не найден`)
      } else {
        setMessage(e instanceof Error ? e.message : 'Ошибка')
      }
    }
  }

  return (
    <div className="relative">
      <input
        ref={input}
        data-picker
        autoFocus={autoFocus}
        className="w-full text-base"
        placeholder={placeholder}
        value={text}
        onChange={(e) => {
          setText(e.target.value)
          setMessage(null)
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            void submit()
          } else if (e.key === 'ArrowDown') {
            e.preventDefault()
            setActive((a) => Math.min(a + 1, results.length - 1))
          } else if (e.key === 'ArrowUp') {
            e.preventDefault()
            setActive((a) => Math.max(a - 1, 0))
          } else if (e.key === 'Escape') {
            setResults([])
          }
        }}
      />
      {message && <div className="mt-1 text-sm text-rose-600">{message}</div>}
      {results.length > 0 && (
        <ul className="absolute z-30 mt-1 max-h-80 w-full overflow-y-auto rounded-md border border-slate-200 bg-white shadow-lg">
          {results.map((p, i) => (
            <li key={p.id}>
              <button
                type="button"
                className={`flex w-full items-center justify-between gap-3 px-3 py-2 text-left text-sm ${i === active ? 'bg-sky-50' : 'hover:bg-slate-50'}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(p)}
              >
                <span>
                  <span className="font-medium">{p.name}</span>
                  {p.article && <span className="ml-2 text-slate-500">{p.article}</span>}
                </span>
                <span className="shrink-0 text-right text-xs text-slate-500">
                  {formatSom(p.sale_price_tyiyn)}
                  <br />
                  {p.unit === 'ml' && p.container_ml ? formatOilStock(p.stock_qty, p.container_ml) : `${p.stock_qty} шт`}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/** Остаток товара текстом. */
export function stockText(p: Pick<Product, 'unit' | 'container_ml' | 'stock_qty'>): string {
  return p.unit === 'ml' && p.container_ml ? formatOilStock(p.stock_qty, p.container_ml) : `${p.stock_qty} шт`
}
