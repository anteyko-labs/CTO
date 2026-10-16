// Просмотр чека, печать и частичный возврат (SPEC-04).
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { lineQtyText, printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, Notice, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { batteryShare, paymentLabel } from '../lib/battery'
import { formatDateTime, formatKg, formatLiters, formatSom, parseLiters } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { printDebtDoc, type DebtDocSettings, type Reconciliation } from '../lib/debtDocs'
import { divRound } from '../lib/money'
import { PAYMENT_LABELS, type Party, type PaymentMethod, type Sale, type SaleLine } from '../lib/types'

interface Returned {
  qty: number
  amount: number
}

interface ReturnedAll {
  lines: Map<number, Returned>
  /** Сколько уже вернули аккумулятором и всем остальным (положительные). */
  battery: number
  other: number
}

/** Возвращённое по строкам исходного чека: количество и сумма (положительные); оплаты возвратов. */
async function loadReturned(sale: Sale): Promise<ReturnedAll> {
  const map = new Map<number, Returned>()
  let battery = 0
  let other = 0
  const returns = await Promise.all(sale.returns.map((r) => get<Sale>(`/sales/${r.id}`)))
  for (const r of returns) {
    for (const l of r.lines) {
      const cur = map.get(l.line_no) ?? { qty: 0, amount: 0 }
      map.set(l.line_no, { qty: cur.qty + l.qty, amount: cur.amount - l.amount_tyiyn })
    }
    for (const p of r.payments) {
      if (p.method === 'battery') battery -= p.amount_tyiyn
      else other -= p.amount_tyiyn
    }
  }
  return { lines: map, battery, other }
}


/** Сумма возврата строки — то же правило, что на сервере: последняя часть забирает остаток. */
function refundOf(l: SaleLine, done: Returned, qty: number): number {
  if (done.qty + qty === l.qty) return l.amount_tyiyn - done.amount
  return divRound(l.amount_tyiyn * qty, l.qty)
}

function ReturnModal({ sale, onClose }: { sale: Sale; onClose: () => void }) {
  const navigate = useNavigate()
  const returned = useLoad(() => loadReturned(sale), [sale.id])
  // Наличные из кассы возвращаются только при открытой смене (ADR-065): предупреждаем заранее.
  const shift = useLoad(() => get<{ id: string } | null>('/shifts/current'), [])
  const [qty, setQty] = useState<Record<number, string>>({})
  // Возврат — тем же способом, каким платили: долговой чек списывается с долга, а не выдаётся наличными.
  const paidBy = [...sale.payments].sort((a, b) => b.amount_tyiyn - a.amount_tyiyn).map((p) => p.method)
  // Аккумулятор деньгами не возвращается — его часть уходит отдельно (ниже).
  const methods: PaymentMethod[] = [...new Set<PaymentMethod>([...paidBy, 'cash', 'card', 'transfer'])].filter((m) => m !== 'battery')
  const [method, setMethod] = useState<PaymentMethod>(paidBy.find((m) => m !== 'battery') ?? 'cash')
  const [comment, setComment] = useState('')
  // Лом из оплаты уже продан — часть «аккумулятором» владелец возвращает деньгами (SPEC-27).
  const [batteryCash, setBatteryCash] = useState(false)
  const owner = useUser().role === 'owner'
  const [opId] = useState(newOpId)
  const { busy, error, run } = useAction()

  const rows = sale.lines.map((l) => {
    const done = returned.data?.lines.get(l.line_no) ?? { qty: 0, amount: 0 }
    const left = l.qty - done.qty
    const text = qty[l.line_no]?.trim() ?? ''
    // Розлив — в литрах, вес — в килограммах (тысячные: мл и граммы), остальное — штуками.
    const n = !text ? 0 : l.kind === 'pour' || l.kind === 'weight' ? parseLiters(text) : /^\d+$/.test(text) ? Number(text) : null
    const ok = n !== null && n >= 0 && n <= left
    return { l, left, n, ok, refund: ok && n ? refundOf(l, done, n) : 0 }
  })
  const chosen = rows.filter((r) => r.ok && r.n)
  const total = chosen.reduce((acc, r) => acc + r.refund, 0)
  const invalid = rows.some((r) => !r.ok)
  const noShift = method === 'cash' && total > 0 && shift.data === null
  const paidBattery = sale.payments.filter((p) => p.method === 'battery').reduce((acc, p) => acc + p.amount_tyiyn, 0)
  const paidOther = sale.payments.filter((p) => p.method !== 'battery').reduce((acc, p) => acc + p.amount_tyiyn, 0)
  const batteryPart = batteryCash
    ? 0
    : batteryShare(total, paidBattery - (returned.data?.battery ?? 0), paidOther - (returned.data?.other ?? 0))
  const payments = [
    ...(total - batteryPart > 0 ? [{ method, amount_tyiyn: total - batteryPart }] : []),
    ...(batteryPart > 0 ? [{ method: 'battery' as const, amount_tyiyn: batteryPart }] : []),
  ]

  const submit = () =>
    run(async () => {
      const res = await post<Sale>(`/sales/${sale.id}/return`, {
        op_id: opId,
        comment,
        lines: chosen.map((r) => ({ line_no: r.l.line_no, qty: r.n })),
        payments,
        battery_cash: batteryCash,
      })
      navigate(`/sales/${res.id}`)
    })

  return (
    <Modal title={`Возврат по чеку № ${sale.number}`} onClose={onClose} wide>
      {returned.loading ? (
        <Loading />
      ) : (
        <div className="flex flex-col gap-3">
          <Table head={['Товар', 'Продано', 'Можно вернуть', 'Вернуть', 'Сумма']}>
            {rows.map(({ l, left, ok, refund }) => (
              <tr key={l.line_no}>
                <td className="px-2 py-2">{l.name}</td>
                <td className="px-2 py-2">{lineQtyText(l)}</td>
                <td className="px-2 py-2">{l.kind === 'pour' ? formatLiters(left) : l.kind === 'weight' ? formatKg(left) : left}</td>
                <td className="px-2 py-2">
                  <input
                    className={`w-24 ${ok ? '' : 'border-rose-400'}`}
                    inputMode={l.kind === 'pour' || l.kind === 'weight' ? 'decimal' : 'numeric'}
                    placeholder={l.kind === 'pour' ? 'литры' : l.kind === 'weight' ? 'кг' : '0'}
                    disabled={left <= 0}
                    value={qty[l.line_no] ?? ''}
                    onChange={(e) => setQty({ ...qty, [l.line_no]: e.target.value })}
                  />
                </td>
                <td className="px-2 py-2 text-right">{refund ? formatSom(refund) : '—'}</td>
              </tr>
            ))}
          </Table>
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label="Как вернуть"
              hint={
                method === 'debt'
                  ? 'Деньги не выдаются: сумма снимается с долга клиента'
                  : method === 'bonus'
                    ? 'Баллы вернутся клиенту'
                    : method === 'cash'
                      ? 'Наличные из кассы'
                      : 'Возврат на карту / QR через терминал'
              }
            >
              <select value={method} onChange={(e) => setMethod(e.target.value as PaymentMethod)}>
                {methods.map((m) => (
                  <option key={m} value={m}>
                    {m === 'debt' ? 'С долга по чеку' : PAYMENT_LABELS[m]}
                    {paidBy.includes(m) ? ' (как платили)' : ''}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Причина">
              <input value={comment} onChange={(e) => setComment(e.target.value)} />
            </Field>
          </div>
          {paidBattery > 0 && (
            <Notice tone={batteryCash ? 'rose' : 'amber'}>
              {batteryPart > 0 ? (
                <>
                  Часть чека оплачена аккумулятором: <b>{formatSom(batteryPart)}</b> возвращается аккумулятором — вес уйдёт со склада, деньгами
                  за него не выдаём.
                </>
              ) : batteryCash ? (
                <>Часть «аккумулятором» вернётся деньгами — по решению владельца, отметка уйдёт в журнал.</>
              ) : (
                <>Чек частично оплачен аккумулятором: эта часть возвращается аккумулятором, когда деньги по чеку закончатся.</>
              )}
              {owner && (
                <label className="mt-2 flex items-center gap-2">
                  <input type="checkbox" checked={batteryCash} onChange={(e) => setBatteryCash(e.target.checked)} />
                  Лом уже продан — вернуть эту часть деньгами
                </label>
              )}
            </Notice>
          )}
          <div className="text-right text-lg font-semibold">К возврату: {formatSom(total)}</div>
          {noShift && (
            <Notice tone="rose">
              <b>Смена не открыта.</b> Наличными вернуть нельзя: откройте смену на экране{' '}
              <Link to="/shift" className="font-semibold underline">
                «Смена»
              </Link>{' '}
              или верните на карту.
            </Notice>
          )}
          <ErrorBox error={error ?? returned.error} />
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={onClose}>
              Отмена
            </Button>
            <Button variant="danger" disabled={busy || invalid || noShift || chosen.length === 0 || !comment.trim()} onClick={() => void submit()}>
              Оформить возврат
            </Button>
          </div>
        </div>
      )}
    </Modal>
  )
}

export default function SaleView() {
  const { id = '' } = useParams()
  const user = useUser()
  const owner = user.role === 'owner'
  const { data: sale, error, loading } = useLoad(() => get<Sale>(`/sales/${id}`), [id])
  const [returning, setReturning] = useState(false)
  const docAct = useAction()

  if (loading && !sale) return <Loading />
  if (!sale) return <ErrorBox error={error} />

  const cost = sale.lines.reduce((acc, l) => acc + (l.cost_tyiyn ?? 0), 0)
  const fullyReturned = sale.kind === 'sale' && sale.returns.reduce((acc, r) => acc + r.total_tyiyn, 0) === -sale.total_tyiyn

  // Расписка физлица в двух экземплярах или накладная юрлица (SPEC-10).
  const printDoc = () =>
    void docAct.run(async () => {
      if (!sale.party_id) return
      const [party, settings, history] = await Promise.all([
        get<Party>(`/parties/${sale.party_id}`),
        get<DebtDocSettings>('/settings/debt-docs'),
        get<Reconciliation>(`/parties/${sale.party_id}/reconciliation`),
      ])
      printDebtDoc({ sale, party, settings, history })
    })

  return (
    <div className="flex flex-col gap-4">
      <ErrorBox error={docAct.error} />
      <PageHeader
        title={`${sale.kind === 'return' ? 'Возврат' : 'Чек'} № ${sale.number}`}
        back={
          <Link to="/sales" className="text-sky-700 hover:underline">
            ← К чекам
          </Link>
        }
        actions={
          <>
            <Button variant="secondary" onClick={() => printSale(sale)}>
              Печать
            </Button>
            {sale.kind === 'sale' && sale.party_id && sale.payments.some((p) => p.method === 'debt') && (
              <Button variant="secondary" disabled={docAct.busy} onClick={printDoc}>
                Документ о долге
              </Button>
            )}
            {sale.kind === 'sale' && !fullyReturned && (
              <Button variant="danger-outline" onClick={() => setReturning(true)}>
                Возврат
              </Button>
            )}
          </>
        }
      />
      <Card className="grid gap-2 text-sm sm:grid-cols-2">
        <div>Дата: {formatDateTime(sale.created_at)}</div>
        <div>Тип: {sale.sale_type === 'service' ? 'В сервис' : 'На вынос'}</div>
        <div>Кассир: {sale.cashier_name}</div>
        <div>Мастер: {sale.master_name ?? '—'}</div>
        {sale.party_name && (
          <div>
            Клиент:{' '}
            <Link className="text-sky-700 underline" to={`/clients/${sale.party_id}`}>
              {sale.party_name}
            </Link>
            {sale.contact_name && ` · ${sale.contact_name}`}
            {sale.vehicle_plate && ` · ${sale.vehicle_plate}`}
          </div>
        )}
        {sale.master_fee_tyiyn !== 0 && <div>Мастеру за замену: {formatSom(sale.master_fee_tyiyn)}</div>}
        <div>Провёл: {sale.user_name}</div>
        {sale.delivery_address && <div>Доставка: {sale.delivery_address}</div>}
        {sale.comment && <div>Комментарий: {sale.comment}</div>}
        {sale.reversal_of && (
          <div>
            Возврат по <Link className="text-sky-700 hover:underline" to={`/sales/${sale.reversal_of}`}>исходному чеку</Link>
          </div>
        )}
        {sale.returns.length > 0 && (
          <div className="flex flex-wrap gap-2">
            Возвраты:
            {sale.returns.map((r) => (
              <Link key={r.id} className="text-sky-700 hover:underline" to={`/sales/${r.id}`}>
                № {r.number} ({formatSom(r.total_tyiyn)})
              </Link>
            ))}
          </div>
        )}
      </Card>
      <Card>
        <Table head={['Наименование', 'Кол-во', 'Цена', 'Сумма', ...(owner ? ['Себестоимость'] : [])]}>
          {sale.lines.map((l) => (
            <tr key={l.line_no}>
              <td className="px-2 py-2">
                {l.name}
                {l.gift && (
                  <span className="ml-2">
                    <Badge tone="green">подарок</Badge>
                  </span>
                )}
                {!l.gift && l.unit_price_tyiyn !== l.list_price_tyiyn && (
                  <span className="ml-2">
                    <Badge tone="amber">прайс {formatSom(l.list_price_tyiyn)}</Badge>
                  </span>
                )}
              </td>
              <td className="px-2 py-2">{lineQtyText(l)}</td>
              <td className="px-2 py-2">
                {formatSom(l.unit_price_tyiyn)}
                {l.kind === 'pour' && '/л'}
              </td>
              <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums">{formatSom(l.amount_tyiyn)}</td>
              {owner && <td className="whitespace-nowrap px-2 py-2 text-right tabular-nums text-slate-500">{formatSom(l.cost_tyiyn ?? 0)}</td>}
            </tr>
          ))}
        </Table>
        <div className="mt-3 flex flex-col items-end gap-1 text-sm">
          <div className="text-lg font-semibold">Итого: {formatSom(sale.total_tyiyn)}</div>
          {sale.payments.map((p) => (
            <div key={p.method}>
              {paymentLabel(p)}: {formatSom(p.amount_tyiyn)}
            </div>
          ))}
          {owner && <div className="text-slate-500">Валовая прибыль: {formatSom(sale.total_tyiyn - cost)}</div>}
        </div>
      </Card>
      {returning && <ReturnModal sale={sale} onClose={() => setReturning(false)} />}
    </div>
  )
}
