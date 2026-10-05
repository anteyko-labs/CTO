import { useState } from 'react'
import { Link } from 'react-router-dom'
import { ProductPicker, stockText } from '../components/ProductPicker'
import { printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { formatLiters, formatSom, parseLiters, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { pourAmount } from '../lib/money'
import { PAYMENT_LABELS, type Employee, type PaymentMethod, type Product, type Sale, type SaleLineKind, type Service } from '../lib/types'

interface CartLine {
  key: string
  kind: SaleLineKind
  product?: Product
  service?: Service
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

function listPrice(l: Pick<CartLine, 'kind' | 'product' | 'service'>): number {
  if (l.kind === 'service') return l.service?.price_tyiyn ?? 0
  if (l.kind === 'pour') return l.product?.pour_price_per_l_tyiyn ?? 0
  return l.product?.sale_price_tyiyn ?? 0
}

/** Количество строки в единицах запроса: шт, канистры, услуги или мл для розлива. */
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

let keySeq = 0
const nextKey = () => String(++keySeq)

export default function Cashier() {
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const services = useLoad(() => get<Service[]>('/services'), [])
  const [saleType, setSaleType] = useState<'takeaway' | 'service'>('takeaway')
  const [cashierId, setCashierId] = useState(() => remembered(CASHIER_KEY))
  const [masterId, setMasterId] = useState('')
  const [lines, setLines] = useState<CartLine[]>([])
  const [comment, setComment] = useState('')
  const [payMode, setPayMode] = useState<PayMode>('cash')
  const [received, setReceived] = useState('')
  const [split, setSplit] = useState<Record<PaymentMethod, string>>({ cash: '', card: '', transfer: '' })
  const [opId, setOpId] = useState(newOpId)
  const [done, setDone] = useState<{ sale: Sale; change: number | null } | null>(null)
  const { busy, error, setError, run } = useAction()

  const cashiers = employees.data?.filter((e) => e.active && e.is_cashier) ?? []
  const masters = employees.data?.filter((e) => e.active && e.is_master) ?? []
  const activeServices = services.data?.filter((s) => s.active) ?? []

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

  const addService = (s: Service) => {
    setDone(null)
    setLines((ls) => {
      const same = ls.find((l) => l.service?.id === s.id)
      if (same) return ls.map((l) => (l === same ? { ...l, qtyText: String((lineQty(l) ?? 0) + 1) } : l))
      return [...ls, { key: nextKey(), kind: 'service', service: s, qtyText: '1', priceText: somInput(s.price_tyiyn) }]
    })
  }

  const switchOil = (l: CartLine, kind: 'container' | 'pour') => {
    if (l.kind === kind) return
    update(l.key, { kind, qtyText: '1', priceText: somInput(listPrice({ kind, product: l.product })) })
  }

  const changeType = (t: 'takeaway' | 'service') => {
    setSaleType(t)
    if (t === 'takeaway') setLines((ls) => ls.filter((l) => l.kind !== 'service'))
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
  if (saleType === 'service' && !lines.some((l) => l.kind === 'service')) blockers.push('добавьте услугу')
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
          service_id: l.service?.id ?? null,
          qty: lineQty(l),
          unit_price_tyiyn: parseSom(l.priceText),
        })),
        payments,
      })
      setDone({ sale, change: payMode === 'cash' ? change : null })
      setOpId(newOpId())
      reset()
    })

  return (
    <div className="grid gap-4 lg:grid-cols-[1fr_380px]">
      <div className="flex min-w-0 flex-col gap-4">
        <Card>
          <ProductPicker onPick={addProduct} />
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
                return (
                  <li key={l.key} className="flex flex-wrap items-center gap-3 p-3">
                    <div className="min-w-48 flex-1">
                      <div className="font-medium">{l.product?.name ?? l.service?.name}</div>
                      <div className="text-xs text-slate-500">
                        {l.product ? `Остаток: ${stockText(l.product)}` : 'Услуга'}
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
                      <input
                        className="w-20"
                        inputMode={l.kind === 'pour' ? 'decimal' : 'numeric'}
                        value={l.qtyText}
                        onChange={(e) => update(l.key, { qtyText: e.target.value })}
                      />
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
              <div className="flex flex-wrap gap-2">
                {activeServices.length === 0 && <span className="text-sm text-slate-500">Нет услуг в справочнике</span>}
                {activeServices.map((s) => (
                  <Button key={s.id} variant="secondary" onClick={() => addService(s)}>
                    + {s.name}
                  </Button>
                ))}
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
            <Field label="Получено, с" hint={change !== null && change >= 0 ? `Сдача: ${formatSom(change)}` : undefined}>
              <input inputMode="decimal" value={received} onChange={(e) => setReceived(e.target.value)} />
            </Field>
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
          <Button className="py-3 text-base" disabled={busy || blockers.length > 0} onClick={() => void submit()}>
            Провести чек
          </Button>
          {lines.length > 0 && (
            <Button variant="ghost" onClick={reset}>
              Очистить
            </Button>
          )}
        </Card>
      </div>
    </div>
  )
}
