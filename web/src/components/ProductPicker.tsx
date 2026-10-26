// Поле сканера и поиска товара для кассы и прихода (SPEC-02, SPEC-04); без сети ищет по снимку каталога (SPEC-09).
// Где виден Умаг (приход, ревизия — `umag`), подсказывает и строки старой базы Умаг (SPEC-20,
// SPEC-29 §1, SPEC-35 §2); на кассе и в остальных выборах товара Умага нет.
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { ApiError, get, qs } from '../lib/api'
import { productTitle } from '../lib/battery'
import { formatKg, formatOilStock, formatSom } from '../lib/format'
import { useDebounced } from '../lib/hooks'
import { resolveEnter, resultsMatch } from '../lib/pickerResults'
import { findByBarcode, readSnapshot, searchSnapshot } from '../lib/offline'
import type { OldCatalogHit, Product } from '../lib/types'
import { searchUmag, umagStep, unlinkedHit } from '../lib/umagLink'
import { UmagHitModal, UmagPickerBlock, type UmagDone } from './Umag'

/** Как выбрали товар: сканом (код целиком) или из списка найденных. */
export type PickHow = 'scan' | 'search'

type Entry = { kind: 'own'; p: Product } | { kind: 'old'; h: OldCatalogHit }

/**
 * Поле для сканера и поиска. Enter со строкой из цифр — точный поиск по штрихкоду,
 * иначе — выбор из списка найденных. Набранный или вставленный код ищется и без Enter:
 * сервер ищет его среди кодов товара наравне с названием и артикулом.
 * Незнакомый код передаётся в `onUnknownCode`; где виден Умаг — сначала ищется в старой базе Умаг.
 */
export function ProductPicker({
  onPick,
  onUnknownCode,
  onCreate,
  umag = false,
  umagAddLabel,
  onPickOld,
  autoFocus = true,
  placeholder = 'Сканируйте штрихкод, введите название или бренд',
  inputId,
}: {
  /** `umagDone` — товар заведён из строки Умага или связан с ней в окне Умага (закупка оттуда). */
  onPick: (p: Product, how: PickHow, umagDone?: UmagDone) => void
  onUnknownCode?: (code: string) => void
  /** «Новый товар» прямо из списка: передаётся набранный текст. */
  onCreate?: (text: string) => void
  /** Умаг виден (SPEC-35 §2): блок «Из старой базы Умаг» в списке и окно строки по коду, которого нет
   *  у товаров. Только «Приход» и ревизия; на кассе и в остальных выборах товара — нет. */
  umag?: boolean
  /** Подпись «Завести» в окне Умага: в приходе — «Завести и добавить». */
  umagAddLabel?: string
  /** Своя обработка несвязанной строки Умага (ревизия заполняет свою карточку) вместо окна Умага;
   *  включает Умаг. */
  onPickOld?: (hit: OldCatalogHit, code: string) => void
  autoFocus?: boolean
  placeholder?: string
  inputId?: string
}) {
  const [text, setText] = useState('')
  const [results, setResults] = useState<Product[]>([])
  // По какому запросу найдены `results`: после выбора список не стирается, а скрывается — иначе
  // тот же запрос, набранный снова до срабатывания задержки, не перезапрашивался и давал «Ничего не нашли».
  const [resultsFor, setResultsFor] = useState('')
  const [old, setOld] = useState<OldCatalogHit[]>([])
  const [active, setActive] = useState(0)
  const [message, setMessage] = useState<string | null>(null)
  /** Окно строки Умага: из списка (`list`) — сразу карточка, по коду — сведения и кнопки. */
  const [umagHit, setUmagHit] = useState<{ hit: OldCatalogHit; code: string; list: boolean } | null>(null)
  const input = useRef<HTMLInputElement>(null)
  const query = useDebounced(text.trim(), 200)
  const withOld = umag || Boolean(onPickOld)
  // Что в поле сейчас — для Enter, который ждёт ответа сервера (SPEC-30 §13).
  const textNow = useRef(text)
  useLayoutEffect(() => {
    textNow.current = text
  }, [text])
  // Строку выбрали стрелками при этом тексте: Enter берёт её, а не первое из нового ответа.
  const navigated = useRef<string | null>(null)
  const entering = useRef(false)

  useEffect(() => {
    if (query.length < 2) {
      setResults([])
      setResultsFor(query)
      setOld([])
      return
    }
    let alive = true
    get<Product[]>(`/products${qs({ q: query, limit: 15 })}`)
      .then((r) => {
        if (alive) {
          setResults(r)
          setResultsFor(query)
          setActive(0)
        }
      })
      .catch(async () => {
        // Нет связи — ищем в снимке каталога на устройстве (SPEC-09).
        const snap = await readSnapshot()
        if (alive) {
          setResults(snap ? searchSnapshot(snap, query) : [])
          setResultsFor(query)
          setActive(0)
        }
      })
    if (withOld && !/^\d{6,}$/.test(query)) {
      get<OldCatalogHit[]>(`/old-catalog/search${qs({ q: query })}`)
        .then((h) => alive && setOld(h.filter(unlinkedHit).slice(0, 8)))
        .catch(() => alive && setOld([]))
    } else {
      setOld([])
    }
    return () => {
      alive = false
    }
  }, [query, withOld])

  // Показываем только найденное по набранному тексту (или по его началу, пока идёт набор).
  const fresh = resultsFor.length >= 2 && resultsMatch(text, resultsFor)
  const shownResults = fresh ? results : []
  const shownOld = fresh ? old : []
  const entries: Entry[] = [...shownResults.map((p) => ({ kind: 'own' as const, p })), ...shownOld.map((h) => ({ kind: 'old' as const, h }))]

  const clear = () => {
    setText('')
    setActive(0)
    setMessage(null)
  }

  const pick = (p: Product, how: PickHow, umagDone?: UmagDone) => {
    onPick(p, how, umagDone)
    clear()
    input.current?.focus()
  }

  const pickOld = (h: OldCatalogHit, code: string) => {
    clear()
    if (onPickOld) onPickOld(h, code)
    else setUmagHit({ hit: h, code, list: code === '' })
  }

  /** Кода нет у товаров — есть ли он в Умаге (SPEC-35 §2): связанная строка — её товар,
   *  несвязанная — окно Умага. Нет в Умаге — false. */
  const fromUmag = async (code: string): Promise<boolean> => {
    const step = umagStep(await searchUmag({ barcode: code }).catch(() => []))
    if (step.kind === 'none') return false
    if (step.kind === 'umag') {
      pickOld(step.hit, code)
      return true
    }
    try {
      pick(await get<Product>(`/products/${step.productId}`), 'scan')
    } catch (e) {
      setMessage(e instanceof Error ? e.message : 'Ошибка')
    }
    return true
  }

  const choose = (e: Entry | undefined) => {
    if (!e) return
    if (e.kind === 'own') pick(e.p, 'search')
    else pickOld(e.h, '')
  }

  /** Найденное по всему тексту — сразу, без задержки набора: свои товары, затем старая программа. */
  const searchFull = async (q: string): Promise<Entry[]> => {
    const own = await get<Product[]>(`/products${qs({ q, limit: 15 })}`).catch(async () => {
      const snap = await readSnapshot()
      return snap ? searchSnapshot(snap, q) : []
    })
    const hits =
      withOld && own.length === 0
        ? (await get<OldCatalogHit[]>(`/old-catalog/search${qs({ q })}`).catch(() => [])).filter(unlinkedHit).slice(0, 8)
        : []
    if (textNow.current.trim() === q) {
      setResults(own)
      setOld(hits)
      setResultsFor(q)
      setActive(0)
    }
    return [...own.map((p) => ({ kind: 'own' as const, p })), ...hits.map((h) => ({ kind: 'old' as const, h }))]
  }

  const submit = async () => {
    const code = text.trim()
    if (!code || entering.current) return
    // Enter берёт товар только из найденного по всему набранному тексту (SPEC-30 §13).
    entering.current = true
    const out = await resolveEnter(
      { text, foundFor: fresh ? resultsFor : '', found: entries, active, navigated: navigated.current === code },
      searchFull,
      () => textNow.current,
    ).finally(() => {
      entering.current = false
    })
    if (out.kind === 'stale') return
    if (out.kind === 'pick') {
      choose(out.item)
      return
    }
    try {
      pick(await get<Product>(`/products/by-barcode/${encodeURIComponent(code)}`), 'scan')
    } catch (e) {
      if (e instanceof ApiError && e.status === 0) {
        const snap = await readSnapshot()
        const found = snap ? findByBarcode(snap, code) : undefined
        if (found) {
          pick(found, 'scan')
          return
        }
        setMessage(
          withOld
            ? `Без сети: код ${code} среди товаров не найден. Если он есть в базе Умаг, завести товар можно, когда будет связь`
            : `Без сети: код ${code} в снимке каталога не найден`,
        )
        return
      }
      if (e instanceof ApiError && e.status === 404) {
        // Нет у нас — может, есть в старой базе Умаг: связанная строка даст товар, несвязанная — окно.
        if (withOld && (await fromUmag(code))) return
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
        id={inputId}
        data-picker
        autoFocus={autoFocus}
        className="w-full text-base"
        placeholder={placeholder}
        value={text}
        onChange={(e) => {
          setText(e.target.value)
          setMessage(null)
          navigated.current = null
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.preventDefault()
            void submit()
          } else if (e.key === 'ArrowDown') {
            e.preventDefault()
            navigated.current = text.trim()
            setActive((a) => Math.min(a + 1, entries.length - 1))
          } else if (e.key === 'ArrowUp') {
            e.preventDefault()
            navigated.current = text.trim()
            setActive((a) => Math.max(a - 1, 0))
          } else if (e.key === 'Escape') {
            setResults([])
            setOld([])
          }
        }}
      />
      {message && <div className="mt-1 text-sm text-rose-600">{message}</div>}
      {query.length >= 2 && text.trim().length >= 2 && (entries.length > 0 || Boolean(onCreate)) && (
        <ul className="absolute z-30 mt-1 max-h-96 w-full overflow-y-auto rounded-md border border-slate-200 bg-white shadow-lg">
          {fresh && results.length === 0 && <li className="px-3 py-2 text-sm text-slate-500">Ничего не нашли по «{resultsFor}»</li>}
          {shownResults.map((p, i) => (
            <li key={p.id}>
              <button
                type="button"
                className={`flex w-full items-center justify-between gap-3 px-3 py-2 text-left text-sm ${i === active ? 'bg-sky-50' : 'hover:bg-slate-50'}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => pick(p, 'search')}
              >
                <span>
                  <span className="font-medium">{productTitle(p)}</span>
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
          {shownOld.length > 0 && (
            <UmagPickerBlock hits={shownOld} offset={shownResults.length} active={active} onHover={setActive} onChoose={(h) => pickOld(h, '')} />
          )}
          {onCreate && (
            <li className="border-t border-slate-200">
              <button
                type="button"
                className="w-full px-3 py-2 text-left text-sm font-medium text-sky-700 hover:bg-sky-50"
                onClick={() => {
                  const t = query
                  clear()
                  onCreate(t)
                }}
              >
                + Завести товар «{query}»
              </button>
            </li>
          )}
        </ul>
      )}
      {umagHit && (
        <UmagHitModal
          hit={umagHit.hit}
          code={umagHit.code}
          start={umagHit.list ? 'create' : 'info'}
          addLabel={umagAddLabel}
          onClose={() => {
            setUmagHit(null)
            input.current?.focus()
          }}
          onDone={(p, done) => {
            setUmagHit(null)
            pick(p, 'search', done)
          }}
        />
      )}
    </div>
  )
}

/** Остаток товара текстом. */
export function stockText(p: Pick<Product, 'unit' | 'container_ml' | 'stock_qty'>): string {
  if (p.unit === 'g') return formatKg(p.stock_qty)
  return p.unit === 'ml' && p.container_ml ? formatOilStock(p.stock_qty, p.container_ml) : `${p.stock_qty} шт`
}
