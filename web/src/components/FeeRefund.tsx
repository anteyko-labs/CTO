// Возврат ошибочно списанной комиссии банка за день (задача 029, ADR-189): только владелец,
// чеки не меняются, прибыль сходится. Границы проверяет сервер.
import { useState } from 'react'
import { newOpId, post } from '../lib/api'
import { formatSom, parseSom, todayBishkek } from '../lib/format'
import { useAction } from '../lib/hooks'
import type { CashAccount } from '../lib/types'
import { Button, ErrorBox, Field, Modal, toast } from './ui'

export function FeeRefund({ account, onDone }: { account: CashAccount; onDone: () => void }) {
  const [open, setOpen] = useState(false)
  const [day, setDay] = useState(todayBishkek())
  const [sum, setSum] = useState('')
  const [opId, setOpId] = useState(newOpId)
  const act = useAction()
  const value = parseSom(sum)
  const save = () =>
    void act.run(async () => {
      if (value === null || value <= 0) return
      await post(`/cash/accounts/${account.id}/fee-refund`, { op_id: opId, day, amount_tyiyn: value })
      toast(`Комиссия возвращена: ${formatSom(value)}`)
      setOpen(false)
      setSum('')
      setOpId(newOpId())
      onDone()
    })
  return (
    <>
      <button type="button" className="self-start text-sm text-sky-700 hover:underline" data-testid="fee-refund-open" onClick={() => setOpen(true)}>
        Вернуть ошибочную комиссию банка
      </button>
      {open && (
        <Modal title={`Вернуть комиссию · ${account.name}`} onClose={() => setOpen(false)}>
          <div className="flex flex-col gap-3" data-testid="fee-refund">
            <Field label="За какой день" required>
              <input id="fee-refund-day" type="date" value={day} onChange={(e) => setDay(e.target.value)} />
            </Field>
            <Field label="Сумма, с" required>
              <input id="fee-refund-sum" inputMode="decimal" value={sum} onChange={(e) => setSum(e.target.value)} />
            </Field>
            <ErrorBox error={act.error} />
            <Button disabled={act.busy || value === null || value <= 0} onClick={save}>
              Вернуть
            </Button>
          </div>
        </Modal>
      )}
    </>
  )
}
