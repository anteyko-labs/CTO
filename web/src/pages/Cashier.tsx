import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { ProductFormModal } from '../components/ProductFormModal'
import { ProductPicker, stockText } from '../components/ProductPicker'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatLiters, formatSom, parseLiters, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { pourAmount } from '../lib/money'
import { PAYMENT_LABELS, type Employee, type PaymentMethod, type Product, type Sale, type SaleLineKind } from '../lib/types'

interface CartLine {
  key: string
  kind: SaleLineKind
  product?: Product
  qtyText: string
  priceText: string
}

type PayMode = PaymentMethod | 'mixed'

const CASHIER_KEY = 'avtodom.cashier_id'

function remembered(key: string): string {
  try {
    return localStorage.getItem(key) ?? ''
  } catch {
    return ''
  }
}

function remember(key: string, value: string): void {
  try {
    localStorage.setItem(key, value)
  } catch {
    // Хранилище недоступно — выбор просто не запомнится.
  }
}

function listPrice(l: Pick<CartLine, 'kind' | 'product'>): number {
  if (l.kind === 'pour') return l.product?.pour_price_per_l_tyiyn ?? 0
  return l.product?.sale_price_tyiyn ?? 0
}

/** Количество строки в единицах запроса: шт, канистры или мл для розлива. */
function lineQty(l: CartLine): number | null {
  if (l.kind === 'pour') {
    const ml = parseLiters(l.qtyText)
    return ml && ml > 0 ? ml : null
  }
  return /^\d+$/.test(l.qtyText.trim()) && Number(l.qtyText) > 0 ? Number(l.qtyText) : null
}

function lineAmount(l: CartLine): number | null {
  const qty = lineQty(l)
  const price = parseSom(l.priceText)
  if (qty === null || price === null) return null
  const amount = l.kind === 'pour' ? pourAmount(qty, price) : qty * price
  return Number.isSafeInteger(amount) ? amount : null
}

const nextKey = () => crypto.randomUUID()

/** Незавершённый чек переживает переход на другой экран и перезагрузку вкладки. */
const DRAFT_KEY = 'avtodom.cart'

interface Draft {
  saleType: 'takeaway' | 'service'
  masterId: string
  lines: CartLine[]
  comment: string
}

function loadDraft(): Draft | null {
  try {
    const raw = sessionStorage.getItem(DRAFT_KEY)
    return raw ? (JSON.parse(raw) as Draft) : null
  } catch {
    return null
  }
}

function saveDraft(d: Draft): void {
  try {
    if (d.lines.length === 0 && !d.comment) sessionStorage.removeItem(DRAFT_KEY)
    else sessionStorage.setItem(DRAFT_KEY, JSON.stringify(d))
  } catch {
    // Без хранилища черновик просто не сохранится.
  }
}

/** Единицы учёта строки (шт или мл) для сравнения с остатком. */
function lineUnits(l: CartLine): number {
  const qty = lineQty(l) ?? 0
  if (l.kind === 'container') return qty * (l.product?.container_ml ?? 0)
  return qty
}

/** Купюры для быстрого ввода полученной суммы, тыйын. */
const NOTES = [10_000, 20_000, 50_000, 100_000, 200_000, 500_000]

const focusPicker = () => document.querySelector<HTMLInputElement>('[data-picker]')?.focus()

export default function Cashier() {
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const [draft] = useState(loadDraft)
  const [saleType, setSaleType] = useState<'takeaway' | 'service'>(draft?.saleType ?? 'takeaway')
  const [cashierId, setCashierId] = useState(() => remembered(CASHIER_KEY))
  const [masterId, setMasterId] = useState(draft?.masterId ?? '')
  const [lines, setLines] = useState<CartLine[]>(draft?.lines ?? [])
  const [comment, setComment] = useState(draft?.comment ?? '')
  const [payMode, setPayMode] = useState<PayMode>('cash')
  const [received, setReceived] = useState('')
  const [split, setSplit] = useState<Record<PaymentMethod, string>>({ cash: '', card: '', transfer: '' })
  const [opId, setOpId] = useState(newOpId)
  const [done, setDone] = useState<{ sale: Sale; change: number | null } | null>(null)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  const [newProductCode, setNewProductCode] = useState<string | null>(null)
  const { busy, error, setError, run } = useAction()

  const cashiers = employees.data?.filter((e) => e.active && e.is_cashier) ?? []
  const masters = employees.data?.filter((e) => e.active && e.is_master) ?? []

  useEffect(() => saveDraft({ saleType, masterId, lines, comment }), [saleType, masterId, lines, comment])

  // Единственный кассир или мастер выбирается сам; выбор несуществующего сбрасывается.
  useEffect(() => {
    if (!employees.data) return
    const ids = cashiers.map((e) => e.id)
    if (!ids.includes(cashierId)) setCashierId(ids.length === 1 ? ids[0] : '')
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [employees.data])
  useEffect(() => {
    if (saleType === 'service' && !masterId && masters.length === 1) setMasterId(masters[0].id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [saleType, employees.data])

  // Сколько единиц каждого товара уже в чеке — для предупреждения о нехватке.
  const unitsInCart = new Map<string, number>()
  for (const l of lines) {
    if (l.product) unitsInCart.set(l.product.id, (unitsInCart.get(l.product.id) ?? 0) + lineUnits(l))
  }

  const amounts = lines.map(lineAmount)
  const valid = lines.length > 0 && amounts.every((a) => a !== null)
  const total = amounts.reduce<number>((acc, a) => acc + (a ?? 0), 0)

  const payments = ((): { method: PaymentMethod; amount_tyiyn: number }[] | null => {
    if (payMode !== 'mixed') return [{ method: payMode, amount_tyiyn: total }]
    const list: { method: PaymentMethod; amount_tyiyn: number }[] = []
    for (const m of ['cash', 'card', 'transfer'] as const) {
      if (!split[m].trim()) continue
      const v = parseSom(split[m])
      if (v === null) return null
      if (v > 0) list.push({ method: m, amount_tyiyn: v })
    }
    return list
  })()
  const paid = payments?.reduce((acc, p) => acc + p.amount_tyiyn, 0) ?? null
  const receivedValue = payMode === 'cash' && received.trim() ? parseSom(received) : null
  const change = receivedValue !== null ? receivedValue - total : null

  const update = (key: string, patch: Partial<CartLine>) =>
    setLines((ls) => ls.map((l) => (l.key === key ? { ...l, ...patch } : l)))

  const addProduct = (p: Product) => {
    setDone(null)
    const kind: SaleLineKind = p.unit === 'ml' ? 'container' : 'piece'
    setLines((ls) => {
      const same = ls.find((l) => l.product?.id === p.id && l.kind === kind)
      if (same) {
        const n = lineQty(same) ?? 0
        return ls.map((l) => (l === same ? { ...l, qtyText: String(n + 1) } : l))
      }
      return [...ls, { key: nextKey(), kind, product: p, qtyText: '1', priceText: somInput(listPrice({ kind, product: p })) }]
    })
  }

  const switchOil = (l: CartLine, kind: 'container' | 'pour') => {
    if (l.kind === kind) return
    update(l.key, { kind, qtyText: '1', priceText: somInput(listPrice({ kind, product: l.product })) })
  }

  // Замена масла — отметка чека, а не строка услуги: цена товара от этого не меняется (ADR-027).
  const changeType = (t: 'takeaway' | 'service') => setSaleType(t)

  /** Шаг количества: 1 шт или канистра, 0,5 л для розлива. */
  const step = (l: CartLine, dir: 1 | -1) => {
    const q = lineQty(l) ?? 0
    if (l.kind === 'pour') {
      const ml = Math.max(500, q + dir * 500)
      update(l.key, { qtyText: formatLiters(ml).replace(/ л$/, '').replace(/ /g, '') })
    } else {
      update(l.key, { qtyText: String(Math.max(1, q + dir)) })
    }
  }

  const reset = () => {
    setLines([])
    setComment('')
    setReceived('')
    setSplit({ cash: '', card: '', transfer: '' })
    setPayMode('cash')
    setMasterId('')
    setSaleType('takeaway')
    setError(null)
  }

  const blockers: string[] = []
  if (!cashierId) blockers.push('выберите кассира')
  if (saleType === 'service' && !masterId) blockers.push('выберите мастера')
  if (lines.length === 0) blockers.push('добавьте товары')
  else if (!valid) blockers.push('проверьте количество и цены')
  if (payments === null) blockers.push('неверная сумма оплаты')
  else if (paid !== total) blockers.push(`оплата ${formatSom(paid ?? 0)} не равна итогу`)
  if (payMode === 'cash' && change !== null && change < 0) blockers.push('получено меньше итога')

  const submit = () =>
    run(async () => {
      const sale = await post<Sale>('/sales', {
        op_id: opId,
        client_time: new Date().toISOString(),
        sale_type: saleType,
        cashier_id: cashierId,
        master_id: saleType === 'service' ? masterId : null,
        comment,
        lines: lines.map((l) => ({
          kind: l.kind,
          product_id: l.product?.id ?? null,
          service_id: null,
          qty: lineQty(l),
          unit_price_tyiyn: parseSom(l.priceText),
        })),
        payments,
      })
      setDone({ sale, change: payMode === 'cash' ? change : null })
      setOpId(newOpId())
      reset()
      focusPicker()
    })

  const canSubmit = !busy && blockers.length === 0
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && canSubmit) {
        e.preventDefault()
        void submit()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_380px]">
      <div className="flex min-w-0 flex-col gap-4">
        <Card>
          <ProductPicker onPick={addProduct} onUnknownCode={setUnknownCode} />
        </Card>

        {done && (
          <Card className="border-emerald-300 bg-emerald-50">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <div className="font-semibold text-emerald-800">Чек № {done.sale.number} проведён</div>
                <div className="text-sm text-emerald-900">
                  Итог {formatSom(done.sale.total_tyiyn)}
                  {done.change !== null && done.change > 0 && <> · сдача {formatSom(done.change)}</>}
                </div>
              </div>
              <div className="flex gap-2">
                <Button variant="secondary" onClick={() => printSale(done.sale, done.change)}>
                  Печать чека
                </Button>
                <Link className="self-center text-sm text-sky-700 hover:underline" to={`/sales/${done.sale.id}`}>
                  Открыть
                </Link>
              </div>
            </div>
          </Card>
        )}

        <Card className="p-0">
          {lines.length === 0 ? (
            <div className="p-8 text-center text-sm text-slate-500">Чек пуст. Сканируйте товар или найдите его по названию.</div>
          ) : (
            <ul className="divide-y divide-slate-100">
              {lines.map((l, i) => {
                const amount = amounts[i]
                const changed = parseSom(l.priceText) !== listPrice(l)
                const short = l.product ? (unitsInCart.get(l.product.id) ?? 0) > l.product.stock_qty : false
                return (
                  <li key={l.key} className="flex flex-wrap items-center gap-3 p-3">
                    <div className="min-w-48 flex-1">
                      <div className="font-medium">{l.product?.name}</div>
                      <div className="text-xs text-slate-500">
                        {l.product ? `Остаток: ${stockText(l.product)}` : 'Услуга'}
                        {short && (
                          <span className="ml-2">
                            <Badge tone="rose">больше, чем на складе</Badge>
                          </span>
                        )}
                        {l.product?.needs_review && (
                          <span className="ml-2">
                            <Badge tone="amber">проверить</Badge>
                          </span>
                        )}
                      </div>
                      {l.product?.unit === 'ml' && (
                        <div className="mt-1 inline-flex overflow-hidden rounded-md border border-slate-300 text-xs">
                          {(['container', 'pour'] as const).map((k) => (
                            <button
                              key={k}
                              type="button"
                              className={`px-2 py-1 ${l.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                              onClick={() => switchOil(l, k)}
                            >
                              {k === 'container' ? `Канистра ${l.product?.container_ml ? formatLiters(l.product.container_ml) : ''}` : 'Розлив, л'}
                            </button>
                          ))}
                        </div>
                      )}
                    </div>
                    <label className="flex flex-col text-xs text-slate-500">
                      {l.kind === 'pour' ? 'Литры' : l.kind === 'container' ? 'Канистры' : 'Кол-во'}
                      <span className="flex items-stretch">
                        <button
                          type="button"
                          className="rounded-l-md border border-r-0 border-slate-300 px-2.5 text-base text-slate-700 hover:bg-slate-50"
                          aria-label="Меньше"
                          onClick={() => step(l, -1)}
                        >
                          −
                        </button>
                        <input
                          className="w-16 rounded-none text-center"
                          inputMode={l.kind === 'pour' ? 'decimal' : 'numeric'}
                          value={l.qtyText}
                          onChange={(e) => update(l.key, { qtyText: e.target.value })}
                        />
                        <button
                          type="button"
                          className="rounded-r-md border border-l-0 border-slate-300 px-2.5 text-base text-slate-700 hover:bg-slate-50"
                          aria-label="Больше"
                          onClick={() => step(l, 1)}
                        >
                          +
                        </button>
                      </span>
                    </label>
                    <label className="flex flex-col text-xs text-slate-500">
                      {l.kind === 'pour' ? 'Цена за л' : 'Цена'}
                      <input
                        className={`w-28 ${changed ? 'border-amber-400 bg-amber-50' : ''}`}
                        inputMode="decimal"
                        value={l.priceText}
                        onChange={(e) => update(l.key, { priceText: e.target.value })}
                        title={changed ? `Прайс: ${formatSom(listPrice(l))}` : undefined}
                      />
                    </label>
                    <div className="w-28 text-right font-medium">{amount === null ? '—' : formatSom(amount)}</div>
                    <button
                      type="button"
                      className="text-slate-400 hover:text-rose-600"
                      aria-label="Удалить строку"
                      onClick={() => setLines((ls) => ls.filter((x) => x.key !== l.key))}
                    >
                      ✕
                    </button>
                  </li>
                )
              })}
            </ul>
          )}
        </Card>
      </div>

      <div className="flex flex-col gap-4">
        <Card className="flex flex-col gap-3">
          <div className="grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
            {(['takeaway', 'service'] as const).map((t) => (
              <button
                key={t}
                type="button"
                className={`py-2 ${saleType === t ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => changeType(t)}
              >
                {t === 'takeaway' ? 'На вынос' : 'В сервис'}
              </button>
            ))}
          </div>
          <Field label="Кассир">
            <select
              value={cashierId}
              onChange={(e) => {
                setCashierId(e.target.value)
                remember(CASHIER_KEY, e.target.value)
              }}
            >
              <option value="">— выберите —</option>
              {cashiers.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.full_name}
                </option>
              ))}
            </select>
          </Field>
          {saleType === 'service' && (
            <>
              <Field label="Мастер">
                <select value={masterId} onChange={(e) => setMasterId(e.target.value)}>
                  <option value="">— выберите —</option>
                  {masters.map((e) => (
                    <option key={e.id} value={e.id}>
                      {e.full_name}
                    </option>
                  ))}
                </select>
              </Field>
              <div className="text-xs text-slate-500">
                Замена в чеке строкой не печатается, цена масла та же. Мастеру начисляется ставка за этот чек.
              </div>
            </>
          )}
          {employees.data && cashiers.length === 0 && (
            <div className="text-sm text-amber-700">
              В справочнике нет кассиров. <Link className="underline" to="/employees">Добавить сотрудника</Link>
            </div>
          )}
        </Card>

        <Card className="flex flex-col gap-3">
          <div className="flex items-baseline justify-between">
            <span className="text-sm text-slate-500">Итого</span>
            <span className="text-3xl font-bold">{formatSom(total)}</span>
          </div>
          <div className="grid grid-cols-4 overflow-hidden rounded-md border border-slate-300 text-xs">
            {(['cash', 'card', 'transfer', 'mixed'] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={`py-2 ${payMode === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => setPayMode(m)}
              >
                {m === 'mixed' ? 'Смешанная' : PAYMENT_LABELS[m]}
              </button>
            ))}
          </div>
          {payMode === 'cash' && (
            <div className="flex flex-col gap-2">
              <Field label="Получено, с">
                <input
                  inputMode="decimal"
                  value={received}
                  onChange={(e) => setReceived(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' && canSubmit) void submit()
                  }}
                />
              </Field>
              {total > 0 && (
                <div className="flex flex-wrap gap-1.5">
                  <button
                    type="button"
                    className="rounded-md border border-slate-300 px-2.5 py-1 text-xs hover:bg-slate-50"
                    onClick={() => setReceived(somInput(total))}
                  >
                    Без сдачи
                  </button>
                  {NOTES.filter((n) => n > total)
                    .slice(0, 3)
                    .map((n) => (
                      <button
                        key={n}
                        type="button"
                        className="rounded-md border border-slate-300 px-2.5 py-1 text-xs hover:bg-slate-50"
                        onClick={() => setReceived(somInput(n))}
                      >
                        {formatSom(n)}
                      </button>
                    ))}
                </div>
              )}
              {change !== null && change >= 0 && (
                <div className="flex items-baseline justify-between rounded-md bg-emerald-50 px-3 py-2 text-emerald-800">
                  <span className="text-sm">Сдача</span>
                  <span className="text-xl font-semibold">{formatSom(change)}</span>
                </div>
              )}
            </div>
          )}
          {payMode === 'mixed' && (
            <div className="grid grid-cols-3 gap-2">
              {(['cash', 'card', 'transfer'] as const).map((m) => (
                <Field key={m} label={PAYMENT_LABELS[m]}>
                  <input inputMode="decimal" value={split[m]} onChange={(e) => setSplit({ ...split, [m]: e.target.value })} />
                </Field>
              ))}
            </div>
          )}
          <Field label="Комментарий">
            <input value={comment} onChange={(e) => setComment(e.target.value)} />
          </Field>
          <ErrorBox error={error} />
          {blockers.length > 0 && lines.length > 0 && <div className="text-xs text-slate-500">Чтобы провести: {blockers.join(', ')}</div>}
          <Button className="py-3 text-base" disabled={!canSubmit} onClick={() => void submit()}>
            Провести чек
          </Button>
          <div className="hidden text-center text-xs text-slate-400 lg:block">Ctrl + Enter — провести</div>
          {lines.length > 0 && (
            <Button variant="ghost" onClick={reset}>
              Очистить
            </Button>
          )}
        </Card>
      </div>
      {unknownCode !== null && (
        <UnknownCodeModal
          code={unknownCode}
          onClose={() => setUnknownCode(null)}
          onLinked={(p) => {
            setUnknownCode(null)
            addProduct(p)
            focusPicker()
          }}
          onCreateNew={() => {
            setNewProductCode(unknownCode)
            setUnknownCode(null)
          }}
        />
      )}
      {newProductCode !== null && (
        <ProductFormModal
          presetBarcode={newProductCode}
          onClose={() => setNewProductCode(null)}
          onSaved={(p) => {
            setNewProductCode(null)
            addProduct(p)
            focusPicker()
          }}
        />
      )}
    </div>
  )
}
