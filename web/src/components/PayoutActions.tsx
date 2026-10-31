// «Исправить» и «Отменить» выплату работнику вне «Расчёта» (задача 003): те же форма исправления
// (ADR-085) и сторно с причиной (ADR-064, I4), что в окне «Начисления и выплаты». Только владелец —
// проверяет сервер (I12, I14).
import { useState } from 'react'
import { newOpId, post } from '../lib/api'
import { useAction } from '../lib/hooks'
import { payoutReverseText } from '../lib/payouts'
import { FixDraft } from './FixDraft'
import { PayoutForm, type PayoutDoc } from './PayrollForms'
import { Button, ErrorBox, Field, Modal, toast } from './ui'

export interface PayoutRef {
  id: string
  number: number | null
  employee_id: string
  /** Кому: имя работника. */
  payee: string
  amount_tyiyn: number
  purpose?: string | null
  account_name: string | null
}

/** Окно «Исправить выплату»: форма исправления заполнена исходной выплатой. */
export function PayoutFixModal({ payout, onClose, onDone }: { payout: PayoutRef; onClose: () => void; onDone: () => void }) {
  return (
    <Modal title={`Исправить: выплата № ${payout.number ?? ''} — ${payout.payee}`} onClose={onClose}>
      <FixDraft<PayoutDoc> kind="payout" id={payout.id} onCancel={onClose}>
        {(fix) => <PayoutForm employeeId={payout.employee_id} fix={fix} onCancel={onClose} onDone={onDone} />}
      </FixDraft>
    </Modal>
  )
}

/** Окно «Отменить выплату»: причина обязательна, отмена — записью с минусом. */
export function PayoutReverseModal({ payout, onClose, onDone }: { payout: PayoutRef; onClose: () => void; onDone: () => void }) {
  const [reason, setReason] = useState('')
  const [opId] = useState(newOpId)
  const act = useAction()
  const confirm = () =>
    void act.run(async () => {
      await post(`/payouts/${payout.id}/reverse`, { op_id: opId, comment: reason.trim() })
      toast('Отменено')
      onDone()
    })
  return (
    <Modal title={`Отменить выплату — ${payout.payee}`} onClose={onClose}>
      <div className="flex flex-col gap-3 text-sm">
        <div>{payoutReverseText(payout)}</div>
        <Field label="Причина" required>
          <input id="payout-reverse-reason" autoFocus value={reason} onChange={(e) => setReason(e.target.value)} />
        </Field>
        <ErrorBox error={act.error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose}>
            Не отменять
          </Button>
          <Button variant="danger" disabled={act.busy || !reason.trim()} onClick={confirm}>
            Отменить выплату
          </Button>
        </div>
      </div>
    </Modal>
  )
}
