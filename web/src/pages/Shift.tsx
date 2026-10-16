import { useEffect, useState } from 'react'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, Notice, PageHeader, Table, toast } from '../components/ui'
import { Icon } from '../components/icons'
import { Pager, usePageState, usePager } from '../components/Pager'
import { get, getPage, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { StaffPayList } from '../components/StaffPayList'
import { formatDate, formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad, usePolling } from '../lib/hooks'
import { MOVEMENT_LABELS } from '../lib/income'
import { pendingCount, syncOutbox, watchOutbox } from '../lib/offline'
import type { CashAccount, CashMovement, Employee, PayrollRow, Shift as ShiftRow } from '../lib/types'

/** Виды движений словами — общие с «Счётом» и «Куда сколько поступило». */
const KIND_LABELS = MOVEMENT_LABELS

/** Движения наличных словами: «+ продажи наличными». */
const CASH_LABELS: Record<string, string> = {
  sale: 'Продажи наличными',
  sale_return: 'Возвраты наличными',
  cash_in: 'Внесли',
  cash_out: 'Изъяли',
  transfer_in: 'Переведено в кассу',
  transfer_out: 'Переведено из кассы',
  expense: 'Расходы из кассы',
  payout: 'Выплаты сотрудникам',
  supplier_payment: 'Оплата поставщикам',
  supplier_refund: 'Возмещения за брак',
  debt_repayment: 'Погашения долгов',
  battery_intake: 'Приём аккумуляторов',
  count_diff: 'Пересчёт',
  reversal: 'Сторно',
}

/** Строка отчёта: подпись и сумма, со знаком для движений. */
function Row({ label, value, signed = false, strong = false }: { label: string; value: number; signed?: boolean; strong?: boolean }) {
  return (
    <div className={`flex items-baseline justify-between gap-3 py-0.5 ${strong ? 'mt-1 border-t border-slate-200 pt-1 font-semibold' : 'text-slate-700'}`}>
      <span>{label}</span>
      <span className={`whitespace-nowrap tabular-nums ${signed && value < 0 ? 'text-rose-700' : ''}`}>
        {signed ? `${value < 0 ? '−' : '+'} ${formatSom(Math.abs(value))}` : formatSom(value)}
      </span>
    </div>
  )
}

/** Выплата сотруднику при закрытии смены: галочка, сумма (не больше к выдаче) и свой op_id. */
type PayLine = { employee_id: string; name: string; owed: number; on: boolean; sum: string; opId: string }

type Closing = {
  /** null — пересчёт не правили: подставляется «должно быть» после выплат. */
  counted: string | null
  comment: string
  opId: string
  force: boolean
  pay: PayLine[]
}

type MoneyForm = { kind: 'cash_in' | 'cash_out' | 'transfer'; sum: string; comment: string; from: string; to: string; opId: string }

/** Смена и кассы: открыть, внести, изъять, перевести, закрыть с пересчётом и сдать кассу (SPEC-05). */
export default function Shift() {
  const owner = useUser().role === 'owner'
  const current = useLoad(() => get<ShiftRow | null>('/shifts/current'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const past = useLoad(() => get<ShiftRow[]>('/shifts'), [])
  // «Должно быть в кассе» и остатки касс живые: продажи с другого устройства видны сразу.
  usePolling(current.reload)
  usePolling(accounts.reload)
  const [open, setOpen] = useState<{ cashier: string; counted: string; opId: string } | null>(null)
  const [money, setMoney] = useState<MoneyForm | null>(null)
  const [closing, setClosing] = useState<Closing | null>(null)
  // Чеки этого устройства, ещё не ушедшие на сервер: их наличные уже в кассе, а в «должно быть» —
  // ещё нет. Закрыть смену до их отправки — ложная недостача (ADR-059).
  const [unsent, setUnsent] = useState(0)
  useEffect(() => watchOutbox((items) => setUnsent(pendingCount(items))), [])
  const [handover, setHandover] = useState<{ shift: ShiftRow; sum: string; opId: string } | null>(null)
  const [reopen, setReopen] = useState<{ shift: ShiftRow; reason: string; opId: string } | null>(null)
  const [history, setHistory] = useState<string>('')
  const [reverse, setReverse] = useState<{ m: CashMovement; comment: string; opId: string } | null>(null)
  // Движения открытой кассы перечитываются по счётчику: после любой операции на экране.
  const [movesTick, setMovesTick] = useState(0)
  const act = useAction()

  const shift = current.data ?? null
  const cashiers = (employees.data ?? []).filter((e) => e.active && e.is_cashier)
  const accs = accounts.data ?? []
  // Выручку сдают в сейф магазина; в сейф владельца деньги перекладывает он сам (ADR-037).
  const shopSafe = accs.find((a) => a.kind === 'safe' && !a.owner_only)
  const pending = (past.data ?? []).find((s) => s.handover_pending)
  const reloadAll = () => {
    current.reload()
    accounts.reload()
    past.reload()
    setMovesTick((t) => t + 1)
  }
  const pastPager = usePager(past.data ?? [], 'shift.past')
  const closeDialogs = () => {
    act.setError(null)
    setOpen(null)
    setMoney(null)
    setClosing(null)
    setHandover(null)
    setReopen(null)
    setReverse(null)
  }

  const openShift = () =>
    void act.run(async () => {
      if (!open) return
      const counted = open.counted.trim() ? parseSom(open.counted) : null
      if (open.counted.trim() && counted === null) throw new Error('Неверная сумма')
      await post('/shifts/open', {
        op_id: open.opId,
        cashier_employee_id: open.cashier,
        counted_tyiyn: counted,
      })
      setOpen(null)
      reloadAll()
      toast('Смена открыта')
    })

  const startClosing = () =>
    void act.run(async () => {
      closeDialogs()
      if (!shift) return
      // Заодно толкаем отправку очереди: связь могла вернуться. Не ждём — без ответа сервера это
      // до 20 с, а число неотправленных в окне обновится само.
      void syncOutbox()
      // Сумму «должно быть» берём свежую с сервера: за время на экране могли пройти чеки с другого
      // устройства, и старая сумма дала бы недостачу, которой нет.
      const fresh = await get<ShiftRow | null>('/shifts/current')
      current.reload()
      if (fresh) current.setData(fresh)
      // Кому сколько к выдаче: заработанное сегодня и невыплаченное за прошлые дни (SPEC-27).
      const staff = await get<PayrollRow[]>('/payroll/day')
      const pay = staff
        .filter((r) => r.balance_tyiyn > 0)
        .map((r) => ({ employee_id: r.employee_id, name: r.full_name, owed: r.balance_tyiyn, on: false, sum: somInput(r.balance_tyiyn), opId: newOpId() }))
      setClosing({ counted: null, comment: '', opId: newOpId(), force: false, pay })
    })

  const closeShift = () =>
    void act.run(async () => {
      if (!shift || !closing) return
      const counted = parseSom(countedText)
      if (counted === null) throw new Error('Неверная сумма')
      // Выплаты и закрытие — одна операция на сервере: либо всё, либо ничего (SPEC-27).
      const payouts = closing.pay
        .filter((p) => p.on)
        .map((p) => ({ op_id: p.opId, employee_id: p.employee_id, amount_tyiyn: parseSom(p.sum) ?? 0 }))
      const res = await post<ShiftRow>(`/shifts/${shift.id}/close`, {
        op_id: closing.opId,
        counted_tyiyn: counted,
        comment: closing.comment,
        payouts,
      })
      setClosing(null)
      reloadAll()
      toast(payouts.length > 0 ? `Выплачено ${payouts.length}, смена закрыта` : 'Смена закрыта')
      // Деньги сдают сразу: подставляем всё, что пересчитали, кассир оставит размен.
      setHandover({ shift: res, sum: somInput(res.counted_tyiyn ?? 0), opId: newOpId() })
    })

  const submitHandover = (amount: number) =>
    void act.run(async () => {
      if (!handover) return
      await post(`/shifts/${handover.shift.id}/handover`, { op_id: handover.opId, amount_tyiyn: amount })
      setHandover(null)
      reloadAll()
      toast(amount > 0 ? 'Выручка в сейфе' : 'Отмечено: деньги остались в кассе')
    })

  const submitReopen = () =>
    void act.run(async () => {
      if (!reopen) return
      await post(`/shifts/${reopen.shift.id}/reopen`, { op_id: reopen.opId, reason: reopen.reason })
      setReopen(null)
      reloadAll()
      toast('Смена переоткрыта')
    })

  const submitMoney = () =>
    void act.run(async () => {
      if (!money) return
      const sum = parseSom(money.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      if (money.kind === 'transfer') {
        await post('/cash/transfers', {
          op_id: money.opId,
          from_account_id: money.from,
          to_account_id: money.to,
          amount_tyiyn: sum,
          comment: money.comment,
        })
      } else {
        await post('/cash/movements', {
          op_id: money.opId,
          kind: money.kind,
          amount_tyiyn: sum,
          comment: money.comment,
        })
      }
      setMoney(null)
      reloadAll()
      toast('Записано')
    })

  const submitReverse = () =>
    void act.run(async () => {
      if (!reverse) return
      const { m } = reverse
      const path = m.doc_type === 'transfer' ? `/cash/transfers/${m.doc_id}/reverse` : `/cash/movements/${m.id}/reverse`
      await post(path, { op_id: reverse.opId, comment: reverse.comment })
      setReverse(null)
      reloadAll()
      toast('Сторно проведено')
    })

  const startMoney = (kind: MoneyForm['kind']) => {
    closeDialogs()
    setMoney({
      kind,
      sum: '',
      comment: '',
      from: kind === 'transfer' ? (shift?.account_id ?? '') : '',
      to: kind === 'transfer' ? (shopSafe?.id ?? '') : '',
      opId: newOpId(),
    })
  }

  // Администратор переводит только из кассы смены (SPEC-05, права).
  const fromAccounts = owner ? accs : accs.filter((a) => a.kind === 'register')
  const openMissing = missingWithFocus([Boolean(open?.cashier), 'кассира', '#shift-cashier'])
  // Выплаты при закрытии уменьшают «должно быть»: пересчитывают то, что осталось после них.
  const payLines = closing?.pay ?? []
  const payBad = payLines.some((p) => {
    const v = parseSom(p.sum)
    return p.on && (v === null || v <= 0 || v > p.owed)
  })
  const payTotal = payLines.reduce((acc, p) => (p.on ? acc + Math.max(parseSom(p.sum) ?? 0, 0) : acc), 0)
  const payCount = payLines.filter((p) => p.on).length
  const expectedAfter = (shift?.expected_tyiyn ?? 0) - payTotal
  const countedText = closing?.counted ?? somInput(expectedAfter)
  const setPay = (i: number, patch: Partial<PayLine>) =>
    closing && setClosing({ ...closing, pay: closing.pay.map((p, j) => (j === i ? { ...p, ...patch } : p)) })
  const closeDiff = (() => {
    if (!closing || !shift) return null
    const counted = parseSom(countedText)
    return counted === null ? null : counted - expectedAfter
  })()
  const closeMissing = missingWithFocus(
    [!payBad, 'суммы выплат: больше нуля и не больше к выдаче', '#close-pay'],
    [payTotal <= (shift?.expected_tyiyn ?? 0), 'выплат больше, чем в кассе', '#close-pay'],
    [parseSom(countedText) !== null, 'сколько денег в кассе', '#close-counted'],
    [!closeDiff || Boolean(closing?.comment.trim()), 'комментарий к расхождению', '#close-comment'],
    [unsent === 0 || Boolean(closing?.force), 'отметку «закрыть всё равно»', '#close-force input'],
  )
  const moneyMissing = money
    ? missingWithFocus(
        [(parseSom(money.sum) ?? 0) > 0, 'сумму', '#money-sum'],
        [money.kind !== 'transfer' || Boolean(money.from), 'откуда', '#money-from'],
        [money.kind !== 'transfer' || Boolean(money.to), 'куда', '#money-to'],
        [money.kind !== 'transfer' || money.from !== money.to, 'разные кассы', '#money-to'],
        [money.kind === 'transfer' || Boolean(money.comment.trim()), 'за что', '#money-comment'],
      )
    : []
  const handoverSum = handover ? parseSom(handover.sum) : null
  const handoverMax = handover ? (handover.shift.counted_tyiyn ?? 0) : 0

  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Смена и кассы" />
      <ErrorBox error={current.error ?? accounts.error} />

      {pending && !handover && (
        <Card className="flex flex-wrap items-center justify-between gap-3 border-amber-300 bg-amber-50">
          <div>
            <div className="font-medium">Смена № {pending.number} закрыта, касса не сдана</div>
            <div className="text-sm text-slate-600">
              Пересчитали {formatSom(pending.counted_tyiyn ?? 0)}. Переведите выручку в сейф или отметьте, что деньги остались в кассе.
            </div>
          </div>
          <Button
            onClick={() => {
              closeDialogs()
              setHandover({ shift: pending, sum: somInput(pending.counted_tyiyn ?? 0), opId: newOpId() })
            }}
          >
            Сдать кассу
          </Button>
        </Card>
      )}

      {current.loading && !current.data ? (
        <Loading />
      ) : shift ? (
        <Card className="flex flex-col gap-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="text-xs text-slate-500">
                Смена № {shift.number} от {formatDate(shift.business_date)}, кассир {shift.cashier_name}
              </div>
              <div className="text-xs text-slate-500">Открыл {shift.opened_by}, {formatDateTime(shift.opened_at)}</div>
            </div>
            <div className="text-right">
              <div className="text-xs text-slate-500">Должно быть в кассе</div>
              <div className="text-3xl font-bold tabular-nums">{formatSom(shift.expected_tyiyn)}</div>
            </div>
          </div>
          <div className="grid gap-4 text-sm lg:grid-cols-3">
            <div>
              <div className="mb-1 font-medium">Наличные в кассе</div>
              <Row label="На начало (размен)" value={shift.opening_expected_tyiyn} />
              {shift.breakdown
                .filter((b) => b.sum_tyiyn !== 0)
                .map((b) => (
                  <Row key={b.kind} label={CASH_LABELS[b.kind] ?? KIND_LABELS[b.kind] ?? b.kind} value={b.sum_tyiyn} signed />
                ))}
              <Row label="Должно быть в кассе" value={shift.expected_tyiyn} strong />
            </div>
            <div>
              <div className="mb-1 font-medium">
                Продано: {shift.sales_count ?? 0} чек. на {formatSom(shift.sales_total_tyiyn ?? 0)}
              </div>
              <Row label="Наличными" value={shift.cash_sales_tyiyn} />
              <Row label="Картой" value={shift.card_tyiyn} />
              <Row label="QR" value={shift.transfer_tyiyn} />
              {shift.debt_tyiyn !== 0 && <Row label="В долг" value={shift.debt_tyiyn} />}
              {(shift.bonus_tyiyn ?? 0) !== 0 && <Row label="Баллами" value={shift.bonus_tyiyn ?? 0} />}
              {(shift.battery_tyiyn ?? 0) !== 0 && <Row label="Аккумуляторами" value={shift.battery_tyiyn ?? 0} />}
              {shift.bank_fee_tyiyn > 0 && <Row label="Комиссия банка" value={-shift.bank_fee_tyiyn} signed />}
              {(shift.returns_tyiyn ?? 0) !== 0 && <Row label="Возвраты" value={-(shift.returns_tyiyn ?? 0)} signed />}
            </div>
            <div>
              <div className="mb-1 font-medium">Заработали в этот день</div>
              <StaffPayList staff={shift.pay ?? []} />
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="secondary" onClick={() => startMoney('cash_in')}>
              Внести
            </Button>
            <Button variant="secondary" onClick={() => startMoney('cash_out')}>
              Изъять
            </Button>
            <Button variant="secondary" onClick={() => startMoney('transfer')}>
              Перевести
            </Button>
            <Button onClick={startClosing}>Закрыть смену</Button>
          </div>
        </Card>
      ) : (
        <Card className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <div className="font-medium">Смена не открыта</div>
            <div className="text-sm text-slate-500">Продавать можно и так, но деньги лучше считать сменами.</div>
          </div>
          <div className="flex flex-wrap gap-2">
            {owner && (
              <Button variant="secondary" onClick={() => startMoney('transfer')}>
                Перевести
              </Button>
            )}
            <Button
              onClick={() => {
                closeDialogs()
                setOpen({ cashier: cashiers.length === 1 ? cashiers[0].id : '', counted: '', opId: newOpId() })
              }}
            >
              Открыть смену
            </Button>
          </div>
        </Card>
      )}

      <Card>
        <h2 className="mb-3 font-semibold">Кассы</h2>
        {accs.length === 0 ? (
          <Loading />
        ) : (
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {accs.map((a) => (
              <button
                key={a.id}
                type="button"
                aria-pressed={history === a.id}
                className={`flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-left transition hover:border-sky-400 hover:bg-slate-50 ${
                  history === a.id ? 'border-sky-500 bg-sky-50' : 'border-slate-200'
                }`}
                onClick={() => setHistory(history === a.id ? '' : a.id)}
              >
                <div className="min-w-0">
                  <div className="text-xs text-slate-500">
                    {a.name}
                    {a.owner_only && ' · только владелец'}
                  </div>
                  <Money value={a.balance_tyiyn} tone="negative" className="text-xl font-bold" />
                </div>
                <Icon name="chevron" className={`h-4 w-4 shrink-0 text-slate-400 transition ${history === a.id ? 'rotate-90 text-sky-600' : ''}`} />
              </button>
            ))}
          </div>
        )}
        {!history && accs.length > 0 && <div className="mt-2 text-xs text-slate-400">Нажмите на кассу, чтобы увидеть её движения.</div>}
        {history && (
          <AccountMoves
            id={history}
            tick={movesTick}
            onReverse={(m) => {
              closeDialogs()
              setReverse({ m, comment: '', opId: newOpId() })
            }}
          />
        )}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Прошлые смены</h2>
        <ErrorBox error={past.error} />
        {(past.data ?? []).length === 0 ? (
          <Empty>Смен ещё не было</Empty>
        ) : (
          <Table head={['№', 'День', 'Кассир', 'Ожидали', 'Факт', 'Расхождение', 'В сейф', '']}>
            {pastPager.shown.map((s, i) => (
              <tr key={s.id}>
                <td className="px-2 py-2 font-medium">{s.number}</td>
                <td className="whitespace-nowrap px-2 py-2 tabular-nums">{formatDate(s.business_date)}</td>
                <td className="px-2 py-2">{s.cashier_name}</td>
                <td className="px-2 py-2 text-right">{s.counted_tyiyn === null ? '—' : <Money value={s.expected_tyiyn} />}</td>
                <td className="px-2 py-2 text-right">{s.counted_tyiyn === null ? <Badge tone="green">открыта</Badge> : <Money value={s.counted_tyiyn} />}</td>
                <td className="whitespace-nowrap px-2 py-2 text-right">
                  {s.diff_tyiyn ? <Badge tone={s.diff_tyiyn < 0 ? 'rose' : 'amber'}>{formatSom(s.diff_tyiyn)}</Badge> : '—'}
                </td>
                <td className="whitespace-nowrap px-2 py-2 text-right">
                  {s.handover_pending ? <Badge tone="amber">не сдана</Badge> : s.to_safe_tyiyn === null ? '—' : <Money value={s.to_safe_tyiyn} />}
                </td>
                <td className="px-2 py-2 text-right">
                  {/* Переоткрыть можно только последнюю смену: после более поздней — 409 (ADR-071). */}
                  {owner && pastPager.page === 0 && i === 0 && s.counted_tyiyn !== null && !shift && (
                    <Button
                      variant="secondary"
                      className="min-h-9 px-3 py-1"
                      onClick={() => {
                        closeDialogs()
                        setReopen({ shift: s, reason: '', opId: newOpId() })
                      }}
                    >
                      Переоткрыть
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
        )}
        <Pager page={pastPager.page} size={pastPager.size} total={pastPager.total} onPage={pastPager.setPage} onSize={pastPager.setSize} label="Страницы смен" />
      </Card>

      {open && (
        <Modal title="Открыть смену" onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <Field label="Кассир" required>
              <select id="shift-cashier" value={open.cashier} onChange={(e) => setOpen({ ...open, cashier: e.target.value })}>
                <option value="">— выберите —</option>
                {cashiers.map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.full_name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Пересчитал, с" hint="Если не пересчитывали — оставьте пустым">
              <input inputMode="decimal" value={open.counted} onChange={(e) => setOpen({ ...open, counted: e.target.value })} />
            </Field>
            <Missing items={openMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || openMissing.length > 0} onClick={openShift}>
                Открыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {closing && shift && (
        <Modal title="Закрыть смену" onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            {unsent > 0 && (
              <Notice tone="rose">
                <div className="font-medium">
                  {unsent} {unsent === 1 ? 'чек ещё не отправлен' : unsent < 5 ? 'чека ещё не отправлены' : 'чеков ещё не отправлены'} — дождитесь связи
                </div>
                <div className="mt-1">
                  Их деньги уже в кассе, но в «должно быть» их ещё нет: закрытие сейчас покажет недостачу, которой нет. Когда чеки уйдут,
                  предупреждение исчезнет само.
                </div>
                <div id="close-force" className="mt-2">
                  <Checkbox label="Закрыть всё равно" checked={closing.force} onChange={(v) => setClosing({ ...closing, force: v })} />
                </div>
              </Notice>
            )}
            {closing.pay.length > 0 && (
              <div id="close-pay" className="flex flex-col gap-2 rounded-md border border-slate-200 p-3">
                <div className="text-sm font-medium">Выплатить из кассы</div>
                {closing.pay.map((p, i) => (
                  <div key={p.employee_id} className="flex flex-wrap items-center gap-2" data-testid="close-pay-row">
                    <div className="min-w-0 flex-1">
                      <Checkbox label={p.name} checked={p.on} onChange={(v) => setPay(i, { on: v })} />
                      <div className="pl-7 text-xs text-slate-500">к выдаче {formatSom(p.owed)}</div>
                    </div>
                    <input
                      aria-label={`Сумма для ${p.name}`}
                      className="w-32"
                      inputMode="decimal"
                      disabled={!p.on}
                      value={p.sum}
                      onChange={(e) => setPay(i, { sum: e.target.value })}
                    />
                  </div>
                ))}
                <div className="text-xs text-slate-500">
                  Заработанное сегодня и невыплаченное за прошлые дни. Можно выдать меньше; аванс сверх заработанного — только владелец, в
                  «Расчёте».
                </div>
              </div>
            )}
            <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-700" data-testid="close-noncash">
              Безнал за смену: картой <b className="tabular-nums">{formatSom(shift.card_tyiyn)}</b> · QR{' '}
              <b className="tabular-nums">{formatSom(shift.transfer_tyiyn)}</b>
              <div className="text-xs text-slate-500">В кассе этих денег нет — сверьте с терминалом и выпиской банка.</div>
            </div>
            <div className="text-sm text-slate-600">
              Должно быть в кассе: {formatSom(shift.expected_tyiyn)}
              {payTotal > 0 && (
                <>
                  {' '}
                  − выплаты {formatSom(payTotal)} = <b>{formatSom(expectedAfter)}</b>
                </>
              )}
            </div>
            <Field label="Факт в кассе, с" required hint={payTotal > 0 ? 'Пересчитайте после выдачи денег' : undefined}>
              <input
                id="close-counted"
                autoFocus
                inputMode="decimal"
                value={countedText}
                onChange={(e) => setClosing({ ...closing, counted: e.target.value })}
              />
            </Field>
            {closeDiff !== null && closeDiff !== 0 && (
              <div className={`rounded-md px-3 py-2 text-sm ${closeDiff < 0 ? 'bg-rose-50 text-rose-800' : 'bg-amber-50 text-amber-800'}`}>
                {closeDiff < 0 ? 'Недостача' : 'Излишек'} {formatSom(Math.abs(closeDiff))}
                {closeDiff < 0 && ` · будет удержана с кассира ${shift.cashier_name}`}
              </div>
            )}
            <Field label="Комментарий" required={Boolean(closeDiff)} hint="Обязателен при расхождении">
              <input id="close-comment" value={closing.comment} onChange={(e) => setClosing({ ...closing, comment: e.target.value })} />
            </Field>
            <Missing items={closeMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || closeMissing.length > 0} onClick={closeShift}>
                {payCount > 0 ? 'Выплатить отмеченным и закрыть' : 'Закрыть'}
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {handover && (
        <Modal title={`Сдать кассу · смена № ${handover.shift.number}`} onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              В кассе {formatSom(handoverMax)}. Сколько переложить в {shopSafe?.name ?? 'сейф магазина'}? Остальное останется на размен.
            </div>
            <Field label="В сейф, с" required>
              <input autoFocus inputMode="decimal" value={handover.sum} onChange={(e) => setHandover({ ...handover, sum: e.target.value })} />
            </Field>
            {handoverSum !== null && handoverSum >= 0 && handoverSum <= handoverMax && (
              <div className="text-sm text-slate-600">Останется в кассе: {formatSom(handoverMax - handoverSum)}</div>
            )}
            {handoverSum !== null && handoverSum > handoverMax && (
              <div className="text-sm text-rose-700">В кассе столько нет</div>
            )}
            <ErrorBox error={act.error} />
            <div className="flex flex-wrap justify-end gap-2">
              <Button variant="secondary" disabled={act.busy} onClick={() => submitHandover(0)}>
                Деньги остались в кассе
              </Button>
              <Button
                disabled={act.busy || handoverSum === null || handoverSum <= 0 || handoverSum > handoverMax}
                onClick={() => handoverSum !== null && submitHandover(handoverSum)}
              >
                Перевести в сейф
              </Button>
            </div>
            <div className="text-xs text-slate-400">Владелец получит уведомление: сколько было, сколько пересчитали, что ушло в сейф.</div>
          </div>
        </Modal>
      )}

      {reopen && (
        <Modal title={`Переоткрыть смену № ${reopen.shift.number}`} onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              Пересчёт и удержание прошлого закрытия останутся в истории; при новом закрытии пересчёт пойдёт от текущего остатка.
            </div>
            <Field label="Причина" required>
              <input id="reopen-reason" autoFocus value={reopen.reason} onChange={(e) => setReopen({ ...reopen, reason: e.target.value })} />
            </Field>
            <Missing items={missingWithFocus([Boolean(reopen.reason.trim()), 'причину', '#reopen-reason'])} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || !reopen.reason.trim()} onClick={submitReopen}>
                Переоткрыть
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {reverse && (
        <Modal title="Сторно" onClose={closeDialogs}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              {KIND_LABELS[reverse.m.kind] ?? reverse.m.kind} на {formatSom(Math.abs(reverse.m.amount_tyiyn))}
              {reverse.m.comment && ` — ${reverse.m.comment}`}. Запись останется в истории, рядом появится обратная.
            </div>
            <Field label="Причина" required>
              <input id="reverse-comment" autoFocus value={reverse.comment} onChange={(e) => setReverse({ ...reverse, comment: e.target.value })} />
            </Field>
            <Missing items={missingWithFocus([Boolean(reverse.comment.trim()), 'причину', '#reverse-comment'])} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button variant="danger" disabled={act.busy || !reverse.comment.trim()} onClick={submitReverse}>
                Провести сторно
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {money && (
        <Modal
          title={money.kind === 'cash_in' ? 'Внести в кассу' : money.kind === 'cash_out' ? 'Изъять из кассы' : 'Перевести между кассами'}
          onClose={closeDialogs}
        >
          <div className="flex flex-col gap-3">
            {money.kind === 'transfer' && (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Откуда" required>
                  <select id="money-from" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                    <option value="">—</option>
                    {fromAccounts.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Куда" required>
                  <select id="money-to" value={money.to} onChange={(e) => setMoney({ ...money, to: e.target.value })}>
                    <option value="">—</option>
                    {accs.map((a) => (
                      <option key={a.id} value={a.id}>
                        {a.name}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
            )}
            <Field label="Сумма, с" required>
              <input id="money-sum" autoFocus inputMode="decimal" value={money.sum} onChange={(e) => setMoney({ ...money, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий" required={money.kind !== 'transfer'}>
              <input id="money-comment" value={money.comment} onChange={(e) => setMoney({ ...money, comment: e.target.value })} />
            </Field>
            <Missing items={moneyMissing} />
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              <Button disabled={act.busy || moneyMissing.length > 0} onClick={submitMoney}>
                Записать
              </Button>
            </div>
          </div>
        </Modal>
      )}

      {!owner && <div className="text-xs text-slate-400">Сейф владельца и его остаток администратору не показываются.</div>}
    </div>
  )
}

/** Движения выбранной кассы, новые сверху; страницу режет сервер. `tick` — перечитать после операции. */
function AccountMoves({ id, tick, onReverse }: { id: string; tick: number; onReverse: (m: CashMovement) => void }) {
  const pager = usePageState('shift.movements', id)
  const moves = useLoad(() => getPage<CashMovement>(`/cash/accounts/${id}`, { limit: pager.size, offset: pager.page * pager.size }), [id, pager.page, pager.size, tick])
  const rows = moves.data?.items ?? []
  return (
    <div className="mt-4">
      <ErrorBox error={moves.error} />
      {moves.loading && !moves.data ? (
        <Loading />
      ) : (moves.data?.total ?? 0) === 0 ? (
        <Empty>Движений ещё не было</Empty>
      ) : (
        <>
          <Table head={['Когда', 'Что', 'Сумма', 'Комментарий', 'Кто', '']}>
            {rows.map((m) => (
              <tr key={m.id}>
                <td className="whitespace-nowrap px-2 py-2">{formatDateTime(m.created_at)}</td>
                <td className="px-2 py-2">{KIND_LABELS[m.kind] ?? m.kind}</td>
                <td className="px-2 py-2 text-right">
                  <Money value={m.amount_tyiyn} tone="negative" className="font-medium" />
                </td>
                <td className="px-2 py-2 text-slate-600">{m.comment}</td>
                <td className="px-2 py-2">{m.user_name}</td>
                <td className="px-2 py-2 text-right">
                  {m.reversible && (
                    <Button variant="ghost" onClick={() => onReverse(m)}>
                      Сторно
                    </Button>
                  )}
                </td>
              </tr>
            ))}
          </Table>
          <Pager page={pager.page} size={pager.size} total={moves.data?.total ?? 0} onPage={pager.setPage} onSize={pager.setSize} label="Страницы движений кассы" />
        </>
      )}
    </div>
  )
}
