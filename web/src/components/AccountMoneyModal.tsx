// «Внести», «Изъять», «Перевести» у счёта на экране «Счета» (SPEC-29 §11): те же операции, что на
// экране «Смена» (SPEC-05) — внесение и изъятие `/cash/movements`, перевод `/cash/transfers`.
// Права и минус проверяет сервер (ADR-076/077): у владельца — любой действующий счёт.
import { useState } from 'react'
import { accountLabel } from '../lib/accounts'
import { newOpId, post } from '../lib/api'
import { formatSom, parseSom } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction } from '../lib/hooks'
import type { CashAccount } from '../lib/types'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

export type AccountMoneyKind = 'cash_in' | 'cash_out' | 'transfer'

const TITLES: Record<AccountMoneyKind, string> = {
  cash_in: 'Внести',
  cash_out: 'Изъять',
  transfer: 'Перевести',
}

export function AccountMoneyModal({
  kind,
  account: fixed,
  accounts,
  choices,
  onClose,
  onDone,
}: {
  kind: AccountMoneyKind
  /** Счёт из строки «Обзора счетов»; без него счёт выбирают в окне из `choices`. */
  account?: CashAccount
  /** Все счета филиала: куда перевести — любой действующий, кроме этого. */
  accounts: CashAccount[]
  /** С какого счёта можно (SPEC-29 §25): владельцу — любой действующий, администратору — касса смены. */
  choices?: CashAccount[]
  onClose: () => void
  onDone: () => void
}) {
  const [picked, setPicked] = useState(fixed?.id ?? choices?.[0]?.id ?? '')
  const account = fixed ?? choices?.find((a) => a.id === picked)
  const targets = accounts.filter((a) => a.id !== account?.id && a.active !== false)
  const [sum, setSum] = useState('')
  const [comment, setComment] = useState('')
  const [toPick, setTo] = useState('')
  const to = targets.some((a) => a.id === toPick) ? toPick : (targets[0]?.id ?? '')
  // Один op_id на открытое окно: повтор после потерянного ответа не задвоит операцию.
  const [opId] = useState(newOpId)
  const act = useAction()
  const value = parseSom(sum)
  const missing = missingWithFocus(
    [Boolean(account), 'счёт', '#account-money-from'],
    [value !== null && value > 0, 'сумму', '#account-money-sum'],
    [kind !== 'transfer' || Boolean(to), 'куда перевести', '#account-money-to'],
    [kind === 'transfer' || Boolean(comment.trim()), 'за что', '#account-money-comment'],
  )

  const save = () =>
    void act.run(async () => {
      if (value === null || !account) return
      if (kind === 'transfer') {
        await post('/cash/transfers', { op_id: opId, from_account_id: account.id, to_account_id: to, amount_tyiyn: value, comment })
      } else {
        await post('/cash/movements', { op_id: opId, account_id: account.id, kind, amount_tyiyn: value, comment })
      }
      const target = targets.find((a) => a.id === to)
      toast(
        kind === 'transfer'
          ? `Переведено ${formatSom(value)}: ${account.name} → ${target?.name ?? ''}`
          : `${kind === 'cash_in' ? 'Внесено' : 'Изъято'} ${formatSom(value)}: ${account.name}`,
      )
      onDone()
    })

  return (
    <Modal title={fixed ? `${TITLES[kind]} · ${fixed.name}` : TITLES[kind]} onClose={onClose}>
      <div className="flex flex-col gap-3" data-testid="account-money">
        {!fixed && (
          <Field label={kind === 'cash_in' ? 'На какой счёт' : 'С какого счёта'} required>
            <select id="account-money-from" value={picked} onChange={(e) => setPicked(e.target.value)}>
              {(choices ?? []).length === 0 && <option value="">— нет доступных счетов —</option>}
              {(choices ?? []).map((a) => (
                <option key={a.id} value={a.id}>
                  {accountLabel(a)}
                </option>
              ))}
            </select>
          </Field>
        )}
        {account && <div className="text-sm text-slate-600">Сейчас на счёте: {formatSom(account.balance_tyiyn)}</div>}
        {kind === 'transfer' && (
          <Field label="Куда" required>
            <select id="account-money-to" value={to} onChange={(e) => setTo(e.target.value)}>
              {targets.map((a) => (
                <option key={a.id} value={a.id}>
                  {accountLabel(a)}
                </option>
              ))}
            </select>
          </Field>
        )}
        <Field label="Сумма, с" required>
          <input id="account-money-sum" autoFocus inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
        </Field>
        <Field label="Комментарий" required={kind !== 'transfer'}>
          <input id="account-money-comment" value={comment} onChange={(e) => setComment(e.target.value)} />
        </Field>
        <Missing items={missing} />
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || missing.length > 0} onClick={save}>
            {TITLES[kind]}
          </Button>
        </div>
      </div>
    </Modal>
  )
}
