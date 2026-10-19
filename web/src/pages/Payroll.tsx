import { useState } from 'react'
import { PayrollEntries } from '../components/PayrollEntries'
import { ExtraForm, PayoutForm, SalaryForm } from '../components/PayrollForms'
import { PayrollMonth } from '../components/PayrollMonth'
import { Button, Card, Empty, ErrorBox, Field, Loading, Modal, Money, PageHeader, RowMenu, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatSom, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import type { PayrollRow } from '../lib/types'

/** Расчёт за день: кто сколько заработал и сколько ему должны (SPEC-07). */
export default function Payroll() {
  const owner = useUser().role === 'owner'
  const [view, setView] = useState<'day' | 'month'>('day')
  const [date, setDate] = useState(todayBishkek())
  const [paying, setPaying] = useState<PayrollRow | null>(null)
  const [extra, setExtra] = useState<{ row: PayrollRow; kind: 'bonus' | 'penalty' } | null>(null)
  const [salary, setSalary] = useState<PayrollRow | null>(null)
  const [entries, setEntries] = useState<PayrollRow | null>(null)
  const rows = useLoad(() => get<PayrollRow[]>(`/payroll/day${qs({ date })}`), [date])

  const close = () => {
    setPaying(null)
    setExtra(null)
    setSalary(null)
  }
  const done = () => {
    close()
    rows.reload()
  }

  const list = rows.data ?? []
  const toPay = list.reduce((acc, r) => acc + Math.max(0, r.balance_tyiyn), 0)

  return (
    <div>
      <PageHeader title="Расчёт сотрудников" />
      <div className="mb-4 inline-grid grid-cols-2 overflow-hidden rounded-md border border-slate-300 text-sm">
        {(
          [
            ['day', 'За день'],
            ['month', 'За месяц'],
          ] as const
        ).map(([v, label]) => (
          <button
            key={v}
            type="button"
            className={`min-h-[42px] px-4 py-2 ${view === v ? 'bg-sky-600 text-white' : 'bg-white hover:bg-slate-50'}`}
            onClick={() => setView(v)}
          >
            {label}
          </button>
        ))}
      </div>
      {view === 'month' ? (
        <PayrollMonth />
      ) : (
      <>
      <Card className="mb-4 flex flex-wrap items-end gap-4">
        <Field label="День">
          <input type="date" className="w-44" value={date} onChange={(e) => setDate(e.target.value)} />
        </Field>
        <div className="ml-auto text-right">
          <div className="text-xs text-slate-500">К выплате всего</div>
          <Money value={toPay} className="text-2xl font-bold" />
        </div>
      </Card>

      <Card>
        <ErrorBox error={rows.error} />
        {rows.loading && !rows.data ? (
          <Loading />
        ) : list.length === 0 ? (
          <Empty>За этот день никто ничего не заработал</Empty>
        ) : (
          // Суммы — по правому краю ровными цифрами, ноль — прочерком.
          <div className="[&_th:not(:first-child)]:text-right">
            <Table head={['Сотрудник', 'Не выплачено ранее', 'За замены', 'Процент', 'Прочее', 'Выплачено', 'К выплате', '']}>
              {list.map((r) => (
                <tr key={r.employee_id}>
                  <td className="px-2 py-2 font-medium">{r.full_name}</td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.opening_tyiyn} tone="negative" zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.service_fee_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.percent_tyiyn} zero="—" />
                    {owner && r.base_tyiyn !== null && r.base_tyiyn !== 0 && (
                      <div className="text-xs whitespace-nowrap text-slate-500">с валовой {formatSom(r.base_tyiyn)}</div>
                    )}
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.other_tyiyn} tone="negative" zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.paid_tyiyn} zero="—" />
                  </td>
                  <td className="px-2 py-2 font-semibold md:text-right">
                    <Money value={r.balance_tyiyn} tone="negative" zero="—" />
                  </td>
                  <td className="overflow-visible! px-2 py-2 text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        className="px-3 py-1 text-xs"
                        disabled={r.balance_tyiyn <= 0 && !owner}
                        onClick={() => setPaying(r)}
                      >
                        Выплатить
                      </Button>
                      {owner && (
                        <RowMenu
                          label="Начисления сотруднику"
                          items={[
                            { label: 'Оклад', onClick: () => setSalary(r) },
                            { label: 'Бонус', onClick: () => setExtra({ row: r, kind: 'bonus' }) },
                            { label: 'Удержать', onClick: () => setExtra({ row: r, kind: 'penalty' }), danger: true },
                            { label: 'Начисления и выплаты за день', onClick: () => setEntries(r) },
                          ]}
                        />
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </Table>
          </div>
        )}
      </Card>

      {paying && (
        <Modal title={`Выплата: ${paying.full_name}`} onClose={close}>
          <PayoutForm employeeId={paying.employee_id} balance={paying.balance_tyiyn} onCancel={close} onDone={done} />
        </Modal>
      )}

      {extra && (
        <Modal title={`${extra.kind === 'bonus' ? 'Бонус' : 'Удержание'}: ${extra.row.full_name}`} onClose={close}>
          <ExtraForm employeeId={extra.row.employee_id} kind={extra.kind} date={date} onCancel={close} onDone={done} />
        </Modal>
      )}

      {entries && (
        <PayrollEntries
          employee={entries}
          from={date}
          to={date}
          onClose={(changed) => {
            setEntries(null)
            if (changed) rows.reload()
          }}
        />
      )}

      {salary && (
        <Modal title={`Оклад за месяц: ${salary.full_name}`} onClose={close}>
          <SalaryForm
            employeeId={salary.employee_id}
            month={`${date.slice(0, 7)}-01`}
            hint={`Месяц ${date.slice(0, 7)}. Пустая сумма — возьмём из правила оплаты.`}
            optional
            onCancel={close}
            onDone={done}
          />
        </Modal>
      )}
      </>
      )}
    </div>
  )
}
