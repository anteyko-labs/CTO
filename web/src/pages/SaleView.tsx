// Просмотр чека, печать и частичный возврат (SPEC-04).
import { useState } from 'react'
import { Link, useNavigate, useParams } from 'react-router-dom'
import { CorrectionLinks, FixButton } from '../components/Correction'
import { PayChangeButton, PayChangeModal } from '../components/PayChange'
import { lineQtyText, printSale } from '../components/salePrint'
import { Badge, Button, Card, ErrorBox, Field, Loading, Modal, Notice, PageHeader, Table } from '../components/ui'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { can } from '../lib/permissions'
import { batteryShare, paymentLabel } from '../lib/battery'
import { formatDateTime, formatKg, formatLiters, formatSom, parseLiters, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { printDebtDoc, type DebtDocSettings, type Reconciliation } from '../lib/debtDocs'
import { divRound } from '../lib/money'
import { returnWithholds } from '../lib/serviceSale'
import { returnDebtCap, splitReturn } from '../lib/returnSplit'
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
  /** Сколько из `other` уже снято с долга. */
  debt: number
}

/** Возвращённое по строкам исходного чека: количество и сумма (положительные); оплаты возвратов. */
async function loadReturned(sale: Sale): Promise<ReturnedAll> {
  const map = new Map<number, Returned>()
  let battery = 0
  let other = 0
  let debt = 0
  const returns = await Promise.all(sale.returns.map((r) => get<Sale>(`/sales/${r.id}`)))
  for (const r of returns) {
    for (const l of r.lines) {
      // Удержание за замену — не возврат строки (ADR-104).
      if (l.withheld_for != null) continue
      const cur = map.get(l.line_no) ?? { qty: 0, amount: 0 }
      map.set(l.line_no, { qty: cur.qty + l.qty, amount: cur.amount - l.amount_tyiyn })
    }
    for (const p of r.payments) {
      if (p.method === 'battery') battery -= p.amount_tyiyn
      else other -= p.amount_tyiyn
      if (p.method === 'debt') debt -= p.amount_tyiyn
    }
  }
  return { lines: map, battery, other, debt }
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
  // Возврат — тем же способом, каким платили. Долговая часть снимается с долга отдельным полем
  // (SPEC-36 §8), остаток — деньгами.
  const paidBy = [...sale.payments].sort((a, b) => b.amount_tyiyn - a.amount_tyiyn).map((p) => p.method)
  // Аккумулятор деньгами не возвращается — его часть уходит отдельно (ниже).
  const methods: PaymentMethod[] = [...new Set<PaymentMethod>([...paidBy, 'cash', 'card', 'transfer'])].filter(
    (m) => m !== 'battery' && m !== 'debt',
  )
  const [method, setMethod] = useState<PaymentMethod>(paidBy.find((m) => m !== 'battery' && m !== 'debt') ?? 'cash')
  const paidDebt = sale.payments.filter((p) => p.method === 'debt').reduce((acc, p) => acc + p.amount_tyiyn, 0)
  // Сколько клиент должен сейчас: долг могли погасить после чека — тогда возврат деньгами.
  const party = useLoad(
    () => (paidDebt > 0 && sale.party_id ? get<Party>(`/parties/${sale.party_id}`) : Promise.resolve(null)),
    [sale.id],
  )
  // Последняя правка «С долга» или «Деньгами»; null — делит окно.
  const [edit, setEdit] = useState<{ field: 'debt' | 'money'; text: string } | null>(null)
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
  const returnedSum = chosen.reduce((acc, r) => acc + r.refund, 0)
  // Масло вернули ниже канистры, а бесплатная замена остаётся — её цена удерживается (ADR-104).
  const withholds = returnWithholds(
    sale.lines,
    (n) => returned.data?.lines.get(n)?.qty ?? 0,
    (n) => chosen.find((r) => r.l.line_no === n)?.n ?? 0,
    returnedSum,
  )
  const withheld = withholds.reduce((acc, w) => acc + w.amount, 0)
  const total = returnedSum - withheld
  const invalid = rows.some((r) => !r.ok)
  const paidBattery = sale.payments.filter((p) => p.method === 'battery').reduce((acc, p) => acc + p.amount_tyiyn, 0)
  const paidOther = sale.payments.filter((p) => p.method !== 'battery').reduce((acc, p) => acc + p.amount_tyiyn, 0)
  const batteryPart = batteryCash
    ? 0
    : batteryShare(total, paidBattery - (returned.data?.battery ?? 0), paidOther - (returned.data?.other ?? 0))
  const debtLeft = paidDebt - (returned.data?.debt ?? 0)
  const owes = Math.max(0, party.data?.balance_tyiyn ?? 0)
  const split = splitReturn(
    total - batteryPart,
    returnDebtCap(debtLeft, owes),
    edit && { field: edit.field, value: parseSom(edit.text) },
  )
  const noShift = method === 'cash' && split.money > 0 && shift.data === null
  const payments = [
    ...(split.money > 0 ? [{ method, amount_tyiyn: split.money }] : []),
    ...(split.debt > 0 ? [{ method: 'debt' as const, amount_tyiyn: split.debt }] : []),
    ...(batteryPart > 0 ? [{ method: 'battery' as const, amount_tyiyn: batteryPart }] : []),
  ]
  const shown = (field: 'debt' | 'money') =>
    edit?.field === field ? edit.text : edit && parseSom(edit.text) === null ? '' : somInput(split[field])

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
      {returned.loading || party.loading ? (
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
                    onChange={(e) => {
                      setQty({ ...qty, [l.line_no]: e.target.value })
                      setEdit(null)
                    }}
                  />
                </td>
                <td className="px-2 py-2 text-right">{refund ? formatSom(refund) : '—'}</td>
              </tr>
            ))}
          </Table>
          {paidDebt > 0 && (
            <div className="flex flex-col gap-2">
              <Notice tone="amber">
                <span data-testid="return-debt-hint">
                  Клиент должен {formatSom(owes)}, по этому чеку в долг {formatSom(Math.max(0, debtLeft))}
                </span>
              </Notice>
              <div className="grid gap-3 sm:grid-cols-2">
                <Field label="С долга" hint="Деньги не выдаются: сумма снимается с долга клиента" error={split.ok ? null : 'С долга — не больше, чем клиент должен по этому чеку'}>
                  <input
                    id="return-debt"
                    inputMode="decimal"
                    value={shown('debt')}
                    onChange={(e) => setEdit({ field: 'debt', text: e.target.value })}
                  />
                </Field>
                <Field label="Деньгами">
                  <input
                    id="return-money"
                    inputMode="decimal"
                    value={shown('money')}
                    onChange={(e) => setEdit({ field: 'money', text: e.target.value })}
                  />
                </Field>
              </div>
            </div>
          )}
          <div className="grid gap-3 sm:grid-cols-2">
            <Field
              label={paidDebt > 0 ? 'Как вернуть деньги' : 'Как вернуть'}
              hint={
                method === 'bonus'
                  ? 'Баллы вернутся клиенту'
                  : method === 'cash'
                    ? 'Наличные из кассы'
                    : method === 'invoice'
                      ? 'Перечислением со счёта, куда пришла оплата по счёту'
                      : 'Возврат на карту / QR через терминал'
              }
            >
              <select id="return-method" value={method} onChange={(e) => setMethod(e.target.value as PaymentMethod)}>
                {methods.map((m) => (
                  <option key={m} value={m}>
                    {PAYMENT_LABELS[m]}
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
          {withheld > 0 && (
            <div className="text-right" data-testid="withheld">
              Удержано за замену: <b>{formatSom(withheld)}</b>
              <div className="text-xs text-slate-500">Масла осталось меньше канистры — замена больше не бесплатная</div>
            </div>
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
          <ErrorBox error={error ?? returned.error ?? party.error} />
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={onClose}>
              Отмена
            </Button>
            <Button variant="danger" disabled={busy || invalid || !split.ok || noShift || chosen.length === 0 || !comment.trim()} onClick={() => void submit()}>
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
  const navigate = useNavigate()
  const user = useUser()
  const owner = user.role === 'owner'
  const { data: sale, error, loading } = useLoad(() => get<Sale>(`/sales/${id}`), [id])
  const [returning, setReturning] = useState(false)
  const [payChange, setPayChange] = useState(false)
  const docAct = useAction()

  if (loading && !sale) return <Loading />
  if (!sale) return <ErrorBox error={error} />

  const cost = sale.lines.reduce((acc, l) => acc + (l.cost_tyiyn ?? 0), 0)
  // Удержание за замену (ADR-104) — не строка товара: показывается под таблицей.
  const goods = sale.lines.filter((l) => l.withheld_for == null)
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
        title={`${sale.kind === 'return' ? (sale.no_receipt ? 'Возврат без чека' : 'Возврат') : 'Чек'} № ${sale.number}`}
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
            {sale.kind === 'sale' && sale.returns.length === 0 && (
              // Исправить чек целиком: сторно и верный чек одним действием (SPEC-29 §24).
              <FixButton onClick={() => navigate(`/sales/${sale.id}/correct`)} />
            )}
            {sale.kind === 'sale' && sale.returns.length === 0 && (
              // Изменить только оплату: сторно и новый чек, пересчитанные наличные не трогаем (SPEC-37).
              <PayChangeButton onClick={() => setPayChange(true)} />
            )}
            {/* Возврат — право администратора (ADR-097). */}
            {sale.kind === 'sale' && !fullyReturned && can(user, 'sale_return') && (
              <Button variant="danger-outline" onClick={() => setReturning(true)}>
                Возврат
              </Button>
            )}
          </>
        }
      />
      <CorrectionLinks id={sale.id} hrefOf={(doc) => `/sales/${doc}`} />
      {payChange && (
        <PayChangeModal
          saleId={sale.id}
          onClose={() => setPayChange(false)}
          onSaved={(newId) => {
            setPayChange(false)
            navigate(`/sales/${newId}`)
          }}
        />
      )}
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
        {sale.master_fee_tyiyn !== 0 && (
          <div data-testid="master-fee">
            Мастеру за замену: {formatSom(sale.master_fee_tyiyn)}
            {/* Разбивка по видам замены (ADR-096); у старых чеков ставка — за чек, без строк. */}
            {sale.lines.some((l) => l.master_fee_tyiyn !== 0) && (
              <ul className="ml-4 list-disc text-slate-600">
                {sale.lines
                  .filter((l) => l.master_fee_tyiyn !== 0)
                  .map((l) => (
                    <li key={l.line_no}>
                      {l.name}
                      {l.qty > 1 && ` × ${l.qty}`} — {formatSom(l.master_fee_tyiyn)}
                    </li>
                  ))}
              </ul>
            )}
          </div>
        )}
        <div>Провёл: {sale.user_name}</div>
        {sale.delivery_address && <div>Доставка: {sale.delivery_address}</div>}
        {sale.comment && (
          <div>
            {sale.kind === 'return' ? 'Причина' : 'Комментарий'}: {sale.comment}
          </div>
        )}
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
          {goods.map((l) => (
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
          {sale.lines
            .filter((l) => l.withheld_for != null)
            .map((l) => (
              <div key={l.line_no} data-testid="withheld">
                Удержано за замену: {formatSom(l.amount_tyiyn)}
              </div>
            ))}
          <div className="text-lg font-semibold">Итого: {formatSom(sale.total_tyiyn)}</div>
          {sale.payments.map((p) => (
            <div key={`${p.method}:${p.account_id ?? ''}`}>
              {paymentLabel(p)}
              {p.account_name && <span className="text-slate-500"> · {p.account_name}</span>}: {formatSom(p.amount_tyiyn)}
            </div>
          ))}
          {owner && <div className="text-slate-500">Валовая прибыль: {formatSom(sale.total_tyiyn - cost)}</div>}
        </div>
      </Card>
      {returning && <ReturnModal sale={sale} onClose={() => setReturning(false)} />}
    </div>
  )
}
