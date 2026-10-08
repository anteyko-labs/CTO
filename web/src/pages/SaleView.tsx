// Просмотр чека, печать и частичный возврат (SPEC-04).
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { lineQtyText, printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDateTime, formatLiters, formatSom, parseLiters } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { divRound } from '../lib/money'
import { PAYMENT_LABELS, type PaymentMethod, type Sale, type SaleLine } from '../lib/types'

interface Returned {
  qty: number
  amount: number
}

/** Возвращённое по строкам исходного чека: количество и сумма (положительные). */
async function loadReturned(sale: Sale): Promise<Map<number, Returned>> {
  const map = new Map<number, Returned>()
  const returns = await Promise.all(sale.returns.map((r) => get<Sale>(`/sales/${r.id}`)))
  for (const r of returns) {
    for (const l of r.lines) {
      const cur = map.get(l.line_no) ?? { qty: 0, amount: 0 }
      map.set(l.line_no, { qty: cur.qty + l.qty, amount: cur.amount - l.amount_tyiyn })
    }
  }
  return map
}

/** Сумма возврата строки — то же правило, что на сервере: последняя часть забирает остаток. */
function refundOf(l: SaleLine, done: Returned, qty: number): number {
  if (done.qty + qty === l.qty) return l.amount_tyiyn - done.amount
  return divRound(l.amount_tyiyn * qty, l.qty)
}

function ReturnModal({ sale, onClose }: { sale: Sale; onClose: () => void }) {
  const navigate = useNavigate()
  const returned = useLoad(() => loadReturned(sale), [sale.id])
  const [qty, setQty] = useState<Record<number, string>>({})
  const [method, setMethod] = useState<PaymentMethod>('cash')
  const [comment, setComment] = useState('')
  const [opId] = useState(newOpId)
  const { busy, error, run } = useAction()

  const rows = sale.lines.map((l) => {
    const done = returned.data?.get(l.line_no) ?? { qty: 0, amount: 0 }
    const left = l.qty - done.qty
    const text = qty[l.line_no]?.trim() ?? ''
    const n = !text ? 0 : l.kind === 'pour' ? parseLiters(text) : /^\d+$/.test(text) ? Number(text) : null
    const ok = n !== null && n >= 0 && n <= left
    return { l, left, n, ok, refund: ok && n ? refundOf(l, done, n) : 0 }
  })
  const chosen = rows.filter((r) => r.ok && r.n)
  const total = chosen.reduce((acc, r) => acc + r.refund, 0)
  const invalid = rows.some((r) => !r.ok)

  const submit = () =>
    run(async () => {
      const res = await post<Sale>(`/sales/${sale.id}/return`, {
        op_id: opId,
        comment,
        lines: chosen.map((r) => ({ line_no: r.l.line_no, qty: r.n })),
        payments: [{ method, amount_tyiyn: total }],
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
                <td className="px-2 py-2">{l.kind === 'pour' ? formatLiters(left) : left}</td>
                <td className="px-2 py-2">
                  <input
                    className={`w-24 ${ok ? '' : 'border-rose-400'}`}
                    inputMode={l.kind === 'pour' ? 'decimal' : 'numeric'}
                    placeholder={l.kind === 'pour' ? 'литры' : '0'}
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
            <Field label="Вернуть деньги">
              <select value={method} onChange={(e) => setMethod(e.target.value as PaymentMethod)}>
                {(['cash', 'card', 'transfer'] as const).map((m) => (
                  <option key={m} value={m}>
                    {PAYMENT_LABELS[m]}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Причина">
              <input value={comment} onChange={(e) => setComment(e.target.value)} />
            </Field>
          </div>
          <div className="text-right text-lg font-semibold">К возврату: {formatSom(total)}</div>
          <ErrorBox error={error ?? returned.error} />
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={onClose}>
              Отмена
            </Button>
            <Button variant="danger" disabled={busy || invalid || chosen.length === 0 || !comment.trim()} onClick={() => void submit()}>
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

  if (loading && !sale) return <Loading />
  if (!sale) return <ErrorBox error={error} />

  const cost = sale.lines.reduce((acc, l) => acc + (l.cost_tyiyn ?? 0), 0)
  const fullyReturned = sale.kind === 'sale' && sale.returns.reduce((acc, r) => acc + r.total_tyiyn, 0) === -sale.total_tyiyn

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={`${sale.kind === 'return' ? 'Возврат' : 'Чек'} № ${sale.number}`}
        actions={
          <>
            <Button variant="secondary" onClick={() => printSale(sale)}>
              Печать
            </Button>
            {sale.kind === 'sale' && !fullyReturned && (
              <Button variant="danger" onClick={() => setReturning(true)}>
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
                {l.unit_price_tyiyn !== l.list_price_tyiyn && (
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
              <td className="px-2 py-2 text-right">{formatSom(l.amount_tyiyn)}</td>
              {owner && <td className="px-2 py-2 text-right text-slate-500">{formatSom(l.cost_tyiyn ?? 0)}</td>}
            </tr>
          ))}
        </Table>
        <div className="mt-3 flex flex-col items-end gap-1 text-sm">
          <div className="text-lg font-semibold">Итого: {formatSom(sale.total_tyiyn)}</div>
          {sale.payments.map((p) => (
            <div key={p.method}>
              {PAYMENT_LABELS[p.method]}: {formatSom(p.amount_tyiyn)}
            </div>
          ))}
          {owner && <div className="text-slate-500">Валовая прибыль: {formatSom(sale.total_tyiyn - cost)}</div>}
        </div>
      </Card>
      {returning && <ReturnModal sale={sale} onClose={() => setReturning(false)} />}
    </div>
  )
}
