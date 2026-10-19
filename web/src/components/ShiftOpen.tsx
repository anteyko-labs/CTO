// Окно «Открыть смену» (SPEC-05, SPEC-29 §15–16): кассир, PIN кассира или пароль владельца,
// пересчёт по остатку кассы. Общее для экрана «Смена» и режима кассы.
import { useState } from 'react'
import { newOpId, post } from '../lib/api'
import { useNumField } from '../lib/cashierMode'
import { formatSom, parseSom, somInput } from '../lib/format'
import { missingWithFocus } from '../lib/forms'
import { useAction } from '../lib/hooks'
import type { CashAccount, Employee } from '../lib/types'
import { Button, ErrorBox, Field, Missing, Modal, toast } from './ui'

type Opening = { cashier: string; counted: string; opId: string; pin: string; byOwner: boolean; password: string }

export function ShiftOpenModal({
  owner,
  cashiers,
  till,
  onClose,
  onOpened,
}: {
  owner: boolean
  /** Действующие кассиры. */
  cashiers: Employee[]
  /** Касса смены: её остаток — «Пересчитал» по умолчанию (SPEC-29 §16). */
  till: CashAccount | undefined
  onClose: () => void
  onOpened: () => void
}) {
  const num = useNumField()
  const act = useAction()
  // Пересчёт по умолчанию — остаток кассы: что оставили с прошлой смены и что было после её
  // закрытия; кассир правит, если насчитал другое.
  const [open, setOpen] = useState<Opening>(() => ({
    cashier: cashiers.length === 1 ? cashiers[0].id : '',
    counted: till ? somInput(Math.max(till.balance_tyiyn, 0)) : '',
    opId: newOpId(),
    pin: '',
    byOwner: false,
    password: '',
  }))

  const missing = missingWithFocus(
    [Boolean(open.cashier), 'кассира', '#shift-cashier'],
    [open.byOwner || /^\d{4}$/.test(open.pin), 'PIN кассира — 4 цифры', '#shift-pin'],
    [!open.byOwner || Boolean(open.password), 'ваш пароль', '#shift-owner-password'],
    [!open.counted.trim() || parseSom(open.counted) !== null, 'сумму пересчёта числом', '#shift-counted'],
  )
  const openCashier = cashiers.find((e) => e.id === open.cashier)

  const submit = () =>
    void act.run(async () => {
      const counted = open.counted.trim() ? parseSom(open.counted) : null
      if (open.counted.trim() && counted === null) throw new Error('Неверная сумма')
      await post('/shifts/open', {
        op_id: open.opId,
        cashier_employee_id: open.cashier,
        counted_tyiyn: counted,
        // Кассир подтверждает PIN; владелец может открыть своим паролем (ADR-081).
        ...(open.byOwner ? { owner_password: open.password } : { cashier_pin: open.pin }),
      })
      toast('Смена открыта')
      onOpened()
    })

  return (
    <Modal title="Открыть смену" onClose={onClose}>
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
        {open.byOwner ? (
          <Field label="Ваш пароль" required hint="Владелец открывает смену своим паролем вместо PIN кассира">
            <input
              id="shift-owner-password"
              type="password"
              autoComplete="current-password"
              value={open.password}
              onChange={(e) => setOpen({ ...open, password: e.target.value })}
            />
          </Field>
        ) : (
          <Field
            label="PIN кассира"
            required
            hint={openCashier && openCashier.has_pin === false ? 'У кассира нет PIN — его задаёт владелец в карточке сотрудника' : '4 цифры'}
          >
            <input
              id="shift-pin"
              className="w-32 text-center text-lg tracking-[0.4em]"
              type="password"
              {...num('int')}
              autoComplete="off"
              maxLength={4}
              value={open.pin}
              onChange={(e) => setOpen({ ...open, pin: e.target.value.replace(/\D/g, '').slice(0, 4) })}
            />
          </Field>
        )}
        {owner && (
          <button
            type="button"
            className="-mt-1 self-start text-xs text-sky-700 underline"
            onClick={() => setOpen({ ...open, byOwner: !open.byOwner, pin: '', password: '' })}
          >
            {open.byOwner ? 'Ввести PIN кассира' : 'Открыть своим паролем'}
          </button>
        )}
        <Field
          label="Пересчитал, с"
          hint={
            till
              ? `По остатку кассы: ${formatSom(till.balance_tyiyn)}. Насчитали другое — исправьте; пусто — без пересчёта`
              : 'Если не пересчитывали — оставьте пустым'
          }
        >
          <input id="shift-counted" {...num('decimal')} value={open.counted} onChange={(e) => setOpen({ ...open, counted: e.target.value })} />
        </Field>
        <Missing items={missing} />
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Отмена
          </Button>
          <Button disabled={act.busy || missing.length > 0} onClick={submit}>
            Открыть
          </Button>
        </div>
      </div>
    </Modal>
  )
}
