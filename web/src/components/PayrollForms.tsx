// Формы расчёта сотрудника: выплата по видам (ADR-122), бонус/удержание, оклад за месяц (SPEC-07). Те же формы
// исправляют проведённое владельцем (SPEC-29 §24, ADR-085): поля заполнены из исходного документа.
import { useState, type ReactNode } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { accountLabel, isBank, isTill, payAccounts } from '../lib/accounts'
import { amountInput, fixedText } from '../lib/docFix'
import { formatSom, parseSom, somInput } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { useNumField, useTouchMode } from '../lib/cashierMode'
import { initialLines, payoutLines, PURPOSE_LABELS, type PayoutLineDraft } from '../lib/payouts'
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

/** Оклад за месяц в виде запроса проведения. */
export interface SalaryDoc {
  type?: 'salary'
  employee_id: string
  month: string
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
  fix,
  onCancel,
  onDone,
}: {
  employeeId: string
  balance?: number
  fix?: FixMode<PayoutDoc>
  onCancel: () => void
  onDone: () => void
}) {
  return fix ? <PayoutFixForm fix={fix} onCancel={onCancel} onDone={onDone} /> : <PayoutLinesForm employeeId={employeeId} onCancel={onCancel} onDone={onDone} />
}

/** Откуда платим: владелец — любой действующий счёт, администратор — касса смены (SPEC-29 §13, ADR-076). */
function usePayFrom(initial: string) {
  const owner = useUser().role === 'owner'
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const payFrom = payAccounts(accounts.data ?? [], owner)
  const [fromPick, setFromPick] = useState(initial)
  const fromId = payFrom.some((a) => a.id === fromPick) ? fromPick : (payFrom[0]?.id ?? '')
  const fromAcc = payFrom.find((a) => a.id === fromId)
  const field =
    payFrom.length > 1 ? (
      <Field label="Откуда">
        <select id="payout-from" value={fromId} onChange={(e) => setFromPick(e.target.value)}>
          {payFrom.map((a) => (
            <option key={a.id} value={a.id}>
              {accountLabel(a)}
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

function PayoutLinesForm({ employeeId, onCancel, onDone }: { employeeId: string; onCancel: () => void; onDone: () => void }) {
  const owner = useUser().role === 'owner'
  // Аванс выдаёт владелец; администратор — с правом «Аванс мастерам» (ADR-122).
  const mayAdvance = useCan()('payroll_advance')
  const rem = useLoad(() => get<PayoutRemains>(`/payroll/remains${qs({ employee_id: employeeId })}`), [employeeId])
  const [edited, setEdited] = useState<PayoutLineDraft[] | null>(null)
  const lines = edited ?? (rem.data ? initialLines(rem.data, { owner, advance: mayAdvance }, somInput) : [])
  const set = (i: number, patch: Partial<PayoutLineDraft>) => setEdited(lines.map((l, j) => (j === i ? { ...l, ...patch } : l)))
  const [comment, setComment] = useState('')
  const from = usePayFrom('')
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
      })
      toast(from.fromAcc && !isTill(from.fromAcc) ? `Выплачено: ${from.fromAcc.name}` : 'Выплачено из кассы')
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
      {from.field}
      {lines.length === 0 ? (
        <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-600">Выплачивать нечего: по всем видам выдано.</div>
      ) : (
        <PayoutLinesEditor
          lines={lines}
          onChange={set}
          total={result.total}
          numProps={num('decimal')}
          advanceHint={`в счёт ${rem.data?.advance_to === 'salary' ? 'оклада' : 'оплаты за услуги'}${owner ? '; можно сверх заработанного' : ''}`}
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
      <Footer label="Выплатить" fix={false} onCancel={onCancel} onSubmit={pay} disabled={act.busy || !rem.data || result.error !== null} />
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
  const act = useAction()

  const pay = () =>
    void act.run(async () => {
      const amount = parseSom(sum)
      if (amount === null || amount <= 0) throw new Error('Неверная сумма')
      const out = await fix.save({ ...fix.doc, amount_tyiyn: amount, account_id: from.fromId || fix.doc.account_id, comment })
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
  hint,
  initialSum = '',
  optional = false,
  fix,
  onCancel,
  onDone,
}: {
  employeeId: string
  month: string
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
      await post('/payroll/salary', { op_id: opId, employee_id: employeeId, month, amount_tyiyn: amount })
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
