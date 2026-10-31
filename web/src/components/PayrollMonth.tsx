// Расчёт за месяц: остаток на начало, начисления по видам, выплаты, остаток на конец (SPEC-07).
// У строки — те же действия, что в «За день» (задача 002, П13): выплата, аванс, бонус, удержание,
// исправление последней выплаты; права те же, проверяет сервер.
import { useState } from 'react'
import { get, qs } from '../lib/api'
import { useUser } from '../lib/auth'
import { formatDate, formatSom, somInput, todayBishkek } from '../lib/format'
import { useAction, useLoad } from '../lib/hooks'
import { useCan } from '../lib/permissions'
import { accrualDate, lastPayout, type MonthPayout } from '../lib/payrollMonth'
import { isCalendar, payoutDate, periodLabel } from '../lib/payrollPeriod'
import { FixDraft } from './FixDraft'
import { ExportButton } from './ExportButton'
import { listSheet, xl, type ExportCol } from '../lib/exportTable'
import { PayrollEntries } from './PayrollEntries'
import { ExtraForm, PayoutForm, SalaryForm, type PayoutDoc } from './PayrollForms'
import { Badge, Button, Card, Empty, ErrorBox, Loading, Modal, Money, RowMenu, Table } from './ui'
import { BranchScope } from './BranchScope'
import { branchParam, useAllBranches } from '../lib/branchReport'

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
  /** Из выданного — авансом (ADR-122). */
  advance_tyiyn?: number
  closing_tyiyn: number
  salary_done: boolean
  salary_rule_tyiyn: number | null
  /** День расчёта оклада и период, который заканчивается в этом месяце (задача 003 Н11). */
  salary_day?: number
  salary_from?: string
  salary_to?: string
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
  const user = useUser()
  // «Все филиалы» (SPEC-33 §3) — только просмотр: оклад и подробности — в своём филиале.
  const [all, setAll] = useAllBranches(user.role === 'owner' && Boolean(user.several_branches))
  const owner = user.role === 'owner' && !all
  const [month, setMonth] = useState(shiftMonth(todayBishkek(), 0))
  const data = useLoad(() => get<MonthOut>(`/payroll/month${qs({ month, ...branchParam(all) })}`), [month, all])
  const [salary, setSalary] = useState<MonthRow | null>(null)
  const [entries, setEntries] = useState<MonthRow | null>(null)
  // Действия у строки (задача 002, П13).
  const can = useCan()
  const acts = !all
  const mayPay = acts && can('payroll_pay')
  const mayAdvance = mayPay && can('payroll_advance')
  const [paying, setPaying] = useState<{ row: MonthRow; advance: boolean } | null>(null)
  const [extra, setExtra] = useState<{ row: MonthRow; kind: 'bonus' | 'penalty' } | null>(null)
  const [fixing, setFixing] = useState<{ row: MonthRow; payout: MonthPayout } | null>(null)
  const act = useAction()
  const [y, m] = month.split('-').map(Number)
  const today = todayBishkek()
  const current = shiftMonth(today, 0)
  // Дата выплаты в расчёте (задача 003 Н11): владелец платит и в прошлом месяце — выплата ложится
  // его последним днём (`payoutDate`), деньги уходят сегодня. Администратор ставит только сегодня,
  // поэтому у него выплата и аванс — в текущем месяце. Бонус и удержание прошлого месяца ложатся
  // его последним днём (`accrualDate`).
  const isCurrent = month === current
  const payable = isCurrent || owner
  const payDate = payoutDate(month, today)
  const extraDate = accrualDate(month, today)
  // Администратор без смены не платит: только из кассы смены.
  const shift = useLoad(() => (owner ? Promise.resolve({ id: 'owner' }) : get<{ id: string } | null>('/shifts/current')), [owner])
  const noShift = !owner && shift.data === null
  const closeAct = () => {
    setPaying(null)
    setExtra(null)
    setFixing(null)
  }
  const doneAct = () => {
    closeAct()
    data.reload()
  }
  /** «Исправить последнюю выплату»: последняя действующая выплата месяца — в форму исправления. */
  const fixLast = (r: MonthRow) =>
    void act.run(async () => {
      if (!data.data) return
      const out = await get<{ payouts: (MonthPayout & { amount_tyiyn: number })[] }>(
        `/payroll/entries${qs({ employee_id: r.employee_id, from: data.data.from, to: data.data.to })}`,
      )
      const p = lastPayout(out.payouts)
      if (!p) throw new Error(`${r.full_name}: в этом месяце нет выплат, которые можно исправить`)
      setFixing({ row: r, payout: p })
    })
  const rows = data.data?.rows ?? []
  const total = (key: keyof MonthRow) => rows.reduce((acc, r) => acc + (r[key] as number), 0)
  const advances = rows.reduce((acc, r) => acc + (r.advance_tyiyn ?? 0), 0)

  return (
    <div className="flex flex-col gap-4">
      <div>
        <BranchScope user={user} all={all} onChange={setAll} />
      </div>
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
        {/* Все сотрудники месяца с итогом (задача 003 Н13). */}
        <ExportButton
          file={`Расчёт ${MONTHS[m - 1]} ${y}`}
          disabled={rows.length === 0}
          sheets={() => [
            listSheet(`Расчёт сотрудников: ${MONTHS[m - 1]} ${y}`, MONTH_COLS, rows, {
              sheet: 'За месяц',
              filter: [all && 'все филиалы'],
              total: [
                'Итого',
                null,
                xl.money(total('opening_tyiyn')),
                xl.money(total('salary_tyiyn')),
                null,
                xl.money(total('percent_tyiyn')),
                xl.money(total('service_fee_tyiyn')),
                xl.money(total('shift_fee_tyiyn')),
                xl.money(total('bonus_tyiyn')),
                xl.money(total('penalty_tyiyn') + total('shortage_tyiyn')),
                xl.money(total('accrued_tyiyn')),
                xl.money(total('paid_tyiyn') - advances),
                xl.money(advances),
                xl.money(total('closing_tyiyn')),
              ],
            }),
          ]}
        />
        <div className="text-xs text-slate-500">
          «На конец» — сколько ещё должны сотруднику; с минусом — выдано больше заработанного.
          {/* Кассиров администратор видит с правом «Заработок кассиров» (задача 003 Н8, ADR-122). */}
          {user.role !== 'owner' && !can('payroll_view_all') && ' Кассиров видит владелец.'}
        </div>
      </Card>
      <ErrorBox error={data.error ?? act.error} />
      {data.loading && !data.data ? (
        <Loading />
      ) : rows.length === 0 ? (
        <Empty>В этом месяце начислений не было</Empty>
      ) : (
        <Card className="p-0 [&_th:not(:first-child)]:text-right">
          <Table head={['Сотрудник', 'На начало', 'Оклад', '% с прибыли', 'Мастеру', 'За смену', 'Бонус', 'Удержано', 'Начислено', 'Выплачено', 'Аванс', 'На конец', '']}>
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
                  {r.salary_done ? (
                    cell(r.salary_tyiyn)
                  ) : r.salary_rule_tyiyn ? (
                    <span className="inline-flex items-center gap-1">
                      <Money value={r.salary_rule_tyiyn} /> <Badge tone="amber">не начислен</Badge>
                    </span>
                  ) : (
                    '—'
                  )}
                  {r.salary_from && r.salary_to && !isCalendar(r.salary_from, r.salary_to) && (r.salary_done || r.salary_rule_tyiyn) && (
                    <div className="text-xs text-slate-500">за {periodLabel(r.salary_from, r.salary_to)}</div>
                  )}
                </td>
                <td className="px-2 py-2 md:text-right">{cell(r.percent_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.service_fee_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.shift_fee_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.bonus_tyiyn)}</td>
                <td className="px-2 py-2 text-rose-700 md:text-right">{cell(r.penalty_tyiyn + r.shortage_tyiyn)}</td>
                <td className="px-2 py-2 font-medium md:text-right">{cell(r.accrued_tyiyn)}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.paid_tyiyn - (r.advance_tyiyn ?? 0))}</td>
                <td className="px-2 py-2 md:text-right">{cell(r.advance_tyiyn ?? 0)}</td>
                <td className="px-2 py-2 font-semibold md:text-right">
                  <Money value={r.closing_tyiyn} tone="negative" />
                </td>
                <td className="overflow-visible! px-2 py-2 text-right whitespace-nowrap">
                  <div className="flex items-center justify-end gap-1">
                    {mayPay && payable && (owner || !r.is_cashier) && (
                      <Button
                        className="px-2 py-1 text-xs"
                        disabled={(r.closing_tyiyn <= 0 && !owner && !mayAdvance) || noShift}
                        title={noShift ? 'Откройте смену — администратор платит только из кассы смены' : undefined}
                        onClick={() => setPaying({ row: r, advance: false })}
                      >
                        Выплатить
                      </Button>
                    )}
                    {mayAdvance && payable && (owner || !r.is_cashier) && (
                      <Button
                        variant="secondary"
                        className="px-2 py-1 text-xs"
                        disabled={noShift}
                        title={noShift ? 'Откройте смену — администратор платит только из кассы смены' : undefined}
                        onClick={() => setPaying({ row: r, advance: true })}
                      >
                        Аванс
                      </Button>
                    )}
                    {owner && (
                      <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setEntries(r)}>
                        Подробно
                      </Button>
                    )}
                    {owner && !r.salary_done && month <= current && (
                      <Button variant="secondary" className="px-2 py-1 text-xs" onClick={() => setSalary(r)}>
                        Оклад
                      </Button>
                    )}
                    {owner && (
                      <RowMenu
                        label={`Начисления и выплаты: ${r.full_name}`}
                        items={[
                          { label: 'Бонус', onClick: () => setExtra({ row: r, kind: 'bonus' }) },
                          { label: 'Удержать', onClick: () => setExtra({ row: r, kind: 'penalty' }), danger: true },
                          { label: 'Исправить последнюю выплату', onClick: () => fixLast(r) },
                        ]}
                      />
                    )}
                  </div>
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
              <td className="px-2 py-2 md:text-right">{cell(total('paid_tyiyn') - advances)}</td>
              <td className="px-2 py-2 md:text-right">{cell(advances)}</td>
              <td className="px-2 py-2 md:text-right">
                <Money value={total('closing_tyiyn')} tone="negative" />
              </td>
              <td />
            </tr>
          </Table>
        </Card>
      )}

      {entries && data.data && (
        <PayrollEntries
          employee={entries}
          from={data.data.from}
          to={data.data.to}
          onClose={(changed) => {
            setEntries(null)
            if (changed) data.reload()
          }}
        />
      )}

      {paying && (
        <Modal title={`${paying.advance ? 'Аванс' : 'Выплата'}: ${paying.row.full_name}`} onClose={closeAct}>
          <PayoutForm employeeId={paying.row.employee_id} advance={paying.advance} date={owner ? payDate : undefined} onCancel={closeAct} onDone={doneAct} />
        </Modal>
      )}

      {extra && (
        <Modal title={`${extra.kind === 'bonus' ? 'Бонус' : 'Удержание'} за ${formatDate(extraDate)}: ${extra.row.full_name}`} onClose={closeAct}>
          <ExtraForm employeeId={extra.row.employee_id} kind={extra.kind} date={extraDate} onCancel={closeAct} onDone={doneAct} />
        </Modal>
      )}

      {fixing && (
        // Проведённая выплата не правится: сторно и новая выплата одной операцией (I4, ADR-085).
        <Modal title={`Исправить выплату № ${fixing.payout.number}: ${fixing.row.full_name}`} onClose={closeAct}>
          <FixDraft<PayoutDoc> key={fixing.payout.id} kind="payout" id={fixing.payout.id} onCancel={closeAct}>
            {(fix) => <PayoutForm employeeId={fixing.row.employee_id} fix={fix} onCancel={closeAct} onDone={doneAct} />}
          </FixDraft>
        </Modal>
      )}

      {salary && (
        <Modal
          title={`Оклад за ${salary.salary_from && salary.salary_to && !isCalendar(salary.salary_from, salary.salary_to) ? periodLabel(salary.salary_from, salary.salary_to) : MONTHS[m - 1]}: ${salary.full_name}`}
          onClose={() => setSalary(null)}
        >
          <SalaryForm
            employeeId={salary.employee_id}
            month={month}
            period={salary.salary_from && salary.salary_to ? { from: salary.salary_from, to: salary.salary_to } : undefined}
            hint={`По правилу ${salary.salary_rule_tyiyn ? formatSom(salary.salary_rule_tyiyn) : 'оклада нет'}. За неполный месяц укажите сумму, которую решили платить.`}
            initialSum={salary.salary_rule_tyiyn ? somInput(salary.salary_rule_tyiyn) : ''}
            onCancel={() => setSalary(null)}
            onDone={() => {
              setSalary(null)
              data.reload()
            }}
          />
        </Modal>
      )}
    </div>
  )
}

/** Столбцы выгрузки «За месяц» — как в таблице; неначисленный оклад — отдельным столбцом. */
const MONTH_COLS: ExportCol<MonthRow>[] = [
  { label: 'Сотрудник', value: (r) => r.full_name },
  { label: 'Кто', value: (r) => [r.is_cashier && 'кассир', r.is_master && 'мастер'].filter(Boolean).join(', ') },
  { label: 'На начало', value: (r) => xl.money(r.opening_tyiyn) },
  { label: 'Оклад', value: (r) => (r.salary_done ? xl.money(r.salary_tyiyn) : null) },
  {
    label: 'Оклад не начислен',
    value: (r) => (!r.salary_done && r.salary_rule_tyiyn ? xl.money(r.salary_rule_tyiyn) : null),
  },
  { label: '% с прибыли', value: (r) => xl.money(r.percent_tyiyn) },
  { label: 'Мастеру', value: (r) => xl.money(r.service_fee_tyiyn) },
  { label: 'За смену', value: (r) => xl.money(r.shift_fee_tyiyn) },
  { label: 'Бонус', value: (r) => xl.money(r.bonus_tyiyn) },
  { label: 'Удержано', value: (r) => xl.money(r.penalty_tyiyn + r.shortage_tyiyn) },
  { label: 'Начислено', value: (r) => xl.money(r.accrued_tyiyn) },
  { label: 'Выплачено', value: (r) => xl.money(r.paid_tyiyn - (r.advance_tyiyn ?? 0)) },
  { label: 'Аванс', value: (r) => xl.money(r.advance_tyiyn ?? 0) },
  { label: 'На конец', value: (r) => xl.money(r.closing_tyiyn) },
  { label: 'Период оклада', value: (r) => (r.salary_from && r.salary_to && !isCalendar(r.salary_from, r.salary_to) ? periodLabel(r.salary_from, r.salary_to) : '') },
]
