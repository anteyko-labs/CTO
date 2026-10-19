// Формы расчёта сотрудника: выплата, бонус/удержание, оклад за месяц (SPEC-07). Те же формы
// исправляют проведённое владельцем (SPEC-29 §24, ADR-085): поля заполнены из исходного документа.
import { useState, type ReactNode } from 'react'
import { get, newOpId, post } from '../lib/api'
import { useUser } from '../lib/auth'
import { accountLabel, isTill, payAccounts } from '../lib/accounts'
import { amountInput, fixedText } from '../lib/docFix'
import { formatSom, parseSom } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import type { CashAccount } from '../lib/types'
import { FIX_SUBMIT, FixBanner, type FixMode } from './FixDraft'
import { Button, ErrorBox, Field, toast } from './ui'

/** Выплата в виде запроса проведения (без op_id). */
export interface PayoutDoc {
  employee_id: string
  amount_tyiyn: number
  source: 'account'
  account_id?: string | null
  advance: boolean
  comment: string
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
 * Выплата сотруднику. `balance` — сколько ему должны: больше — только авансом с отметки владельца.
 * В исправлении остатка нет: аванс уже отмечен в исходной выплате, решает сервер.
 */
export function PayoutForm({
  employeeId,
  balance,
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
  const owner = useUser().role === 'owner'
  const [sum, setSum] = useState(fix ? amountInput(fix.doc.amount_tyiyn) : amountInput(Math.max(0, balance ?? 0)))
  const [comment, setComment] = useState(fix?.doc.comment ?? '')
  const [advance, setAdvance] = useState(false)
  // Выплата с любого счёта — владелец; администратор — из кассы смены (SPEC-29 §13, ADR-076).
  const accounts = useLoad(() => get<CashAccount[]>('/cash/accounts'), [])
  const payFrom = payAccounts(accounts.data ?? [], owner)
  const [fromPick, setFromPick] = useState(fix?.doc.account_id ?? '')
  const fromId = payFrom.some((a) => a.id === fromPick) ? fromPick : (payFrom[0]?.id ?? '')
  const fromAcc = payFrom.find((a) => a.id === fromId)
  // Один op_id на открытую форму: повтор после потерянного ответа не задвоит операцию.
  const [opId] = useState(newOpId)
  const act = useAction()
  const overpay = !fix && balance !== undefined && (parseSom(sum) ?? 0) > balance

  const pay = () =>
    void act.run(async () => {
      const amount = parseSom(sum)
      if (amount === null || amount <= 0) throw new Error('Неверная сумма')
      if (fix) {
        const out = await fix.save({ ...fix.doc, amount_tyiyn: amount, account_id: fromId || fix.doc.account_id, comment })
        if (!out) return
        toast(fixedText(out))
        onDone()
        return
      }
      await post('/payouts', {
        op_id: opId,
        employee_id: employeeId,
        amount_tyiyn: amount,
        source: 'account',
        account_id: fromId || undefined,
        // Аванс сверх заработанного — только с явной отметки владельца (SPEC-07).
        advance: balance !== undefined && amount > balance && advance,
        comment,
      })
      toast(fromAcc && !isTill(fromAcc) ? `Выплачено: ${fromAcc.name}` : 'Выплачено из кассы')
      onDone()
    })

  return (
    <div className="flex flex-col gap-3">
      {fix && <FixBanner fix={fix} />}
      <div className="text-sm text-slate-600">
        {balance !== undefined && `К выплате ${formatSom(balance)}, `}деньги уйдут {fromAcc && !isTill(fromAcc) ? `со счёта «${fromAcc.name}»` : 'из кассы'}
      </div>
      {payFrom.length > 1 ? (
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
      )}
      <Field label="Сумма, с" required>
        <input autoFocus={!fix} inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
      </Field>
      <Field label="Комментарий">
        <input value={comment} onChange={(e) => setComment(e.target.value)} />
      </Field>
      {overpay &&
        balance !== undefined &&
        (owner ? (
          <label className="flex items-center gap-2 rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">
            <input type="checkbox" checked={advance} onChange={(e) => setAdvance(e.target.checked)} />
            Это больше заработанного на {formatSom((parseSom(sum) ?? 0) - balance)}. Выдать как аванс?
          </label>
        ) : (
          <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-800">
            Это больше заработанного: аванс выдаёт только владелец. Уменьшите сумму до {formatSom(Math.max(balance, 0))}.
          </div>
        ))}
      <ErrorBox error={act.error ?? fix?.error} />
      <Footer
        fix={Boolean(fix)}
        label="Выплатить"
        onCancel={onCancel}
        onSubmit={pay}
        disabled={act.busy || Boolean(fix?.busy) || !sum.trim() || (overpay && (!owner || !advance)) || (fix !== undefined && !fix.comment.trim())}
      />
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
