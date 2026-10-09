// Акт сверки за период: печать из карточки клиента или поставщика (SPEC-10).
import { useState } from 'react'
import { get, qs } from '../lib/api'
import { printReconciliation, type DebtDocSettings, type Reconciliation } from '../lib/debtDocs'
import { todayBishkek } from '../lib/format'
import { useAction } from '../lib/hooks'
import { Button, ErrorBox, Field, Modal } from './ui'

export function ReconciliationButton({ partyId }: { partyId: string }) {
  const [open, setOpen] = useState(false)
  const [from, setFrom] = useState('')
  const [to, setTo] = useState(todayBishkek())
  const { busy, error, run } = useAction()

  const print = () =>
    void run(async () => {
      const [act, settings] = await Promise.all([
        get<Reconciliation>(`/parties/${partyId}/reconciliation${qs({ from, to })}`),
        get<DebtDocSettings>('/settings/debt-docs'),
      ])
      printReconciliation(act, settings.seller)
      setOpen(false)
    })

  return (
    <>
      <Button variant="secondary" onClick={() => setOpen(true)}>
        Акт сверки
      </Button>
      {open && (
        <Modal title="Акт сверки взаимных расчётов" onClose={() => setOpen(false)}>
          <div className="flex flex-col gap-3">
            <div className="grid grid-cols-2 gap-3">
              <Field label="С" hint="Пусто — с первой операции">
                <input type="date" value={from} onChange={(e) => setFrom(e.target.value)} />
              </Field>
              <Field label="По">
                <input type="date" value={to} onChange={(e) => setTo(e.target.value)} />
              </Field>
            </div>
            <div className="text-xs text-slate-500">
              В акт попадают все отгрузки, возвраты, оплаты и правки долга за период, входящее и исходящее сальдо. Реквизиты точки — в «Настройки → Документы».
            </div>
            <ErrorBox error={error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setOpen(false)}>
                Отмена
              </Button>
              <Button disabled={busy} onClick={print}>
                Печать
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </>
  )
}
