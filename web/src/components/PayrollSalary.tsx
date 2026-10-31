// Вкладка «Оклад» (задача 003 Н11): по каждому работнику на окладе — его период (с дня расчёта по день
// до него), ставка, начислено, авансы и оклад, выданные в периоде, долг прошлых периодов и сколько к
// выплате в день расчёта. Всегда видно — и до «Начислить оклад». Администратор видит только тех, кому
// платит (мастеров не кассиров); права проверяет сервер. Владелец с несколькими филиалами смотрит и
// «Все филиалы» (SPEC-33 §3) — как «За месяц», только просмотр: начислять и платить — в своём филиале.
import { useState } from 'react'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { branchByRow, branchParam, useAllBranches, type AllBranches } from '../lib/branchReport'
import { BranchScope } from './BranchScope'
import { formatDate, somInput, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { addMonths, isCalendar, periodLabel, shortDate } from '../lib/payrollPeriod'
import type { SalaryNow } from '../lib/types'
import { PayoutForm, SalaryForm } from './PayrollForms'
import { ExportButton } from './ExportButton'
import { colsIf, listSheet, xl, type ExportCol } from '../lib/exportTable'
import { Badge, Button, Card, Empty, ErrorBox, Loading, Modal, Money, Table } from './ui'

interface SalaryOut {
  date: string
  today: string
  rows: SalaryNow[]
}

const cell = (v: number) => <Money value={v} tone="negative" zero="—" />

export function PayrollSalary() {
  const user = useUser()
  const [all, setAll] = useAllBranches(user.role === 'owner' && Boolean(user.several_branches))
  const owner = user.role === 'owner' && !all
  const can = useCan()
  const mayPay = !all && can('payroll_pay')
  const [date, setDate] = useState(todayBishkek())
  const data = useLoad(() => get<AllBranches<SalaryOut>>(`/payroll/salary${qs({ date, ...branchParam(all) })}`), [date, all])
  const branchOf = all ? branchByRow(data.data?.branches, (r) => r.rows, (x) => x.employee_id) : null
  // Администратор без смены не платит: только из кассы смены (задача 003 Н11).
  const isOwner = user.role === 'owner'
  const shift = useLoad(() => (isOwner ? Promise.resolve({ id: 'owner' }) : get<{ id: string } | null>('/shifts/current')), [isOwner])
  const noShift = !isOwner && shift.data === null
  const [salary, setSalary] = useState<SalaryNow | null>(null)
  const [paying, setPaying] = useState<SalaryNow | null>(null)
  const today = data.data?.today ?? todayBishkek()
  const rows = data.data?.rows ?? []
  const close = () => {
    setSalary(null)
    setPaying(null)
  }
  const done = () => {
    close()
    data.reload()
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <BranchScope user={user} all={all} onChange={setAll} />
      </div>
      <Card className="flex flex-wrap items-center gap-3">
        <Button variant="secondary" onClick={() => setDate(addMonths(date, -1))} aria-label="Прошлый период">
          ←
        </Button>
        <div className="min-w-40 text-center font-semibold">{date === today ? 'Сейчас' : `на ${formatDate(date)}`}</div>
        <Button variant="secondary" disabled={date >= today} onClick={() => setDate(addMonths(date, 1) > today ? today : addMonths(date, 1))} aria-label="Следующий период">
          →
        </Button>
        {/* Все работники на окладе (задача 003 Н13). */}
        <ExportButton
          file="Оклад"
          period={formatDate(date)}
          disabled={rows.length === 0}
          sheets={() => [
            listSheet(
              `Оклад на ${formatDate(date)}`,
              [...colsIf<SalaryNow>(Boolean(branchOf), [{ label: 'Филиал', value: (r) => branchOf?.get(r.employee_id) ?? '' }]), ...SALARY_COLS],
              rows,
              { sheet: 'Оклад', filter: [all && 'все филиалы'] },
            ),
          ]}
        />
        <div className="text-xs text-slate-500">
          У каждого — свой период: с дня расчёта по день до него. «К выплате» — начислено (пока не начислено — ставка) минус авансы и выданный
          оклад плюс долг прошлых периодов.
        </div>
      </Card>
      <ErrorBox error={data.error} />
      {noShift && mayPay && (
        <div className="rounded-md bg-amber-50 px-3 py-2 text-sm text-amber-900">Откройте смену — администратор платит только из кассы смены.</div>
      )}
      {data.loading && !data.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>Работников на окладе нет — оклад назначается в карточке сотрудника, раздел «Оплата»</Empty>
      ) : (
        <Card className="p-0 [&_th:not(:first-child)]:text-right" data-testid="payroll-salary">
          <Table head={['Сотрудник', 'Период', 'Ставка', 'Начислено', 'Авансом', 'Выдано окладом', 'С прошлых', 'К выплате', '']}>
            {rows.map((r) => (
              <tr key={r.employee_id}>
                <td className="min-w-36 px-2 py-2 font-medium">
                  {r.full_name}
                  {branchOf?.get(r.employee_id) && <div className="text-xs font-normal text-slate-500">{branchOf.get(r.employee_id)}</div>}
                </td>
                <td className="px-2 py-2 whitespace-nowrap md:text-right">
                  {periodLabel(r.period_from, r.period_to)}
                  {!isCalendar(r.period_from, r.period_to) && <div className="text-xs text-slate-500">день расчёта {r.salary_day}-е</div>}
                </td>
                <td className="px-2 py-2 md:text-right">{r.rule_tyiyn !== null ? cell(r.rule_tyiyn) : '—'}</td>
                <td className="px-2 py-2 whitespace-nowrap md:text-right">
                  {r.accrued_tyiyn !== null ? cell(r.accrued_tyiyn) : <Badge tone="amber">не начислен</Badge>}
                </td>
                <td className="px-2 py-2 md:text-right">{cell(r.advance_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.paid_salary_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.carry_tyiyn)}</td>
                <td className="px-2 py-2 font-semibold whitespace-nowrap md:text-right">
                  <Money value={r.expected_tyiyn} tone="negative" />
                  <div className="text-xs font-normal text-slate-500">{shortDate(r.pay_date)}</div>
                  {r.to_date_tyiyn !== undefined && (
                    <div className="text-xs font-normal text-slate-500">
                      на сегодня ≈ <Money value={r.to_date_tyiyn} />
                    </div>
                  )}
                </td>
                <td className="overflow-visible! px-2 py-2 text-right whitespace-nowrap">
                  <div className="flex items-center justify-end gap-1">
                    {owner && r.accrued_tyiyn === null && r.period_from <= today && (
                      <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setSalary(r)}>
                        Начислить оклад
                      </Button>
                    )}
                    {mayPay && (
                      <Button
                        className="px-2 py-1 text-xs"
                        disabled={noShift}
                        title={noShift ? 'Откройте смену — администратор платит только из кассы смены' : undefined}
                        onClick={() => setPaying(r)}
                      >
                        Выплатить
                      </Button>
                    )}
                  </div>
                </td>
              </tr>
            ))}
          </Table>
        </Card>
      )}

      {salary && (
        <Modal title={`Оклад за ${periodLabel(salary.period_from, salary.period_to)}: ${salary.full_name}`} onClose={close}>
          <SalaryForm
            employeeId={salary.employee_id}
            period={{ from: salary.period_from, to: salary.period_to }}
            hint={`По правилу ${salary.rule_tyiyn !== null ? somInput(salary.rule_tyiyn) + ' с' : 'оклада нет'}. За неполный период укажите сумму, которую решили платить.`}
            initialSum={salary.rule_tyiyn !== null ? somInput(salary.rule_tyiyn) : ''}
            onCancel={close}
            onDone={done}
          />
        </Modal>
      )}

      {paying && (
        <Modal title={`Выплата: ${paying.full_name}`} onClose={close}>
          <PayoutForm employeeId={paying.employee_id} date={date < today ? date : undefined} onCancel={close} onDone={done} />
        </Modal>
      )}
    </div>
  )
}

/** Столбцы выгрузки вкладки «Оклад» — как в таблице, без кнопок. */
const SALARY_COLS: ExportCol<SalaryNow>[] = [
  { label: 'Сотрудник', value: (r) => r.full_name },
  { label: 'С', value: (r) => xl.date(r.period_from) },
  { label: 'По', value: (r) => xl.date(r.period_to) },
  { label: 'День расчёта', value: (r) => (r.salary_day ? xl.int(r.salary_day) : null) },
  { label: 'Ставка', value: (r) => xl.maybeMoney(r.rule_tyiyn) },
  { label: 'Начислено', value: (r) => (r.accrued_tyiyn !== null ? xl.money(r.accrued_tyiyn) : 'не начислен') },
  { label: 'Авансом', value: (r) => xl.money(r.advance_tyiyn) },
  { label: 'Выдано окладом', value: (r) => xl.money(r.paid_salary_tyiyn) },
  { label: 'С прошлых', value: (r) => xl.money(r.carry_tyiyn) },
  { label: 'К выплате', value: (r) => xl.money(r.expected_tyiyn) },
  { label: 'Дата выплаты', value: (r) => xl.date(r.pay_date) },
  { label: 'На сегодня', value: (r) => xl.maybeMoney(r.to_date_tyiyn) },
]
