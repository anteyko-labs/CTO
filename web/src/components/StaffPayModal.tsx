// «Выплатить работнику» и «Бонус» из «Расходов» (задача 003 Н12б): выбор работника и те же окна,
// что в «Расчёте» (PayrollForms). Работники — из «Расчёта за месяц»: сервер отдаёт администратору
// только тех, кого он видит в «Расчёте» (ADR-151); кассирам выплачивает владелец (SPEC-30 §6).
import { useState } from 'react'
import { get } from '../lib/api'
import { useUser } from '../lib/auth'
import { todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { ExtraForm, PayoutForm } from './PayrollForms'
import { ErrorBox, Field, Loading, Modal } from './ui'

interface Staff {
  employee_id: string
  full_name: string
  is_cashier: boolean
}

export function StaffPayModal({ what, onClose, onDone }: { what: 'payout' | 'bonus'; onClose: () => void; onDone: () => void }) {
  const owner = useUser().role === 'owner'
  const staff = useLoad(() => get<{ rows: Staff[] }>('/payroll/month'), [])
  const [picked, setPicked] = useState('')
  const people = (staff.data?.rows ?? []).filter((r) => owner || !r.is_cashier)
  const today = todayBishkek()
  const title = what === 'bonus' ? 'Бонус работнику' : 'Выплатить работнику'
  return (
    <Modal title={title} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <ErrorBox error={staff.error} />
        {staff.loading && !staff.data ? (
          <Loading />
        ) : (
          <Field label="Работник" required>
            <select id="staff-pay-employee" value={picked} onChange={(e) => setPicked(e.target.value)}>
              <option value="">— выберите —</option>
              {people.map((p) => (
                <option key={p.employee_id} value={p.employee_id}>
                  {p.full_name}
                </option>
              ))}
            </select>
          </Field>
        )}
        {picked &&
          (what === 'bonus' ? (
            <ExtraForm key={picked} employeeId={picked} kind="bonus" date={today} onCancel={onClose} onDone={onDone} />
          ) : (
            <PayoutForm key={picked} employeeId={picked} date={owner ? today : undefined} onCancel={onClose} onDone={onDone} />
          ))}
      </div>
    </Modal>
  )
}
