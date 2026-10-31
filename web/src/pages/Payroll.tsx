import { useState } from 'react'
import { useSearchParams } from 'react-router-dom'
import { PayrollEntries } from '../components/PayrollEntries'
import { ExportButton } from '../components/ExportButton'
import { colsIf, listSheet, totalBy, xl, type ExportCol } from '../lib/exportTable'
import { ExtraForm, PayoutForm, SalaryForm } from '../components/PayrollForms'
import { PayrollMonth } from '../components/PayrollMonth'
import { PayrollSalary } from '../components/PayrollSalary'
import { Button, Card, Empty, ErrorBox, Field, Loading, Modal, Money, PageHeader, RowMenu, Table } from '../components/ui'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatSom, todayBishkek } from '../lib/format'
import { useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import type { PayrollRow } from '../lib/types'

/** Расчёт за день: кто сколько заработал и сколько ему должны (SPEC-07). */
export default function Payroll() {
  const owner = useUser().role === 'owner'
  // Выплаты — право администратора; кассирам выплачивает владелец, сервер отдаёт администратору
  // только мастеров (SPEC-30 §6).
  const can = useCan()
  const mayPay = can('payroll_pay')
  const mayAdvance = can('payroll_advance')
  const [view, setView] = useState<'day' | 'month' | 'salary'>('day')
  // «Открыть в Расчёте» из «Расходов» (задача 003): `?date=` — день выплаты, `?employee=` — чей.
  const [params] = useSearchParams()
  const linkedDate = /^\d{4}-\d{2}-\d{2}$/.test(params.get('date') ?? '') ? (params.get('date') as string) : null
  const linkedEmployee = params.get('employee')
  const [date, setDate] = useState(linkedDate ?? todayBishkek())
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
  // Пришли по ссылке: строка работника подсвечена, владельцу сразу открыты его начисления и
  // выплаты за день — там «Исправить» и «Отменить» (ADR-085). Закрыл — больше не открываются.
  const [linkClosed, setLinkClosed] = useState(false)
  const linkedRow = !linkClosed && owner && linkedEmployee && date === linkedDate ? (list.find((x) => x.employee_id === linkedEmployee) ?? null) : null
  const shownEntries = entries ?? linkedRow
  const toPay = list.reduce((acc, r) => acc + Math.max(0, r.balance_tyiyn), 0)

  return (
    <div>
      <PageHeader title="Расчёт сотрудников" />
      <div className="mb-4 inline-grid grid-cols-3 overflow-hidden rounded-md border border-slate-300 text-sm">
        {(
          [
            ['day', 'За день'],
            ['month', 'За месяц'],
            ['salary', 'Оклад'],
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
      {view === 'salary' ? (
        <PayrollSalary />
      ) : view === 'month' ? (
        <PayrollMonth />
      ) : (
      <>
      <Card className="mb-4 flex flex-wrap items-end gap-4">
        <Field label="День">
          <input type="date" className="w-44" value={date} onChange={(e) => setDate(e.target.value)} />
        </Field>
        {/* Все сотрудники дня; база процента — только владельцу, как на экране (задача 003 Н13, I13). */}
        <ExportButton
          file="Расчёт за день"
          period={formatDate(date)}
          disabled={list.length === 0}
          sheets={() => {
            const cols = payrollDayCols(owner)
            return [
              listSheet('Расчёт сотрудников за день', cols, list, {
                sheet: 'За день',
                filter: [`день ${formatDate(date)}`],
                total: totalBy(cols, { Сотрудник: 'Итого', 'К выплате': xl.money(toPay) }),
              }),
            ]
          }}
        />
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
            <Table head={['Сотрудник', 'Не выплачено ранее', 'Мастеру за услуги', 'Процент', 'Оклад', 'Бонус', 'Удержано', 'Выплачено', 'Аванс', 'К выплате', '']}>
              {list.map((r) => (
                <tr
                  key={r.employee_id}
                  data-testid={r.employee_id === linkedEmployee ? 'payroll-linked-row' : undefined}
                  className={r.employee_id === linkedEmployee && date === linkedDate ? 'bg-sky-50' : undefined}
                >
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
                  {/* Вместо «Прочее» — оклад, бонус и удержания отдельно (ADR-122). */}
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.salary_tyiyn ?? 0} zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.bonus_tyiyn ?? 0} zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.withheld_tyiyn ?? 0} tone="negative" zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.paid_tyiyn - (r.advance_tyiyn ?? 0)} zero="—" />
                  </td>
                  <td className="px-2 py-2 md:text-right">
                    <Money value={r.advance_tyiyn ?? 0} zero="—" />
                  </td>
                  <td className="px-2 py-2 font-semibold md:text-right">
                    <Money value={r.balance_tyiyn} tone="negative" zero="—" />
                  </td>
                  <td className="overflow-visible! px-2 py-2 text-right">
                    <div className="flex items-center justify-end gap-1">
                      {mayPay && (
                        <Button
                          className="px-3 py-1 text-xs"
                          disabled={r.balance_tyiyn <= 0 && !owner && !mayAdvance}
                          onClick={() => setPaying(r)}
                        >
                          Выплатить
                        </Button>
                      )}
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
          {/* Выбранный день — дата выплаты в расчёте; другую дату ставит владелец (задача 003 Н11). */}
          <PayoutForm employeeId={paying.employee_id} date={owner ? date : undefined} onCancel={close} onDone={done} />
        </Modal>
      )}

      {extra && (
        <Modal title={`${extra.kind === 'bonus' ? 'Бонус' : 'Удержание'}: ${extra.row.full_name}`} onClose={close}>
          <ExtraForm employeeId={extra.row.employee_id} kind={extra.kind} date={date} onCancel={close} onDone={done} />
        </Modal>
      )}

      {shownEntries && (
        <PayrollEntries
          employee={shownEntries}
          from={date}
          to={date}
          onClose={(changed) => {
            setEntries(null)
            setLinkClosed(true)
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

/** Столбцы выгрузки «За день» — как в таблице; база процента (валовая) — только владельцу (I13). */
function payrollDayCols(owner: boolean): ExportCol<PayrollRow>[] {
  return [
    { label: 'Сотрудник', value: (r) => r.full_name },
    { label: 'Не выплачено ранее', value: (r) => xl.money(r.opening_tyiyn) },
    { label: 'Мастеру за услуги', value: (r) => xl.money(r.service_fee_tyiyn) },
    { label: 'Процент', value: (r) => xl.money(r.percent_tyiyn) },
    ...colsIf<PayrollRow>(owner, [{ label: 'С валовой', value: (r) => xl.maybeMoney(r.base_tyiyn) }]),
    { label: 'Оклад', value: (r) => xl.money(r.salary_tyiyn ?? 0) },
    { label: 'Бонус', value: (r) => xl.money(r.bonus_tyiyn ?? 0) },
    { label: 'Удержано', value: (r) => xl.money(r.withheld_tyiyn ?? 0) },
    { label: 'Выплачено', value: (r) => xl.money(r.paid_tyiyn - (r.advance_tyiyn ?? 0)) },
    { label: 'Аванс', value: (r) => xl.money(r.advance_tyiyn ?? 0) },
    { label: 'К выплате', value: (r) => xl.money(r.balance_tyiyn) },
  ]
}
