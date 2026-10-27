// «Выплата сотруднику» на кассе (SPEC-36 §10–11, ADR-122): выбрать сотрудника — дальше то же окно
// «Выплата», что в «Расчёте». Правила — как на сервере: администратор выплачивает только мастерам
// (не кассирам), кассирам — владелец; аванс можно и тому, у кого сегодня начислений нет. Кассир
// открытой смены сам себе не выплачивает — его в списке нет, ему выплачивают в «Расчёте».
import { useState } from 'react'
import { get } from '../lib/api'
import { formatSom } from '../lib/format'
import { useUser } from '../lib/auth'
import { useLoad } from '../lib/hooks'
import type { Employee, PayrollRow } from '../lib/types'
import { PayoutForm } from './PayrollForms'
import { ErrorBox, Field, Loading, Modal } from './ui'

export function StaffPayout({ shiftCashier, onClose }: { shiftCashier: string | null; onClose: () => void }) {
  const owner = useUser().role === 'owner'
  const staff = useLoad(
    () =>
      Promise.all([get<Employee[]>('/employees'), get<PayrollRow[]>('/payroll/day')]).then(([emps, day]) => {
        const owed = new Map(day.map((r) => [r.employee_id, r.balance_tyiyn]))
        return emps
          .filter((e) => e.active && (e.is_cashier || e.is_master) && (owner || (e.is_master && !e.is_cashier)))
          .map((e) => ({ employee_id: e.id, full_name: e.full_name, balance_tyiyn: owed.get(e.id) ?? 0 }))
      }),
    [owner],
  )
  const [pick, setPick] = useState('')
  const list = (staff.data ?? []).filter((r) => r.employee_id !== shiftCashier)
  const chosen = list.find((r) => r.employee_id === pick)
  return (
    <Modal title={chosen ? `Выплата: ${chosen.full_name}` : 'Выплата сотруднику'} onClose={onClose}>
      <div className="flex flex-col gap-3">
        <ErrorBox error={staff.error} />
        {staff.loading && !staff.data ? (
          <Loading />
        ) : list.length === 0 ? (
          <div className="rounded-md bg-slate-50 px-3 py-2 text-sm text-slate-600">Выплатить некому.</div>
        ) : (
          <Field label="Сотрудник" required>
            <select id="staff-payout-employee" className="min-h-[56px]" value={pick} onChange={(e) => setPick(e.target.value)}>
              <option value="">— выберите —</option>
              {list.map((r) => (
                <option key={r.employee_id} value={r.employee_id}>
                  {r.full_name} · к выдаче {formatSom(Math.max(r.balance_tyiyn, 0))}
                </option>
              ))}
            </select>
          </Field>
        )}
        {chosen && <PayoutForm key={chosen.employee_id} employeeId={chosen.employee_id} onCancel={onClose} onDone={onClose} />}
      </div>
    </Modal>
  )
}
