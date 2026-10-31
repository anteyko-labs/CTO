// Формы расчёта сотрудника: выплата по видам (ADR-122), бонус/удержание, оклад за месяц (SPEC-07). Те же формы
// исправляют проведённое владельцем (SPEC-29 §24, ADR-085): поля заполнены из исходного документа.
import { useState, type ReactNode } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { accountLabel, isBank, isTill, payAccounts, payFromPick } from '../lib/accounts'
import { amountInput, fixedText } from '../lib/docFix'
import { formatSom, parseSom, somInput, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { useNumField, useTouchMode } from '../lib/cashierMode'
import { advanceOnly, initialLines, payoutLines, PURPOSE_LABELS, type PayoutLineDraft } from '../lib/payouts'
import { dueWhen, earliestPayDate, payDateHint, salaryText, shortDate } from '../lib/payrollPeriod'
import type { CashAccount, PayoutRemains } from '../lib/types'
import { FIX_SUBMIT, FixBanner, type FixMode } from './FixDraft'
import { Button, ErrorBox, Field, Loading, Money, toast } from './ui'

/** Выплата в виде запроса проведения (без op_id). */
export interface PayoutDoc {
  employee_id: string
  amount_tyiyn: number
  source: 'account'
  account_id?: string | null
  advance: boolean
  comment: string
  /** Вид выплаты (ADR-122): при исправлении сохраняется. */
  purpose?: string
  /** Дата выплаты в расчёте (задача 003 Н11): деньги уходят сейчас. */
  pay_date?: string
}

/** Бонус или удержание в виде запроса проведения. */
export interface ExtraDoc {
  type?: 'accrual'
  employee_id: string
  kind: 'bonus' | 'penalty'
  amount_tyiyn: number
  business_date: string
  comment: string
}

/** Оклад за период в виде запроса проведения (задача 003 Н11: период работника). */
export interface SalaryDoc {
  type?: 'salary'
  employee_id: string
  month?: string
  period_from?: string
  period_to?: string
  amount_tyiyn: number | null
}

function Footer({ onCancel, disabled, onSubmit, label, fix }: { onCancel: () => void; disabled: boolean; onSubmit: () => void; label: string; fix: boolean }) {
  return (
    <div className="flex justify-end gap-2">
      <Button variant="secondary" onClick={onCancel}>
        Отмена
      </Button>
      <Button disabled={disabled} onClick={onSubmit}>
        {fix ? FIX_SUBMIT : label}
      </Button>
    </div>
  )
}

/**
 * Выплата сотруднику (ADR-122): строки видов с остатком и галочкой, одно «Откуда», одна
 * операция. В исправлении — одна сумма того же вида: вид ставится при проведении.
 * `balance` больше не нужен: остатки по видам окно берёт с сервера.
 */
export function PayoutForm({
  employeeId,
  advance = false,
  date,
  noDate = false,
  fix,
  onCancel,
  onDone,
}: {
  employeeId: string
  balance?: number
  /** «Аванс» из «Расчёта за месяц» (задача 002, П13): в окне только строка аванса. */
  advance?: boolean
  /** Дата выплаты по умолчанию (задача 003 Н11): день «За день» или месяц «За месяц». Меняет владелец. */
  date?: string
  /** Без поля даты — касса (всегда сегодня). */
  noDate?: boolean
  fix?: FixMode<PayoutDoc>
  onCancel: () => void
  onDone: () => void
}) {
  return fix ? (
    <PayoutFixForm fix={fix} onCancel={onCancel} onDone={onDone} />
  ) : (
    <PayoutLinesForm employeeId={employeeId} advance={advance} date={date} noDate={noDate} onCancel={onCancel} onDone={onDone} />
  )
}

/**
 * Откуда платим: владелец — любой действующий счёт, администратор — касса смены (SPEC-29 §13, ADR-076).
 * Без открытой смены касса смены недоступна, по умолчанию — сейф или счёт (задача 003 Н11).
 */
function usePayFrom(initial: string, shiftOpen = true) {
  const owner = useUser().role === 'owner'
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const payFrom = payAccounts(accounts.data ?? [], owner)
  const [fromPick, setFromPick] = useState(initial)
  const fromId = payFromPick(payFrom, shiftOpen, fromPick)
  const fromAcc = payFrom.find((a) => a.id === fromId)
  const closedTill = (a: CashAccount) => !shiftOpen && isTill(a)
  const field =
    payFrom.length > 1 ? (
      <Field label="Откуда">
        <select id="payout-from" value={fromId} onChange={(e) => setFromPick(e.target.value)}>
          {payFrom.map((a) => (
            <option key={a.id} value={a.id} disabled={closedTill(a)}>
              {accountLabel(a)}
              {closedTill(a) ? ' — смена не открыта' : ''}
            </option>
          ))}
        </select>
      </Field>
    ) : (
      fromAcc && <div className="text-xs text-slate-500">{accountLabel(fromAcc)}</div>
    )
  const where = fromAcc ? `${isBank(fromAcc) ? 'со счёта' : 'из'} «${fromAcc.name}»` : 'из кассы смены'
  return { fromId, fromAcc, field, where }
}

function PayoutLinesForm({
  employeeId,
  advance,
  date,
  noDate,
  onCancel,
  onDone,
}: {
  employeeId: string
  advance: boolean
  date?: string
  noDate: boolean
  onCancel: () => void
  onDone: () => void
}) {
  const owner = useUser().role === 'owner'
  // Аванс выдаёт владелец; администратор — с правом «Аванс мастерам» (ADR-122).
  const mayAdvance = useCan()('payroll_advance')
  const rem = useLoad(() => get<PayoutRemains>(`/payroll/remains${qs({ employee_id: employeeId })}`), [employeeId])
  const [edited, setEdited] = useState<PayoutLineDraft[] | null>(null)
  const start = (r: PayoutRemains) => {
    const all = initialLines(r, { owner, advance: mayAdvance }, somInput)
    return advance ? advanceOnly(all) : all
  }
  const lines = edited ?? (rem.data ? start(rem.data) : [])
  const set = (i: number, patch: Partial<PayoutLineDraft>) => setEdited(lines.map((l, j) => (j === i ? { ...l, ...patch } : l)))
  const [comment, setComment] = useState('')
  // Смена (задача 003 Н11): без неё касса смены недоступна; администратор без смены не платит.
  const shiftOpen = rem.data?.shift_open ?? true
  const from = usePayFrom('', shiftOpen)
  const blocked = !shiftOpen && (Boolean(rem.data?.requires_shift) || !owner)
  // Дата выплаты в расчёте — только владелец, не позже сегодня и не раньше настройки (ADR-154).
  const today = rem.data?.today ?? todayBishkek()
  const backDays = rem.data?.backdate_days ?? 0
  const dateField = owner && !noDate && backDays > 0
  const [payPick, setPayPick] = useState(date ?? '')
  const payDate = dateField && payPick ? payPick : today
  const salary = rem.data?.salary ?? null
  const num = useNumField()
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит операцию.
  const [opId] = useState(newOpId)
  const act = useAction()
  const result = payoutLines(lines, parseSom)
  const withheld = rem.data?.remains.withheld_tyiyn ?? 0

  const pay = () =>
    void act.run(async () => {
      if (result.error) throw new Error(result.error)
      await post('/payouts', {
        op_id: opId,
        employee_id: employeeId,
        source: 'account',
        account_id: from.fromId || undefined,
        comment,
        lines: result.lines,
        pay_date: payDate !== today ? payDate : undefined,
      })
      const when = payDate !== today ? ` за ${shortDate(payDate)}` : ''
      toast(from.fromAcc && !isTill(from.fromAcc) ? `Выплачено${when}: ${from.fromAcc.name}` : `Выплачено${when} из кассы`)
      onDone()
    })

  if (rem.loading && !rem.data) return <Loading />
  return (
    <div className="flex flex-col gap-3">
      <ErrorBox error={rem.error} />
      {rem.data && (
        <div className="text-sm text-slate-600">
          Всего к выплате {formatSom(rem.data.balance_tyiyn)}, деньги уйдут {from.where}
        </div>
      )}
      {!shiftOpen && (
        <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900" data-testid="payout-no-shift">
          {rem.data?.requires_shift
            ? 'Смена не открыта: выплаты — только при открытой смене (Настройки → Зарплата). Откройте смену.'
            : owner
              ? `Смена не открыта: из кассы смены выплатить нельзя. Деньги уйдут ${from.where}. Или откройте смену.`
              : 'Откройте смену — администратор платит только из кассы смены.'}
        </div>
      )}
      {from.field}
      {dateField && (
        <Field label="Дата выплаты" hint={payDateHint(payDate, today, from.where) || 'в какой день выплата видна в расчёте'}>
          <input
            id="payout-date"
            type="date"
            className="w-44"
            min={earliestPayDate(today, backDays)}
            max={today}
            value={payDate}
            onChange={(e) => setPayPick(e.target.value)}
          />
        </Field>
      )}
      {salary && (
        <div className="rounded-md bg-sky-50 px-3 py-2 text-sm text-sky-900" data-testid="payout-salary">
          Оклад: {salaryText(salary, today)}
        </div>
      )}
      {lines.length === 0 ? (
        <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-600">
          {advance ? 'Аванс выдаёт владелец или администратор с правом «Аванс мастерам».' : 'Выплачивать нечего: по всем видам выдано.'}
        </div>
      ) : (
        <PayoutLinesEditor
          lines={lines}
          onChange={set}
          total={result.total}
          numProps={num('decimal')}
          advanceHint={`в счёт ${rem.data?.advance_to === 'salary' ? 'оклада' : 'оплаты за услуги'}${owner ? '; можно сверх заработанного' : ''}${
            salary ? ` · по окладу к выплате ${dueWhen(salary, today)} — ${formatSom(salary.expected_tyiyn)}` : ''
          }`}
        />
      )}
      {withheld < 0 && (
        <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">
          Удержано {formatSom(-withheld)}: вычесть пока не из чего — вычтется из следующих начислений.
        </div>
      )}
      <Field label="Комментарий">
        <input value={comment} onChange={(e) => setComment(e.target.value)} />
      </Field>
      <ErrorBox error={act.error ?? (lines.length > 0 && result.lines.length > 0 ? result.error : null)} />
      <Footer
        label={advance ? 'Выдать аванс' : 'Выплатить'}
        fix={false}
        onCancel={onCancel}
        onSubmit={pay}
        disabled={act.busy || !rem.data || result.error !== null || blocked || !from.fromId}
      />
    </div>
  )
}

/**
 * Строки выплаты по видам (ADR-122): галочка, к выдаче, сумма; внизу итог. Одно и то же в «Расчёте»,
 * при закрытии смены и на кассе. `numProps` — поле суммы с экранной клавиатурой режима кассы.
 */
export function PayoutLinesEditor({
  lines,
  onChange,
  total,
  advanceHint,
  numProps,
  name,
}: {
  lines: PayoutLineDraft[]
  onChange: (i: number, patch: Partial<PayoutLineDraft>) => void
  total: number
  advanceHint?: string
  numProps?: Record<string, unknown>
  /** Чьи строки — в подписях полей, когда в окне несколько сотрудников. */
  name?: string
}) {
  const touch = useTouchMode()
  const of = name ? ` (${name})` : ''
  return (
    <div className="flex flex-col divide-y divide-slate-100 rounded-md border border-slate-200" data-testid="payout-lines">
      {lines.map((l, i) => {
        const label = PURPOSE_LABELS[l.purpose]
        const over = l.checked && l.purpose !== 'advance' && (parseSom(l.sum) ?? 0) > l.remain
        return (
          <div key={l.purpose} className="flex flex-wrap items-center gap-3 px-3 py-2">
            <label className={`flex min-w-48 flex-1 items-center gap-2 text-sm ${touch ? 'min-h-[56px]' : 'min-h-[42px]'}`}>
              <input
                type="checkbox"
                className={touch ? 'h-6 w-6' : undefined}
                aria-label={`Выплатить: ${label}${of}`}
                checked={l.checked}
                onChange={(e) => onChange(i, { checked: e.target.checked })}
              />
              <span>
                <span className="font-medium">{label}</span>
                <span className="block text-xs text-slate-500">{l.purpose === 'advance' ? (advanceHint ?? '') : `к выдаче ${formatSom(l.remain)}`}</span>
              </span>
            </label>
            <input
              className={`w-36 text-right ${touch ? 'min-h-[56px]' : ''} ${over ? 'border-rose-400' : ''}`}
              inputMode="decimal"
              {...numProps}
              aria-label={`Сумма: ${label}${of}`}
              value={l.sum}
              disabled={!l.checked}
              onChange={(e) => onChange(i, { sum: e.target.value })}
            />
          </div>
        )
      })}
      <div className="flex items-center justify-between px-3 py-2 font-semibold">
        <span>Итого{of}</span>
        <Money value={total} />
      </div>
    </div>
  )
}

/** Исправление проведённой выплаты: та же сумма тем же видом, решает сервер (ADR-085, ADR-122). */
function PayoutFixForm({ fix, onCancel, onDone }: { fix: FixMode<PayoutDoc>; onCancel: () => void; onDone: () => void }) {
  const [sum, setSum] = useState(amountInput(fix.doc.amount_tyiyn))
  const [comment, setComment] = useState(fix.doc.comment)
  const from = usePayFrom(fix.doc.account_id ?? '')
  const today = todayBishkek()
  // Дата в расчёте — исходной выплаты, её можно поменять (задача 003 Н11).
  const [payDate, setPayDate] = useState(fix.doc.pay_date ?? today)
  const act = useAction()

  const pay = () =>
    void act.run(async () => {
      const amount = parseSom(sum)
      if (amount === null || amount <= 0) throw new Error('Неверная сумма')
      const out = await fix.save({
        ...fix.doc,
        amount_tyiyn: amount,
        account_id: from.fromId || fix.doc.account_id,
        comment,
        pay_date: payDate || today,
      })
      if (!out) return
      toast(fixedText(out))
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      <FixBanner fix={fix} />
      <div className="text-sm text-slate-600">
        {fix.doc.purpose && fix.doc.purpose !== 'general' ? `${PURPOSE_LABELS[fix.doc.purpose] ?? fix.doc.purpose}, ` : ''}деньги уйдут {from.where}
      </div>
      {from.field}
      <Field label="Дата выплаты" hint={payDateHint(payDate, today, from.where)}>
        <input id="payout-date" type="date" className="w-44" max={today} value={payDate} onChange={(e) => setPayDate(e.target.value)} />
      </Field>
      <Field label="Сумма, с" required>
        <input inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
      </Field>
      <Field label="Комментарий">
        <input value={comment} onChange={(e) => setComment(e.target.value)} />
      </Field>
      <ErrorBox error={act.error ?? fix.error} />
      <Footer fix label="Выплатить" onCancel={onCancel} onSubmit={pay} disabled={act.busy || fix.busy || !sum.trim() || !fix.comment.trim()} />
    </div>
  )
}

/** Бонус или удержание за день (SPEC-07): сумма и причина обязательны. */
export function ExtraForm({
  employeeId,
  kind,
  date,
  fix,
  onCancel,
  onDone,
}: {
  employeeId: string
  kind: 'bonus' | 'penalty'
  date: string
  fix?: FixMode<ExtraDoc>
  onCancel: () => void
  onDone: () => void
}) {
  const [sum, setSum] = useState(fix ? amountInput(fix.doc.amount_tyiyn) : '')
  const [comment, setComment] = useState(fix?.doc.comment ?? '')
  const [opId] = useState(newOpId)
  const act = useAction()

  const save = () =>
    void act.run(async () => {
      const amount = parseSom(sum)
      if (amount === null || amount <= 0) throw new Error('Неверная сумма')
      if (fix) {
        const out = await fix.save({ ...fix.doc, amount_tyiyn: amount, comment })
        if (!out) return
        toast(fixedText(out))
        onDone()
        return
      }
      await post('/payroll/accruals', { op_id: opId, employee_id: employeeId, kind, amount_tyiyn: amount, business_date: date, comment })
      toast(kind === 'bonus' ? 'Бонус начислен' : 'Удержание записано')
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      {fix && <FixBanner fix={fix} />}
      <Field label="Сумма, с" required>
        <input autoFocus={!fix} inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
      </Field>
      <Field label="Причина" required>
        <input value={comment} onChange={(e) => setComment(e.target.value)} />
      </Field>
      <ErrorBox error={act.error ?? fix?.error} />
      <Footer
        fix={Boolean(fix)}
        label="Записать"
        onCancel={onCancel}
        onSubmit={save}
        disabled={act.busy || Boolean(fix?.busy) || !sum.trim() || !comment.trim() || (fix !== undefined && !fix.comment.trim())}
      />
    </div>
  )
}

/**
 * Оклад за месяц. `optional` — пустая сумма берётся из правила оплаты (экран «За день»);
 * иначе сумма обязательна. В исправлении сумма обязательна всегда.
 */
export function SalaryForm({
  employeeId,
  month,
  period,
  hint,
  initialSum = '',
  optional = false,
  fix,
  onCancel,
  onDone,
}: {
  employeeId: string
  /** Месяц, в котором заканчивается период работника (прежний способ). */
  month?: string
  /** Период работника (задача 003 Н11): с какого по какой день. */
  period?: { from: string; to: string }
  hint: ReactNode
  initialSum?: string
  optional?: boolean
  fix?: FixMode<SalaryDoc>
  onCancel: () => void
  onDone: () => void
}) {
  const [sum, setSum] = useState(fix ? amountInput(fix.doc.amount_tyiyn) : initialSum)
  const [opId] = useState(newOpId)
  const act = useAction()
  const canEmpty = optional && !fix

  const save = () =>
    void act.run(async () => {
      const amount = sum.trim() ? parseSom(sum) : null
      if (canEmpty ? sum.trim() !== '' && amount === null : amount === null || amount <= 0) throw new Error('Неверная сумма')
      if (fix) {
        const out = await fix.save({ ...fix.doc, amount_tyiyn: amount })
        if (!out) return
        toast(fixedText(out))
        onDone()
        return
      }
      await post('/payroll/salary', {
        op_id: opId,
        employee_id: employeeId,
        ...(period ? { period_from: period.from, period_to: period.to } : { month }),
        amount_tyiyn: amount,
      })
      toast('Оклад начислен')
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      {fix && <FixBanner fix={fix} />}
      <div className="text-sm text-slate-600">{hint}</div>
      <Field label="Сумма, с" required={!canEmpty}>
        <input autoFocus={!fix} inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
      </Field>
      <ErrorBox error={act.error ?? fix?.error} />
      <Footer
        fix={Boolean(fix)}
        label="Начислить"
        onCancel={onCancel}
        onSubmit={save}
        disabled={act.busy || Boolean(fix?.busy) || (!canEmpty && !sum.trim()) || (fix !== undefined && !fix.comment.trim())}
      />
    </div>
  )
}
