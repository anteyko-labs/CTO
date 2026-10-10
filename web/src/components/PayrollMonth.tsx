// Расчёт за месяц: остаток на начало, начисления по видам, выплаты, остаток на конец (SPEC-07).
import { useState } from 'react'
import { get, newOpId, post, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, parseSom, somInput, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { Badge, Button, Card, Empty, ErrorBox, Field, Loading, Modal, Money, Table, toast } from './ui'

interface MonthRow {
  employee_id: string
  full_name: string
  is_cashier: boolean
  is_master: boolean
  opening_tyiyn: number
  salary_tyiyn: number
  percent_tyiyn: number
  service_fee_tyiyn: number
  shift_fee_tyiyn: number
  bonus_tyiyn: number
  penalty_tyiyn: number
  shortage_tyiyn: number
  accrued_tyiyn: number
  paid_tyiyn: number
  closing_tyiyn: number
  salary_done: boolean
  salary_rule_tyiyn: number | null
}

interface MonthOut {
  from: string
  to: string
  rows: MonthRow[]
}

const MONTHS = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь']

/** Первый день месяца со сдвигом на `delta` месяцев, без часового пояса устройства. */
function shiftMonth(iso: string, delta: number): string {
  const [y, m] = iso.split('-').map(Number)
  const t = y * 12 + (m - 1) + delta
  return `${Math.trunc(t / 12)}-${String((t % 12) + 1).padStart(2, '0')}-01`
}

/** Сумма в ячейке: ровные цифры, ноль — прочерком, минус — красным. */
const cell = (v: number) => <Money value={v} tone="negative" zero="—" />

export function PayrollMonth() {
  const owner = useUser().role === 'owner'
  const [month, setMonth] = useState(shiftMonth(todayBishkek(), 0))
  const data = useLoad(() => get<MonthOut>(`/payroll/month${qs({ month })}`), [month])
  const [salary, setSalary] = useState<{ row: MonthRow; sum: string; opId: string } | null>(null)
  const act = useAction()
  const [y, m] = month.split('-').map(Number)
  const current = shiftMonth(todayBishkek(), 0)
  const rows = data.data?.rows ?? []
  const total = (key: keyof MonthRow) => rows.reduce((acc, r) => acc + (r[key] as number), 0)

  const accrue = () =>
    void act.run(async () => {
      if (!salary) return
      const sum = parseSom(salary.sum)
      if (sum === null || sum <= 0) throw new Error('Неверная сумма')
      await post('/payroll/salary', { op_id: salary.opId, employee_id: salary.row.employee_id, month, amount_tyiyn: sum })
      setSalary(null)
      data.reload()
      toast('Оклад начислен')
    })

  return (
    <div className="flex flex-col gap-4">
      <Card className="flex flex-wrap items-center gap-3">
        <Button variant="secondary" onClick={() => setMonth(shiftMonth(month, -1))} aria-label="Прошлый месяц">
          ←
        </Button>
        <div className="min-w-40 text-center font-semibold">
          {MONTHS[m - 1]} {y}
        </div>
        <Button variant="secondary" disabled={month >= current} onClick={() => setMonth(shiftMonth(month, 1))} aria-label="Следующий месяц">
          →
        </Button>
        <div className="text-xs text-slate-500">
          «На конец» — сколько ещё должны сотруднику; с минусом — выдано больше заработанного.
        </div>
      </Card>
      <ErrorBox error={data.error ?? act.error} />
      {data.loading && !data.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>В этом месяце начислений не было</Empty>
      ) : (
        <Card className="p-0 [&_th:not(:first-child)]:text-right">
          <Table head={['Сотрудник', 'На начало', 'Оклад', '% с прибыли', 'Мастеру', 'За смену', 'Бонус', 'Удержано', 'Начислено', 'Выплачено', 'На конец', '']}>
            {rows.map((r) => (
              <tr key={r.employee_id}>
                <td className="min-w-36 px-2 py-2 font-medium">
                  {r.full_name}
                  <div className="text-xs font-normal text-slate-500">
                    {[r.is_cashier && 'кассир', r.is_master && 'мастер'].filter(Boolean).join(', ')}
                  </div>
                </td>
                <td className="px-2 py-2 md:text-right">{cell(r.opening_tyiyn)}</td>
                <td className="px-2 py-2 whitespace-nowrap md:text-right">
                  {r.salary_done ? cell(r.salary_tyiyn) : r.salary_rule_tyiyn ? <Badge tone="amber">не начислен</Badge> : '—'}
                </td>
                <td className="px-2 py-2 md:text-right">{cell(r.percent_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.service_fee_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.shift_fee_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.bonus_tyiyn)}</td>
                <td className="px-2 py-2 text-rose-700 md:text-right">{cell(r.penalty_tyiyn + r.shortage_tyiyn)}</td>
                <td className="px-2 py-2 font-medium md:text-right">{cell(r.accrued_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.paid_tyiyn)}</td>
                <td className="px-2 py-2 font-semibold md:text-right">
                  <Money value={r.closing_tyiyn} tone="negative" />
                </td>
                <td className="px-2 py-2 text-right">
                  {owner && !r.salary_done && month <= current && (
                    <Button
                      variant="secondary"
                      className="px-2 py-1 text-xs"
                      onClick={() => setSalary({ row: r, sum: r.salary_rule_tyiyn ? somInput(r.salary_rule_tyiyn) : '', opId: newOpId() })}
                    >
                      Оклад
                    </Button>
                  )}
                </td>
              </tr>
            ))}
            <tr className="bg-slate-50 font-semibold">
              <td className="px-2 py-2">Итого</td>
              <td className="px-2 py-2 md:text-right">{cell(total('opening_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('salary_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('percent_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('service_fee_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('shift_fee_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('bonus_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('penalty_tyiyn') + total('shortage_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('accrued_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">{cell(total('paid_tyiyn'))}</td>
              <td className="px-2 py-2 md:text-right">
                <Money value={total('closing_tyiyn')} tone="negative" />
              </td>
              <td />
            </tr>
          </Table>
        </Card>
      )}

      {salary && (
        <Modal title={`Оклад за ${MONTHS[m - 1]}: ${salary.row.full_name}`} onClose={() => setSalary(null)}>
          <div className="flex flex-col gap-3">
            <div className="text-sm text-slate-600">
              По правилу {salary.row.salary_rule_tyiyn ? formatSom(salary.row.salary_rule_tyiyn) : 'оклада нет'}. За неполный месяц укажите сумму, которую решили платить.
            </div>
            <Field label="Сумма, с" required>
              <input autoFocus inputMode="decimal" value={salary.sum} onChange={(e) => setSalary({ ...salary, sum: e.target.value })} />
            </Field>
            <ErrorBox error={act.error} />
            <div className="flex justify-end gap-2">
              <Button variant="secondary" onClick={() => setSalary(null)}>
                Отмена
              </Button>
              <Button disabled={act.busy || !salary.sum.trim()} onClick={accrue}>
                Начислить
              </Button>
            </div>
          </div>
        </Modal>
      )}
    </div>
  )
}
