// Возврат без чека (SPEC-30 §16, ADR-105): только владелец. Товары (поиск, скан), количество, сумма
// строки (по умолчанию цена продажи), как вернуть деньги, клиент необязательно, причина обязательна.
// Товар вернётся на склад по средней себестоимости; 2 % кассиру и ставки мастеру не трогаются.
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { get, newOpId, post, randomId } from '../lib/api'
import { productTitle } from '../lib/battery'
import { formatSom, parseLiters, parseSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { pourAmount, pourPricePerL } from '../lib/money'
import { PAYMENT_LABELS, type Employee, type Party, type Product, type Sale, type SaleLineKind, type Shift } from '../lib/types'
import { ClientPicker } from './ClientPicker'
import { ProductPicker } from './ProductPicker'
import { printSale } from './salePrint'
import { Button, ErrorBox, Field, Missing, Modal, Notice } from './ui'

type Method = 'cash' | 'card' | 'transfer' | 'invoice'
const METHODS: Method[] = ['cash', 'card', 'transfer', 'invoice']

interface Line {
  key: string
  product: Product
  kind: SaleLineKind
  qtyText: string
  /** Сумма строки, если её поменяли; пусто — по цене продажи. */
  sumText: string
}

/** Количество строки в её единицах: штуки, канистры, мл розлива или граммы. */
function lineQty(l: Line): number | null {
  if (l.kind === 'pour' || l.kind === 'weight') {
    const v = parseLiters(l.qtyText)
    return v && v > 0 ? v : null
  }
  return /^\d+$/.test(l.qtyText.trim()) && Number(l.qtyText) > 0 ? Number(l.qtyText) : null
}

/** Сумма по текущей цене продажи: канистры и штуки — по цене, розлив — от цены канистры, вес — за кг. */
function defaultSum(product: Pick<Product, 'unit' | 'container_ml' | 'sale_price_tyiyn'>, kind: SaleLineKind, qty: number): number {
  if (kind === 'pour') return pourAmount(qty, pourPricePerL(product) ?? 0)
  if (kind === 'weight') return pourAmount(qty, product.sale_price_tyiyn)
  return qty * product.sale_price_tyiyn
}

function lineSum(l: Line): number | null {
  const qty = lineQty(l)
  if (qty === null) return null
  if (l.sumText.trim()) return parseSom(l.sumText)
  return defaultSum(l.product, l.kind, qty)
}

export function NoReceiptReturn({ onClose, touch = false }: { onClose: () => void; touch?: boolean }) {
  const shift = useLoad(() => get<Shift | null>('/shifts/current'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const [lines, setLines] = useState<Line[]>([])
  const [method, setMethod] = useState<Method>('card')
  const [party, setParty] = useState<Party | null>(null)
  const [cashierId, setCashierId] = useState('')
  const [reason, setReason] = useState('')
  // Один op_id на окно: повтор после потерянного ответа не проведёт возврат дважды.
  const [opId] = useState(newOpId)
  const [done, setDone] = useState<Sale | null>(null)
  const { busy, error, run } = useAction()

  const add = (p: Product) =>
    setLines((ls) => {
      const kind: SaleLineKind = p.unit === 'ml' ? 'container' : p.unit === 'g' ? 'weight' : 'piece'
      const same = ls.find((l) => l.product.id === p.id && l.kind === kind && (kind === 'piece' || kind === 'container'))
      if (same) return ls.map((l) => (l === same ? { ...l, qtyText: String((lineQty(l) ?? 0) + 1) } : l))
      return [...ls, { key: randomId(), product: p, kind, qtyText: kind === 'weight' ? '' : '1', sumText: '' }]
    })
  const update = (key: string, patch: Partial<Line>) => setLines((ls) => ls.map((l) => (l.key === key ? { ...l, ...patch } : l)))

  const sums = lines.map(lineSum)
  const total = sums.reduce<number>((acc, s) => acc + (s ?? 0), 0)
  const shiftOpen = Boolean(shift.data)
  const cashiers = (employees.data ?? []).filter((e) => e.is_cashier && e.active)
  const missing = [
    ...(lines.length === 0 ? ['товар'] : []),
    ...(sums.some((s) => s === null || s < 0) ? ['количество и сумму'] : []),
    ...(!shiftOpen && !cashierId ? ['кассира'] : []),
    ...(!reason.trim() ? ['причину'] : []),
  ]
  const big = touch ? 'min-h-[56px] text-lg' : ''

  const submit = () =>
    run(async () => {
      const res = await post<Sale>('/sales/return-without-receipt', {
        op_id: opId,
        lines: lines.map((l, i) => ({ kind: l.kind, product_id: l.product.id, qty: lineQty(l), amount_tyiyn: sums[i] })),
        method,
        party_id: party?.id ?? null,
        cashier_id: shiftOpen ? null : cashierId || null,
        comment: reason.trim(),
      })
      setDone(res)
    })

  if (done) {
    return (
      <Modal title="Возврат без чека" onClose={onClose}>
        <div className="flex flex-col gap-3" data-testid="no-receipt-done">
          <div className="text-lg font-semibold text-emerald-800">Возврат без чека № {done.number} оформлен</div>
          <div>
            Выдано {formatSom(-done.total_tyiyn)} — {PAYMENT_LABELS[method]}
          </div>
          <div className="flex flex-wrap justify-end gap-2">
            <Button variant="secondary" className={big} onClick={() => printSale(done)}>
              Печать
            </Button>
            <Link to={`/sales/${done.id}`} className={`inline-flex items-center rounded-md border border-slate-300 px-4 ${touch ? 'min-h-[56px] text-lg' : 'min-h-[42px]'}`}>
              Открыть
            </Link>
            <Button className={big} onClick={onClose}>
              Готово
            </Button>
          </div>
        </div>
      </Modal>
    )
  }

  return (
    <Modal title="Возврат без чека" onClose={onClose} full>
      <div className="flex flex-col gap-4" data-testid="no-receipt-return">
        <Notice tone="amber">
          Только владелец. Товар вернётся на склад по средней себестоимости, выручка уменьшится на сумму возврата; 2 % кассиру и ставки мастеру не
          меняются.
        </Notice>
        <ProductPicker autoFocus={!touch} placeholder="Сканируйте или найдите возвращаемый товар" onPick={add} inputId="no-receipt-product" />
        {lines.length > 0 && (
          <ul className="divide-y divide-slate-100 rounded-lg border border-slate-200">
            {lines.map((l, i) => {
              const qty = lineQty(l)
              const dflt = qty === null ? null : defaultSum(l.product, l.kind, qty)
              return (
                <li key={l.key} className="flex flex-wrap items-end gap-3 p-3" data-testid="no-receipt-line">
                  <div className="min-w-48 flex-1">
                    <div className={`font-medium ${touch ? 'text-lg' : ''}`}>{productTitle(l.product)}</div>
                    {l.product.unit === 'ml' && (
                      <div className="mt-1 inline-flex overflow-hidden rounded-md border border-slate-300 text-sm">
                        {(['container', 'pour'] as const).map((k) => (
                          <button
                            key={k}
                            type="button"
                            className={`px-3 ${touch ? 'min-h-[56px]' : 'min-h-9'} ${l.kind === k ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
                            onClick={() => update(l.key, { kind: k, qtyText: k === 'pour' ? '' : '1', sumText: '' })}
                          >
                            {k === 'container' ? 'Канистры' : 'Розлив, л'}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <Field label={l.kind === 'pour' ? 'Литры' : l.kind === 'weight' ? 'Вес, кг' : l.kind === 'container' ? 'Канистры' : 'Кол-во'}>
                    <input
                      aria-label="Количество"
                      className={`w-24 ${big}`}
                      inputMode={l.kind === 'pour' || l.kind === 'weight' ? 'decimal' : 'numeric'}
                      value={l.qtyText}
                      onChange={(e) => update(l.key, { qtyText: e.target.value })}
                    />
                  </Field>
                  <Field label="Сумма, с" hint={dflt !== null ? `по цене продажи ${formatSom(dflt)}` : undefined}>
                    <input
                      aria-label="Сумма строки"
                      className={`w-32 ${big} ${sums[i] === null ? 'border-rose-400' : ''}`}
                      inputMode="decimal"
                      placeholder={dflt !== null ? String(dflt / 100) : ''}
                      value={l.sumText}
                      onChange={(e) => update(l.key, { sumText: e.target.value })}
                    />
                  </Field>
                  <button
                    type="button"
                    className={`flex items-center justify-center rounded-lg border border-rose-200 bg-rose-50 text-rose-700 ${touch ? 'min-h-[56px] min-w-[56px] text-2xl' : 'min-h-[42px] min-w-[42px]'}`}
                    aria-label="Убрать строку"
                    onClick={() => setLines((ls) => ls.filter((x) => x.key !== l.key))}
                  >
                    ✕
                  </button>
                </li>
              )
            })}
          </ul>
        )}
        <div className="flex flex-col gap-2">
          <span className="text-sm font-medium text-slate-700">Как вернуть деньги</span>
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {METHODS.map((m) => (
              <button
                key={m}
                type="button"
                aria-pressed={method === m}
                className={`rounded-lg border px-3 ${touch ? 'min-h-[56px] text-lg' : 'min-h-[42px]'} ${method === m ? 'border-sky-600 bg-sky-600 text-white' : 'border-slate-300 bg-white'}`}
                onClick={() => setMethod(m)}
              >
                {PAYMENT_LABELS[m]}
              </button>
            ))}
          </div>
          {method === 'cash' && shift.data === null && (
            <div className="text-sm text-rose-700">Наличными — только при открытой смене. Откройте смену или верните на карту.</div>
          )}
          {method !== 'cash' && <div className="text-xs text-slate-500">Со счёта по умолчанию для этого способа</div>}
        </div>
        {shift.data === null && (
          <Field label="Кассир" required hint="Смена не открыта — укажите, чей это возврат">
            <select id="no-receipt-cashier" className={big} value={cashierId} onChange={(e) => setCashierId(e.target.value)}>
              <option value="">— выберите —</option>
              {cashiers.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.full_name}
                </option>
              ))}
            </select>
          </Field>
        )}
        <div>
          <ClientPicker party={party} contactId="" vehicleId="" onParty={setParty} onContact={() => undefined} onVehicle={() => undefined} />
        </div>
        <Field label="Причина" required>
          <input id="no-receipt-reason" className={big} maxLength={300} value={reason} onChange={(e) => setReason(e.target.value)} placeholder="Например: принёс целую канистру без чека" />
        </Field>
        <ErrorBox error={error ?? shift.error} />
        <div className="flex flex-wrap items-center justify-end gap-3">
          <Missing items={missing} className="mr-auto" />
          <span className="text-lg font-semibold" data-testid="no-receipt-total">
            К возврату: {formatSom(total)}
          </span>
          <Button variant="secondary" className={big} onClick={onClose}>
            Отмена
          </Button>
          <Button variant="danger" className={big} disabled={busy || missing.length > 0 || total <= 0 || (method === 'cash' && !shiftOpen)} onClick={() => void submit()}>
            Оформить возврат
          </Button>
        </div>
      </div>
    </Modal>
  )
}
