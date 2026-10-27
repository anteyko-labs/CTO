// Закрытие смены и «Сдать кассу» (SPEC-05, SPEC-27, SPEC-29 §13–14): одно и то же окно на экране
// «Смена» и в полосе режима кассы (SPEC-36 §10). После «Сдать кассу» или «Деньги остались в кассе»
// чек смены печатается сам, если владелец не выключил это настройкой `shift.print_on_close`.
import { useEffect, useState, type ReactNode } from 'react'
import { Button, Checkbox, ErrorBox, Field, Missing, Modal, Notice, toast } from './ui'
import { ShiftBank } from './ShiftBank'
import { printShift } from './shiftPrint'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { useCan } from '../lib/permissions'
import { formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction } from '../lib/hooks'
import { useNumField } from '../lib/cashierMode'
import { accountLabel, handoverChoices, payAccounts } from '../lib/accounts'
import { pendingCount, syncOutbox, watchOutbox } from '../lib/offline'
import type { CashAccount, PayrollRow, Shift, ShiftSettings } from '../lib/types'

/** Выплата сотруднику при закрытии смены: галочка, сумма (не больше к выдаче) и свой op_id. */
type PayLine = { employee_id: string; name: string; owed: number; on: boolean; sum: string; opId: string }

type Closing = {
  shift: Shift
  /** null — пересчёт не правили: подставляется «должно быть» после выплат. */
  counted: string | null
  comment: string
  opId: string
  force: boolean
  pay: PayLine[]
  /** Откуда выплатить отмеченным: касса смены или, у владельца, любой счёт (SPEC-29 §13). */
  payFrom: string
}

type Handover = { shift: Shift; sum: string; opId: string; to: string }

/** Напечатать чек смены после сдачи кассы, если это не выключено. Ошибка печати не мешает работе. */
async function autoPrint(shiftId: string): Promise<void> {
  const settings = await get<ShiftSettings>('/settings/shift').catch(() => ({ print_on_close: true }))
  if (!settings.print_on_close) return
  await printShift(shiftId).catch(() => toast('Чек смены не напечатался — напечатайте из «Смены»'))
}

/**
 * Окна «Закрыть смену» и «Сдать кассу». `onChanged` — перечитать экран после закрытия и сдачи.
 * Возвращает действия и разметку окон (`modals` — поставить в разметку экрана).
 */
export function useShiftClose(onChanged: () => void): {
  startClosing: () => void
  startHandover: (s: Shift) => void
  cancel: () => void
  open: boolean
  modals: ReactNode
} {
  const owner = useUser().role === 'owner'
  const can = useCan()
  const act = useAction()
  // В режиме кассы суммы — экранной клавиатурой (SPEC-29), как в окне «Открыть смену».
  const num = useNumField()
  const [closing, setClosing] = useState<Closing | null>(null)
  const [handover, setHandover] = useState<Handover | null>(null)
  const [accs, setAccs] = useState<CashAccount[]>([])
  // Чеки этого устройства, ещё не ушедшие на сервер: их наличные уже в кассе, а в «должно быть» —
  // ещё нет. Закрыть смену до их отправки — ложная недостача (ADR-059).
  const [unsent, setUnsent] = useState(0)
  useEffect(() => watchOutbox((items) => setUnsent(pendingCount(items))), [])

  const loadAccounts = () =>
    get<CashAccount[]>('/cash/accounts')
      .then(setAccs)
      .catch(() => undefined)

  const cancel = () => {
    act.setError(null)
    setClosing(null)
    setHandover(null)
  }

  const startClosing = () =>
    void act.run(async () => {
      setHandover(null)
      // Заодно толкаем отправку очереди: связь могла вернуться. Не ждём — без ответа сервера это
      // до 20 с, а число неотправленных в окне обновится само.
      void syncOutbox()
      void loadAccounts()
      // Сумму «должно быть» берём свежую с сервера: за время на экране могли пройти чеки с другого
      // устройства, и старая сумма дала бы недостачу, которой нет.
      const fresh = await get<Shift | null>('/shifts/current')
      if (!fresh) {
        onChanged()
        toast('Смена уже закрыта')
        return
      }
      // Кому сколько к выдаче: заработанное сегодня и невыплаченное за прошлые дни (SPEC-27).
      // Администратору сервер отдаёт только мастеров: кассирам выплачивает владелец (SPEC-30 §6).
      const staff = can('payroll_pay') ? await get<PayrollRow[]>('/payroll/day') : []
      const pay = staff
        .filter((r) => r.balance_tyiyn > 0)
        .map((r) => ({ employee_id: r.employee_id, name: r.full_name, owed: r.balance_tyiyn, on: false, sum: somInput(r.balance_tyiyn), opId: newOpId() }))
      setClosing({ shift: fresh, counted: null, comment: '', opId: newOpId(), force: false, pay, payFrom: fresh.account_id })
    })

  /** Окно «Сдать кассу»: куда — предлагает сервер (куда сдавали в прошлый раз или сейф магазина). */
  const startHandover = (s: Shift) => {
    act.setError(null)
    setClosing(null)
    void loadAccounts()
    setHandover({ shift: s, sum: somInput(s.counted_tyiyn ?? 0), opId: newOpId(), to: '' })
    void get<{ account_id: string | null }>('/shifts/handover-target')
      .then((t) => setHandover((h) => (h && h.shift.id === s.id && !h.to ? { ...h, to: t.account_id ?? '' } : h)))
      .catch(() => undefined)
  }

  const shift = closing?.shift ?? null
  // Выплаты при закрытии: владелец — с любого счёта, администратор — из кассы смены (SPEC-29 §13).
  const payFromList = payAccounts(accs, owner)
  const fromTill = !closing || !shift || closing.payFrom === shift.account_id || !closing.payFrom
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

  const closeShift = () =>
    void act.run(async () => {
      if (!shift || !closing) return
      const counted = parseSom(countedText)
      if (counted === null) throw new Error('Неверная сумма')
      // Выплаты и закрытие — одна операция на сервере: либо всё, либо ничего (SPEC-27).
      const payouts = closing.pay
        .filter((p) => p.on)
        .map((p) => ({ op_id: p.opId, employee_id: p.employee_id, amount_tyiyn: parseSom(p.sum) ?? 0, account_id: closing.payFrom || undefined }))
      const res = await post<Shift>(`/shifts/${shift.id}/close`, {
        op_id: closing.opId,
        counted_tyiyn: counted,
        comment: closing.comment,
        payouts,
      })
      setClosing(null)
      onChanged()
      toast(payouts.length > 0 ? `Выплачено ${payouts.length}, смена закрыта` : 'Смена закрыта')
      // Деньги сдают сразу: подставляем всё, что пересчитали, кассир оставит размен.
      startHandover(res)
    })

  const { list: handoverList, pick: handoverPick } = handoverChoices(accs, handover?.to)
  const handoverTo = handover?.to && handoverList.some((a) => a.id === handover.to) ? handover.to : handoverPick
  const handoverAcc = accs.find((a) => a.id === handoverTo)
  const handoverSum = handover ? parseSom(handover.sum) : null
  const handoverMax = handover ? (handover.shift.counted_tyiyn ?? 0) : 0

  const submitHandover = (amount: number) =>
    void act.run(async () => {
      if (!handover) return
      const id = handover.shift.id
      await post(`/shifts/${id}/handover`, {
        op_id: handover.opId,
        amount_tyiyn: amount,
        ...(amount > 0 && handoverTo ? { to_account_id: handoverTo } : {}),
      })
      setHandover(null)
      onChanged()
      toast(amount > 0 ? `Касса сдана: ${formatSom(amount)} → ${handoverAcc?.name ?? 'сейф'}` : 'Отмечено: деньги остались в кассе')
      // Чек смены — сам, после сдачи кассы (SPEC-36 §10): в нём уже видно, сколько сдали и куда.
      await autoPrint(id)
    })

  const modals = (
    <>
      {closing && shift && (
        <Modal title="Закрыть смену" onClose={cancel}>
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
                      {...num('decimal')}
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
            <div className="rounded-md bg-slate-50 px-3 py-2" data-testid="close-noncash">
              <ShiftBank list={shift.bank_split ?? []} testId="close-noncash-banks" />
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
                {...num('decimal')}
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
              <Button variant="secondary" onClick={cancel}>
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
        <Modal title={`Сдать кассу · смена № ${handover.shift.number}`} onClose={cancel}>
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
              <input autoFocus {...num('decimal')} value={handover.sum} onChange={(e) => setHandover({ ...handover, sum: e.target.value })} />
            </Field>
            {handoverSum !== null && handoverSum >= 0 && handoverSum <= handoverMax && (
              <div className="text-sm text-slate-600">Останется в кассе: {formatSom(handoverMax - handoverSum)}</div>
            )}
            {handoverSum !== null && handoverSum > handoverMax && <div className="text-sm text-rose-700">В кассе столько нет</div>}
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
    </>
  )

  return { startClosing, startHandover, cancel, open: Boolean(closing || handover), modals }
}
