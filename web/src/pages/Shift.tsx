import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { Badge, Button, Card, Checkbox, Empty, ErrorBox, Field, Loading, Missing, Modal, Money, Notice, PageHeader, Table, toast } from '../components/ui'
import { Icon } from '../components/icons'
import { Pager, usePageState, usePager } from '../components/Pager'
import { CorrectionBadges, CorrectionBanner, FixButton, useCorrection } from '../components/Correction'
import { get, getPage, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { useCan } from '../lib/permissions'
import { ShiftOpenModal } from '../components/ShiftOpen'
import { StaffPayList } from '../components/StaffPayList'
import { formatDate, formatDateTime, formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction, useLoad, usePolling } from '../lib/hooks'
import { MOVEMENT_LABELS, shiftSales } from '../lib/income'
import { accountLabel, handoverChoices, isTill, overdrawText, payAccounts } from '../lib/accounts'
import { pendingCount, syncOutbox, watchOutbox } from '../lib/offline'
import { correctionMarks, loadCorrections, numberText } from '../lib/corrections'
import { movementDoc, movementFields, transferDoc, transferFields, type MovementDoc, type TransferDoc } from '../lib/moneyFix'
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
  /** Откуда выплатить отмеченным: касса смены или, у владельца, любой счёт (SPEC-29 §13). */
  payFrom: string
}

/** Исправление проведённого внесения, изъятия или перевода (SPEC-29 §24): то же окно, заполненное
 *  исходным документом; `filled` — поля уже взяты из него. */
type MoneyFix = { kind: 'cash_movement' | 'cash_transfer'; id: string; filled: boolean }

type MoneyForm = { kind: 'cash_in' | 'cash_out' | 'transfer'; sum: string; comment: string; from: string; to: string; opId: string; fix?: MoneyFix }

/** Смена и кассы: открыть, внести, изъять, перевести, закрыть с пересчётом и сдать кассу (SPEC-05). */
export default function Shift() {
  const owner = useUser().role === 'owner'
  // Внести, изъять, перевести и выплатить — права администратора (ADR-097).
  const can = useCan()
  const current = useLoad(() => get<ShiftRow | null>('/shifts/current'), [])
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const employees = useLoad(() => get<Employee[]>('/employees'), [])
  const past = useLoad(() => get<ShiftRow[]>('/shifts'), [])
  // «Должно быть в кассе» и остатки касс живые: продажи с другого устройства видны сразу.
  usePolling(current.reload)
  usePolling(accounts.reload)
  // Окно «Открыть смену»: кассир, PIN или пароль владельца, пересчёт (ShiftOpenModal).
  const [open, setOpen] = useState(false)
  const [money, setMoney] = useState<MoneyForm | null>(null)
  const correction = useCorrection<MovementDoc | TransferDoc>(money?.fix?.kind ?? 'cash_movement', money?.fix?.id ?? null)
  const [closing, setClosing] = useState<Closing | null>(null)
  // Чеки этого устройства, ещё не ушедшие на сервер: их наличные уже в кассе, а в «должно быть» —
  // ещё нет. Закрыть смену до их отправки — ложная недостача (ADR-059).
  const [unsent, setUnsent] = useState(0)
  useEffect(() => watchOutbox((items) => setUnsent(pendingCount(items))), [])
  const [handover, setHandover] = useState<{ shift: ShiftRow; sum: string; opId: string; to: string } | null>(null)
  const [reopen, setReopen] = useState<{ shift: ShiftRow; reason: string; opId: string } | null>(null)
  const [history, setHistory] = useState<string>('')
  const [reverse, setReverse] = useState<{ m: CashMovement; comment: string; opId: string } | null>(null)
  // Движения открытой кассы перечитываются по счётчику: после любой операции на экране.
  const [movesTick, setMovesTick] = useState(0)
  const act = useAction()

  const shift = current.data ?? null
  // Продано − возвраты = итого = сумма по способам оплаты: числа на экране складываются.
  const sales = shift ? shiftSales(shift) : null
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
    setOpen(false)
    setMoney(null)
    setClosing(null)
    setHandover(null)
    setReopen(null)
    setReverse(null)
  }

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
      // Администратору сервер отдаёт только мастеров: кассирам выплачивает владелец (SPEC-30 §6).
      const staff = can('payroll_pay') ? await get<PayrollRow[]>('/payroll/day') : []
      const pay = staff
        .filter((r) => r.balance_tyiyn > 0)
        .map((r) => ({ employee_id: r.employee_id, name: r.full_name, owed: r.balance_tyiyn, on: false, sum: somInput(r.balance_tyiyn), opId: newOpId() }))
      setClosing({ counted: null, comment: '', opId: newOpId(), force: false, pay, payFrom: fresh?.account_id ?? shift.account_id })
    })

  const closeShift = () =>
    void act.run(async () => {
      if (!shift || !closing) return
      const counted = parseSom(countedText)
      if (counted === null) throw new Error('Неверная сумма')
      // Выплаты и закрытие — одна операция на сервере: либо всё, либо ничего (SPEC-27).
      const payouts = closing.pay
        .filter((p) => p.on)
        .map((p) => ({ op_id: p.opId, employee_id: p.employee_id, amount_tyiyn: parseSom(p.sum) ?? 0, account_id: closing.payFrom || undefined }))
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
      startHandover(res)
    })

  /** Окно «Сдать кассу»: куда — предлагает сервер (куда сдавали в прошлый раз или сейф магазина). */
  const startHandover = (s: ShiftRow) => {
    setHandover({ shift: s, sum: somInput(s.counted_tyiyn ?? 0), opId: newOpId(), to: '' })
    void get<{ account_id: string | null }>('/shifts/handover-target')
      .then((t) => setHandover((h) => (h && h.shift.id === s.id && !h.to ? { ...h, to: t.account_id ?? '' } : h)))
      .catch(() => undefined)
  }

  const submitHandover = (amount: number) =>
    void act.run(async () => {
      if (!handover) return
      await post(`/shifts/${handover.shift.id}/handover`, {
        op_id: handover.opId,
        amount_tyiyn: amount,
        ...(amount > 0 && handoverTo ? { to_account_id: handoverTo } : {}),
      })
      setHandover(null)
      reloadAll()
      toast(amount > 0 ? `Касса сдана: ${formatSom(amount)} → ${handoverAcc?.name ?? 'сейф'}` : 'Отмечено: деньги остались в кассе')
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
          // Владелец вносит и изымает на любом счёте; без выбора — касса смены (ADR-076).
          account_id: money.from || undefined,
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

  const startFix = (m: CashMovement) => {
    closeDialogs()
    correction.setComment('')
    const transfer = m.doc_type === 'transfer'
    setMoney({
      kind: transfer ? 'transfer' : m.kind === 'cash_in' ? 'cash_in' : 'cash_out',
      sum: '',
      comment: '',
      from: '',
      to: '',
      opId: newOpId(),
      fix: { kind: transfer ? 'cash_transfer' : 'cash_movement', id: transfer ? (m.doc_id ?? '') : m.id, filled: false },
    })
  }

  const submitFix = async () => {
    if (!money?.fix) return
    const doc = money.fix.kind === 'cash_transfer' ? transferDoc(money) : movementDoc(money, till?.id ?? null)
    if (!doc) return
    const c = await correction.save(doc)
    if (!c) return
    setMoney(null)
    reloadAll()
    toast(`Исправлено: ${numberText(c.new_number) || (money.kind === 'transfer' ? 'перевод' : 'запись')}`)
  }

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
  const fromAccounts = owner ? accs : accs.filter((a) => isTill(a))
  // Касса смены: её остаток — «должно быть» на начало новой смены (SPEC-29 §16).
  const till = accs.find((a) => isTill(a))
  // Списание больше остатка — предупреждение, не запрет (ADR-077); при исправлении не показываем:
  // исходная сумма вернётся на счёт.
  const moneySource = money && money.kind !== 'cash_in' ? (money.from ? accs.find((a) => a.id === money.from) : till) : undefined
  const moneyOverdraw = money && !money.fix && moneySource ? overdrawText(moneySource.balance_tyiyn, parseSom(money.sum)) : ''
  // Исправление: поля окна заполняются исходным документом, как только он пришёл с сервера.
  const fixDraft = correction.draft.data
  if (money?.fix && !money.fix.filled && fixDraft && fixDraft.original_id === money.fix.id) {
    const fields =
      money.fix.kind === 'cash_transfer' ? transferFields(fixDraft.doc as TransferDoc) : movementFields(fixDraft.doc as MovementDoc, till?.id ?? null)
    setMoney({ ...money, ...fields, fix: { ...money.fix, filled: true } })
  }
  // Выплаты при закрытии: владелец — с любого счёта, администратор — из кассы смены (SPEC-29 §13).
  const payFromList = payAccounts(accs, owner)
  const fromTill = !closing || !shift || closing.payFrom === shift.account_id || !closing.payFrom
  const { list: handoverList, pick: handoverPick } = handoverChoices(accs, handover?.to)
  const handoverTo = handover?.to && handoverList.some((a) => a.id === handover.to) ? handover.to : handoverPick
  const handoverAcc = accs.find((a) => a.id === handoverTo)
  // Выплаты при закрытии уменьшают «должно быть»: пересчитывают то, что осталось после них.
  const payLines = closing?.pay ?? []
  const payBad = payLines.some((p) => {
    const v = parseSom(p.sum)
    return p.on && (v === null || v <= 0 || v > p.owed)
  })
  const payTotal = payLines.reduce((acc, p) => (p.on ? acc + Math.max(parseSom(p.sum) ?? 0, 0) : acc), 0)
  const payCount = payLines.filter((p) => p.on).length
  // «Должно быть» уменьшают только выплаты из кассы смены.
  const tillPay = fromTill ? payTotal : 0
  const expectedAfter = (shift?.expected_tyiyn ?? 0) - tillPay
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
    [tillPay <= (shift?.expected_tyiyn ?? 0), 'выплат больше, чем в кассе', '#close-pay'],
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
        [!money.fix || Boolean(correction.comment.trim()), 'что и почему исправляете', '#correction-comment'],
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
              startHandover(pending)
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
            {sales && (
            <div data-testid="shift-sales">
              <div className="mb-1 font-medium">Продажи смены</div>
              <Row label={`Продано: ${sales.count} чек.`} value={sales.gross} />
              {sales.returns !== 0 && <Row label="Возвраты" value={-sales.returns} signed />}
              {sales.returns !== 0 && <Row label="Итого за вычетом возвратов" value={sales.net} strong />}
              {sales.methods.length > 0 && (
                <div className="mt-2 text-xs text-slate-500">По способам оплаты{sales.returns !== 0 ? ' (с учётом возвратов)' : ''}:</div>
              )}
              {sales.methods.map((m) => (
                <Row key={m.key} label={m.label} value={m.value} />
              ))}
              {sales.rest !== 0 && <Row label="Исправления и прочее" value={sales.rest} signed />}
              {(shift.bank_split ?? []).length > 1 && (
                <>
                  <div className="mt-2 text-xs text-slate-500">Безнал по счетам:</div>
                  {(shift.bank_split ?? []).map((b) => (
                    <Row key={b.account_id} label={`на «${b.name}»`} value={b.total_tyiyn} />
                  ))}
                </>
              )}
              {shift.bank_fee_tyiyn > 0 && <Row label="Комиссия банка" value={-shift.bank_fee_tyiyn} signed />}
            </div>
            )}
            <div>
              <div className="mb-1 font-medium">Заработали в этот день</div>
              <StaffPayList staff={shift.pay ?? []} />
            </div>
          </div>
          <div className="flex flex-wrap gap-2">
            {can('cash_moves') && (
              <>
                <Button variant="secondary" onClick={() => startMoney('cash_in')}>
                  Внести
                </Button>
                <Button variant="secondary" onClick={() => startMoney('cash_out')}>
                  Изъять
                </Button>
                <Button variant="secondary" onClick={() => startMoney('transfer')}>
                  Перевести
                </Button>
              </>
            )}
            {/* Отчёт смены, как в Умаге (SPEC-30 §7): движение денег, выручка, операции, отменённые товары. */}
            <Link
              to={`/shifts/${shift.id}`}
              className="inline-flex min-h-[42px] items-center justify-center gap-2 rounded-md border border-slate-300 bg-white px-4 py-2 text-sm font-medium text-slate-800 hover:bg-slate-50"
            >
              <Icon name="history" className="h-4 w-4" />
              Отчёт смены
            </Link>
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
                setOpen(true)
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
            onFix={startFix}
          />
        )}
      </Card>

      <Card>
        <h2 className="mb-3 font-semibold">Прошлые смены</h2>
        <ErrorBox error={past.error} />
        {(past.data ?? []).length === 0 ? (
          <Empty>Смен ещё не было</Empty>
        ) : (
          <Table head={['№', 'День', 'Кассир', 'Ожидали', 'Факт', 'Расхождение', 'Сдано', '']}>
            {pastPager.shown.map((s, i) => (
              <tr key={s.id}>
                <td className="px-2 py-2 font-medium">
                  <Link to={`/shifts/${s.id}`} className="text-sky-700 hover:underline" aria-label={`Отчёт смены № ${s.number}`}>
                    {s.number}
                  </Link>
                </td>
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
                  <div className="flex flex-wrap items-center justify-end gap-2">
                  <Link to={`/shifts/${s.id}`} className="inline-flex min-h-9 items-center rounded-md border border-slate-300 bg-white px-3 py-1 text-sm font-medium text-slate-800 hover:bg-slate-50">
                    Отчёт
                  </Link>
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
                  </div>
                </td>
              </tr>
            ))}
          </Table>
        )}
        <Pager page={pastPager.page} size={pastPager.size} total={pastPager.total} onPage={pastPager.setPage} onSize={pastPager.setSize} label="Страницы смен" />
      </Card>

      {open && (
        <ShiftOpenModal
          owner={owner}
          cashiers={cashiers}
          till={till}
          onClose={closeDialogs}
          onOpened={() => {
            setOpen(false)
            reloadAll()
          }}
        />
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
                <div className="text-sm font-medium">{fromTill ? 'Выплатить из кассы' : 'Выплатить сотрудникам'}</div>
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
                {owner && payFromList.length > 1 && (
                  <Field label="Выплатить из">
                    <select id="close-pay-from" value={closing.payFrom} onChange={(e) => setClosing({ ...closing, payFrom: e.target.value })}>
                      {payFromList.map((a) => (
                        <option key={a.id} value={a.id}>
                          {accountLabel(a)}
                        </option>
                      ))}
                    </select>
                  </Field>
                )}
                <div className="text-xs text-slate-500">
                  Заработанное сегодня и невыплаченное за прошлые дни. Можно выдать меньше; аванс сверх заработанного — только владелец, в
                  «Расчёте».
                </div>
              </div>
            )}
            <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-700" data-testid="close-noncash">
              Безнал за смену: карта / QR банка <b className="tabular-nums">{formatSom(shift.card_tyiyn)}</b> · QR владельцу{' '}
              <b className="tabular-nums">{formatSom(shift.transfer_tyiyn)}</b>
              {(shift.bank_split ?? []).length > 1 && (
                <ul className="mt-1 text-xs" data-testid="close-noncash-banks">
                  {(shift.bank_split ?? []).map((b) => (
                    <li key={b.account_id}>
                      {b.name}: карта / QR банка {formatSom(b.card_tyiyn)} · QR владельцу {formatSom(b.transfer_tyiyn)}
                    </li>
                  ))}
                </ul>
              )}
              <div className="text-xs text-slate-500">В кассе этих денег нет — сверьте с терминалом и выпиской банка.</div>
            </div>
            <div className="text-sm text-slate-600">
              Должно быть в кассе: {formatSom(shift.expected_tyiyn)}
              {tillPay > 0 && (
                <>
                  {' '}
                  − выплаты {formatSom(tillPay)} = <b>{formatSom(expectedAfter)}</b>
                </>
              )}
            </div>
            <Field label="Факт в кассе, с" required hint={tillPay > 0 ? 'Пересчитайте после выдачи денег' : undefined}>
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
            <div className="text-sm text-slate-600">В кассе {formatSom(handoverMax)}. Сколько сдать? Остальное останется на размен.</div>
            <Field label="Куда сдать" required>
              <select id="handover-to" value={handoverTo} onChange={(e) => setHandover({ ...handover, to: e.target.value })}>
                {handoverList.map((a) => (
                  <option key={a.id} value={a.id}>
                    {accountLabel(a)}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Сдать, с" required>
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
                disabled={act.busy || handoverSum === null || handoverSum <= 0 || handoverSum > handoverMax || !handoverTo}
                onClick={() => handoverSum !== null && submitHandover(handoverSum)}
              >
                Сдать в «{handoverAcc?.name ?? '…'}»
              </Button>
            </div>
            <div className="text-xs text-slate-400">Владелец получит уведомление: сколько сдали, куда и кто кассир.</div>
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
          title={`${money.fix ? 'Исправить: ' : ''}${money.kind === 'cash_in' ? 'Внести в кассу' : money.kind === 'cash_out' ? 'Изъять из кассы' : 'Перевести между кассами'}`}
          onClose={closeDialogs}
        >
          <div className="flex flex-col gap-3">
            {money.fix && !money.fix.filled && (correction.draft.loading ? <Loading /> : null)}
            {money.fix && fixDraft && (
              <CorrectionBanner label={fixDraft.kind_label} number={fixDraft.number} comment={correction.comment} onComment={correction.setComment} />
            )}
            {money.kind === 'transfer' && (
              <div className="grid grid-cols-2 gap-3">
                <Field label="Откуда" required>
                  <select id="money-from" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                    <option value="">—</option>
                    {fromAccounts.map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                  </select>
                </Field>
                <Field label="Куда" required>
                  <select id="money-to" value={money.to} onChange={(e) => setMoney({ ...money, to: e.target.value })}>
                    <option value="">—</option>
                    {accs.map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
            )}
            {money.kind !== 'transfer' && owner && accs.length > 1 && (
              <Field label="Касса или счёт">
                <select id="money-account" value={money.from} onChange={(e) => setMoney({ ...money, from: e.target.value })}>
                  <option value="">Касса смены</option>
                  {accs
                    .filter((a) => !isTill(a))
                    .map((a) => (
                      <option key={a.id} value={a.id}>
                        {accountLabel(a)}
                      </option>
                    ))}
                </select>
              </Field>
            )}
            <Field label="Сумма, с" required>
              <input id="money-sum" autoFocus inputMode="decimal" value={money.sum} onChange={(e) => setMoney({ ...money, sum: e.target.value })} />
            </Field>
            <Field label="Комментарий" required={money.kind !== 'transfer'}>
              <input
                id="money-comment"
                value={money.comment}
                onChange={(e) => setMoney({ ...money, comment: e.target.value })}
                onKeyDown={(e) => {
                  // Enter в последнем поле — как главная кнопка (ui-rules §4).
                  if (e.key !== 'Enter' || moneyMissing.length > 0) return
                  e.preventDefault()
                  if (money.fix) {
                    if (!correction.busy && money.fix.filled) void submitFix()
                  } else if (!act.busy) submitMoney()
                }}
              />
            </Field>
            {moneyOverdraw && (
              <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800" data-testid="money-overdraw">
                {moneyOverdraw}
              </div>
            )}
            <Missing items={moneyMissing} />
            <ErrorBox error={money.fix ? correction.error : act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={closeDialogs}>
                Отмена
              </Button>
              {money.fix ? (
                <Button disabled={correction.busy || !money.fix.filled || moneyMissing.length > 0} onClick={() => void submitFix()}>
                  Сохранить исправление
                </Button>
              ) : (
                <Button disabled={act.busy || moneyMissing.length > 0} onClick={submitMoney}>
                  Записать
                </Button>
              )}
            </div>
          </div>
        </Modal>
      )}

      {!owner && <div className="text-xs text-slate-400">Сейф владельца и его остаток администратору не показываются.</div>}
    </div>
  )
}

/** Движения выбранной кассы, новые сверху; страницу режет сервер. `tick` — перечитать после операции. */
function AccountMoves({ id, tick, onReverse, onFix }: { id: string; tick: number; onReverse: (m: CashMovement) => void; onFix: (m: CashMovement) => void }) {
  const pager = usePageState('shift.movements', id)
  const moves = useLoad(() => getPage<CashMovement>(`/cash/accounts/${id}`, { limit: pager.size, offset: pager.page * pager.size }), [id, pager.page, pager.size, tick])
  // Отметки исправлений: внесения и изъятия — по id движения, переводы — по id перевода.
  const fixes = useLoad(() => Promise.all([loadCorrections('cash_movement'), loadCorrections('cash_transfer')]).then(([a, b]) => correctionMarks([...a, ...b])), [tick])
  const markOf = (m: CashMovement) => fixes.data?.get(m.doc_type === 'transfer' && m.doc_id ? m.doc_id : m.id)
  const rows = moves.data?.items ?? []
  const mayReverse = useCan()('cash_moves')
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
                <td className="px-2 py-2">
                  {KIND_LABELS[m.kind] ?? m.kind}
                  <span className="ml-2 inline-flex flex-wrap gap-1">
                    <CorrectionBadges mark={markOf(m)} />
                  </span>
                </td>
                <td className="px-2 py-2 text-right">
                  <Money value={m.amount_tyiyn} tone="negative" className="font-medium" />
                </td>
                <td className="px-2 py-2 text-slate-600">{m.comment}</td>
                <td className="px-2 py-2">{m.user_name}</td>
                <td className="px-2 py-2 text-right">
                  {m.reversible && mayReverse && (
                    <div className="flex items-center justify-end gap-2">
                      <FixButton small onClick={() => onFix(m)} />
                      <Button variant="ghost" onClick={() => onReverse(m)}>
                        Сторно
                      </Button>
                    </div>
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
