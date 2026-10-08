// Экран «Касса»: чек на вынос или в сервис, масло канистрой и на розлив, оплата, клиент и долг,
// подарки, работа без сети (SPEC-04, SPEC-09, SPEC-10, SPEC-11).
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { balanceText, ClientPicker } from '../components/ClientPicker'
import { GiftPicker } from '../components/GiftPicker'
import { QuickExpense } from '../components/QuickExpense'
import { ProductFormModal } from '../components/ProductFormModal'
import { ProductPicker, stockText } from '../components/ProductPicker'
import { UnknownCodeModal } from '../components/UnknownCodeModal'
import { printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field, Missing, toast } from '../components/ui'
import { get, newOpId, post, qs } from '../lib/api'
import { enqueueSale, syncOutbox } from '../lib/offline'
import { formatLiters, formatSom, parseLiters, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad } from '../lib/hooks'
import { pourAmount } from '../lib/money'
import { PAYMENT_LABELS, type Employee, type GiftRule, type Party, type PaymentMethod, type Product, type Sale, type SaleLineKind } from '../lib/types'

interface CartLine {
  key: string
  kind: SaleLineKind
  product?: Product
  qtyText: string
  priceText: string
  /** Подарок: цена ноль, в чеке помечен (SPEC-11). */
  gift?: boolean
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

/** Отложенные чеки: клиент ушёл за второй канистрой, а касса свободна. */
const PARKED_KEY = 'avtodom.parked'

interface Parked {
  id: string
  at: string
  label: string
  saleType: 'takeaway' | 'service'
  masterId: string
  lines: CartLine[]
  comment: string
  party: Party | null
  contactId: string
  vehicleId: string
}

function loadParked(): Parked[] {
  try {
    const raw = localStorage.getItem(PARKED_KEY)
    return raw ? (JSON.parse(raw) as Parked[]) : []
  } catch {
    return []
  }
}

function saveParked(list: Parked[]): void {
  try {
    localStorage.setItem(PARKED_KEY, JSON.stringify(list))
  } catch {
    // Без хранилища отложенные живут до перезагрузки.
  }
}

const parkedTotal = (p: Parked): number => p.lines.reduce((acc, l) => acc + (lineAmount(l) ?? 0), 0)

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
  const [split, setSplit] = useState<Record<PaymentMethod, string>>({ cash: '', card: '', transfer: '', debt: '' })
  const [opId, setOpId] = useState(newOpId)
  const [done, setDone] = useState<{ sale: Sale; change: number | null } | null>(null)
  const [unknownCode, setUnknownCode] = useState<string | null>(null)
  const [newProductCode, setNewProductCode] = useState<string | null>(null)
  const [newProductName, setNewProductName] = useState<string | null>(null)
  const [party, setParty] = useState<Party | null>(null)
  const [parked, setParked] = useState<Parked[]>(loadParked)
  const [parkedId, setParkedId] = useState<string | null>(null)
  const [giftRule, setGiftRule] = useState<GiftRule | null>(null)
  const [expense, setExpense] = useState(false)
  const limitAsk = useAction()
  const [contactId, setContactId] = useState('')
  const [vehicleId, setVehicleId] = useState('')
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
    for (const m of ['cash', 'card', 'transfer', 'debt'] as const) {
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

  /** Есть ли к этому товару подарки — спрашиваем сразу после добавления. */
  const askGift = (p: Product) => {
    void get<GiftRule[]>(`/gift-rules${qs({ product_id: p.id })}`)
      .then((rules) => {
        const rule = rules.find((r) => r.active && r.items.length > 0)
        if (rule) setGiftRule(rule)
      })
      .catch(() => undefined)
  }

  const addGift = (g: { product_id: string; qty: number; name: string }) => {
    setLines((ls) => [
      ...ls,
      {
        key: nextKey(),
        kind: 'piece',
        product: { id: g.product_id, name: g.name } as Product,
        qtyText: String(g.qty),
        priceText: '0',
        gift: true,
      },
    ])
    setGiftRule(null)
    focusPicker()
  }

  const addProduct = (p: Product) => {
    setDone(null)
    askGift(p)
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
    setSplit({ cash: '', card: '', transfer: '', debt: '' })
    setPayMode('cash')
    setMasterId('')
    setSaleType('takeaway')
    setParty(null)
    setContactId('')
    setVehicleId('')
    setError(null)
  }

  const debtAmount = payments?.filter((p) => p.method === 'debt').reduce((acc, p) => acc + p.amount_tyiyn, 0) ?? 0
  // На сколько долг выходит за лимит: касса продаёт, но просит владельца поднять (SPEC-10).
  const overLimit =
    party?.credit_limit_tyiyn != null && debtAmount > 0
      ? Math.max(0, party.balance_tyiyn + debtAmount - party.credit_limit_tyiyn)
      : 0
  /** Откладывает текущий чек, возвращая новый список отложенных. */
  const parkCurrent = (list: Parked[]): Parked[] => {
    if (lines.length === 0) return list
    const id = parkedId ?? crypto.randomUUID()
    const item: Parked = {
      id,
      at: new Date().toISOString(),
      label: party?.name ?? '',
      saleType,
      masterId,
      lines,
      comment,
      party,
      contactId,
      vehicleId,
    }
    return [...list.filter((p) => p.id !== id), item]
  }

  const park = () => {
    const next = parkCurrent(parked)
    setParked(next)
    saveParked(next)
    setParkedId(null)
    reset()
    focusPicker()
  }

  const resume = (p: Parked) => {
    // Текущий чек не теряется: он уходит в отложенные.
    const next = parkCurrent(parked).filter((x) => x.id !== p.id)
    setParked(next)
    saveParked(next)
    setSaleType(p.saleType)
    setMasterId(p.masterId)
    setLines(p.lines)
    setComment(p.comment)
    setParty(p.party)
    setContactId(p.contactId)
    setVehicleId(p.vehicleId)
    setParkedId(p.id)
    setDone(null)
    focusPicker()
  }

  const dropParked = (id: string) => {
    const next = parked.filter((p) => p.id !== id)
    setParked(next)
    saveParked(next)
  }

  const blockers = missingWithFocus(
    [lines.length > 0, 'товары', '[data-picker]'],
    [lines.length === 0 || valid, 'количество и цены в строках'],
    [Boolean(cashierId), 'кассира', '#cashier-select'],
    [saleType !== 'service' || Boolean(masterId), 'мастера', '#master-select'],
    [payments !== null, 'сумму оплаты', '[data-pay]'],
    [payments === null || paid === total, `оплату: ${formatSom(paid ?? 0)} вместо ${formatSom(total)}`, '[data-pay]'],
    [payMode !== 'cash' || change === null || change >= 0, 'получено меньше итога', '[data-received]'],
    [debtAmount === 0 || party !== null, 'клиента для продажи в долг', '[data-client]'],
  )

  const submit = () =>
    run(async () => {
      const body = {
        op_id: opId,
        client_time: new Date().toISOString(),
        sale_type: saleType,
        cashier_id: cashierId,
        master_id: saleType === 'service' ? masterId : null,
        party_id: party?.id ?? null,
        contact_id: party && contactId ? contactId : null,
        vehicle_id: party && vehicleId ? vehicleId : null,
        comment,
        lines: lines.map((l) => ({
          kind: l.kind,
          gift: Boolean(l.gift),
          product_id: l.product?.id ?? null,
          service_id: null,
          qty: lineQty(l),
          unit_price_tyiyn: parseSom(l.priceText),
        })),
        payments,
      }
      // Чек сначала в очередь, потом на сервер: обрыв связи его не теряет (SPEC-09).
      const queued = await enqueueSale(opId, body)
      let sale: Sale
      try {
        sale = await post<Sale>('/sales', body)
      } catch (e) {
        if (!navigator.onLine) {
          const offlineSale = {
            id: '',
            number: queued.temp_no,
            kind: 'sale',
            sale_type: saleType,
            cashier_name: cashiers.find((c) => c.id === cashierId)?.full_name ?? '',
            master_name: masters.find((m) => m.id === masterId)?.full_name ?? null,
            total_tyiyn: total,
            created_at: new Date().toISOString(),
            lines: lines.map((l, i) => ({
              line_no: i + 1,
              kind: l.kind,
              gift: Boolean(l.gift),
              name: l.product?.name ?? '',
              container_ml: l.product?.container_ml ?? null,
              qty: lineQty(l) ?? 0,
              amount_tyiyn: lineAmount(l) ?? 0,
              unit_price_tyiyn: parseSom(l.priceText) ?? 0,
              list_price_tyiyn: listPrice(l),
            })),
            payments,
          } as unknown as Sale
          setDone({ sale: offlineSale, change: payMode === 'cash' ? change : null })
          setOpId(newOpId())
          if (parkedId) dropParked(parkedId)
          setParkedId(null)
          reset()
          focusPicker()
          return
        }
        throw e
      }
      void syncOutbox()
      setDone({ sale, change: payMode === 'cash' ? change : null })
      if (parkedId) dropParked(parkedId)
      setParkedId(null)
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
        {parked.length > 0 && (
          <Card className="flex flex-wrap items-center gap-2">
            <span className="text-sm text-slate-500">Отложенные:</span>
            {parked.map((p) => (
              <span
                key={p.id}
                className="inline-flex items-center gap-1 rounded-md border border-slate-300 bg-white px-2 py-1 text-sm"
              >
                <button type="button" className="font-medium hover:text-sky-700" onClick={() => resume(p)}>
                  {p.label || `Чек на ${p.lines.length} поз.`} · {formatSom(parkedTotal(p))}
                </button>
                <button
                  type="button"
                  className="text-slate-400 hover:text-rose-600"
                  aria-label="Удалить отложенный чек"
                  onClick={() => dropParked(p.id)}
                >
                  ✕
                </button>
              </span>
            ))}
          </Card>
        )}
        <Card>
          <ProductPicker onPick={addProduct} onUnknownCode={setUnknownCode} onCreate={setNewProductName} />
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
                {done.sale.id ? (
                  <Link className="self-center text-sm text-sky-700 hover:underline" to={`/sales/${done.sale.id}`}>
                    Открыть
                  </Link>
                ) : (
                  <span className="self-center text-xs text-emerald-800">уйдёт на сервер, когда появится сеть</span>
                )}
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
                      <div className="font-medium">
                        {l.product?.name}
                        {l.gift && (
                          <span className="ml-2">
                            <Badge tone="green">подарок</Badge>
                          </span>
                        )}
                      </div>
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
                className={`min-h-[42px] py-2 ${saleType === t ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                onClick={() => changeType(t)}
              >
                {t === 'takeaway' ? 'На вынос' : 'В сервис'}
              </button>
            ))}
          </div>
          <Field label="Кассир" required>
            <select
              id="cashier-select"
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
              <Field label="Мастер" required>
                <select id="master-select" value={masterId} onChange={(e) => setMasterId(e.target.value)}>
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
          <div data-client>
            <ClientPicker
              party={party}
              contactId={contactId}
              vehicleId={vehicleId}
              onParty={(p) => {
                setParty(p)
                setContactId('')
                setVehicleId('')
                if (!p && payMode === 'debt') setPayMode('cash')
              }}
              onContact={setContactId}
              onVehicle={setVehicleId}
            />
          </div>
          <button type="button" className="self-start text-xs text-sky-700 underline" onClick={() => setExpense(true)}>
            Мелкий расход из кассы
          </button>
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
          <div data-pay tabIndex={-1} className="grid grid-cols-5 overflow-hidden rounded-md border border-slate-300 text-xs">
            {(['cash', 'card', 'transfer', 'debt', 'mixed'] as const).map((m) => (
              <button
                key={m}
                type="button"
                className={`min-h-[42px] py-2 ${payMode === m ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
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
                  data-received
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
          {payMode === 'debt' && (
            <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800">
              {party ? (
                <>
                  Весь чек в долг: {party.name}, {balanceText(party.balance_tyiyn)}
                  {party.credit_limit_tyiyn !== null && (
                    <> · осталось по лимиту {formatSom(party.credit_limit_tyiyn - party.balance_tyiyn - total)}</>
                  )}
                  {overLimit > 0 && (
                    <div className="mt-2 flex flex-wrap items-center gap-2">
                      <span>Лимит превышен на {formatSom(overLimit)}.</span>
                      <Button
                        variant="secondary"
                        className="px-2 py-1 text-xs"
                        disabled={limitAsk.busy}
                        onClick={() =>
                          void limitAsk.run(async () => {
                            await post(`/parties/${party.id}/limit-request`, { amount_tyiyn: overLimit })
                            toast('Владельцу отправлено уведомление')
                          })
                        }
                      >
                        Запросить повышение
                      </Button>
                    </div>
                  )}
                </>
              ) : (
                'Выберите клиента выше — без него долг не записать'
              )}
            </div>
          )}
          {payMode === 'mixed' && (
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
              {(['cash', 'card', 'transfer', 'debt'] as const).map((m) => (
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
          <Missing items={blockers} />
          <Button className="py-3 text-base" disabled={!canSubmit} onClick={() => void submit()}>
            Провести чек
          </Button>
          <div className="hidden text-center text-xs text-slate-400 lg:block">Ctrl + Enter — провести</div>
          {lines.length > 0 && (
            <div className="flex gap-2">
              <Button variant="secondary" className="flex-1" onClick={park}>
                Отложить
              </Button>
              <Button variant="ghost" className="flex-1" onClick={reset}>
                Очистить
              </Button>
            </div>
          )}
        </Card>
      </div>
      {lines.length > 0 && (
        <div className="no-print fixed inset-x-0 bottom-[52px] z-20 flex items-center gap-3 border-t border-slate-200 bg-white px-4 py-2 shadow-[0_-2px_8px_rgba(15,23,42,0.08)] md:hidden">
          <div className="min-w-0 flex-1">
            {blockers.length > 0 ? (
              <div className="truncate text-xs text-amber-700">Заполните: {blockers.map((x) => x.label).join(', ')}</div>
            ) : (
              <div className="text-xs text-slate-500">Итого</div>
            )}
            <div className="truncate text-xl font-bold">{formatSom(total)}</div>
          </div>
          <Button className="shrink-0 px-6 py-3 text-base" disabled={!canSubmit} onClick={() => void submit()}>
            Провести чек
          </Button>
        </div>
      )}
      {giftRule && <GiftPicker rule={giftRule} onPick={addGift} onClose={() => setGiftRule(null)} />}
      {expense && <QuickExpense onClose={() => setExpense(false)} />}
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
      {newProductName !== null && (
        <ProductFormModal
          presetName={newProductName}
          onClose={() => setNewProductName(null)}
          onSaved={(p) => {
            setNewProductName(null)
            addProduct(p)
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
